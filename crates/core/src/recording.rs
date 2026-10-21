//! Sermon recordings on disk, kept only until they are transcribed.
//!
//! A recording is a folder under `<data dir>/recordings/<id>/` holding 30-second WAV chunks (16 kHz
//! mono, 16-bit) and a `recording.json` manifest. The manifest is rewritten each time a chunk is
//! closed, so a crash or a killed app loses at most the chunk in progress, and transcription can
//! start on the first chunks while the sermon is still going.
//!
//! On Apple platforms the same samples are also encoded to AAC as they arrive, into ADTS parts of
//! about [`PART_SECONDS`] each (`part-0000.aac`, …), which the manifest lists as they close. The
//! parts are what uploads while the sermon is still going, and their packets, joined, are the M4A's.
//!
//! Capture is platform code (PulseAudio on Linux, ScreenCaptureKit and AVFoundation on Apple); it
//! hands samples to a [`ChunkWriter`] already converted to [`SAMPLE_RATE`] mono.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

#[cfg(all(feature = "apple-audio", any(target_os = "macos", target_os = "ios")))]
pub mod aac;
pub mod mp4;

/// What transcription services expect and what speech needs: 16 kHz mono.
pub const SAMPLE_RATE: u32 = 16_000;
/// Frames (samples) in one AAC-LC packet.
pub const FRAMES_PER_PACKET: u32 = 1024;
/// The MPEG-4 AudioSpecificConfig of the AAC-LC stream (object type 2, 16 kHz, mono), which the
/// muxer's `esds` carries and the decoder needs.
pub const AUDIO_SPECIFIC_CONFIG: [u8; 2] = [0x14, 0x08];
/// Whether this build encodes AAC parts while recording (Apple platforms with `apple-audio`). An
/// app whose recordings should upload as they record asserts it in a test.
pub const ENCODES: bool = cfg!(all(feature = "apple-audio", any(target_os = "macos", target_os = "ios")));
/// Long enough to give the transcriber context, short enough that a crash loses little.
pub const CHUNK_SECONDS: u32 = 30;
/// An AAC part: what a crash can lose of the upload, and what uploads at a time while recording.
pub const PART_SECONDS: u32 = 300;

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

/// The AAC parts closed so far, in order.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Parts {
    /// The encoder's delay: decoded frames before the first recorded sample.
    pub priming_frames: u32,
    pub files: Vec<Part>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Part {
    pub index: u32,
    /// File name inside the recording's folder.
    pub file: String,
    /// ADTS frames in the file, one AAC packet each.
    pub packets: u32,
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
    /// Samples in the closed chunks: exact, unlike the chunks' milliseconds.
    #[serde(default)]
    pub samples: u64,
    /// The AAC parts, on Apple platforms.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub aac: Option<Parts>,
    /// The day it was recorded in the recording device's time zone ("2026-10-18"), which the app
    /// knows and core doesn't. Recordings from before M1b have none.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recorded_on: Option<String>,
    /// Set once the library has all of it (`sermons.finish` went through), so it isn't queued again.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub uploaded: Option<Uploaded>,
}

/// Which account's library a recording went to, and the `seq` that took it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Uploaded {
    pub owner: String,
    pub seq: u64,
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
    read(&recordings_dir(data_dir).join(id))
}

/// The manifest of the recording in `dir`.
pub fn read(dir: &Path) -> Option<Manifest> {
    serde_json::from_slice(&std::fs::read(dir.join(MANIFEST)).ok()?).ok()
}

/// Replaces a finished recording's details.
pub fn save_details(data_dir: &Path, id: &str, details: Details) -> Result<Manifest> {
    let mut m = load(data_dir, id).with_context(|| format!("no recording {id}"))?;
    m.details = details.cleaned();
    write(&recordings_dir(data_dir).join(id), &m)?;
    Ok(m)
}

/// Marks the recording in `dir` as in `owner`'s library as of `seq`.
pub fn mark_uploaded(dir: &Path, owner: &str, seq: u64) -> Result<()> {
    let mut m = read(dir).with_context(|| format!("no recording in {}", dir.display()))?;
    m.uploaded = Some(Uploaded { owner: owner.into(), seq });
    write(dir, &m)
}

/// Ends a recording the app stopped without finishing (it was killed, or the Mac lost power) at
/// its last closed chunk, which is what its WAV and its manifest agree on: the chunk and AAC part
/// that were open are lost. Recordings that ended are returned as they are.
pub fn close_interrupted(dir: &Path) -> Result<Manifest> {
    let mut m = read(dir).with_context(|| format!("no recording in {}", dir.display()))?;
    if m.ended_at.is_none() {
        m.ended_at = Some(m.started_at + (m.samples / u64::from(SAMPLE_RATE)) as i64);
        write(dir, &m)?;
    }
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
    parts: PartWriter,
}

#[cfg(feature = "native")]
// Off Apple platforms `PartWriter` is a unit struct.
#[allow(clippy::default_constructed_unit_structs)]
impl ChunkWriter {
    /// `recorded_on` is the local date, from the caller, who knows the time zone.
    pub fn create(data_dir: &Path, source: Source, recorded_on: Option<String>) -> Result<Self> {
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
            samples: 0,
            aac: None,
            recorded_on,
            uploaded: None,
        };
        let mut w =
            Self { dir, manifest, current: None, in_chunk: 0, before: 0, parts: PartWriter::default() };
        if let Err(e) = w.parts.start(&mut w.manifest) {
            w.drop_parts(e);
        }
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

    /// AAC parts closed so far (none where nothing is encoded).
    pub fn closed_parts(&self) -> u32 {
        self.manifest.aac.as_ref().map_or(0, |a| a.files.len() as u32)
    }

    /// Appends mono samples at [`SAMPLE_RATE`], closing a chunk every [`CHUNK_SECONDS`].
    pub fn push(&mut self, mut samples: &[i16]) -> Result<()> {
        match self.parts.push(&self.dir, &mut self.manifest, samples) {
            Ok(false) => {}
            Ok(true) => self.write_manifest()?,
            Err(e) => {
                self.drop_parts(e);
                self.write_manifest()?;
            }
        }
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
        if let Err(e) = std::mem::take(&mut self.parts).finish(&self.dir, &mut self.manifest) {
            self.drop_parts(e);
        }
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
        self.manifest.samples = self.before;
        self.write_manifest()
    }

    fn write_manifest(&self) -> Result<()> {
        write(&self.dir, &self.manifest)
    }

    /// The AAC parts failed: the recording goes on as WAV chunks alone, which are what keeps it
    /// safe, and its audio is encoded from them afterwards, as an old recording's is.
    fn drop_parts(&mut self, e: anyhow::Error) {
        eprintln!("recording {}: AAC parts stopped, WAV continues: {e:#}", self.manifest.id);
        self.parts = PartWriter::default();
        self.manifest.aac = None;
    }
}

#[cfg(feature = "native")]
fn chunk_name(index: u32) -> String {
    format!("chunk-{index:04}.wav")
}

/// Encodes beside the WAV chunks and writes the AAC parts.
#[cfg(all(feature = "native", feature = "apple-audio", any(target_os = "macos", target_os = "ios")))]
#[derive(Default)]
struct PartWriter {
    encoder: Option<aac::Encoder>,
    open: Option<std::io::BufWriter<std::fs::File>>,
    /// Packets in the open part.
    packets: u32,
}

#[cfg(all(feature = "native", feature = "apple-audio", any(target_os = "macos", target_os = "ios")))]
impl PartWriter {
    /// Packets in a part: the first boundary at or after [`PART_SECONDS`].
    const PACKETS: u32 = (PART_SECONDS * SAMPLE_RATE).div_ceil(FRAMES_PER_PACKET);

    fn start(&mut self, m: &mut Manifest) -> Result<()> {
        let encoder = aac::Encoder::new()?;
        m.aac = Some(Parts { priming_frames: encoder.priming_frames(), files: Vec::new() });
        self.encoder = Some(encoder);
        Ok(())
    }

    /// Encodes `samples`; true when a part closed, so the manifest changed.
    fn push(&mut self, dir: &Path, m: &mut Manifest, samples: &[i16]) -> Result<bool> {
        let Some(encoder) = self.encoder.as_mut() else { return Ok(false) };
        let packets = encoder.push(samples)?;
        self.write(dir, m, packets)
    }

    fn finish(mut self, dir: &Path, m: &mut Manifest) -> Result<()> {
        let Some(encoder) = self.encoder.take() else { return Ok(()) };
        let packets = encoder.finish()?;
        self.write(dir, m, packets)?;
        self.close(m)
    }

    fn write(&mut self, dir: &Path, m: &mut Manifest, packets: Vec<Vec<u8>>) -> Result<bool> {
        use std::io::Write;
        let mut closed = false;
        for packet in packets {
            if self.open.is_none() {
                let path = dir.join(part_name(m.aac.as_ref().map_or(0, |p| p.files.len() as u32)));
                let file =
                    std::fs::File::create(&path).with_context(|| format!("creating {}", path.display()))?;
                self.open = Some(std::io::BufWriter::new(file));
            }
            let w = self.open.as_mut().expect("opened above");
            w.write_all(&aac::adts_header(packet.len()))?;
            w.write_all(&packet)?;
            self.packets += 1;
            if self.packets == Self::PACKETS {
                self.close(m)?;
                closed = true;
            }
        }
        Ok(closed)
    }

    fn close(&mut self, m: &mut Manifest) -> Result<()> {
        let Some(w) = self.open.take() else { return Ok(()) };
        w.into_inner().map_err(|e| e.into_error())?.sync_all()?;
        let parts = m.aac.as_mut().expect("started");
        let index = parts.files.len() as u32;
        parts.files.push(Part { index, file: part_name(index), packets: std::mem::take(&mut self.packets) });
        Ok(())
    }
}

/// Elsewhere (Linux), nothing is encoded while recording.
#[cfg(all(
    feature = "native",
    not(all(feature = "apple-audio", any(target_os = "macos", target_os = "ios")))
))]
#[derive(Default)]
struct PartWriter;

#[cfg(all(
    feature = "native",
    not(all(feature = "apple-audio", any(target_os = "macos", target_os = "ios")))
))]
impl PartWriter {
    fn start(&mut self, _: &mut Manifest) -> Result<()> {
        Ok(())
    }

    fn push(&mut self, _: &Path, _: &mut Manifest, _: &[i16]) -> Result<bool> {
        Ok(false)
    }

    fn finish(self, _: &Path, _: &mut Manifest) -> Result<()> {
        Ok(())
    }
}

/// Writes the recording's M4A: its parts' packets, unchanged, behind one `moov` that presents
/// exactly the recorded samples. The packet sizes come from the parts' ADTS headers.
#[cfg(all(feature = "native", feature = "apple-audio", any(target_os = "macos", target_os = "ios")))]
pub fn write_m4a(dir: &Path, m: &Manifest, out: &mut impl std::io::Write) -> Result<()> {
    let parts = m.aac.as_ref().context("this recording has no AAC parts")?;
    let files = parts
        .files
        .iter()
        .map(|p| std::fs::read(dir.join(&p.file)).with_context(|| format!("reading {}", p.file)))
        .collect::<Result<Vec<_>>>()?;
    let mut packets = Vec::new();
    for (part, bytes) in parts.files.iter().zip(&files) {
        let these = aac::adts_packets(bytes).with_context(|| format!("reading {}", part.file))?;
        anyhow::ensure!(
            these.len() as u32 == part.packets,
            "{} holds {} packets, not {}",
            part.file,
            these.len(),
            part.packets
        );
        packets.extend(these);
    }
    let sizes: Vec<u32> = packets.iter().map(|p| p.len() as u32).collect();
    // An interrupted recording's WAV goes on past its last closed part: present what the parts hold.
    let encoded =
        (packets.len() as u64 * u64::from(FRAMES_PER_PACKET)).saturating_sub(parts.priming_frames.into());
    let frames = m.samples.min(encoded);
    mp4::write(out, parts.priming_frames, frames, &sizes, &mut Slices(packets.iter(), &[]))
}

/// The packets back to back, read without copying them into one buffer.
#[cfg(all(feature = "native", feature = "apple-audio", any(target_os = "macos", target_os = "ios")))]
struct Slices<'a>(std::slice::Iter<'a, &'a [u8]>, &'a [u8]);

#[cfg(all(feature = "native", feature = "apple-audio", any(target_os = "macos", target_os = "ios")))]
impl std::io::Read for Slices<'_> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        while self.1.is_empty() {
            match self.0.next() {
                Some(next) => self.1 = next,
                None => return Ok(0),
            }
        }
        self.1.read(buf)
    }
}

#[cfg(all(feature = "native", feature = "apple-audio", any(target_os = "macos", target_os = "ios")))]
fn part_name(index: u32) -> String {
    format!("part-{index:04}.aac")
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
        let mut w = ChunkWriter::create(&data, Source::SystemAudio, None).unwrap();
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
        let mut w = ChunkWriter::create(&data, Source::Microphone, None).unwrap();
        w.push(&vec![0i16; (SAMPLE_RATE * 45) as usize]).unwrap();
        // The app dies here: no finish().
        drop(w);
        let m = &list(&data)[0];
        assert_eq!(m.ended_at, None);
        assert_eq!(m.chunks.len(), 1);
        assert_eq!(m.duration_ms(), 30_000);
    }

    /// Speech-like enough for the encoder: a tone whose pitch moves.
    #[cfg(all(feature = "apple-audio", any(target_os = "macos", target_os = "ios")))]
    fn sweep(seconds: u32) -> Vec<i16> {
        (0..SAMPLE_RATE * seconds)
            .map(|i| {
                let t = f64::from(i) / f64::from(SAMPLE_RATE);
                ((t * (300.0 + 50.0 * (t * 0.7).sin()) * std::f64::consts::TAU).sin() * 6000.0) as i16
            })
            .collect()
    }

    #[cfg(all(feature = "apple-audio", any(target_os = "macos", target_os = "ios")))]
    fn part_packets(dir: &Path, parts: &Parts) -> Vec<Vec<u8>> {
        parts
            .files
            .iter()
            .flat_map(|p| {
                let bytes = std::fs::read(dir.join(&p.file)).unwrap();
                let packets: Vec<Vec<u8>> =
                    aac::adts_packets(&bytes).unwrap().iter().map(|p| p.to_vec()).collect();
                assert_eq!(packets.len() as u32, p.packets, "{}", p.file);
                packets
            })
            .collect()
    }

    #[test]
    #[cfg(all(feature = "apple-audio", any(target_os = "macos", target_os = "ios")))]
    fn the_parts_join_to_one_encoders_packets() {
        let data = temp("parts");
        let pcm = sweep(11 * 60 + 7);
        let mut w = ChunkWriter::create(&data, Source::Microphone, None).unwrap();
        for block in pcm.chunks(1600) {
            w.push(block).unwrap();
        }
        let dir = w.dir().to_path_buf();
        let m = w.finish().unwrap();
        assert_eq!(m.samples, pcm.len() as u64);

        let mut one = aac::Encoder::new().unwrap();
        let mut expected = one.push(&pcm).unwrap();
        let priming = one.priming_frames();
        expected.extend(one.finish().unwrap());

        let parts = m.aac.as_ref().unwrap();
        assert_eq!(parts.priming_frames, priming);
        let sizes: Vec<u32> = parts.files.iter().map(|p| p.packets).collect();
        let per_part = (PART_SECONDS * SAMPLE_RATE).div_ceil(FRAMES_PER_PACKET);
        assert_eq!(sizes[..2], [per_part, per_part]);
        assert_eq!(sizes.len(), 3);
        assert_eq!(part_packets(&dir, parts), expected);

        // The M4A from the parts is the one the encoder's own packets make, presenting every
        // recorded sample.
        let mut from_parts = Vec::new();
        write_m4a(&dir, &m, &mut from_parts).unwrap();
        let mut direct = Vec::new();
        let sizes: Vec<u32> = expected.iter().map(|p| p.len() as u32).collect();
        mp4::write(&mut direct, priming, pcm.len() as u64, &sizes, &mut expected.concat().as_slice())
            .unwrap();
        assert!(from_parts == direct, "the M4A from the parts differs");
    }

    #[test]
    #[cfg(all(feature = "apple-audio", any(target_os = "macos", target_os = "ios")))]
    fn a_failing_part_leaves_the_wav_recording() {
        let data = temp("partfail");
        let mut w = ChunkWriter::create(&data, Source::Microphone, None).unwrap();
        // The second part can't be created.
        std::fs::create_dir(w.dir().join("part-0001.aac")).unwrap();
        let pcm = sweep(6 * 60);
        for block in pcm.chunks(1600) {
            w.push(block).unwrap();
        }
        let m = w.finish().unwrap();
        assert_eq!(m.aac, None);
        assert_eq!(m.samples, pcm.len() as u64);
        assert_eq!(load(&data, &m.id).unwrap(), m);
    }

    #[test]
    #[cfg(all(feature = "apple-audio", any(target_os = "macos", target_os = "ios")))]
    fn a_killed_recording_loses_only_its_open_part() {
        let data = temp("killed");
        let mut w = ChunkWriter::create(&data, Source::Microphone, None).unwrap();
        w.push(&sweep(7 * 60)).unwrap();
        let dir = w.dir().to_path_buf();
        // The app dies here: no finish().
        drop(w);
        let m = load(&data, &list(&data)[0].id).unwrap();
        let parts = m.aac.as_ref().unwrap();
        assert_eq!(parts.files.len(), 1, "the second part was open");
        let packets = part_packets(&dir, parts);
        // The closed part holds the first five minutes, whole.
        assert_eq!(
            packets.len() as u32 * FRAMES_PER_PACKET,
            (PART_SECONDS * SAMPLE_RATE).next_multiple_of(FRAMES_PER_PACKET)
        );
    }

    #[test]
    fn level_is_relative_to_full_scale() {
        assert_eq!(level(&[]), 0.0);
        assert!((level(&[i16::MAX, i16::MIN + 1]) - 1.0).abs() < 1e-6);
        assert!((level(&[16384; 8]) - 0.5).abs() < 1e-3);
    }
}
