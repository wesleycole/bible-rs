//! Recording on macOS. The microphone is the default input device, read through cpal. The
//! computer's audio is a Core Audio process tap: a private aggregate device whose only input is a
//! mono mixdown of every app's output, whichever device it plays on. Taps arrived in macOS 14.2
//! and ask for "system audio recording" permission rather than screen recording, which
//! ScreenCaptureKit would need. Both deliver f32 at the hardware's rate, which [`Downsample`]
//! turns into 16 kHz.

use std::collections::VecDeque;
use std::ffi::{CStr, c_void};
use std::ptr::{self, NonNull};
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use anyhow::{Context, Result, anyhow, bail};
use block2::RcBlock;
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use linger_core::recording::{SAMPLE_RATE, Source};
use objc2::AllocAnyThread;
use objc2::rc::Retained;
use objc2::runtime::{AnyObject, Bool};
use objc2_app_kit::NSRunningApplication;
use objc2_av_foundation::{AVAuthorizationStatus, AVCaptureDevice, AVMediaTypeAudio};
use objc2_core_audio::{
    AudioDeviceCreateIOProcID, AudioDeviceDestroyIOProcID, AudioDeviceIOProcID, AudioDeviceStart,
    AudioDeviceStop, AudioHardwareCreateAggregateDevice, AudioHardwareDestroyAggregateDevice,
    AudioObjectGetPropertyData, AudioObjectGetPropertyDataSize, AudioObjectID, AudioObjectPropertyAddress,
    CATapDescription, kAudioAggregateDeviceIsPrivateKey, kAudioAggregateDeviceNameKey,
    kAudioAggregateDeviceTapAutoStartKey, kAudioAggregateDeviceTapListKey, kAudioAggregateDeviceUIDKey,
    kAudioHardwarePropertyProcessObjectList, kAudioObjectPropertyElementMain,
    kAudioObjectPropertyScopeGlobal, kAudioObjectSystemObject, kAudioProcessPropertyIsRunningOutput,
    kAudioProcessPropertyPID, kAudioSubTapDriftCompensationKey, kAudioSubTapUIDKey, kAudioTapPropertyFormat,
};
use objc2_core_audio_types::{
    AudioBuffer, AudioBufferList, AudioStreamBasicDescription, AudioTimeStamp, kAudioFormatFlagIsFloat,
    kAudioFormatLinearPCM,
};
use objc2_core_foundation::CFDictionary;
use objc2_foundation::{NSArray, NSDictionary, NSNumber, NSString, NSUUID};

pub fn supported(source: Source) -> bool {
    match source {
        Source::Microphone => true,
        Source::SystemAudio => tap_api().is_some(),
        Source::File => false,
    }
}

/// The answer to a permission prompt, stored by the prompt's callback.
const ASKING: u8 = 0;
const ALLOWED: u8 = 1;
const REFUSED: u8 = 2;

/// How long [`Capture::read`] waits before checking on permission or the device again.
const TICK: Duration = Duration::from_millis(250);

/// Mono samples at the device's rate, or why capture stopped.
type Block = Result<Vec<f32>, String>;

pub struct Capture {
    source: Source,
    /// None until the user allows the source. Capture stops when it drops.
    input: Option<Input>,
    tx: Sender<Block>,
    blocks: Receiver<Block>,
    consent: Consent,
    down: Downsample,
    ready: VecDeque<i16>,
}

/// Held only to be dropped.
#[expect(dead_code, reason = "the fields are read by their Drop impls")]
enum Input {
    Microphone(cpal::Stream),
    System(Tap),
}

/// Fails at once if the user already refused the source. Otherwise the first time asks, and
/// doesn't wait for the answer: `Recording::start` blocks the window until `open` returns, and the
/// prompt can sit there a while. [`Capture::read`] opens the device once the user allows it.
pub fn open(source: Source) -> Result<Capture> {
    let consent = match source {
        Source::Microphone => Consent::microphone()?,
        Source::SystemAudio => {
            tap_api().context("recording this computer's audio needs macOS 14.2 or later")?;
            Consent::audio_capture()?
        }
        Source::File => bail!("files are imported, not captured"),
    };
    let (tx, blocks) = mpsc::channel();
    let mut capture = Capture {
        source,
        input: None,
        tx,
        blocks,
        consent,
        down: Downsample::new(SAMPLE_RATE),
        ready: VecDeque::new(),
    };
    // Already allowed: open the device now, so a problem with it fails the start.
    if capture.consent.answer() == ALLOWED {
        capture.start()?;
    }
    Ok(capture)
}

impl Capture {
    /// Fills `out`, or returns false without filling it while macOS is still asking the user for
    /// permission, so the caller can check whether to stop. The recording starts at the answer.
    pub fn read(&mut self, out: &mut [i16]) -> Result<bool> {
        if self.input.is_none() {
            match self.consent.answer() {
                ASKING => {
                    std::thread::sleep(TICK);
                    return Ok(false);
                }
                REFUSED => bail!(self.consent.refused),
                _ => self.start()?,
            }
        }
        let mut waited = Duration::ZERO;
        while self.ready.len() < out.len() {
            match self.blocks.recv_timeout(TICK) {
                Ok(block) => {
                    waited = Duration::ZERO;
                    for x in block.map_err(|e| anyhow!("recording failed: {e}"))? {
                        self.down.push(x, &mut self.ready);
                    }
                }
                Err(_) => {
                    waited += TICK;
                    if waited >= Duration::from_secs(2) {
                        bail!("the audio device stopped sending audio");
                    }
                }
            }
        }
        let n = out.len();
        for (s, r) in out.iter_mut().zip(self.ready.drain(..n)) {
            *s = r;
        }
        Ok(true)
    }

    fn start(&mut self) -> Result<()> {
        let (input, rate) = match self.source {
            Source::SystemAudio => {
                let (tap, rate) = Tap::open(self.tx.clone())?;
                (Input::System(tap), rate)
            }
            _ => {
                let (stream, rate) = open_microphone(self.tx.clone())?;
                (Input::Microphone(stream), rate)
            }
        };
        self.input = Some(input);
        self.down = Downsample::new(rate);
        Ok(())
    }
}

/// The user's answer for a source: [`ASKING`], [`ALLOWED`] or [`REFUSED`].
struct Consent {
    answer: Arc<AtomicU8>,
    refused: &'static str,
}

impl Consent {
    fn answer(&self) -> u8 {
        self.answer.load(Ordering::Relaxed)
    }

    /// AVFoundation's answer for the microphone.
    fn microphone() -> Result<Consent> {
        let consent = Consent {
            answer: Arc::new(AtomicU8::new(ALLOWED)),
            refused: "Linger isn't allowed to use the microphone. Turn it on in System Settings → \
                      Privacy & Security → Microphone, then record again.",
        };
        // SAFETY: AVFoundation defines the constant, and both calls take any thread.
        unsafe {
            let audio = AVMediaTypeAudio.context("AVFoundation has no audio media type")?;
            match AVCaptureDevice::authorizationStatusForMediaType(audio) {
                AVAuthorizationStatus::Authorized => {}
                AVAuthorizationStatus::NotDetermined => {
                    consent.answer.store(ASKING, Ordering::Relaxed);
                    let answer = consent.answer.clone();
                    let handler = RcBlock::new(move |ok: Bool| {
                        answer.store(if ok.as_bool() { ALLOWED } else { REFUSED }, Ordering::Relaxed);
                    });
                    AVCaptureDevice::requestAccessForMediaType_completionHandler(audio, &handler);
                }
                _ => bail!(consent.refused),
            }
        }
        Ok(consent)
    }

    /// TCC's answer for "System Audio Recording". macOS has no public way to ask for it, only to
    /// have a running tap trigger the prompt, and a tap made before the answer stays silent after
    /// it. So this asks through the private TCC framework, as other tap-based recorders do:
    /// `TCCAccessPreflight` (0 allowed, 1 refused, else not asked) and `TCCAccessRequest`. The
    /// preflight answer is cached for the life of the process, so it's read only here. Without the
    /// framework the tap opens at once and macOS asks then.
    fn audio_capture() -> Result<Consent> {
        let consent = Consent {
            answer: Arc::new(AtomicU8::new(ALLOWED)),
            refused: "Linger isn't allowed to record this computer's audio. Turn it on in System \
                      Settings → Privacy & Security → Screen & System Audio Recording, then record \
                      again.",
        };
        let Some(tcc) = tcc_api() else { return Ok(consent) };
        let service = NSString::from_str("kTCCServiceAudioCapture");
        // SAFETY: an NSString is a CFString; no options are allowed; TCC copies the block and calls
        // it once, on its own queue.
        unsafe {
            match (tcc.preflight)(&*service, ptr::null()) {
                0 => {}
                1 => bail!(consent.refused),
                _ => {
                    consent.answer.store(ASKING, Ordering::Relaxed);
                    let answer = consent.answer.clone();
                    let handler = RcBlock::new(move |granted: u8| {
                        answer.store(if granted != 0 { ALLOWED } else { REFUSED }, Ordering::Relaxed);
                    });
                    (tcc.request)(&*service, ptr::null(), &*handler);
                }
            }
        }
        Ok(consent)
    }
}

/// `TCCAccessPreflight` and `TCCAccessRequest` from the private TCC framework.
struct TccApi {
    preflight: Preflight,
    request: Request,
}

type Preflight = unsafe extern "C" fn(*const NSString, *const c_void) -> i32;
type Request = unsafe extern "C" fn(*const NSString, *const c_void, *const block2::Block<dyn Fn(u8)>);

fn tcc_api() -> Option<&'static TccApi> {
    static API: OnceLock<Option<TccApi>> = OnceLock::new();
    API.get_or_init(|| {
        // SAFETY: dlopen and dlsym take any NUL-terminated strings; both functions have had these
        // signatures since TCC began.
        unsafe {
            let tcc = libc::dlopen(
                c"/System/Library/PrivateFrameworks/TCC.framework/TCC".as_ptr(),
                libc::RTLD_LAZY,
            );
            if tcc.is_null() {
                return None;
            }
            let preflight = libc::dlsym(tcc, c"TCCAccessPreflight".as_ptr());
            let request = libc::dlsym(tcc, c"TCCAccessRequest".as_ptr());
            (!preflight.is_null() && !request.is_null()).then(|| TccApi {
                preflight: std::mem::transmute::<*mut c_void, Preflight>(preflight),
                request: std::mem::transmute::<*mut c_void, Request>(request),
            })
        }
    })
    .as_ref()
}

fn open_microphone(tx: Sender<Block>) -> Result<(cpal::Stream, u32)> {
    let device = cpal::default_host().default_input_device().context("this Mac has no microphone")?;
    let config = device.default_input_config().context("couldn't read the microphone's format")?;
    let channels = usize::from(config.channels());
    // macOS hands input over as f32 whatever the hardware's format. Keep the device's own rate and
    // channel count: asking for 16 kHz would change the rate for every app using it.
    let errors = tx.clone();
    let stream = device
        .build_input_stream::<f32, _, _>(
            config.config(),
            move |data, _| {
                let mono = data.chunks(channels).map(|f| f.iter().sum::<f32>() / channels as f32);
                let _ = tx.send(Ok(mono.collect()));
            },
            move |e| {
                use cpal::ErrorKind::*;
                // The device switched underneath us or a buffer was late: capture goes on.
                if !matches!(e.kind(), DeviceChanged | Xrun | RealtimeDenied) {
                    let _ = errors.send(Err(e.to_string()));
                }
            },
            None,
        )
        .context("couldn't open the microphone")?;
    stream.play().context("couldn't start the microphone")?;
    Ok((stream, config.sample_rate()))
}

/// `AudioHardwareCreateProcessTap` and `AudioHardwareDestroyProcessTap`, which are new in macOS
/// 14.2. build.rs links Core Audio weakly so they're null on older macOS, but Rust takes a
/// function's address to be non-null, so they're looked up at run time instead.
struct TapApi {
    create: CreateTap,
    destroy: DestroyTap,
}

type CreateTap = unsafe extern "C" fn(*const CATapDescription, *mut AudioObjectID) -> i32;
type DestroyTap = unsafe extern "C" fn(AudioObjectID) -> i32;

fn tap_api() -> Option<&'static TapApi> {
    static API: OnceLock<Option<TapApi>> = OnceLock::new();
    API.get_or_init(|| {
        // SAFETY: dlsym takes any NUL-terminated name, and Core Audio declares both functions
        // with exactly these signatures.
        unsafe {
            let create = libc::dlsym(libc::RTLD_DEFAULT, c"AudioHardwareCreateProcessTap".as_ptr());
            let destroy = libc::dlsym(libc::RTLD_DEFAULT, c"AudioHardwareDestroyProcessTap".as_ptr());
            (!create.is_null() && !destroy.is_null()).then(|| TapApi {
                create: std::mem::transmute::<*mut c_void, CreateTap>(create),
                destroy: std::mem::transmute::<*mut c_void, DestroyTap>(destroy),
            })
        }
    })
    .as_ref()
}

/// A tap of everything the computer plays and the private aggregate device that reads it. Each
/// part is torn down on drop, including after a failed [`open`](Tap::open).
struct Tap {
    tap: AudioObjectID,
    device: AudioObjectID,
    proc_id: AudioDeviceIOProcID,
    /// The IO proc's client data, freed once the proc is gone.
    tx: *mut Sender<Block>,
}

impl Tap {
    fn open(tx: Sender<Block>) -> Result<(Self, u32)> {
        let api = tap_api().context("recording this computer's audio needs macOS 14.2 or later")?;
        let mut this = Tap { tap: 0, device: 0, proc_id: None, tx: Box::into_raw(Box::new(tx)) };
        // SAFETY: every pointer handed to Core Audio is valid for the call; `this.tx` outlives the
        // IO proc because Drop stops and destroys the proc before freeing it.
        unsafe {
            // A mono mixdown of every process. Excluding none is fine: Linger plays no audio of
            // its own while recording. Private keeps the tap out of other apps' device lists.
            let empty: Retained<NSArray<NSNumber>> = NSArray::new();
            let desc =
                CATapDescription::initMonoGlobalTapButExcludeProcesses(CATapDescription::alloc(), &empty);
            desc.setName(&NSString::from_str("Linger sermon recording"));
            desc.setPrivate(true);
            check((api.create)(&*desc, &mut this.tap), "couldn't tap the computer's audio")?;

            let format: AudioStreamBasicDescription = property(this.tap, kAudioTapPropertyFormat)
                .context("couldn't read the computer's audio format")?;
            if format.mFormatID != kAudioFormatLinearPCM
                || format.mFormatFlags & kAudioFormatFlagIsFloat == 0
                || format.mBitsPerChannel != 32
            {
                bail!("the computer's audio arrives in a format Linger can't read");
            }

            let tap_list = dict(&[
                (kAudioSubTapUIDKey, &*desc.UUID().UUIDString()),
                (kAudioSubTapDriftCompensationKey, &*NSNumber::new_bool(true)),
            ]);
            let aggregate = dict(&[
                (kAudioAggregateDeviceNameKey, &*NSString::from_str("Linger")),
                (kAudioAggregateDeviceUIDKey, &*NSUUID::new().UUIDString()),
                (kAudioAggregateDeviceIsPrivateKey, &*NSNumber::new_bool(true)),
                (kAudioAggregateDeviceTapAutoStartKey, &*NSNumber::new_bool(true)),
                (kAudioAggregateDeviceTapListKey, &*NSArray::from_retained_slice(&[tap_list])),
            ]);
            let aggregate = &*(Retained::as_ptr(&aggregate) as *const CFDictionary);
            check(
                AudioHardwareCreateAggregateDevice(aggregate, NonNull::from(&mut this.device)),
                "couldn't create a device to read the computer's audio",
            )?;
            check(
                AudioDeviceCreateIOProcID(
                    this.device,
                    Some(tap_io),
                    this.tx.cast(),
                    NonNull::from(&mut this.proc_id),
                ),
                "couldn't read the computer's audio",
            )?;
            check(
                AudioDeviceStart(this.device, this.proc_id),
                "couldn't start reading the computer's audio",
            )?;
            Ok((this, format.mSampleRate as u32))
        }
    }
}

impl Drop for Tap {
    fn drop(&mut self) {
        // SAFETY: each id was created by `open` (zero or None if it never got that far), and the
        // IO proc can't run once AudioDeviceStop returns and the proc is destroyed.
        unsafe {
            if self.proc_id.is_some() {
                AudioDeviceStop(self.device, self.proc_id);
                AudioDeviceDestroyIOProcID(self.device, self.proc_id);
            }
            if self.device != 0 {
                AudioHardwareDestroyAggregateDevice(self.device);
            }
            if self.tap != 0
                && let Some(api) = tap_api()
            {
                (api.destroy)(self.tap);
            }
            drop(Box::from_raw(self.tx));
        }
    }
}

/// Core Audio's IO thread: the aggregate device's input is the tap alone, as f32 in one or more
/// buffers (interleaved or not). Mixed down to mono in case macOS hands over more than one channel.
unsafe extern "C-unwind" fn tap_io(
    _device: AudioObjectID,
    _now: NonNull<AudioTimeStamp>,
    input: NonNull<AudioBufferList>,
    _input_time: NonNull<AudioTimeStamp>,
    _output: NonNull<AudioBufferList>,
    _output_time: NonNull<AudioTimeStamp>,
    client: *mut c_void,
) -> i32 {
    // SAFETY: `client` is the Tap's sender, alive while the proc is; the buffer list holds
    // `mNumberBuffers` buffers of f32 (checked in `Tap::open`).
    unsafe {
        let tx = &*client.cast::<Sender<Block>>();
        let list = input.as_ref();
        let buffers: &[AudioBuffer] =
            std::slice::from_raw_parts(list.mBuffers.as_ptr(), list.mNumberBuffers as usize);
        let channels: u32 = buffers.iter().map(|b| b.mNumberChannels).sum();
        let frames = buffers
            .iter()
            .filter(|b| b.mNumberChannels > 0)
            .map(|b| b.mDataByteSize as usize / 4 / b.mNumberChannels as usize)
            .min()
            .unwrap_or(0);
        if frames == 0 || channels == 0 {
            return 0;
        }
        let mut mono = vec![0f32; frames];
        for b in buffers.iter().filter(|b| b.mNumberChannels > 0 && !b.mData.is_null()) {
            let n = b.mNumberChannels as usize;
            let data = std::slice::from_raw_parts(b.mData.cast::<f32>(), frames * n);
            for (m, frame) in mono.iter_mut().zip(data.chunks_exact(n)) {
                *m += frame.iter().sum::<f32>();
            }
        }
        for m in &mut mono {
            *m /= channels as f32;
        }
        let _ = tx.send(Ok(mono));
    }
    0
}

/// An NSDictionary with Core Audio's C-string keys.
fn dict(entries: &[(&CStr, &AnyObject)]) -> Retained<NSDictionary<NSString, AnyObject>> {
    let keys: Vec<Retained<NSString>> = entries
        .iter()
        .map(|(k, _)| NSString::from_str(k.to_str().expect("Core Audio keys are ASCII")))
        .collect();
    let keys: Vec<&NSString> = keys.iter().map(|k| &**k).collect();
    let values: Vec<&AnyObject> = entries.iter().map(|(_, v)| *v).collect();
    NSDictionary::from_slices(&keys, &values)
}

/// Reads a fixed-size property of a Core Audio object.
unsafe fn property<T>(object: AudioObjectID, selector: u32) -> Result<T> {
    let address = AudioObjectPropertyAddress {
        mSelector: selector,
        mScope: kAudioObjectPropertyScopeGlobal,
        mElement: kAudioObjectPropertyElementMain,
    };
    let mut value = std::mem::MaybeUninit::<T>::zeroed();
    let mut size = size_of::<T>() as u32;
    // SAFETY: `value` has room for `size` bytes, and the caller names a property of type T.
    unsafe {
        check(
            AudioObjectGetPropertyData(
                object,
                NonNull::from(&address),
                0,
                ptr::null(),
                NonNull::from(&mut size),
                NonNull::from(&mut value).cast(),
            ),
            "Core Audio didn't answer",
        )?;
        Ok(value.assume_init())
    }
}

/// Reads a property that is a list of Core Audio objects.
unsafe fn object_list(object: AudioObjectID, selector: u32) -> Result<Vec<AudioObjectID>> {
    let address = AudioObjectPropertyAddress {
        mSelector: selector,
        mScope: kAudioObjectPropertyScopeGlobal,
        mElement: kAudioObjectPropertyElementMain,
    };
    let mut size = 0u32;
    // SAFETY: the list has room for `size` bytes, as Core Audio reported.
    unsafe {
        check(
            AudioObjectGetPropertyDataSize(
                object,
                NonNull::from(&address),
                0,
                ptr::null(),
                NonNull::from(&mut size),
            ),
            "Core Audio didn't answer",
        )?;
        let mut list = vec![0 as AudioObjectID; size as usize / size_of::<AudioObjectID>()];
        let mut size = (list.len() * size_of::<AudioObjectID>()) as u32;
        check(
            AudioObjectGetPropertyData(
                object,
                NonNull::from(&address),
                0,
                ptr::null(),
                NonNull::from(&mut size),
                NonNull::new(list.as_mut_ptr()).expect("a Vec's pointer").cast(),
            ),
            "Core Audio didn't answer",
        )?;
        list.truncate(size as usize / size_of::<AudioObjectID>());
        Ok(list)
    }
}

/// The apps playing sound right now, by name: "Google Chrome". A helper process, such as a
/// browser's audio service or Safari's WebKit, is named by the app it works for.
pub fn playing_apps() -> Vec<String> {
    let me = std::process::id() as libc::pid_t;
    let mut names = Vec::new();
    // SAFETY: the system object and the process objects it lists answer these properties, with
    // these types.
    unsafe {
        let Ok(processes) =
            object_list(kAudioObjectSystemObject as u32, kAudioHardwarePropertyProcessObjectList)
        else {
            return names;
        };
        for p in processes {
            if property::<u32>(p, kAudioProcessPropertyIsRunningOutput).unwrap_or(0) == 0 {
                continue;
            }
            let Ok(pid) = property::<libc::pid_t>(p, kAudioProcessPropertyPID) else { continue };
            if pid == me {
                continue;
            }
            if let Some(name) = app_name(pid)
                && !names.contains(&name)
            {
                names.push(name);
            }
        }
    }
    names
}

/// The name of the app a process belongs to.
fn app_name(pid: libc::pid_t) -> Option<String> {
    type Responsible = unsafe extern "C" fn(libc::pid_t) -> libc::pid_t;
    static RESPONSIBLE: OnceLock<Option<Responsible>> = OnceLock::new();
    // The process macOS holds responsible for a helper: the app it works for. Private, but it is
    // what Activity Monitor groups processes by; without it a helper is named as itself.
    let responsible = RESPONSIBLE.get_or_init(|| {
        // SAFETY: dlsym takes any NUL-terminated name; the function takes and returns a pid.
        unsafe {
            let f = libc::dlsym(libc::RTLD_DEFAULT, c"responsibility_get_pid_responsible_for_pid".as_ptr());
            (!f.is_null()).then(|| std::mem::transmute::<*mut c_void, Responsible>(f))
        }
    });
    // SAFETY: the function only looks the pid up.
    let owner = responsible.map(|f| unsafe { f(pid) }).filter(|&o| o > 0).unwrap_or(pid);
    let app = NSRunningApplication::runningApplicationWithProcessIdentifier(owner)?;
    let name = app.localizedName()?.to_string();
    // A helper that has no responsible app: "Google Chrome Helper (Renderer)" is Google Chrome.
    Some(name.split(" Helper").next().unwrap_or(&name).trim().to_string())
}

fn check(status: i32, what: &str) -> Result<()> {
    if status == 0 { Ok(()) } else { Err(anyhow!("{what} (Core Audio error {status})")) }
}

/// Converts the device's rate (usually 48 or 44.1 kHz) to 16 kHz by averaging the input samples
/// that fall in each output sample's span. The average doubles as the low-pass filter against
/// aliasing, which is plenty for speech headed to transcription.
pub(super) struct Downsample {
    /// Input samples per output sample.
    step: f64,
    pos: f64,
    sum: f32,
    n: u32,
}

impl Downsample {
    pub fn new(rate: u32) -> Self {
        Self { step: f64::from(rate) / f64::from(SAMPLE_RATE), pos: 0.0, sum: 0.0, n: 0 }
    }

    pub fn push(&mut self, x: f32, out: &mut VecDeque<i16>) {
        self.sum += x;
        self.n += 1;
        self.pos += 1.0;
        if self.pos >= self.step {
            let v = (self.sum / self.n as f32).clamp(-1.0, 1.0) * f32::from(i16::MAX);
            // Below 16 kHz one input sample covers more than one output sample.
            while self.pos >= self.step {
                out.push_back(v as i16);
                self.pos -= self.step;
            }
            self.sum = 0.0;
            self.n = 0;
        }
    }
}
