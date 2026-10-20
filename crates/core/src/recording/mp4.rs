//! The M4A muxer: one AAC track, `moov` before `mdat`, the packets copied without re-encoding.
//!
//! Why M4A and not the ADTS parts as they are: ADTS has no index, so players estimate its timeline
//! from the bit rate and a transcript timestamp an hour in lands minutes from its sentence. An
//! M4A carries every packet's size (`stsz`) and timing (`stts`), and an edit list that trims the
//! encoder's priming, so each player reports the exact duration and seeks to the sample.
//!
//! The edit list comes with a `roll` sample group (`sgpd`/`sbgp`, as ffmpeg writes). Without it,
//! Apple's decoders assume AAC's usual 2112 priming frames themselves and apply the edit on top,
//! so AVPlayer runs 0.132 s early; with it they trust the edit list, like Chrome and ffmpeg do.

use std::io::{Read, Write};

use anyhow::{Context, Result, ensure};

use super::{AUDIO_SPECIFIC_CONFIG, FRAMES_PER_PACKET, SAMPLE_RATE};

/// Packets in one `mdat` chunk (`stsc`/`stco`): about 4 s of AAC-LC at 16 kHz.
const PACKETS_PER_CHUNK: usize = 64;

/// Writes the M4A: `ftyp`, `moov` built from the packet `sizes`, then `mdat` copied from
/// `packets`, which holds the packets back to back in order. `priming_frames` is the encoder's
/// delay, which the edit list skips; `frames` is the recorded sample count, which the players
/// present (what the packets hold beyond `priming_frames + frames` is padding).
pub fn write(
    out: &mut impl Write,
    priming_frames: u32,
    frames: u64,
    sizes: &[u32],
    packets: &mut impl Read,
) -> Result<()> {
    let data: u64 = sizes.iter().map(|&s| u64::from(s)).sum();
    ensure!(data + (1 << 20) < u64::from(u32::MAX), "recording too long for 32-bit chunk offsets");
    let ftyp = atom(b"ftyp", &[b"M4A ".as_slice(), &0u32.to_be_bytes(), b"M4A ", b"mp42", b"isom"].concat());
    // The chunk offsets depend on the size of `moov`, which doesn't depend on their values.
    let moov_len = moov(priming_frames, frames, sizes, 0).len();
    let mdat_start = (ftyp.len() + moov_len + 8) as u32;
    let moov = moov(priming_frames, frames, sizes, mdat_start);
    debug_assert_eq!(moov.len(), moov_len);
    out.write_all(&ftyp)?;
    out.write_all(&moov)?;
    out.write_all(&(data as u32 + 8).to_be_bytes())?;
    out.write_all(b"mdat")?;
    let copied = std::io::copy(&mut packets.take(data), out).context("copying packets")?;
    ensure!(copied == data, "packets hold {copied} bytes, sizes say {data}");
    Ok(())
}

fn moov(priming_frames: u32, frames: u64, sizes: &[u32], mdat_start: u32) -> Vec<u8> {
    let packets = sizes.len() as u64;
    let media_duration = packets * u64::from(FRAMES_PER_PACKET);
    // The frames players present: the recording, never more than the packets hold after priming.
    let presented = frames.min(media_duration.saturating_sub(u64::from(priming_frames)));

    let mvhd = full(
        b"mvhd",
        0,
        0,
        &[
            &[0u8; 8],
            &SAMPLE_RATE.to_be_bytes(),
            &(presented as u32).to_be_bytes(),
            &0x0001_0000u32.to_be_bytes(), // rate
            &0x0100u16.to_be_bytes(),      // volume
            &[0u8; 10],
            &MATRIX,
            &[0u8; 24],
            &2u32.to_be_bytes(), // next track id
        ],
    );
    let tkhd = full(
        b"tkhd",
        0,
        7,
        &[
            &[0u8; 8],
            &1u32.to_be_bytes(), // track id
            &[0u8; 4],
            &(presented as u32).to_be_bytes(),
            &[0u8; 8],
            &[0u8; 4],                // layer, alternate group
            &0x0100u16.to_be_bytes(), // volume
            &[0u8; 2],
            &MATRIX,
            &[0u8; 8], // width, height
        ],
    );
    let elst = full(
        b"elst",
        0,
        0,
        &[
            &1u32.to_be_bytes(),
            &(presented as u32).to_be_bytes(),
            &priming_frames.to_be_bytes(),
            &0x0001_0000u32.to_be_bytes(), // media rate 1.0
        ],
    );
    let mdhd = full(
        b"mdhd",
        0,
        0,
        &[
            &[0u8; 8],
            &SAMPLE_RATE.to_be_bytes(),
            &(media_duration as u32).to_be_bytes(),
            &0x55C4u16.to_be_bytes(), // language "und"
            &[0u8; 2],
        ],
    );
    let hdlr = full(b"hdlr", 0, 0, &[&[0u8; 4], b"soun", &[0u8; 12], b"SoundHandler\0"]);
    let dref = full(b"dref", 0, 0, &[&1u32.to_be_bytes(), &full(b"url ", 0, 1, &[])]);

    let seconds = media_duration as f64 / f64::from(SAMPLE_RATE);
    let bytes: u64 = sizes.iter().map(|&s| u64::from(s)).sum();
    let avg_bit_rate = if seconds > 0.0 { (bytes as f64 * 8.0 / seconds) as u32 } else { 0 };
    let max_packet = sizes.iter().copied().max().unwrap_or(0);
    let max_bit_rate = max_packet * 8 * SAMPLE_RATE / FRAMES_PER_PACKET;
    let esds = full(
        b"esds",
        0,
        0,
        &[&descriptor(
            0x03,
            &[
                &[0, 0, 0], // ES_ID 0, no flags
                &descriptor(
                    0x04,
                    &[
                        &[0x40, 0x15],                  // MPEG-4 audio, audio stream
                        &max_packet.to_be_bytes()[1..], // buffer size (24 bits)
                        &max_bit_rate.to_be_bytes(),
                        &avg_bit_rate.to_be_bytes(),
                        &descriptor(0x05, &[&AUDIO_SPECIFIC_CONFIG]),
                    ],
                ),
                &descriptor(0x06, &[&[0x02]]),
            ],
        )],
    );
    let mp4a = atom(
        b"mp4a",
        &[
            [0u8; 6].as_slice(),
            &1u16.to_be_bytes(), // data reference index
            &[0u8; 8],
            &1u16.to_be_bytes(),  // mono
            &16u16.to_be_bytes(), // sample size
            &[0u8; 4],
            &(SAMPLE_RATE << 16).to_be_bytes(),
            &esds,
        ]
        .concat(),
    );
    let stsd = full(b"stsd", 0, 0, &[&1u32.to_be_bytes(), &mp4a]);
    let stts = full(
        b"stts",
        0,
        0,
        &[&1u32.to_be_bytes(), &(packets as u32).to_be_bytes(), &FRAMES_PER_PACKET.to_be_bytes()],
    );

    let chunks = sizes.chunks(PACKETS_PER_CHUNK);
    let count = chunks.len();
    let mut stsc = Vec::new();
    let mut entries = 0u32;
    let mut stsc_entry = |first: u32, per_chunk: u32| {
        stsc.extend_from_slice(&first.to_be_bytes());
        stsc.extend_from_slice(&per_chunk.to_be_bytes());
        stsc.extend_from_slice(&1u32.to_be_bytes());
        entries += 1;
    };
    if count > 0 {
        stsc_entry(1, PACKETS_PER_CHUNK.min(sizes.len()) as u32);
        let last = sizes.len() - (count - 1) * PACKETS_PER_CHUNK;
        if count > 1 && last != PACKETS_PER_CHUNK {
            stsc_entry(count as u32, last as u32);
        }
    }
    let stsc = full(b"stsc", 0, 0, &[&entries.to_be_bytes(), &stsc]);
    let mut stco = (count as u32).to_be_bytes().to_vec();
    let mut offset = mdat_start;
    for chunk in chunks {
        stco.extend_from_slice(&offset.to_be_bytes());
        offset += chunk.iter().sum::<u32>();
    }
    let stco = full(b"stco", 0, 0, &[&stco]);
    let mut stsz = [0u32.to_be_bytes(), (packets as u32).to_be_bytes()].concat();
    for s in sizes {
        stsz.extend_from_slice(&s.to_be_bytes());
    }
    let stsz = full(b"stsz", 0, 0, &[&stsz]);
    // Every packet is in the one `roll` group: decoding starts one packet before a seek.
    let sgpd =
        full(b"sgpd", 1, 0, &[b"roll", &2u32.to_be_bytes(), &1u32.to_be_bytes(), &(-1i16).to_be_bytes()]);
    let sbgp = full(
        b"sbgp",
        0,
        0,
        &[b"roll", &1u32.to_be_bytes(), &(packets as u32).to_be_bytes(), &1u32.to_be_bytes()],
    );

    let stbl = atom(b"stbl", &[stsd, stts, stsc, stsz, stco, sgpd, sbgp].concat());
    let minf = atom(b"minf", &[full(b"smhd", 0, 0, &[&[0u8; 4]]), atom(b"dinf", &dref), stbl].concat());
    let mdia = atom(b"mdia", &[mdhd, hdlr, minf].concat());
    let trak = atom(b"trak", &[tkhd, atom(b"edts", &elst), mdia].concat());
    atom(b"moov", &[mvhd, trak].concat())
}

/// The identity transformation matrix of `mvhd` and `tkhd`.
const MATRIX: [u8; 36] = {
    let mut m = [0u8; 36];
    m[1] = 1; // 0x00010000
    m[17] = 1;
    m[32] = 0x40; // 0x40000000
    m
};

fn atom(kind: &[u8; 4], payload: &[u8]) -> Vec<u8> {
    let mut b = Vec::with_capacity(8 + payload.len());
    b.extend_from_slice(&((8 + payload.len()) as u32).to_be_bytes());
    b.extend_from_slice(kind);
    b.extend_from_slice(payload);
    b
}

fn full(kind: &[u8; 4], version: u8, flags: u32, parts: &[&[u8]]) -> Vec<u8> {
    let mut payload = vec![version];
    payload.extend_from_slice(&flags.to_be_bytes()[1..]);
    for p in parts {
        payload.extend_from_slice(p);
    }
    atom(kind, &payload)
}

/// An MPEG-4 descriptor: tag, length (short form; these are all small), payload.
fn descriptor(tag: u8, parts: &[&[u8]]) -> Vec<u8> {
    let payload = parts.concat();
    assert!(payload.len() < 128);
    [vec![tag, payload.len() as u8], payload].concat()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The boxes directly inside `data`, as (kind, payload).
    fn boxes(data: &[u8]) -> Vec<(&str, &[u8])> {
        let mut out = Vec::new();
        let mut at = 0;
        while at + 8 <= data.len() {
            let len = u32::from_be_bytes(data[at..at + 4].try_into().unwrap()) as usize;
            out.push((std::str::from_utf8(&data[at + 4..at + 8]).unwrap(), &data[at + 8..at + len]));
            at += len;
        }
        assert_eq!(at, data.len());
        out
    }

    fn find<'a>(data: &'a [u8], path: &[&str]) -> &'a [u8] {
        let mut here = data;
        for kind in path {
            here = boxes(here).into_iter().find(|(k, _)| k == kind).unwrap_or_else(|| panic!("no {kind}")).1;
        }
        here
    }

    fn u32s(data: &[u8]) -> Vec<u32> {
        data.chunks(4).map(|c| u32::from_be_bytes(c.try_into().unwrap())).collect()
    }

    #[test]
    fn writes_moov_first_with_exact_timing() {
        // 70 packets: two chunks of 64 and 6, timing trimmed to the recorded frames.
        let sizes: Vec<u32> = (0..70u32).map(|i| 100 + i).collect();
        let mut packets = Vec::new();
        for (i, &s) in sizes.iter().enumerate() {
            packets.extend(std::iter::repeat_n(i as u8, s as usize));
        }
        let mut m4a = Vec::new();
        write(&mut m4a, 2112, 68_000, &sizes, &mut packets.as_slice()).unwrap();

        let top: Vec<&str> = boxes(&m4a).iter().map(|(k, _)| *k).collect();
        assert_eq!(top, ["ftyp", "moov", "mdat"]);
        assert_eq!(find(&m4a, &["mdat"]), packets);

        let mvhd = find(&m4a, &["moov", "mvhd"]);
        assert_eq!(u32s(&mvhd[12..20]), [16_000, 68_000], "timescale and presented duration");
        let mdhd = find(&m4a, &["moov", "trak", "mdia", "mdhd"]);
        assert_eq!(u32s(&mdhd[12..20]), [16_000, 70 * 1024], "media duration counts every frame");
        let elst = find(&m4a, &["moov", "trak", "edts", "elst"]);
        assert_eq!(u32s(&elst[4..]), [1, 68_000, 2112, 0x1_0000], "one edit skipping the priming");

        let stbl = find(&m4a, &["moov", "trak", "mdia", "minf", "stbl"]);
        assert_eq!(u32s(&find(stbl, &["stts"])[4..]), [1, 70, 1024]);
        assert_eq!(u32s(&find(stbl, &["stsc"])[4..]), [2, 1, 64, 1, 2, 6, 1]);
        assert_eq!(u32s(&find(stbl, &["stsz"])[4..12]), [0, 70]);
        assert_eq!(u32s(&find(stbl, &["stsz"])[12..]), sizes);
        let mdat_start = m4a.len() - packets.len();
        let first_chunk: u32 = sizes[..64].iter().sum();
        assert_eq!(
            u32s(&find(stbl, &["stco"])[4..]),
            [2, mdat_start as u32, mdat_start as u32 + first_chunk]
        );
        assert_eq!(
            &find(stbl, &["sgpd"])[4..],
            b"roll\0\0\0\x02\0\0\0\x01\xff\xff",
            "one pre-roll of a packet"
        );
        assert_eq!(u32s(&find(stbl, &["sbgp"])[8..]), [1, 70, 1], "every packet in it");
        let esds = find(stbl, &["stsd"]);
        assert!(esds.windows(4).any(|w| w == [0x05, 0x02, 0x14, 0x08]), "the config is in esds");
    }

    #[test]
    fn a_short_recording_cannot_present_more_than_its_packets_hold() {
        let mut m4a = Vec::new();
        write(&mut m4a, 2112, 5000, &[10, 10, 10], &mut [7u8; 30].as_slice()).unwrap();
        let mvhd = find(&m4a, &["moov", "mvhd"]);
        assert_eq!(u32s(&mvhd[16..20]), [3 * 1024 - 2112]);
        assert!(write(&mut Vec::new(), 2112, 5000, &[10, 10, 10], &mut [7u8; 20].as_slice()).is_err());
    }

    /// The M1b spike's test files (docs/plans/m1b-sermons-on-convex.md): a five-minute and a
    /// two-hour recording whose every second carries two tones naming that second, encoded once
    /// and written as M4A and as bare ADTS, for the players and `ffprobe` to report on.
    ///
    /// `LINGER_SPIKE_DIR=.amp/in/spike cargo test -p linger-core --release --features apple-audio
    /// -- --ignored spike_files`
    #[test]
    #[ignore]
    #[cfg(all(feature = "apple-audio", feature = "native", target_os = "macos"))]
    fn spike_files() {
        use crate::recording::aac;
        use std::io::Write;

        let dir = std::path::PathBuf::from(std::env::var("LINGER_SPIKE_DIR").expect("LINGER_SPIKE_DIR"));
        std::fs::create_dir_all(&dir).unwrap();

        /// Second `s` of the pattern: 0.35 s tones at 0.10 s and 0.55 s whose frequencies
        /// (500 Hz + 40 Hz steps) spell `s = 85 a + b`, so a decoded second says which it is.
        fn second(s: u32, out: &mut Vec<i16>) {
            let rate = SAMPLE_RATE as usize;
            let tones = [
                (500 + 40 * (s / 85), rate / 10, rate * 45 / 100),
                (500 + 40 * (s % 85), rate * 55 / 100, rate * 9 / 10),
            ];
            for n in 0..rate {
                let mut v = 0.0;
                for &(hz, from, to) in &tones {
                    if (from..to).contains(&n) {
                        v = (f64::from(hz) * n as f64 * std::f64::consts::TAU / f64::from(SAMPLE_RATE)).sin()
                            * 0.3
                            * 32767.0;
                    }
                }
                out.push(v.round() as i16);
            }
        }

        for (name, seconds) in [("five", 300u32), ("two-hours", 7200)] {
            let mut pcm = Vec::with_capacity(seconds as usize * SAMPLE_RATE as usize);
            for s in 0..seconds {
                second(s, &mut pcm);
            }
            let spec = hound::WavSpec {
                channels: 1,
                sample_rate: SAMPLE_RATE,
                bits_per_sample: 16,
                sample_format: hound::SampleFormat::Int,
            };
            let mut w = hound::WavWriter::create(dir.join(format!("{name}.wav")), spec).unwrap();
            let mut w16 = w.get_i16_writer(pcm.len() as u32);
            for &s in &pcm {
                w16.write_sample(s);
            }
            w16.flush().unwrap();
            w.finalize().unwrap();

            let mut enc = aac::Encoder::new().unwrap();
            let priming = enc.priming_frames();
            let started = std::time::Instant::now();
            let mut packets = Vec::new();
            for block in pcm.chunks(SAMPLE_RATE as usize / 10) {
                packets.extend(enc.push(block).unwrap());
            }
            packets.extend(enc.finish().unwrap());
            let took = started.elapsed();
            let bytes: usize = packets.iter().map(Vec::len).sum();
            println!(
                "{name}: {seconds} s → {} packets, {bytes} bytes ({:.1} kbit/s), priming {priming} frames, encoded in {:.2} s ({:.0}× real time)",
                packets.len(),
                bytes as f64 * 8.0 / f64::from(seconds) / 1000.0,
                took.as_secs_f64(),
                f64::from(seconds) / took.as_secs_f64()
            );

            let sizes: Vec<u32> = packets.iter().map(|p| p.len() as u32).collect();
            let data = packets.concat();
            let mut m4a =
                std::io::BufWriter::new(std::fs::File::create(dir.join(format!("{name}.m4a"))).unwrap());
            write(&mut m4a, priming, pcm.len() as u64, &sizes, &mut data.as_slice()).unwrap();
            m4a.flush().unwrap();
            let mut adts =
                std::io::BufWriter::new(std::fs::File::create(dir.join(format!("{name}.aac"))).unwrap());
            for p in &packets {
                adts.write_all(&aac::adts_header(p.len())).unwrap();
                adts.write_all(p).unwrap();
            }
            adts.flush().unwrap();
        }
    }
}
