//! AAC-LC encoding through AudioToolbox's `AudioConverter`: the same C API on macOS and iOS, bound
//! by `objc2-audio-toolbox`, so one encoder serves the Mac's recorder thread and the iOS tap.
//!
//! PCM in ([`SAMPLE_RATE`] mono, 16-bit), raw AAC packets out: one [`Encoder`] per recording,
//! because a new encoder adds its priming silence at every join. Every packet holds
//! [`FRAMES_PER_PACKET`] frames; the first [`Encoder::priming_frames`] decoded frames are the
//! encoder's delay, which the muxer trims with an edit list.

use std::ffi::c_void;
use std::ptr::NonNull;

use anyhow::{Result, bail};
use objc2_audio_toolbox::{
    AudioConverterDispose, AudioConverterFillComplexBuffer, AudioConverterGetProperty, AudioConverterNew,
    AudioConverterPrimeInfo, AudioConverterRef, AudioConverterSetProperty, kAudioConverterEncodeBitRate,
    kAudioConverterPrimeInfo, kAudioConverterPropertyMaximumOutputPacketSize,
};
use objc2_core_audio_types::{
    AudioBuffer, AudioBufferList, AudioStreamBasicDescription, AudioStreamPacketDescription,
    kAudioFormatFlagIsPacked, kAudioFormatFlagIsSignedInteger, kAudioFormatLinearPCM, kAudioFormatMPEG4AAC,
};

use super::{FRAMES_PER_PACKET, SAMPLE_RATE};

/// What speech needs at 16 kHz mono: 32 kbps is 14.4 MB an hour. (The encoder lands under it:
/// 27–30 kbit/s measured.)
pub const BIT_RATE: u32 = 32_000;

/// MPEG-4 `samplingFrequencyIndex` for [`SAMPLE_RATE`] (16 kHz).
const SAMPLE_RATE_INDEX: u8 = 8;
/// `channelConfiguration` for mono.
const CHANNEL_CONFIG: u8 = 1;

/// The 7-byte ADTS header that frames one packet of `len` bytes in a part file (no CRC).
pub fn adts_header(len: usize) -> [u8; 7] {
    let frame = (len + 7) as u32;
    [
        0xFF,
        0xF1,                                                        // MPEG-4, layer 0, no CRC
        (1 << 6) | (SAMPLE_RATE_INDEX << 2) | (CHANNEL_CONFIG >> 2), // AAC-LC profile
        ((CHANNEL_CONFIG & 3) << 6) | ((frame >> 11) & 3) as u8,
        ((frame >> 3) & 0xFF) as u8,
        (((frame & 7) << 5) | 0x1F) as u8,
        0xFC,
    ]
}

/// The packets of an ADTS stream (a part file), in order, without their headers.
pub fn adts_packets(stream: &[u8]) -> Result<Vec<&[u8]>> {
    let mut out = Vec::new();
    let mut at = 0;
    while at < stream.len() {
        let h = stream.get(at..at + 7).filter(|h| h[0] == 0xFF && h[1] & 0xF6 == 0xF0);
        let Some(h) = h else { bail!("no ADTS header at byte {at}") };
        let len = (usize::from(h[3] & 3) << 11) | (usize::from(h[4]) << 3) | usize::from(h[5] >> 5);
        let Some(packet) = stream.get(at + 7..at + len).filter(|_| len > 7) else {
            bail!("a truncated ADTS frame at byte {at}")
        };
        out.push(packet);
        at += len;
    }
    Ok(out)
}

/// One continuous AudioToolbox encoder.
pub struct Encoder {
    converter: AudioConverterRef,
    input: Input,
    max_packet: usize,
    priming: u32,
}

// The converter is used from one thread at a time, the one that owns the `Encoder`; AudioToolbox
// doesn't tie it to the thread that created it.
unsafe impl Send for Encoder {}

/// What the input callback hands the converter: samples pushed but not yet consumed.
struct Input {
    pending: Vec<i16>,
    /// Samples of `pending` already handed over.
    cursor: usize,
    /// After `finish`: hand over everything, then signal the end of the stream.
    finishing: bool,
    /// The samples last handed over. The converter may read them until it calls back again,
    /// which can be in a later `drain`, after `pending` has moved; so they are a copy that only
    /// the next callback replaces.
    handed: Vec<i16>,
}

/// Returned by the input callback when the pushed samples are used up: the converter returns it
/// from `AudioConverterFillComplexBuffer` with the packets it could make, and keeps its state.
const NO_INPUT_YET: i32 = i32::from_be_bytes(*b"wait");

impl Encoder {
    /// An AAC-LC encoder for [`SAMPLE_RATE`] mono at [`BIT_RATE`].
    pub fn new() -> Result<Self> {
        let source = AudioStreamBasicDescription {
            mSampleRate: f64::from(SAMPLE_RATE),
            mFormatID: kAudioFormatLinearPCM,
            mFormatFlags: kAudioFormatFlagIsSignedInteger | kAudioFormatFlagIsPacked,
            mBytesPerPacket: 2,
            mFramesPerPacket: 1,
            mBytesPerFrame: 2,
            mChannelsPerFrame: 1,
            mBitsPerChannel: 16,
            mReserved: 0,
        };
        let destination = AudioStreamBasicDescription {
            mSampleRate: f64::from(SAMPLE_RATE),
            mFormatID: kAudioFormatMPEG4AAC,
            mFormatFlags: 0,
            mBytesPerPacket: 0,
            mFramesPerPacket: FRAMES_PER_PACKET,
            mBytesPerFrame: 0,
            mChannelsPerFrame: 1,
            mBitsPerChannel: 0,
            mReserved: 0,
        };
        let mut converter: AudioConverterRef = std::ptr::null_mut();
        // SAFETY: all three pointers are to live locals.
        check("AudioConverterNew", unsafe {
            AudioConverterNew(
                NonNull::from(&source),
                NonNull::from(&destination),
                NonNull::from(&mut converter),
            )
        })?;
        let mut e = Self {
            converter,
            input: Input { pending: Vec::new(), cursor: 0, finishing: false, handed: Vec::new() },
            max_packet: 0,
            priming: 0,
        };
        e.set(kAudioConverterEncodeBitRate, &BIT_RATE)?;
        e.max_packet = e.get::<u32>(kAudioConverterPropertyMaximumOutputPacketSize)? as usize;
        e.priming = e.get::<AudioConverterPrimeInfo>(kAudioConverterPrimeInfo)?.leadingFrames;
        Ok(e)
    }

    /// The encoder's delay: decoded frames before the first pushed sample.
    pub fn priming_frames(&self) -> u32 {
        self.priming
    }

    /// Encodes `samples`, returning the packets that became complete.
    pub fn push(&mut self, samples: &[i16]) -> Result<Vec<Vec<u8>>> {
        self.input.pending.extend_from_slice(samples);
        self.drain()
    }

    /// Flushes the last, padded packet; the encoder is spent afterwards.
    pub fn finish(mut self) -> Result<Vec<Vec<u8>>> {
        self.input.finishing = true;
        self.drain()
    }

    fn drain(&mut self) -> Result<Vec<Vec<u8>>> {
        const WANT: usize = 64;
        let mut out = Vec::new();
        let mut buf = vec![0u8; self.max_packet * WANT];
        let mut descriptions =
            [AudioStreamPacketDescription { mStartOffset: 0, mVariableFramesInPacket: 0, mDataByteSize: 0 };
                WANT];
        loop {
            let mut list = AudioBufferList {
                mNumberBuffers: 1,
                mBuffers: [AudioBuffer {
                    mNumberChannels: 1,
                    mDataByteSize: buf.len() as u32,
                    mData: buf.as_mut_ptr().cast(),
                }],
            };
            let mut n = WANT as u32;
            // SAFETY: the buffer list, the descriptions and the input live for the call, and the
            // callback only reads `self.input`, which nothing else touches meanwhile.
            let status = unsafe {
                AudioConverterFillComplexBuffer(
                    self.converter,
                    Some(supply),
                    (&raw mut self.input).cast(),
                    NonNull::from(&mut n),
                    NonNull::from(&mut list),
                    descriptions.as_mut_ptr(),
                )
            };
            for d in &descriptions[..n as usize] {
                let start = d.mStartOffset as usize;
                out.push(buf[start..start + d.mDataByteSize as usize].to_vec());
            }
            match status {
                NO_INPUT_YET => break,
                0 if n == 0 => break,
                0 => {}
                e => return Err(error("AudioConverterFillComplexBuffer", e)),
            }
        }
        self.input.pending.drain(..self.input.cursor);
        self.input.cursor = 0;
        Ok(out)
    }

    fn set<T>(&mut self, id: u32, value: &T) -> Result<()> {
        // SAFETY: `value` is a live `T` of the size given.
        check("AudioConverterSetProperty", unsafe {
            AudioConverterSetProperty(
                self.converter,
                id,
                std::mem::size_of::<T>() as u32,
                NonNull::from(value).cast(),
            )
        })
    }

    fn get<T: Copy>(&self, id: u32) -> Result<T> {
        let mut value = std::mem::MaybeUninit::<T>::uninit();
        let mut size = std::mem::size_of::<T>() as u32;
        // SAFETY: `value` has room for `size` bytes; AudioToolbox fills it on success.
        check("AudioConverterGetProperty", unsafe {
            AudioConverterGetProperty(
                self.converter,
                id,
                NonNull::from(&mut size),
                NonNull::from(&mut value).cast(),
            )
        })?;
        if size as usize != std::mem::size_of::<T>() {
            bail!("AudioConverterGetProperty: {size} bytes for a {}-byte property", std::mem::size_of::<T>());
        }
        // SAFETY: checked above that the whole value was written.
        Ok(unsafe { value.assume_init() })
    }

    /// The converter's magic cookie: the MPEG-4 ES descriptor holding the AudioSpecificConfig.
    /// A variable-size property, sized first: the converter refuses any other buffer size.
    #[cfg(test)]
    fn magic_cookie(&self) -> Result<Vec<u8>> {
        use objc2_audio_toolbox::{AudioConverterGetPropertyInfo, kAudioConverterCompressionMagicCookie};
        let id = kAudioConverterCompressionMagicCookie;
        let mut size = 0u32;
        let mut writable = 0u8;
        // SAFETY: both out-pointers are to live locals.
        check("AudioConverterGetPropertyInfo", unsafe {
            AudioConverterGetPropertyInfo(self.converter, id, &mut size, &mut writable)
        })?;
        let mut bytes = vec![0u8; size as usize];
        // SAFETY: `bytes` holds `size` bytes.
        check("AudioConverterGetProperty", unsafe {
            AudioConverterGetProperty(
                self.converter,
                id,
                NonNull::from(&mut size),
                NonNull::new(bytes.as_mut_ptr()).expect("a vec's pointer").cast(),
            )
        })?;
        bytes.truncate(size as usize);
        Ok(bytes)
    }
}

impl Drop for Encoder {
    fn drop(&mut self) {
        // SAFETY: created by `AudioConverterNew` and used by nothing after this.
        unsafe { AudioConverterDispose(self.converter) };
    }
}

/// The converter's input callback: hands over pending samples, or says the stream has ended.
unsafe extern "C-unwind" fn supply(
    _: AudioConverterRef,
    io_packets: NonNull<u32>,
    io_data: NonNull<AudioBufferList>,
    _: *mut *mut AudioStreamPacketDescription,
    user: *mut c_void,
) -> i32 {
    // SAFETY: `user` is the `Input` of the encoder in `drain`, which outlives this call.
    let input = unsafe { &mut *user.cast::<Input>() };
    let available = input.pending.len() - input.cursor;
    let (packets, list) = (io_packets.as_ptr(), io_data.as_ptr());
    // SAFETY: the pointers come from `AudioConverterFillComplexBuffer` and are valid for the call.
    let wanted = unsafe { *packets } as usize;
    let give = available.min(wanted);
    if give == 0 && !input.finishing {
        // SAFETY: as above.
        unsafe { *packets = 0 };
        return NO_INPUT_YET;
    }
    // The converter is done with the previous buffer now that it calls again.
    input.handed.clear();
    input.handed.extend_from_slice(&input.pending[input.cursor..input.cursor + give]);
    // SAFETY: as above; `handed` stays as it is until the next call of this callback.
    unsafe {
        *packets = give as u32;
        (*list).mNumberBuffers = 1;
        (*list).mBuffers[0] = AudioBuffer {
            mNumberChannels: 1,
            mDataByteSize: (give * 2) as u32,
            mData: input.handed.as_mut_ptr().cast(),
        };
    }
    input.cursor += give;
    0
}

fn check(call: &str, status: i32) -> Result<()> {
    if status == 0 { Ok(()) } else { Err(error(call, status)) }
}

fn error(call: &str, status: i32) -> anyhow::Error {
    let code = status.to_be_bytes();
    if code.iter().all(|b| b.is_ascii_graphic() || *b == b' ') {
        anyhow::anyhow!("{call} failed: '{}'", String::from_utf8_lossy(&code))
    } else {
        anyhow::anyhow!("{call} failed: {status}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::recording::AUDIO_SPECIFIC_CONFIG;

    fn tone(seconds: u32) -> Vec<i16> {
        (0..SAMPLE_RATE * seconds)
            .map(|i| {
                ((f64::from(i) * 440.0 * std::f64::consts::TAU / f64::from(SAMPLE_RATE)).sin() * 8000.0)
                    as i16
            })
            .collect()
    }

    #[test]
    fn encodes_the_same_packets_however_samples_are_pushed() {
        let pcm = tone(3);
        let mut whole = Encoder::new().unwrap();
        let mut a = whole.push(&pcm).unwrap();
        a.extend(whole.finish().unwrap());

        let mut pieces = Encoder::new().unwrap();
        let mut b = Vec::new();
        for block in pcm.chunks(1000) {
            b.extend(pieces.push(block).unwrap());
        }
        let priming = pieces.priming_frames();
        b.extend(pieces.finish().unwrap());

        assert_eq!(a, b);
        let frames = pcm.len() as u64 + u64::from(priming);
        let packets = a.len() as u64;
        assert!(
            packets * 1024 >= frames && packets * 1024 < frames + 1024,
            "{packets} packets for {frames} frames"
        );
        assert!(a.iter().all(|p| !p.is_empty() && p.len() <= 1536));
    }

    #[test]
    fn the_stream_is_described_as_the_encoder_describes_it() {
        // AudioSpecificConfig: object type 2 (AAC-LC), the sampling index, the channel config.
        let asc =
            [(2 << 3) | (SAMPLE_RATE_INDEX >> 1), ((SAMPLE_RATE_INDEX & 1) << 7) | (CHANNEL_CONFIG << 3)];
        assert_eq!(asc, AUDIO_SPECIFIC_CONFIG);
        let cookie = Encoder::new().unwrap().magic_cookie().unwrap();
        assert!(cookie.windows(2).any(|w| w == AUDIO_SPECIFIC_CONFIG), "{cookie:02x?}");
        assert_eq!(adts_header(200), [0xFF, 0xF1, 0x60, 0x40, 0x19, 0xFF, 0xFC]);
    }
}
