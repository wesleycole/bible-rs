//! Turning a recorded sermon into a transcript, through AssemblyAI: by way of Linger's proxy
//! ([`transcribe_via_proxy`], which holds the key and the account's allowance), or directly with a
//! developer's own key ([`AssemblyAi`]).
//!
//! The recording's chunks are joined into one file and sent as one job, so the transcriber sees
//! the whole sermon (better punctuation and speaker turns than 30-second pieces). Once the
//! transcript is saved beside the recording as `transcript.json`, the job is deleted from
//! AssemblyAI, which deletes the uploaded audio with it: the service keeps nothing.

use std::io::Read;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use anyhow::{Context, Result, anyhow, bail};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::api::{Api, ApiError};
use crate::books::BookId;
use crate::recording::{self, Manifest};

const API: &str = "https://api.assemblyai.com/v2";
/// Longer than any sermon takes to transcribe (a few minutes for an hour of audio).
const GIVE_UP_AFTER: Duration = Duration::from_secs(2 * 60 * 60);

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Transcript {
    pub recording_id: String,
    /// Unix seconds, when the sermon was recorded.
    pub recorded_at: i64,
    pub duration_ms: u64,
    pub language: Option<String>,
    /// Turns of speech in order. Speakers are "A", "B"… as the transcriber told them apart.
    pub segments: Vec<Segment>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Segment {
    pub start_ms: u64,
    pub end_ms: u64,
    pub speaker: Option<String>,
    pub text: String,
}

impl Transcript {
    /// "Sermon · Oct 19, 2026": recordings have no titles yet.
    pub fn title(&self) -> String {
        let (y, m, d) = crate::study::civil_from_days(self.recorded_at.div_euclid(86_400));
        const MONTHS: [&str; 12] =
            ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];
        format!("Sermon · {} {d}, {y}", MONTHS[(m as usize).clamp(1, 12) - 1])
    }
}

/// `<data>/recordings/<id>/transcript.json`.
pub fn path(data_dir: &Path, recording_id: &str) -> PathBuf {
    recording::recordings_dir(data_dir).join(recording_id).join("transcript.json")
}

pub fn load(data_dir: &Path, recording_id: &str) -> Option<Transcript> {
    serde_json::from_slice(&std::fs::read(path(data_dir, recording_id)).ok()?).ok()
}

/// Every transcribed sermon, newest first.
pub fn all(data_dir: &Path) -> Vec<Transcript> {
    recording::list(data_dir).iter().filter_map(|m| load(data_dir, &m.id)).collect()
}

/// "12:30", "1:02:09": a moment in a sermon.
pub fn clock(ms: u64) -> String {
    let s = ms / 1000;
    let (h, m, s) = (s / 3600, s / 60 % 60, s % 60);
    if h > 0 { format!("{h}:{m:02}:{s:02}") } else { format!("{m}:{s:02}") }
}

/// Reads "12:30", "1:02:09" or "750" (seconds) back into milliseconds.
pub fn parse_clock(s: &str) -> Option<u64> {
    let mut total = 0u64;
    for part in s.trim().split(':') {
        total = total * 60 + part.trim().parse::<u64>().ok()?;
    }
    Some(total * 1000)
}

#[derive(Clone)]
pub struct AssemblyAi {
    key: String,
}

impl AssemblyAi {
    /// The key from `LINGER_ASSEMBLYAI_KEY`, `ASSEMBLYAI_API_KEY` or `ASSEMBLY_AI_API_KEY`.
    pub fn from_env() -> Option<Self> {
        ["LINGER_ASSEMBLYAI_KEY", "ASSEMBLYAI_API_KEY", "ASSEMBLY_AI_API_KEY"]
            .iter()
            .find_map(|k| std::env::var(k).ok().filter(|v| !v.trim().is_empty()))
            .map(|key| Self { key })
    }

    /// Transcribes a finished recording and saves the transcript beside it. `status` hears
    /// "Uploading", "Transcribing"… as the job moves along.
    pub fn transcribe(
        &self,
        data_dir: &Path,
        manifest: &Manifest,
        mut status: impl FnMut(&str),
    ) -> Result<Transcript> {
        if manifest.chunks.is_empty() {
            bail!("the recording is empty");
        }
        let agent: ureq::Agent = ureq::Agent::config_builder()
            .timeout_connect(Some(Duration::from_secs(15)))
            .timeout_recv_response(Some(Duration::from_secs(120)))
            .http_status_as_error(false)
            .user_agent(concat!("linger/", env!("CARGO_PKG_VERSION")))
            .build()
            .into();

        status("Preparing");
        let dir = recording::recordings_dir(data_dir).join(&manifest.id);
        let joined = dir.join("upload.wav");
        join_chunks(&dir, manifest, &joined)?;
        status("Uploading");
        let upload = agent
            .post(format!("{API}/upload"))
            .header("authorization", &self.key)
            .header("content-type", "application/octet-stream")
            .send(std::fs::File::open(&joined)?);
        let _ = std::fs::remove_file(&joined);
        let upload = json_body(upload, "uploading the sermon")?;
        let audio_url = upload["upload_url"].as_str().context("AssemblyAI returned no upload URL")?;

        // Book names help the transcriber spell "Habakkuk" and "Thessalonians".
        let keyterms: Vec<&str> = BookId::all().map(BookId::name).collect();
        let request = json!({
            "audio_url": audio_url,
            "speech_models": ["universal-3-5-pro", "universal-2"],
            "speaker_labels": true,
            "punctuate": true,
            "format_text": true,
            "keyterms_prompt": keyterms,
        });
        let job = agent
            .post(format!("{API}/transcript"))
            .header("authorization", &self.key)
            .header("content-type", "application/json")
            .send(request.to_string());
        let job = json_body(job, "starting the transcription")?;
        let id = job["id"].as_str().context("AssemblyAI returned no job id")?.to_string();

        status("Transcribing");
        let started = Instant::now();
        let done = loop {
            std::thread::sleep(Duration::from_secs(3));
            let poll = agent.get(format!("{API}/transcript/{id}")).header("authorization", &self.key).call();
            let v = json_body(poll, "checking the transcription")?;
            match v["status"].as_str() {
                Some("completed") => break v,
                Some("error") => {
                    self.delete(&agent, &id);
                    bail!(
                        "AssemblyAI couldn't transcribe it: {}",
                        v["error"].as_str().unwrap_or("unknown error")
                    );
                }
                _ if started.elapsed() > GIVE_UP_AFTER => bail!("the transcription took too long"),
                _ => {}
            }
        };
        let transcript = Transcript {
            recording_id: manifest.id.clone(),
            recorded_at: manifest.started_at,
            duration_ms: manifest.duration_ms(),
            language: done["language_code"].as_str().map(str::to_string),
            segments: segments(&done),
        };
        save(data_dir, &transcript)?;
        self.delete(&agent, &id);
        Ok(transcript)
    }

    /// Deletes the job and its audio from AssemblyAI. Best effort: the transcript is already
    /// saved, and AssemblyAI expires uploads on its own.
    fn delete(&self, agent: &ureq::Agent, id: &str) {
        let _ = agent.delete(format!("{API}/transcript/{id}")).header("authorization", &self.key).call();
    }
}

/// Transcribes a finished recording through Linger's proxy and saves the transcript beside it.
///
/// The joined WAV goes straight to the signed upload URL the proxy hands out (not through the
/// proxy), the proxy starts the AssemblyAI job, and the transcript is polled for every 5 s.
/// `status` hears "Preparing", "Uploading", "Transcribing".
pub fn transcribe_via_proxy(
    api: &Api,
    data_dir: &Path,
    manifest: &Manifest,
    mut status: impl FnMut(&str),
) -> Result<Transcript> {
    if manifest.chunks.is_empty() {
        bail!("the recording is empty");
    }
    status("Preparing");
    let dir = recording::recordings_dir(data_dir).join(&manifest.id);
    let joined = dir.join("upload.wav");
    join_chunks(&dir, manifest, &joined)?;
    let result = upload_and_transcribe(api, &joined, manifest, &mut status);
    let _ = std::fs::remove_file(&joined);
    let done = result.map_err(plan_message)?;
    let transcript = Transcript {
        recording_id: manifest.id.clone(),
        recorded_at: manifest.started_at,
        duration_ms: manifest.duration_ms(),
        language: done["transcript"]["language"].as_str().map(str::to_string),
        segments: serde_json::from_value(done["transcript"]["segments"].clone())
            .context("unreadable transcript from Linger's servers")?,
    };
    save(data_dir, &transcript)?;
    Ok(transcript)
}

/// A Free account past its trial: say what Pro adds rather than the bare refusal.
fn plan_message(e: anyhow::Error) -> anyhow::Error {
    match e.downcast_ref::<ApiError>() {
        Some(ApiError::Upgrade { plan_required: true, upgrade, .. }) => {
            anyhow::Error::new(ApiError::Upgrade {
                message: "Cloud transcripts are part of Linger Pro (Free includes one trial hour).".into(),
                upgrade: *upgrade,
                plan_required: true,
            })
        }
        _ => e,
    }
}

fn upload_and_transcribe(
    api: &Api,
    wav: &Path,
    manifest: &Manifest,
    status: &mut impl FnMut(&str),
) -> Result<Value> {
    let bytes = std::fs::metadata(wav)?.len();
    // The proxy checks the size against 16-bit mono at the recording's rate.
    let samples = bytes.saturating_sub(44) / 2;
    let duration_sec = samples as f64 / f64::from(manifest.sample_rate.max(1));
    let created = api.json_call(
        "POST",
        "/v1/sermons",
        Some(&json!({
            "recordingId": manifest.id,
            "durationSec": duration_sec,
            "bytes": bytes,
            "contentType": "audio/wav",
        })),
    )?;
    let id = created["sermonId"].as_str().context("the server sent no sermon id")?.to_string();
    let upload = &created["upload"];
    let url = upload["url"].as_str().context("the server sent no upload URL")?;

    status("Uploading");
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_connect(Some(Duration::from_secs(15)))
        .timeout_send_body(Some(Duration::from_secs(30 * 60)))
        .timeout_recv_response(Some(Duration::from_secs(120)))
        .http_status_as_error(false)
        .user_agent(concat!("linger/", env!("CARGO_PKG_VERSION")))
        .build()
        .into();
    let mut put = agent.put(url).header("content-length", bytes.to_string());
    for (k, v) in upload["headers"].as_object().into_iter().flatten() {
        if let Some(v) = v.as_str() {
            put = put.header(k.as_str(), v);
        }
    }
    let mut resp =
        put.send(std::fs::File::open(wav)?).map_err(|e| anyhow!(e).context("uploading the sermon"))?;
    if !resp.status().is_success() {
        let text = resp.body_mut().read_to_string().unwrap_or_default();
        bail!(
            "uploading the sermon: HTTP {}: {}",
            resp.status().as_u16(),
            text.chars().take(200).collect::<String>()
        );
    }

    api.json_call("POST", &format!("/v1/sermons/{id}/transcribe"), None)?;
    status("Transcribing");
    let started = Instant::now();
    loop {
        std::thread::sleep(Duration::from_secs(5));
        let v = api.json_call("GET", &format!("/v1/sermons/{id}"), None)?;
        match v["status"].as_str() {
            Some("done") => return Ok(v),
            Some("error") => bail!(
                "the sermon couldn't be transcribed: {}",
                v["error"].as_str().unwrap_or("unknown error")
            ),
            _ if started.elapsed() > GIVE_UP_AFTER => bail!("the transcription took too long"),
            _ => {}
        }
    }
}

fn save(data_dir: &Path, t: &Transcript) -> Result<()> {
    let p = path(data_dir, &t.recording_id);
    let tmp = p.with_extension("json.tmp");
    std::fs::write(&tmp, serde_json::to_vec_pretty(t)?)?;
    std::fs::rename(&tmp, &p)?;
    Ok(())
}

/// The recording's chunks as one WAV file at `out`.
fn join_chunks(dir: &Path, m: &Manifest, out: &Path) -> Result<()> {
    let spec = hound::WavSpec {
        channels: 1,
        sample_rate: m.sample_rate,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };
    let mut w = hound::WavWriter::create(out, spec)?;
    for c in &m.chunks {
        let mut r =
            hound::WavReader::open(dir.join(&c.file)).with_context(|| format!("reading {}", c.file))?;
        for s in r.samples::<i16>() {
            w.write_sample(s?)?;
        }
    }
    w.finalize()?;
    Ok(())
}

/// The speaker turns of a finished job; one segment of its whole text when it has none.
fn segments(v: &Value) -> Vec<Segment> {
    let turns: Vec<Segment> = v["utterances"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|u| {
            Some(Segment {
                start_ms: u["start"].as_u64()?,
                end_ms: u["end"].as_u64()?,
                speaker: u["speaker"].as_str().map(str::to_string),
                text: u["text"].as_str()?.trim().to_string(),
            })
        })
        .filter(|s| !s.text.is_empty())
        .collect();
    if !turns.is_empty() {
        return turns;
    }
    match v["text"].as_str().map(str::trim).filter(|t| !t.is_empty()) {
        Some(text) => vec![Segment {
            start_ms: 0,
            end_ms: v["audio_duration"].as_f64().map_or(0, |s| (s * 1000.0) as u64),
            speaker: None,
            text: text.to_string(),
        }],
        None => Vec::new(),
    }
}

fn json_body(
    resp: std::result::Result<ureq::http::Response<ureq::Body>, ureq::Error>,
    what: &str,
) -> Result<Value> {
    let mut resp = resp.map_err(|e| anyhow!(e).context(format!("{what}: network error")))?;
    let status = resp.status().as_u16();
    let mut body = String::new();
    resp.body_mut().as_reader().read_to_string(&mut body).ok();
    let v: Value = serde_json::from_str(&body).unwrap_or(Value::Null);
    match status {
        200..=299 => Ok(v),
        401 | 403 => bail!("{what}: the AssemblyAI key was rejected (HTTP {status})"),
        _ => bail!(
            "{what}: HTTP {status}: {}",
            v["error"].as_str().map(str::to_string).unwrap_or_else(|| body.chars().take(200).collect())
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn titles_carry_the_day() {
        let t = Transcript {
            recording_id: "x".into(),
            recorded_at: 1_792_368_000,
            duration_ms: 0,
            language: None,
            segments: vec![],
        };
        assert_eq!(t.title(), "Sermon · Oct 19, 2026");
    }

    #[test]
    fn clocks_round_trip() {
        assert_eq!(clock(0), "0:00");
        assert_eq!(clock(750_400), "12:30");
        assert_eq!(clock(3_729_000), "1:02:09");
        assert_eq!(parse_clock("12:30"), Some(750_000));
        assert_eq!(parse_clock("1:02:09"), Some(3_729_000));
        assert_eq!(parse_clock("90"), Some(90_000));
        assert_eq!(parse_clock("ten"), None);
    }

    #[test]
    fn speaker_turns_become_segments() {
        let v = json!({
            "text": "Welcome. Turn to John three.",
            "utterances": [
                {"speaker": "A", "start": 0, "end": 1200, "text": "Welcome."},
                {"speaker": "B", "start": 1300, "end": 4000, "text": " Turn to John three. "},
                {"speaker": "A", "start": 4100, "end": 4200, "text": ""}
            ]
        });
        let s = segments(&v);
        assert_eq!(s.len(), 2, "empty turns are dropped");
        assert_eq!(
            (s[1].start_ms, s[1].speaker.as_deref(), s[1].text.as_str()),
            (1300, Some("B"), "Turn to John three.")
        );
        // No turns (diarization found none): the whole text, as one segment.
        let s = segments(&json!({"text": "Grace and peace.", "audio_duration": 2.5, "utterances": null}));
        assert_eq!(
            s,
            [Segment { start_ms: 0, end_ms: 2500, speaker: None, text: "Grace and peace.".into() }]
        );
    }

    #[test]
    fn chunks_join_into_one_file() {
        let data = std::env::temp_dir().join(format!("linger-join-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&data);
        let mut w = recording::ChunkWriter::create(&data, recording::Source::Microphone).unwrap();
        let samples: Vec<i16> =
            (0..(recording::SAMPLE_RATE * 45) as usize).map(|i| (i % 1000) as i16).collect();
        w.push(&samples).unwrap();
        let dir = w.dir().to_path_buf();
        let m = w.finish().unwrap();
        assert_eq!(m.chunks.len(), 2);
        let out = dir.join("joined.wav");
        join_chunks(&dir, &m, &out).unwrap();
        let joined: Vec<i16> = hound::WavReader::open(&out).unwrap().samples().map(Result::unwrap).collect();
        assert_eq!(joined, samples, "every sample, in order, across the chunk boundary");
    }
}
