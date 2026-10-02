use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use hound::{WavSpec, WavWriter};
use std::io::Cursor;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::thread;

enum AudioCommand {
    /// Device name (None = system default); replies Ok(true) when the named device was
    /// missing and the default was used instead.
    Start(Option<String>, Option<StreamInput>, u32, Sender<Result<bool, String>>),
    /// Skip this many ms at the start (the start chime) and hand the samples back.
    Stop(u32, Sender<Result<Recording, String>>),
    Cancel,
}

/// Newly captured mono audio, at the microphone's native sample rate.
pub struct AudioChunk {
    pub samples: Vec<f32>,
    pub rate: u32,
}

#[derive(Clone)]
pub struct StreamInput {
    pub tx: tokio::sync::mpsc::Sender<AudioChunk>,
    /// A full queue invalidates the live transcript rather than silently losing speech.
    pub overflowed: Arc<AtomicBool>,
}

struct StreamFeed {
    input: Option<StreamInput>,
    rate: u32,
    skip: usize,
    generation: u64,
}

impl StreamFeed {
    fn push(&mut self, samples: &[f32]) {
        let Some(input) = self.input.as_ref() else { return };
        if input.overflowed.load(Ordering::Acquire) {
            self.input = None;
            return;
        }
        let skip = self.skip.min(samples.len());
        self.skip -= skip;
        let samples = &samples[skip..];
        if samples.is_empty() {
            return;
        }
        // The audio callback must never wait for a network consumer.
        match input.tx.try_send(AudioChunk { samples: samples.to_vec(), rate: self.rate }) {
            Ok(()) => {}
            Err(tokio::sync::mpsc::error::TrySendError::Full(_)) => {
                input.overflowed.store(true, Ordering::Release);
                self.input = None;
            }
            Err(tokio::sync::mpsc::error::TrySendError::Closed(_)) => self.input = None,
        }
    }
}

/// Raw mono samples from a finished recording; `process` turns them into the upload.
pub struct Recording {
    pub samples: Vec<f32>,
    pub rate: u32,
    /// Partial audio remains available when a microphone disconnects or the stream fails.
    pub interruption: Option<String>,
}

#[derive(Default)]
struct CaptureState {
    samples: Vec<f32>,
    generation: u64,
    active: bool,
    interruption: Option<String>,
}

impl CaptureState {
    fn begin(&mut self) -> u64 {
        self.generation = self.generation.wrapping_add(1);
        self.active = true;
        self.samples.clear();
        self.interruption = None;
        self.generation
    }

    fn interrupt(&mut self, generation: u64, error: String) -> bool {
        if !self.active || self.generation != generation || self.interruption.is_some() { return false; }
        self.interruption = Some(error);
        true
    }

    fn finish(&mut self, rate: u32, skip_ms: u32) -> Result<Recording, String> {
        self.active = false;
        let mut samples = std::mem::take(&mut self.samples);
        if samples.is_empty() && self.interruption.is_none() {
            return Err("No audio samples recorded".into());
        }
        // A short healthy hold trimmed to nothing remains a successful no-speech recording.
        let skip = (rate as usize * skip_ms as usize / 1000).min(samples.len());
        samples.drain(..skip);
        Ok(Recording { samples, rate, interruption: self.interruption.clone() })
    }

    fn cancel(&mut self) {
        self.active = false;
        self.samples.clear();
        self.interruption = None;
    }
}

impl Recording {
    /// Noise reduction + speech check + 16 kHz WAV. CPU-heavy: run it off the audio thread and
    /// outside any lock (lib.rs does it in spawn_blocking).
    pub fn process(self) -> Result<(Vec<u8>, bool), String> {
        let (clean, has_speech) = denoise(&self.samples, self.rate);
        Ok((encode_wav(&clean, OUTPUT_RATE)?, has_speech))
    }
}

/// Output rate of `denoise`: what Whisper uses internally, and 3x smaller uploads than 48 kHz.
pub const OUTPUT_RATE: u32 = 16_000;

/// Resamples mono audio (linear interpolation; ponytail: fine for speech, not for music).
pub(crate) fn resample(samples: &[f32], from: u32, to: u32) -> Vec<f32> {
    if from == to || samples.is_empty() {
        return samples.to_vec();
    }
    let len = (samples.len() as u64 * to as u64 / from as u64) as usize;
    let step = from as f64 / to as f64;
    (0..len)
        .map(|i| {
            let pos = i as f64 * step;
            let (idx, frac) = (pos as usize, (pos.fract()) as f32);
            let a = samples[idx.min(samples.len() - 1)];
            let b = samples[(idx + 1).min(samples.len() - 1)];
            a + (b - a) * frac
        })
        .collect()
}

/// RNNoise at 48 kHz, then 16 kHz out. Returns the audio and whether speech was detected:
/// 150 ms in a row with voice probability > 0.6. Steady noise only scores in scattered frames
/// (hiss at -35 dBFS: runs of ≤ 10 in 3 s), while even "yes" is one run of ~330 ms.
pub fn denoise(samples: &[f32], rate: u32) -> (Vec<f32>, bool) {
    use nnnoiseless::DenoiseState;
    const FRAME: usize = DenoiseState::FRAME_SIZE;
    let input = resample(samples, rate, 48_000);
    let mut state = DenoiseState::new();
    let (mut frame_in, mut frame_out) = ([0f32; FRAME], [0f32; FRAME]);
    let mut out = Vec::with_capacity(input.len());
    let (mut run, mut longest) = (0, 0);
    for chunk in input.chunks(FRAME) {
        frame_in.fill(0.0);
        for (d, s) in frame_in.iter_mut().zip(chunk) {
            *d = s * 32767.0;
        }
        run = if state.process_frame(&mut frame_out, &frame_in) > 0.6 { run + 1 } else { 0 };
        longest = longest.max(run);
        out.extend(frame_out[..chunk.len()].iter().map(|x| x / 32767.0));
    }
    (resample(&out, 48_000, OUTPUT_RATE), longest >= 15)
}

fn encode_wav(samples: &[f32], sample_rate: u32) -> Result<Vec<u8>, String> {
    let spec = WavSpec { channels: 1, sample_rate, bits_per_sample: 16, sample_format: hound::SampleFormat::Int };
    let mut cursor = Cursor::new(Vec::new());
    let mut writer = WavWriter::new(&mut cursor, spec).map_err(|e| format!("WAV writer error: {}", e))?;
    for &sample in samples {
        writer.write_sample((sample.clamp(-1.0, 1.0) * 32767.0) as i16).map_err(|e| e.to_string())?;
    }
    writer.finalize().map_err(|e| e.to_string())?;
    Ok(cursor.into_inner())
}

/// Seconds of audio per loudness update.
const LEVEL_HOP_S: f32 = 0.016;
/// Strips DC offset and handling rumble before measuring.
const HIGH_PASS_HZ: f32 = 80.0;
/// The noise floor drops quickly into pauses and creeps up under steady noise.
const FLOOR_FALL_S: f32 = 0.5;
const FLOOR_RISE_S: f32 = 5.0;
/// The ceiling catches a louder voice within a syllable, then relaxes over a few seconds.
const CEIL_RISE_S: f32 = 0.08;
const CEIL_FALL_S: f32 = 2.5;
/// Smallest floor-to-ceiling span: at least 0.004 linear RMS (about -48 dBFS) and at least the
/// floor itself (6 dB), so room noise, even a loud fan, never fills the bar.
const MIN_RANGE: f32 = 0.004;
/// Hops (~320 ms) a freshly seeded floor falls fast: the mic may open mid-word, seeding it on voice.
const SETTLE_HOPS: u32 = 20;
/// Levels below this ease towards zero, flattening noise flicker.
const KNEE: f32 = 0.1;

pub struct AudioRecorder {
    tx: Sender<AudioCommand>,
    /// Latest display loudness (0..1, f32 bits), written by the audio callback.
    level: Arc<AtomicU32>,
    capture: Arc<Mutex<CaptureState>>,
}

/// Auto-gained loudness for the dictation bar: ~0 for room tone, towards 1 for speech at any
/// normal volume. Display only; recorded samples are untouched.
struct Loudness {
    hop: usize,
    /// One-pole high-pass: coefficient, previous input and output.
    hp: f32,
    hp_in: f32,
    hp_out: f32,
    /// Sum of squares and sample count in the current hop.
    sum: f32,
    count: usize,
    /// Per-hop smoothing factors for the time constants above.
    floor_fall: f32,
    floor_rise: f32,
    ceil_rise: f32,
    ceil_fall: f32,
    /// Unknown until the first full hop of signal (digital silence says nothing about the room).
    floor: Option<f32>,
    /// One hop with signal seen since digital silence: it may be partly zeros, so it can't seed `floor`.
    primed: bool,
    /// Hops since `floor` was seeded (see SETTLE_HOPS).
    age: u32,
    ceil: f32,
}

impl Loudness {
    fn new(rate: u32) -> Self {
        let rate = rate.max(1) as f32;
        let hop = ((rate * LEVEL_HOP_S) as usize).max(1);
        let keep = |seconds: f32| (-(hop as f32) / (rate * seconds)).exp();
        Self {
            hop,
            hp: 1.0 / (1.0 + std::f32::consts::TAU * HIGH_PASS_HZ / rate),
            hp_in: 0.0,
            hp_out: 0.0,
            sum: 0.0,
            count: 0,
            floor_fall: keep(FLOOR_FALL_S),
            floor_rise: keep(FLOOR_RISE_S),
            ceil_rise: keep(CEIL_RISE_S),
            ceil_fall: keep(CEIL_FALL_S),
            floor: None,
            primed: false,
            age: 0,
            ceil: 0.0,
        }
    }

    fn push(&mut self, samples: &[f32], level: &AtomicU32) {
        for &x in samples {
            let x = if x.is_finite() { x } else { 0.0 };
            self.hp_out = self.hp * (self.hp_out + x - self.hp_in);
            self.hp_in = x;
            self.sum += self.hp_out * self.hp_out;
            self.count += 1;
            if self.count == self.hop {
                let rms = (self.sum / self.hop as f32).sqrt();
                (self.sum, self.count) = (0.0, 0);
                level.store(self.update(rms).to_bits(), Ordering::Relaxed);
            }
        }
    }

    fn update(&mut self, rms: f32) -> f32 {
        if rms < 1e-6 {
            (self.floor, self.primed) = (None, false);
            return 0.0;
        }
        // The first hop after silence can be mostly zeros: seeding the floor that low would show
        // room tone as a voice for seconds while the floor creeps up.
        if self.floor.is_none() && !std::mem::replace(&mut self.primed, true) {
            return 0.0;
        }
        let ease = |from: f32, keep: f32| rms + (from - rms) * keep;
        if self.floor.is_none() {
            self.age = 0;
        }
        self.age = self.age.saturating_add(1);
        // Settling: both bounds fall fast, so a floor seeded on a voice (and the ceiling it pushed
        // up) drop to the room and the voice's real peaks within a syllable or two.
        let settling = self.age <= SETTLE_HOPS;
        let floor = self.floor.get_or_insert(rms);
        *floor = ease(*floor, if rms >= *floor { self.floor_rise } else if settling { self.ceil_rise } else { self.floor_fall });
        let floor = *floor;
        let ceil_keep = if rms > self.ceil || settling { self.ceil_rise } else { self.ceil_fall };
        self.ceil = ease(self.ceil, ceil_keep).max(floor + MIN_RANGE.max(floor));
        let level = ((rms - floor) / (self.ceil - floor)).clamp(0.0, 1.0);
        level * (level / KNEE).min(1.0)
    }
}

/// Names of the available input devices (cpal names, as stored in `input_device`).
pub fn list_input_devices() -> Vec<String> {
    cpal::default_host()
        .input_devices()
        .map(|devices| devices.filter_map(|d| d.name().ok()).collect())
        .unwrap_or_default()
}

/// Calls `changed` on its own thread whenever audio devices come or go (a burst of changes is
/// one call, once things settle). macOS: CoreAudio notifies; elsewhere the lists refresh on
/// focus and when the menu bar menu opens.
#[cfg(target_os = "macos")]
pub fn watch_devices(mut changed: impl FnMut() + Send + 'static) {
    use crate::output::platform::{fourcc, Address, SYSTEM_OBJECT};
    use std::ffi::c_void;
    use std::time::Duration;

    type Listener = extern "C" fn(u32, u32, *const Address, *mut c_void) -> i32;
    #[link(name = "CoreAudio", kind = "framework")]
    extern "C" {
        fn AudioObjectAddPropertyListener(object: u32, address: *const Address, listener: Listener, client: *mut c_void) -> i32;
    }
    extern "C" fn notify(_: u32, _: u32, _: *const Address, client: *mut c_void) -> i32 {
        let _ = unsafe { &*(client as *const Sender<()>) }.send(());
        0
    }

    let (tx, rx) = channel::<()>();
    // CoreAudio keeps calling back for the app's lifetime, so the sender is never freed.
    let client = Box::into_raw(Box::new(tx)).cast();
    // The system object's kAudioHardwarePropertyDevices, global scope, main element.
    let devices = Address { selector: fourcc(b"dev#"), scope: fourcc(b"glob"), element: 0 };
    let status = unsafe { AudioObjectAddPropertyListener(SYSTEM_OBJECT, &devices, notify, client) };
    if status != 0 {
        return eprintln!("Couldn't watch audio devices: CoreAudio error {status}");
    }
    thread::spawn(move || {
        while rx.recv().is_ok() {
            // A device arriving or leaving reports several changes in a row.
            while rx.recv_timeout(Duration::from_millis(300)).is_ok() {}
            changed();
        }
    });
}

/// Whether there is a microphone to record from (listing the devices takes 150+ ms on macOS).
pub fn has_input_device() -> bool {
    cpal::default_host().default_input_device().is_some()
}

// Input/output endpoints can share a name. Skip output-only endpoints, but keep genuine
// configuration failures so the caller reports them instead of silently changing microphones.
fn is_input_config(config: &Result<cpal::SupportedStreamConfig, cpal::DefaultStreamConfigError>) -> bool {
    match config {
        Ok(config) => config.channels() > 0,
        Err(cpal::DefaultStreamConfigError::StreamTypeNotSupported) => false,
        Err(_) => true,
    }
}

// Downmixes one callback chunk to mono, appends it and updates the display level.
fn record_chunk<T: Copy>(
    data: &[T],
    channels: usize,
    to_f32: fn(T) -> f32,
    capture: &Mutex<CaptureState>,
    meter: &mut Loudness,
    level: &AtomicU32,
    feed: &mut StreamFeed,
) {
    let mut capture = capture.lock().unwrap();
    if !capture.active || capture.generation != feed.generation || capture.interruption.is_some() { return; }
    let s = &mut capture.samples;
    let start = s.len();
    s.extend(
        data.chunks(channels)
            .map(|frame| frame.iter().map(|&x| to_f32(x)).sum::<f32>() / channels as f32),
    );
    meter.push(&s[start..], level);
    feed.push(&s[start..]);
}

impl AudioRecorder {
    #[allow(clippy::new_without_default)] // spawns the audio thread; not a plain default value
    pub fn new() -> Self {
        let (tx, rx) = channel::<AudioCommand>();
        let level = Arc::new(AtomicU32::new(0));
        let loop_level = Arc::clone(&level);
        let capture = Arc::new(Mutex::new(CaptureState::default()));
        let loop_capture = Arc::clone(&capture);

        thread::spawn(move || {
            Self::event_loop(rx, loop_level, loop_capture);
        });

        Self { tx, level, capture }
    }

    fn event_loop(rx: Receiver<AudioCommand>, level: Arc<AtomicU32>, capture: Arc<Mutex<CaptureState>>) {
        let mut current_stream: Option<cpal::Stream> = None;
        let mut sample_rate = 16000;

        loop {
            let cmd = rx.recv();
            // Deactivate before pause/drop: teardown callbacks are not recording failures.
            capture.lock().unwrap().active = false;
            // ponytail: CPAL 0.15's CoreAudio listener retains named streams; upgrade CPAL
            // to reclaim them. Pause before drop so stop/cancel/replacement/shutdown end capture.
            if let Some(stream) = current_stream.take() {
                if let Err(e) = stream.pause() {
                    eprintln!("Failed to stop audio stream: {e}");
                }
            }
            level.store(0, Ordering::Relaxed);
            let Ok(cmd) = cmd else { break };
            match cmd {
                AudioCommand::Start(device_name, stream_input, skip_ms, reply) => {
                    let generation = capture.lock().unwrap().begin();
                    let host = cpal::default_host();
                    let named = device_name.as_deref().and_then(|name| {
                        // input_devices() opens audio units for every device on macOS. Only
                        // query matching names, and reuse the config needed to build the stream.
                        host.devices()
                            .ok()?
                            .filter(|d| d.name().is_ok_and(|n| n == name))
                            .find_map(|device| {
                                let config = device.default_input_config();
                                is_input_config(&config).then_some((device, config))
                            })
                    });
                    let fell_back = device_name.is_some() && named.is_none();
                    let input = named.or_else(|| {
                        host.default_input_device().map(|device| {
                            let config = device.default_input_config();
                            (device, config)
                        })
                    });
                    let (device, config) = match input {
                        Some(input) => input,
                        None => {
                            capture.lock().unwrap().active = false;
                            let _ = reply.send(Err("No audio input device found".to_string()));
                            continue;
                        }
                    };

                    let supported_config = match config {
                        Ok(c) => c,
                        Err(e) => {
                            capture.lock().unwrap().active = false;
                            let _ = reply.send(Err(format!("Input config error: {}", e)));
                            continue;
                        }
                    };

                    let channels = supported_config.channels() as usize;
                    sample_rate = supported_config.sample_rate().0;

                    let error_capture = Arc::clone(&capture);
                    let err_fn = move |err| {
                        let message = format!("Microphone recording was interrupted: {err}");
                        if error_capture.lock().unwrap().interrupt(generation, message.clone()) {
                            eprintln!("{message}");
                        }
                    };
                    let samples_clone = Arc::clone(&capture);
                    let level_clone = Arc::clone(&level);
                    let mut feed = StreamFeed {
                        input: stream_input,
                        rate: sample_rate,
                        skip: sample_rate as usize * skip_ms as usize / 1000,
                        generation,
                    };

                    let stream_res = match supported_config.sample_format() {
                        cpal::SampleFormat::F32 => {
                            let samples_clone = Arc::clone(&samples_clone);
                            let level_clone = Arc::clone(&level_clone);
                            let mut meter = Loudness::new(sample_rate);
                            device.build_input_stream(
                                &supported_config.clone().into(),
                                move |data: &[f32], _| {
                                    record_chunk(
                                        data,
                                        channels,
                                        |x| x,
                                        &samples_clone,
                                        &mut meter,
                                        &level_clone,
                                        &mut feed,
                                    )
                                },
                                err_fn,
                                None,
                            )
                        }
                        cpal::SampleFormat::I16 => {
                            let mut meter = Loudness::new(sample_rate);
                            device.build_input_stream(
                                &supported_config.into(),
                                move |data: &[i16], _| {
                                    record_chunk(
                                        data,
                                        channels,
                                        |x| x as f32 / 32768.0,
                                        &samples_clone,
                                        &mut meter,
                                        &level_clone,
                                        &mut feed,
                                    )
                                },
                                err_fn,
                                None,
                            )
                        }
                        _ => {
                            capture.lock().unwrap().active = false;
                            let _ = reply.send(Err("Unsupported sample format".to_string()));
                            continue;
                        }
                    };

                    match stream_res {
                        Ok(stream) => {
                            if let Err(e) = stream.play() {
                                capture.lock().unwrap().active = false;
                                let _ = stream.pause();
                                let _ = reply.send(Err(format!("Failed to start stream: {}", e)));
                            } else {
                                current_stream = Some(stream);
                                let _ = reply.send(Ok(fell_back));
                            }
                        }
                        Err(e) => {
                            capture.lock().unwrap().active = false;
                            let _ = reply.send(Err(format!("Stream build error: {}", e)));
                        }
                    }
                }
                AudioCommand::Stop(skip_ms, reply) => {
                    let _ = reply.send(capture.lock().unwrap().finish(sample_rate, skip_ms));
                }
                AudioCommand::Cancel => capture.lock().unwrap().cancel(),
            }
        }
    }

    /// Starts capturing from `device_name` (None = system default). Returns Ok(true) when that
    /// device is gone and the system default is recording instead.
    pub fn start_recording(&self, device_name: Option<&str>) -> Result<bool, String> {
        self.start_recording_streamed(device_name, None, 0)
    }

    /// Also forwards new mono samples after the first `skip_ms` to a live transcriber.
    /// Use the same trim in `stop_recording` so saved audio and live audio agree.
    pub fn start_recording_streamed(
        &self,
        device_name: Option<&str>,
        stream: Option<StreamInput>,
        skip_ms: u32,
    ) -> Result<bool, String> {
        let (reply_tx, reply_rx) = channel();
        self.tx
            .send(AudioCommand::Start(
                device_name.map(str::to_string),
                stream,
                skip_ms,
                reply_tx,
            ))
            .map_err(|e| e.to_string())?;
        reply_rx.recv().map_err(|e| e.to_string())?
    }

    /// Stops and drops the first `skip_ms` (start chime); see `Recording::process`.
    pub fn stop_recording(&self, skip_ms: u32) -> Result<Recording, String> {
        let (reply_tx, reply_rx) = channel();
        self.tx
            .send(AudioCommand::Stop(skip_ms, reply_tx))
            .map_err(|e| e.to_string())?;
        let recording = reply_rx.recv().map_err(|e| e.to_string())??;
        // Debug builds only: end-to-end tests feed a WAV instead of the room (no speakers needed).
        #[cfg(debug_assertions)]
        if let Ok(path) = std::env::var("OPENGLAIDO_TEST_AUDIO") {
            let mut reader = hound::WavReader::open(&path).map_err(|e| format!("{path}: {e}"))?;
            let spec = reader.spec();
            let samples: Vec<f32> = reader
                .samples::<i16>()
                .filter_map(Result::ok)
                .step_by(spec.channels as usize)
                .map(|s| s as f32 / 32768.0)
                .collect();
            return Ok(Recording {
                samples,
                rate: spec.sample_rate,
                interruption: recording.interruption,
            });
        }
        Ok(recording)
    }

    pub fn cancel(&self) {
        let _ = self.tx.send(AudioCommand::Cancel);
    }

    /// The first failure while capturing; Stop preserves it on the returned Recording.
    pub fn interruption(&self) -> Option<String> {
        let capture = self.capture.lock().unwrap();
        if capture.active { capture.interruption.clone() } else { None }
    }

    /// Current display loudness, 0..1. Zero when not recording.
    pub fn level(&self) -> f32 {
        f32::from_bits(self.level.load(Ordering::Relaxed))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn capture_errors_keep_partial_audio_and_ignore_stale_callbacks() {
        let capture = Mutex::new(CaptureState::default());
        let generation = capture.lock().unwrap().begin();
        let mut feed = StreamFeed { input: None, rate: 16000, skip: 0, generation };
        let level = AtomicU32::new(0);
        let mut meter = Loudness::new(16000);
        record_chunk(&[0.25, 0.5], 1, |x| x, &capture, &mut meter, &level, &mut feed);
        assert!(capture.lock().unwrap().interrupt(generation, "device disconnected".into()));
        assert!(!capture.lock().unwrap().interrupt(generation, "later error".into()));
        record_chunk(&[0.75], 1, |x| x, &capture, &mut meter, &level, &mut feed);
        // Stop deactivates before pausing CPAL, whose expected teardown errors are ignored.
        capture.lock().unwrap().active = false;
        assert!(!capture.lock().unwrap().interrupt(generation, "teardown".into()));
        let recording = capture.lock().unwrap().finish(16000, 0).unwrap();
        assert_eq!(recording.samples, [0.25, 0.5]);
        assert_eq!(recording.interruption.as_deref(), Some("device disconnected"));
        assert_eq!(capture.lock().unwrap().interruption, recording.interruption);
        assert_eq!(&recording.process().unwrap().0[..4], b"RIFF");

        let next = capture.lock().unwrap().begin();
        assert_ne!(generation, next);
        assert!(capture.lock().unwrap().interruption.is_none());
        assert!(!capture.lock().unwrap().interrupt(generation, "old stream".into()));
        record_chunk(&[0.75], 1, |x| x, &capture, &mut meter, &level, &mut feed);
        assert!(capture.lock().unwrap().samples.is_empty());
        feed.generation = next;
        let mut meter = Loudness::new(16000);
        record_chunk(&[0.125], 1, |x| x, &capture, &mut meter, &level, &mut feed);
        assert_eq!(capture.lock().unwrap().samples, [0.125]);
        assert!(capture.lock().unwrap().interrupt(next, "new failure".into()));
        capture.lock().unwrap().cancel();
        assert!(capture.lock().unwrap().samples.is_empty());
        assert!(capture.lock().unwrap().interruption.is_none());
        assert!(!capture.lock().unwrap().interrupt(next, "cancel teardown".into()));
        record_chunk(&[0.5], 1, |x| x, &capture, &mut meter, &level, &mut feed);
        assert!(capture.lock().unwrap().samples.is_empty());
    }

    #[test]
    fn empty_interrupted_capture_is_retained_but_healthy_empty_capture_is_an_error() {
        let mut capture = CaptureState::default();
        capture.begin();
        assert!(capture.finish(16000, 0).is_err());
        let generation = capture.begin();
        assert!(capture.interrupt(generation, "device lost before samples".into()));
        let recording = capture.finish(16000, 60).unwrap();
        assert!(recording.samples.is_empty());
        assert!(recording.interruption.is_some());
        assert_eq!(&recording.process().unwrap().0[..4], b"RIFF");
        capture.begin();
        capture.samples.push(0.5);
        let trimmed = capture.finish(16000, 60).unwrap();
        assert!(trimmed.samples.is_empty());
        assert!(trimmed.interruption.is_none());
    }

    #[test]
    fn live_audio_trims_once_across_callbacks_and_sends_only_new_mono_samples() {
        let (tx, mut rx) = tokio::sync::mpsc::channel(4);
        let overflowed = Arc::new(AtomicBool::new(false));
        let mut feed = StreamFeed {
            input: Some(StreamInput { tx, overflowed: overflowed.clone() }),
            rate: 1000,
            skip: 3,
            generation: 1,
        };
        let samples = Mutex::new(CaptureState { active: true, generation: 1, ..Default::default() });
        let mut meter = Loudness::new(1000);
        let level = AtomicU32::new(0);
        let mut streamed = Vec::new();
        for data in [
            [0.2, 0.4, 0.6, 0.8],
            [0.1, 0.3, 0.5, 0.7],
            [0.2, 0.2, 0.4, 0.4],
        ] {
            record_chunk(&data, 2, |x| x, &samples, &mut meter, &level, &mut feed);
            while let Ok(chunk) = rx.try_recv() {
                assert_eq!(chunk.rate, 1000);
                streamed.extend(chunk.samples);
            }
        }
        let recorded = samples.lock().unwrap();
        assert_eq!(recorded.samples.len(), 6);
        assert_eq!(streamed, recorded.samples[3..]);
        assert_eq!(streamed.len(), 3);
        assert!(!overflowed.load(Ordering::Acquire));
    }

    #[test]
    fn live_audio_overflow_stops_sending_but_preserves_the_recording() {
        let (tx, mut rx) = tokio::sync::mpsc::channel(1);
        let overflowed = Arc::new(AtomicBool::new(false));
        let mut feed = StreamFeed {
            input: Some(StreamInput { tx, overflowed: overflowed.clone() }),
            rate: 16000,
            skip: 0,
            generation: 1,
        };
        let samples = Mutex::new(CaptureState { active: true, generation: 1, ..Default::default() });
        let mut meter = Loudness::new(16000);
        let level = AtomicU32::new(0);
        for sample in [16384i16, 8192] {
            record_chunk(&[sample], 1, |x| x as f32 / 32768.0, &samples, &mut meter, &level, &mut feed);
        }
        assert!(overflowed.load(Ordering::Acquire));
        assert!(feed.input.is_none());
        assert_eq!(rx.try_recv().unwrap().samples, vec![0.5]);
        record_chunk(&[0i16], 1, |x| x as f32 / 32768.0, &samples, &mut meter, &level, &mut feed);
        assert!(rx.try_recv().is_err());
        assert_eq!(samples.lock().unwrap().samples, vec![0.5, 0.25, 0.0]);
    }

    #[test]
    fn canceled_live_consumer_stops_sending_without_overflow() {
        let (tx, rx) = tokio::sync::mpsc::channel(1);
        let overflowed = Arc::new(AtomicBool::new(false));
        let mut feed = StreamFeed {
            input: Some(StreamInput { tx, overflowed: overflowed.clone() }),
            rate: 16000,
            skip: 0,
            generation: 1,
        };
        drop(rx);
        feed.push(&[0.5]);
        assert!(feed.input.is_none());
        assert!(!overflowed.load(Ordering::Acquire));
    }

    #[test]
    fn named_input_skips_output_endpoints_without_hiding_config_failures() {
        use cpal::DefaultStreamConfigError::{DeviceNotAvailable, StreamTypeNotSupported};
        let config = |channels| Ok(cpal::SupportedStreamConfig::new(
            channels,
            cpal::SampleRate(48_000),
            cpal::SupportedBufferSize::Unknown,
            cpal::SampleFormat::F32,
        ));
        assert!(!is_input_config(&Err(StreamTypeNotSupported)));
        assert!(!is_input_config(&config(0)));
        assert!(is_input_config(&config(1)));
        assert!(is_input_config(&Err(DeviceNotAvailable)));
    }

    #[test]
    fn resample_lengths_and_endpoints() {
        let ramp: Vec<f32> = (0..480).map(|i| i as f32 / 480.0).collect();
        let up = resample(&ramp, 16_000, 48_000);
        assert_eq!(up.len(), 1440);
        assert_eq!(resample(&up, 48_000, 16_000).len(), 480);
        assert_eq!(resample(&ramp, 44_100, 44_100), ramp);
        assert!(resample(&[], 16_000, 48_000).is_empty());
    }

    #[test]
    fn silence_has_no_speech_and_keeps_duration() {
        let silence = vec![0.0f32; 44_100]; // 1 s at 44.1 kHz
        let (out, speech) = denoise(&silence, 44_100);
        assert!(!speech);
        assert!((out.len() as i64 - OUTPUT_RATE as i64).abs() < 400, "{}", out.len());
        assert!(out.iter().all(|x| x.abs() < 0.01));
        let wav = encode_wav(&out, OUTPUT_RATE).unwrap();
        assert_eq!(&wav[..4], b"RIFF");
    }

    /// White noise at `db` dBFS RMS (xorshift).
    fn noise(db: f32, n: usize, seed: u64) -> Vec<f32> {
        let mut x = seed;
        let amp = 10f32.powf(db / 20.0) * 3f32.sqrt();
        (0..n)
            .map(|_| {
                x ^= x << 13;
                x ^= x >> 7;
                x ^= x << 17;
                ((x >> 40) as f32 / (1u64 << 24) as f32 * 2.0 - 1.0) * amp
            })
            .collect()
    }

    /// A voiced vowel: 140 Hz harmonics with a 4 Hz syllable envelope, at `db` dBFS RMS.
    fn speech(db: f32, n: usize, rate: u32) -> Vec<f32> {
        let tau = std::f32::consts::TAU;
        let raw: Vec<f32> = (0..n)
            .map(|i| {
                let t = i as f32 / rate as f32;
                let voice: f32 = (1..=25).map(|h| (tau * 140.0 * h as f32 * t).sin() / h as f32).sum();
                voice * (0.5 - 0.5 * (tau * 4.0 * t).cos())
            })
            .collect();
        let rms = (raw.iter().map(|x| x * x).sum::<f32>() / n as f32).sqrt();
        let gain = 10f32.powf(db / 20.0) / rms;
        raw.into_iter().map(|x| x * gain).collect()
    }

    /// The level after each ~10 ms callback.
    fn levels(samples: &[f32], rate: u32) -> Vec<f32> {
        let mut meter = Loudness::new(rate);
        let level = AtomicU32::new(0);
        samples
            .chunks(rate as usize / 100)
            .map(|chunk| {
                meter.push(chunk, &level);
                f32::from_bits(level.load(Ordering::Relaxed))
            })
            .collect()
    }

    fn peak(levels: &[f32]) -> f32 {
        levels.iter().copied().fold(0.0, f32::max)
    }

    #[test]
    fn silence_and_room_noise_stay_flat() {
        for rate in [16_000, 48_000] {
            let ms = |ms: usize| rate as usize * ms / 1000;
            assert_eq!(peak(&levels(&vec![0.0; ms(2000)], rate)), 0.0);
            let room = levels(&noise(-60.0, ms(3000), 0x0bad_cafe), rate);
            assert!(peak(&room[50..]) < 0.05, "{rate} Hz room {}", peak(&room[50..]));
            let fan = levels(&noise(-50.0, ms(6000), 0x1234_5678), rate);
            assert!(peak(&fan[300..]) <= 0.1, "{rate} Hz fan {}", peak(&fan[300..]));
            // Steady noise stays flat from the very first hop, even while the meter settles.
            for db in [-45.0, -35.0, -25.0] {
                let start = levels(&noise(db, ms(1000), 42), rate);
                assert!(peak(&start) <= 0.15, "{rate} Hz {db} dB noise at start {}", peak(&start));
            }
            // A loud fan right next to the mic still settles flat.
            let loud_fan = levels(&noise(-25.0, ms(8000), 0x2468_ace0), rate);
            assert!(peak(&loud_fan[500..]) <= 0.15, "{rate} Hz loud fan {}", peak(&loud_fan[500..]));
            // Mic warm-up zeros that end mid-hop, then room tone: no phantom voice.
            let mut start = vec![0.0; ms(29)]; // the first signal hop is mostly zeros at 16 and 48 kHz
            start.extend(noise(-45.0, ms(6000), 0x0f0f_0f0f));
            let warm = levels(&start, rate);
            assert!(peak(&warm[30..]) < 0.15, "{rate} Hz warm-up into room tone {}", peak(&warm[30..]));
        }
    }

    #[test]
    fn speech_fills_the_bar_at_any_normal_volume_and_releases_promptly() {
        for rate in [16_000, 48_000] {
            let ms = |ms: usize| rate as usize * ms / 1000;
            for db in [-42.0, -18.0] {
                let mut samples = noise(-60.0, ms(500), 7);
                samples.extend(speech(db, ms(1500), rate).iter().zip(noise(-60.0, ms(1500), 8)).map(|(s, n)| s + n));
                samples.extend(noise(-60.0, ms(1000), 9));
                let trace = levels(&samples, rate);
                assert!(trace.iter().all(|l| (0.0..=1.0).contains(l)), "{rate} Hz {db} dB {trace:?}");
                let first_second = peak(&trace[50..150]);
                assert!(first_second >= 0.6, "{rate} Hz {db} dB onset {first_second}");
                let after = peak(&trace[230..]);
                assert!(after < 0.05, "{rate} Hz {db} dB release {after}");
            }
            // Already talking when the mic opens (mid-syllable): the floor seeds on the voice and
            // must drop out of it within a few hundred ms.
            let mut talking = vec![0.0; ms(40)];
            talking.extend_from_slice(&speech(-25.0, ms(2000), rate)[ms(110)..]);
            let onset = peak(&levels(&talking, rate)[..39]);
            assert!(onset >= 0.5, "{rate} Hz mid-word start {onset}");
        }
    }

    #[test]
    fn display_level_does_not_change_recorded_or_streamed_samples() {
        let samples = Mutex::new(CaptureState { active: true, generation: 1, ..Default::default() });
        let (tx, mut rx) = tokio::sync::mpsc::channel(1);
        let mut feed = StreamFeed {
            input: Some(StreamInput { tx, overflowed: Arc::new(AtomicBool::new(false)) }),
            rate: 48_000,
            skip: 0,
            generation: 1,
        };
        let quiet = speech(-45.0, 18_000, 48_000); // 375 ms: ends on a syllable peak, not a pause
        let level = AtomicU32::new(0);
        record_chunk(&quiet, 1, |sample| sample, &samples, &mut Loudness::new(48_000), &level, &mut feed);
        assert_eq!(samples.lock().unwrap().samples, quiet);
        assert_eq!(rx.try_recv().unwrap().samples, quiet);
        assert!(f32::from_bits(level.load(Ordering::Relaxed)) > 0.0);
    }

    #[test]
    fn steady_hiss_is_not_speech() {
        // 10 s of white noise at -38 dBFS: scattered voice-like frames, never a run.
        assert!(!denoise(&noise(-38.0, 48_000 * 10, 0x0bad_cafe_1234_5678), 48_000).1);
    }
}
