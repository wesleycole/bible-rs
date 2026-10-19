//! Recording a sermon: the room through the microphone, or whatever the computer is playing.
//!
//! Capture runs on its own thread and hands 100 ms blocks to `linger_core::recording::ChunkWriter`,
//! which writes 30-second chunks to the data folder. On Linux it reads from PulseAudio (or
//! PipeWire's PulseAudio server): the default source for the microphone, the default sink's
//! monitor for system audio. The server resamples to 16 kHz mono for us. On macOS it reads the
//! default input device through Core Audio, or a process tap of everything playing (macOS 14.2
//! and later), and resamples itself; see `recorder/macos.rs`.

use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::thread::JoinHandle;
use std::time::Duration;

use anyhow::{Result, anyhow};
use linger_core::recording::{self, ChunkWriter, Manifest, SAMPLE_RATE, Source};

/// A recording in progress. Dropping it without [`stop`](Self::stop) still stops capture and
/// closes the chunks.
pub struct Recording {
    pub source: Source,
    id: String,
    shared: Arc<Shared>,
    thread: Option<JoinHandle<Result<Manifest>>>,
}

#[derive(Default)]
struct Shared {
    stop: AtomicBool,
    /// macOS is asking the user for permission; no audio yet.
    waiting: AtomicBool,
    /// f32 bits: the level of the latest block, 0–1.
    level: AtomicU32,
    samples: AtomicU64,
}

impl Recording {
    /// Starts capturing, or fails at once if the source can't be opened.
    pub fn start(data_dir: &Path, source: Source) -> Result<Self> {
        let mut writer = ChunkWriter::create(data_dir, source)?;
        let id = writer.id().to_string();
        let shared = Arc::new(Shared::default());
        let (opened_tx, opened_rx) = std::sync::mpsc::channel();
        let thread = {
            let shared = shared.clone();
            std::thread::Builder::new().name("recorder".into()).spawn(move || {
                let mut capture = match backend::open(source) {
                    Ok(c) => {
                        let _ = opened_tx.send(Ok(()));
                        c
                    }
                    Err(e) => {
                        let _ = opened_tx.send(Err(e));
                        return writer.finish();
                    }
                };
                let mut block = vec![0i16; SAMPLE_RATE as usize / 10];
                while !shared.stop.load(Ordering::Relaxed) {
                    // Nothing yet: macOS is asking the user for permission.
                    let got = capture.read(&mut block)?;
                    shared.waiting.store(!got, Ordering::Relaxed);
                    if !got {
                        continue;
                    }
                    writer.push(&block)?;
                    shared.level.store(recording::level(&block).to_bits(), Ordering::Relaxed);
                    shared.samples.store(writer.samples(), Ordering::Relaxed);
                }
                writer.finish()
            })?
        };
        let dir = recording::recordings_dir(data_dir).join(&id);
        match opened_rx.recv() {
            Ok(Ok(())) => Ok(Self { source, id, shared, thread: Some(thread) }),
            Ok(Err(e)) => {
                let _ = thread.join();
                let _ = std::fs::remove_dir_all(dir);
                Err(e)
            }
            Err(_) => Err(anyhow!("the recorder stopped unexpectedly")),
        }
    }

    pub fn id(&self) -> &str {
        &self.id
    }

    /// macOS is asking the user for permission, and the recording starts at their answer.
    pub fn waiting(&self) -> bool {
        self.shared.waiting.load(Ordering::Relaxed)
    }

    /// Loudness of the last 100 ms, 0–1.
    pub fn level(&self) -> f32 {
        f32::from_bits(self.shared.level.load(Ordering::Relaxed))
    }

    /// Length of audio captured so far.
    pub fn elapsed(&self) -> Duration {
        Duration::from_millis(self.shared.samples.load(Ordering::Relaxed) * 1000 / u64::from(SAMPLE_RATE))
    }

    /// Capture ended by itself (the device went away); [`stop`](Self::stop) returns why.
    pub fn failed(&self) -> bool {
        self.thread.as_ref().is_some_and(|t| t.is_finished())
    }

    pub fn stop(mut self) -> Result<Manifest> {
        self.shared.stop.store(true, Ordering::Relaxed);
        let t = self.thread.take().expect("joined only here or on drop");
        t.join().map_err(|_| anyhow!("the recorder panicked"))?
    }
}

impl Drop for Recording {
    fn drop(&mut self) {
        self.shared.stop.store(true, Ordering::Relaxed);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

/// Whether this build, on this machine, can record from `source` at all.
pub fn supported(source: Source) -> bool {
    backend::supported(source)
}

#[cfg(target_os = "linux")]
mod backend {
    use anyhow::{Context, Result};
    use libpulse_binding::def::BufferAttr;
    use libpulse_binding::sample::{Format, Spec};
    use libpulse_binding::stream::Direction;
    use libpulse_simple_binding::Simple;
    use linger_core::recording::{SAMPLE_RATE, Source};

    pub fn supported(source: Source) -> bool {
        matches!(source, Source::Microphone | Source::SystemAudio)
    }

    pub struct Capture {
        pulse: Simple,
        bytes: Vec<u8>,
    }

    pub fn open(source: Source) -> Result<Capture> {
        let spec = Spec { format: Format::S16le, channels: 1, rate: SAMPLE_RATE };
        // `@DEFAULT_MONITOR@` follows the output the user picked, as PulseAudio and PipeWire both
        // understand it.
        let device = match source {
            Source::Microphone => None,
            Source::SystemAudio => Some("@DEFAULT_MONITOR@"),
            Source::File => anyhow::bail!("files are imported, not captured"),
        };
        // Deliver 100 ms at a time rather than the server's default of up to two seconds.
        let fragment = SAMPLE_RATE / 10 * 2;
        let attr = BufferAttr {
            maxlength: u32::MAX,
            tlength: u32::MAX,
            prebuf: u32::MAX,
            minreq: u32::MAX,
            fragsize: fragment,
        };
        let stream = match source {
            Source::SystemAudio => "Sermon (computer audio)",
            _ => "Sermon (microphone)",
        };
        let pulse = Simple::new(None, "Linger", Direction::Record, device, stream, &spec, None, Some(&attr))
            .map_err(|e| anyhow::anyhow!("{e}"))
            .context("couldn't connect to the sound server (PulseAudio or PipeWire)")?;
        Ok(Capture { pulse, bytes: Vec::new() })
    }

    impl Capture {
        pub fn read(&mut self, out: &mut [i16]) -> Result<bool> {
            self.bytes.resize(out.len() * 2, 0);
            self.pulse.read(&mut self.bytes).map_err(|e| anyhow::anyhow!("recording failed: {e}"))?;
            let (pairs, _) = self.bytes.as_chunks::<2>();
            for (s, b) in out.iter_mut().zip(pairs) {
                *s = i16::from_le_bytes(*b);
            }
            Ok(true)
        }
    }
}

#[cfg(target_os = "macos")]
mod macos;
#[cfg(target_os = "macos")]
use macos as backend;

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
mod backend {
    use anyhow::Result;
    use linger_core::recording::Source;

    pub fn supported(_: Source) -> bool {
        false
    }

    pub struct Capture;

    pub fn open(_: Source) -> Result<Capture> {
        anyhow::bail!("recording isn't available on this platform yet")
    }

    impl Capture {
        pub fn read(&mut self, _: &mut [i16]) -> Result<bool> {
            unreachable!("open never succeeds")
        }
    }
}

/// "4:05", "1:02:09": a recording's length.
pub fn duration_label(secs: u64) -> String {
    let (h, m, s) = (secs / 3600, secs / 60 % 60, secs % 60);
    if h > 0 { format!("{h}:{m:02}:{s:02}") } else { format!("{m}:{s:02}") }
}

#[cfg(test)]
mod tests {
    #[test]
    fn duration_labels() {
        assert_eq!(super::duration_label(0), "0:00");
        assert_eq!(super::duration_label(245), "4:05");
        assert_eq!(super::duration_label(3729), "1:02:09");
    }

    /// A second of audio at any device rate is a second at 16 kHz, and a steady signal keeps its
    /// level.
    #[cfg(target_os = "macos")]
    #[test]
    fn downsampling_keeps_time_and_level() {
        for rate in [8_000, 16_000, 44_100, 48_000, 96_000] {
            let mut down = super::backend::Downsample::new(rate);
            let mut out = std::collections::VecDeque::new();
            for _ in 0..rate * 3 {
                down.push(0.5, &mut out);
            }
            assert!(out.len().abs_diff(48_000) <= 1, "{rate} Hz gave {} samples for 3 s", out.len());
            assert!(out.iter().all(|&s| s == 16383), "{rate} Hz changed the level");
        }
    }
}
