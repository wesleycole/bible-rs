//! Sermon recordings on disk, kept only until they are transcribed.
//!
//! A recording is a folder under `<data dir>/recordings/<id>/` holding 30-second WAV chunks (16 kHz
//! mono, 16-bit) and a `recording.json` manifest. The manifest is rewritten each time a chunk is
//! closed, so a crash or a killed app loses at most the chunk in progress, and transcription can
//! start on the first chunks while the sermon is still going.
//!
//! Capture is platform code (PulseAudio on Linux, ScreenCaptureKit and AVFoundation on Apple); it
//! hands samples to a [`ChunkWriter`] already converted to [`SAMPLE_RATE`] mono.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

/// What transcription services expect and what speech needs: 16 kHz mono.
pub const SAMPLE_RATE: u32 = 16_000;
/// Long enough to give the transcriber context, short enough that a crash loses little.
pub const CHUNK_SECONDS: u32 = 30;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Source {
    /// The room, through a microphone.
    Microphone,
    /// Whatever the computer is playing: a livestream, a video.
    SystemAudio,
    /// An audio or video file the user imported.
    File,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Chunk {
    pub index: u32,
    /// File name inside the recording's folder.
    pub file: String,
    pub start_ms: u64,
    pub duration_ms: u64,
}

/// What a recording is: what the user said about it, and what Linger noticed. Ask attributes what
/// was said by it ("John Piper, Desiring God") rather than taking every sermon for the user's
/// church's.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Details {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    /// Who preached: "John Piper".
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub speaker: Option<String>,
    /// The church, ministry or channel it came from: "Grace Church", "Desiring God".
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub from: Option<String>,
    /// The app that played it, for the computer's audio: "Google Chrome".
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub app: Option<String>,
}

impl Details {
    /// Trimmed, with blank fields as `None`.
    pub fn cleaned(self) -> Self {
        let clean = |s: Option<String>| s.map(|s| s.trim().to_string()).filter(|s| !s.is_empty());
        Self {
            title: clean(self.title),
            speaker: clean(self.speaker),
            from: clean(self.from),
            app: clean(self.app),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Manifest {
    pub id: String,
    pub source: Source,
    /// Unix seconds.
    pub started_at: i64,
    /// Unix seconds; `None` while recording (or if the app stopped before finishing).
    pub ended_at: Option<i64>,
    pub sample_rate: u32,
    pub chunks: Vec<Chunk>,
    #[serde(default)]
    pub details: Details,
}

impl Manifest {
    pub fn duration_ms(&self) -> u64 {
        self.chunks.last().map_or(0, |c| c.start_ms + c.duration_ms)
    }
}

pub fn recordings_dir(data_dir: &Path) -> PathBuf {
    data_dir.join("recordings")
}

/// Every recording on disk, newest first. Folders without a readable manifest are skipped.
pub fn list(data_dir: &Path) -> Vec<Manifest> {
    let mut out: Vec<Manifest> = std::fs::read_dir(recordings_dir(data_dir))
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| std::fs::read(e.path().join(MANIFEST)).ok())
        .filter_map(|b| serde_json::from_slice(&b).ok())
        .collect();
    out.sort_by_key(|m| std::cmp::Reverse(m.started_at));
    out
}

const MANIFEST: &str = "recording.json";

/// One recording's manifest.
pub fn load(data_dir: &Path, id: &str) -> Option<Manifest> {
    serde_json::from_slice(&std::fs::read(recordings_dir(data_dir).join(id).join(MANIFEST)).ok()?).ok()
}

/// Replaces a finished recording's details.
pub fn save_details(data_dir: &Path, id: &str, details: Details) -> Result<Manifest> {
    let mut m = load(data_dir, id).with_context(|| format!("no recording {id}"))?;
    m.details = details.cleaned();
    write(&recordings_dir(data_dir).join(id), &m)?;
    Ok(m)
}

/// Writes then renames, so a reader never sees half a manifest.
fn write(dir: &Path, m: &Manifest) -> Result<()> {
    let tmp = dir.join(format!("{MANIFEST}.tmp"));
    std::fs::write(&tmp, serde_json::to_vec_pretty(m)?)?;
    std::fs::rename(&tmp, dir.join(MANIFEST))?;
    Ok(())
}

/// Writes one recording: call [`push`](Self::push) with samples as they arrive, then
/// [`finish`](Self::finish).
#[cfg(feature = "native")]
pub struct ChunkWriter {
    dir: PathBuf,
    manifest: Manifest,
    current: Option<hound::WavWriter<std::io::BufWriter<std::fs::File>>>,
    /// Samples in the open chunk, and in all closed chunks before it.
    in_chunk: u64,
    before: u64,
}

#[cfg(feature = "native")]
impl ChunkWriter {
    pub fn create(data_dir: &Path, source: Source) -> Result<Self> {
        let id = uuid::Uuid::new_v4().to_string();
        let dir = recordings_dir(data_dir).join(&id);
        std::fs::create_dir_all(&dir).with_context(|| format!("creating {}", dir.display()))?;
        let manifest = Manifest {
            id,
            source,
            started_at: crate::time::now(),
            ended_at: None,
            sample_rate: SAMPLE_RATE,
            chunks: Vec::new(),
            details: Details::default(),
        };
        let w = Self { dir, manifest, current: None, in_chunk: 0, before: 0 };
        w.write_manifest()?;
        Ok(w)
    }

    pub fn id(&self) -> &str {
        &self.manifest.id
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// Samples recorded so far.
    pub fn samples(&self) -> u64 {
        self.before + self.in_chunk
    }

    /// The chunks closed so far (those ready to transcribe).
    pub fn closed_chunks(&self) -> &[Chunk] {
        &self.manifest.chunks
    }

    /// Appends mono samples at [`SAMPLE_RATE`], closing a chunk every [`CHUNK_SECONDS`].
    pub fn push(&mut self, mut samples: &[i16]) -> Result<()> {
        let per_chunk = u64::from(SAMPLE_RATE * CHUNK_SECONDS);
        while !samples.is_empty() {
            if self.current.is_none() {
                let index = self.manifest.chunks.len() as u32;
                let spec = hound::WavSpec {
                    channels: 1,
                    sample_rate: SAMPLE_RATE,
                    bits_per_sample: 16,
                    sample_format: hound::SampleFormat::Int,
                };
                let path = self.dir.join(chunk_name(index));
                self.current = Some(
                    hound::WavWriter::create(&path, spec)
                        .with_context(|| format!("creating {}", path.display()))?,
                );
            }
            let room = (per_chunk - self.in_chunk) as usize;
            let (now, rest) = samples.split_at(room.min(samples.len()));
            let w = self.current.as_mut().expect("opened above");
            for &s in now {
                w.write_sample(s)?;
            }
            self.in_chunk += now.len() as u64;
            samples = rest;
            if self.in_chunk == per_chunk {
                self.close_chunk()?;
            }
        }
        Ok(())
    }

    /// Notes the app that played what was recorded, saved when the recording finishes.
    pub fn set_app(&mut self, app: Option<String>) {
        self.manifest.details.app = app;
    }

    /// Closes the last chunk and marks the recording finished.
    pub fn finish(mut self) -> Result<Manifest> {
        self.close_chunk()?;
        self.manifest.ended_at = Some(crate::time::now());
        self.write_manifest()?;
        Ok(self.manifest)
    }

    fn close_chunk(&mut self) -> Result<()> {
        let Some(w) = self.current.take() else { return Ok(()) };
        w.finalize()?;
        let index = self.manifest.chunks.len() as u32;
        self.manifest.chunks.push(Chunk {
            index,
            file: chunk_name(index),
            start_ms: self.before * 1000 / u64::from(SAMPLE_RATE),
            duration_ms: self.in_chunk * 1000 / u64::from(SAMPLE_RATE),
        });
        self.before += self.in_chunk;
        self.in_chunk = 0;
        self.write_manifest()
    }

    fn write_manifest(&self) -> Result<()> {
        write(&self.dir, &self.manifest)
    }
}

#[cfg(feature = "native")]
fn chunk_name(index: u32) -> String {
    format!("chunk-{index:04}.wav")
}

/// Root mean square of a block of samples, 0.0–1.0, for level meters.
pub fn level(samples: &[i16]) -> f32 {
    if samples.is_empty() {
        return 0.0;
    }
    let sum: f64 = samples.iter().map(|&s| f64::from(s) * f64::from(s)).sum();
    ((sum / samples.len() as f64).sqrt() / f64::from(i16::MAX)) as f32
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("linger-rec-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        d
    }

    #[test]
    fn splits_into_chunks_with_running_offsets() {
        let data = temp("chunks");
        let mut w = ChunkWriter::create(&data, Source::SystemAudio).unwrap();
        let second = vec![1000i16; SAMPLE_RATE as usize];
        // 70.5 s in uneven blocks: chunks of 30 s, 30 s and 10.5 s.
        for _ in 0..70 {
            w.push(&second[..SAMPLE_RATE as usize / 3]).unwrap();
            w.push(&second[SAMPLE_RATE as usize / 3..]).unwrap();
        }
        w.push(&second[..SAMPLE_RATE as usize / 2]).unwrap();
        assert_eq!(w.closed_chunks().len(), 2, "the third chunk is still open");
        let dir = w.dir().to_path_buf();
        let m = w.finish().unwrap();

        let spans: Vec<(u64, u64)> = m.chunks.iter().map(|c| (c.start_ms, c.duration_ms)).collect();
        assert_eq!(spans, [(0, 30_000), (30_000, 30_000), (60_000, 10_500)]);
        assert_eq!(m.duration_ms(), 70_500);
        assert!(m.ended_at.is_some());

        let r = hound::WavReader::open(dir.join("chunk-0002.wav")).unwrap();
        assert_eq!(r.spec().sample_rate, SAMPLE_RATE);
        assert_eq!(r.len(), SAMPLE_RATE * 21 / 2);
        assert_eq!(list(&data), [m]);
    }

    #[test]
    fn an_unfinished_recording_keeps_its_closed_chunks() {
        let data = temp("crash");
        let mut w = ChunkWriter::create(&data, Source::Microphone).unwrap();
        w.push(&vec![0i16; (SAMPLE_RATE * 45) as usize]).unwrap();
        // The app dies here: no finish().
        drop(w);
        let m = &list(&data)[0];
        assert_eq!(m.ended_at, None);
        assert_eq!(m.chunks.len(), 1);
        assert_eq!(m.duration_ms(), 30_000);
    }

    #[test]
    fn level_is_relative_to_full_scale() {
        assert_eq!(level(&[]), 0.0);
        assert!((level(&[i16::MAX, i16::MIN + 1]) - 1.0).abs() < 1e-6);
        assert!((level(&[16384; 8]) - 0.5).abs() < 1e-3);
    }
}
