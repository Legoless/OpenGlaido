use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use hound::{WavSpec, WavWriter};
use std::io::Cursor;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::thread;

enum AudioCommand {
    /// Device name (None = system default); replies Ok(true) when the named device was
    /// missing and the default was used instead.
    Start(Option<String>, Sender<Result<bool, String>>),
    /// Skip this many ms at the start (the start chime) and hand the samples back.
    Stop(u32, Sender<Result<Recording, String>>),
    Cancel,
}

/// Raw mono samples from a finished recording; `process` turns them into the upload.
pub struct Recording {
    pub samples: Vec<f32>,
    pub rate: u32,
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

/// Bars in the dictation pill. Each one follows a different part of the voice.
const BAR_COUNT: usize = 10;
/// Samples per level update. 256 at 48 kHz is about 5 ms.
const HOP: usize = 256;
/// Band centers from the fundamental up to sibilants. Spaced a little over an octave apart.
const BAND_HZ: [f32; 5] = [170.0, 420.0, 1_050.0, 2_600.0, 6_200.0];
const BAND_Q: f32 = 1.35;
/// Long enough that one pitch pulse does not kick a bar, short enough that a syllable still arrives.
const ATTACK_MS: f32 = 40.0;
/// Close together on purpose: lows linger a little, and the row rises and falls as one shape.
const RELEASE_MS: [f32; 5] = [250.0, 230.0, 210.0, 190.0, 175.0];

pub struct AudioRecorder {
    tx: Sender<AudioCommand>,
    /// Latest bar levels (0..1, f32 bits), written by the audio callback.
    meter: Arc<BarMeter>,
}

struct BarMeter {
    bars: [AtomicU32; BAR_COUNT],
}

impl BarMeter {
    fn new() -> Self {
        Self {
            bars: std::array::from_fn(|_| AtomicU32::new(0)),
        }
    }

    fn clear(&self) {
        for bar in &self.bars {
            bar.store(0, Ordering::Relaxed);
        }
    }

    fn store(&self, bars: [f32; BAR_COUNT]) {
        for (bar, value) in self.bars.iter().zip(bars) {
            bar.store(value.clamp(0.0, 1.0).to_bits(), Ordering::Relaxed);
        }
    }

    fn load(&self) -> [f32; BAR_COUNT] {
        std::array::from_fn(|i| f32::from_bits(self.bars[i].load(Ordering::Relaxed)))
    }
}

/// One bandpass (Audio EQ Cookbook, peak gain 0 dB). A sine at the center frequency
/// comes back out at the same amplitude, so the bars track how loud that part of the voice is.
struct Biquad {
    b0: f32,
    b2: f32,
    a1: f32,
    a2: f32,
    x1: f32,
    x2: f32,
    y1: f32,
    y2: f32,
}

impl Biquad {
    fn bandpass(rate: u32, freq: f32, q: f32) -> Self {
        let nyquist = rate as f32 * 0.45;
        let freq = freq.clamp(1.0, nyquist.max(1.0));
        let w0 = 2.0 * std::f32::consts::PI * freq / rate as f32;
        let alpha = w0.sin() / (2.0 * q);
        let a0 = 1.0 + alpha;
        Self {
            b0: alpha / a0,
            b2: -alpha / a0,
            a1: -2.0 * w0.cos() / a0,
            a2: (1.0 - alpha) / a0,
            x1: 0.0,
            x2: 0.0,
            y1: 0.0,
            y2: 0.0,
        }
    }

    fn tick(&mut self, x: f32) -> f32 {
        let y = self.b0 * x + self.b2 * self.x2 - self.a1 * self.y1 - self.a2 * self.y2;
        self.x2 = self.x1;
        self.x1 = x;
        self.y2 = self.y1;
        self.y1 = y;
        y
    }
}

/// Streaming voice visualizer. Five overlapping bands follow the voice. A normal syllable draws
/// two humps — warmth taller on the left, brightness taller on the right — with a valley between
/// them. Attack and release are eased, and a light blend keeps each hump round.
struct Waveform {
    bands: [Biquad; 5],
    env: [f32; 5],
    /// Sum of absolute band output over the current hop.
    energy: [f32; 5],
    attack: f32,
    release: [f32; 5],
    done: usize,
    left: usize,
}

impl Waveform {
    fn new(rate: u32) -> Self {
        let rate = rate.max(1);
        let coeff = |ms: f32| (-(HOP as f32) / (rate as f32 * ms / 1000.0)).exp();
        Self {
            bands: std::array::from_fn(|i| Biquad::bandpass(rate, BAND_HZ[i], BAND_Q)),
            env: [0.0; 5],
            energy: [0.0; 5],
            attack: coeff(ATTACK_MS),
            release: std::array::from_fn(|i| coeff(RELEASE_MS[i])),
            done: 0,
            left: HOP,
        }
    }

    fn push(&mut self, samples: &[f32], meter: &BarMeter) {
        while self.done < samples.len() {
            let x = samples[self.done];
            self.done += 1;
            for (band, energy) in self.bands.iter_mut().zip(self.energy.iter_mut()) {
                *energy += band.tick(x).abs();
            }
            self.left -= 1;
            if self.left == 0 {
                self.left = HOP;
                // π/2 times the mean absolute value equals the amplitude of a sine, and it
                // ignores a one-sample spike that a peak meter would draw at full height.
                let scale = std::f32::consts::FRAC_PI_2 / HOP as f32;
                for i in 0..self.env.len() {
                    let level = perceptual(self.energy[i] * scale);
                    let coeff = if level >= self.env[i] { self.attack } else { self.release[i] };
                    self.env[i] = level + (self.env[i] - level) * coeff;
                    self.energy[i] = 0.0;
                }
                meter.store(soften(&layout(&self.env)));
            }
        }
    }
}

/// -42 dBFS sits on the rest height, about -8 dBFS fills the bar. Normal speech lands in between,
/// and room tone below the floor does not twitch the pill.
fn perceptual(amplitude: f32) -> f32 {
    if amplitude < 1.0e-5 {
        return 0.0;
    }
    ((20.0 * amplitude.log10() + 42.0) / 34.0).clamp(0.0, 1.0)
}

/// Both humps are driven by the whole voice, so speech does not collapse into one arch in the
/// middle. Warmth leans the left hump up, brightness the right. `env` is already 0..1.
fn layout(env: &[f32; 5]) -> [f32; BAR_COUNT] {
    let [low, body, mid, presence, air] = *env;
    let voice = (low * 0.50 + body * 0.85 + mid * 0.70 + presence * 0.40 + air * 0.22).clamp(0.0, 1.0);
    let left = (voice * (0.90 + low * 0.16 + body * 0.08 - air * 0.10)).clamp(0.0, 1.0);
    let right = (voice * (0.86 + presence * 0.14 + air * 0.20 - low * 0.06)).clamp(0.0, 1.0);
    [
        left * 0.40,
        left * 0.72,
        left,
        left * 0.70,
        left * 0.40 + right * 0.06,
        right * 0.38 + left * 0.06,
        right * 0.72,
        right,
        right * 0.70,
        right * 0.38,
    ]
    .map(|bar| bar.clamp(0.0, 1.0))
}

/// One light blend toward each neighbor. Enough to round a hump, light enough that the valley
/// between the two peaks stays.
fn soften(bars: &[f32; BAR_COUNT]) -> [f32; BAR_COUNT] {
    let prev = bars;
    std::array::from_fn(|i| {
        let left = if i == 0 { prev[1] } else { prev[i - 1] };
        let right = if i + 1 == BAR_COUNT { prev[BAR_COUNT - 2] } else { prev[i + 1] };
        left * 0.12 + prev[i] * 0.76 + right * 0.12
    })
}

/// Names of the available input devices (cpal names, as stored in `input_device`).
pub fn list_input_devices() -> Vec<String> {
    cpal::default_host()
        .input_devices()
        .map(|devices| devices.filter_map(|d| d.name().ok()).collect())
        .unwrap_or_default()
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

// Downmixes one callback chunk to mono, appends it and updates the bar levels.
fn record_chunk<T: Copy>(
    data: &[T],
    channels: usize,
    to_f32: fn(T) -> f32,
    samples: &Mutex<Vec<f32>>,
    wave: &mut Waveform,
    meter: &BarMeter,
) {
    let mut s = samples.lock().unwrap();
    s.extend(
        data.chunks(channels)
            .map(|frame| frame.iter().map(|&x| to_f32(x)).sum::<f32>() / channels as f32),
    );
    wave.push(&s, meter);
}

impl AudioRecorder {
    #[allow(clippy::new_without_default)] // spawns the audio thread; not a plain default value
    pub fn new() -> Self {
        let (tx, rx) = channel::<AudioCommand>();
        let meter = Arc::new(BarMeter::new());
        let loop_meter = Arc::clone(&meter);

        thread::spawn(move || {
            Self::event_loop(rx, loop_meter);
        });

        Self { tx, meter }
    }

    fn event_loop(rx: Receiver<AudioCommand>, meter: Arc<BarMeter>) {
        let mut current_stream: Option<cpal::Stream> = None;
        let samples = Arc::new(Mutex::new(Vec::new()));
        let mut sample_rate = 16000;

        loop {
            let cmd = rx.recv();
            // ponytail: CPAL 0.15's CoreAudio listener retains named streams; upgrade CPAL
            // to reclaim them. Pause before drop so stop/cancel/replacement/shutdown end capture.
            if let Some(stream) = current_stream.take() {
                if let Err(e) = stream.pause() {
                    eprintln!("Failed to stop audio stream: {e}");
                }
            }
            meter.clear();
            let Ok(cmd) = cmd else { break };
            match cmd {
                AudioCommand::Start(device_name, reply) => {
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
                            let _ = reply.send(Err("No audio input device found".to_string()));
                            continue;
                        }
                    };

                    let supported_config = match config {
                        Ok(c) => c,
                        Err(e) => {
                            let _ = reply.send(Err(format!("Input config error: {}", e)));
                            continue;
                        }
                    };

                    let channels = supported_config.channels() as usize;
                    sample_rate = supported_config.sample_rate().0;

                    samples.lock().unwrap().clear();

                    let err_fn = |err| eprintln!("Audio stream error: {err}");
                    let samples_clone = Arc::clone(&samples);
                    let meter_clone = Arc::clone(&meter);

                    let stream_res = match supported_config.sample_format() {
                        cpal::SampleFormat::F32 => {
                            let samples_clone = Arc::clone(&samples_clone);
                            let meter_clone = Arc::clone(&meter_clone);
                            let mut wave = Waveform::new(sample_rate);
                            device.build_input_stream(
                                &supported_config.clone().into(),
                                move |data: &[f32], _| {
                                    record_chunk(
                                        data,
                                        channels,
                                        |x| x,
                                        &samples_clone,
                                        &mut wave,
                                        &meter_clone,
                                    )
                                },
                                err_fn,
                                None,
                            )
                        }
                        cpal::SampleFormat::I16 => {
                            let mut wave = Waveform::new(sample_rate);
                            device.build_input_stream(
                                &supported_config.into(),
                                move |data: &[i16], _| {
                                    record_chunk(
                                        data,
                                        channels,
                                        |x| x as f32 / 32768.0,
                                        &samples_clone,
                                        &mut wave,
                                        &meter_clone,
                                    )
                                },
                                err_fn,
                                None,
                            )
                        }
                        _ => {
                            let _ = reply.send(Err("Unsupported sample format".to_string()));
                            continue;
                        }
                    };

                    match stream_res {
                        Ok(stream) => {
                            if let Err(e) = stream.play() {
                                let _ = stream.pause();
                                let _ = reply.send(Err(format!("Failed to start stream: {}", e)));
                            } else {
                                current_stream = Some(stream);
                                let _ = reply.send(Ok(fell_back));
                            }
                        }
                        Err(e) => {
                            let _ = reply.send(Err(format!("Stream build error: {}", e)));
                        }
                    }
                }
                AudioCommand::Stop(skip_ms, reply) => {
                    let recorded = {
                        let mut s = samples.lock().unwrap();
                        std::mem::take(&mut *s)
                    };
                    let skip = (sample_rate as usize * skip_ms as usize / 1000).min(recorded.len());
                    let samples = recorded[skip..].to_vec();

                    // Only the chime trim left nothing: a very short hold, handled as "no speech".
                    if recorded.is_empty() {
                        let _ = reply.send(Err("No audio samples recorded".to_string()));
                        continue;
                    }
                    let _ = reply.send(Ok(Recording {
                        samples,
                        rate: sample_rate,
                    }));
                }
                AudioCommand::Cancel => {
                    let mut s = samples.lock().unwrap();
                    s.clear();
                }
            }
        }
    }

    /// Starts capturing from `device_name` (None = system default). Returns Ok(true) when that
    /// device is gone and the system default is recording instead.
    pub fn start_recording(&self, device_name: Option<&str>) -> Result<bool, String> {
        let (reply_tx, reply_rx) = channel();
        self.tx
            .send(AudioCommand::Start(
                device_name.map(str::to_string),
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
            });
        }
        Ok(recording)
    }

    pub fn cancel(&self) {
        let _ = self.tx.send(AudioCommand::Cancel);
    }

    /// Current bar levels, each 0..1. Zeros when not recording.
    pub fn bars(&self) -> [f32; BAR_COUNT] {
        self.meter.load()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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

    fn tone(freq: f32, amp: f32, hops: usize, rate: u32) -> [f32; BAR_COUNT] {
        let n = HOP * hops;
        let samples: Vec<f32> = (0..n)
            .map(|i| amp * (2.0 * std::f32::consts::PI * freq * i as f32 / rate as f32).sin())
            .collect();
        let mut wave = Waveform::new(rate);
        let meter = BarMeter::new();
        wave.push(&samples, &meter);
        meter.load()
    }

    #[test]
    fn silence_holds_the_bars_down() {
        let meter = BarMeter::new();
        Waveform::new(48_000).push(&vec![0.0; HOP * 6], &meter);
        assert!(meter.load().iter().all(|v| *v == 0.0), "{:?}", meter.load());
    }

    #[test]
    fn louder_speech_raises_the_bars() {
        let rate = 48_000;
        let freq = BAND_HZ[1];
        let quiet: f32 = tone(freq, 0.04, 8, rate).into_iter().sum();
        let loud: f32 = tone(freq, 0.45, 8, rate).into_iter().sum();
        assert!(loud > quiet + 1.5, "loud {loud} quiet {quiet}");
    }

    fn assert_two_peaks(bars: &[f32; BAR_COUNT], what: &str) {
        let left = bars[2];
        let right = bars[7];
        let valley = (bars[4] + bars[5]) * 0.5;
        let edge = (bars[0] + bars[9]) * 0.5;
        // Relative, so a quiet sibilant has to show the same valley as a loud vowel.
        assert!(left > valley * 1.25 + 0.015, "{what} left {left} valley {valley} {bars:?}");
        assert!(right > valley * 1.25 + 0.015, "{what} right {right} valley {valley} {bars:?}");
        assert!(left > edge && right > edge, "{what} edge {edge} {bars:?}");
    }

    #[test]
    fn warmth_and_brightness_lean_opposite_peaks() {
        let rate = 48_000;
        let low = tone(BAND_HZ[0], 0.5, 20, rate);
        let high = tone(BAND_HZ[4], 0.5, 20, rate);
        assert_two_peaks(&low, "low");
        assert_two_peaks(&high, "high");
        assert!(low[2] > low[7], "warmth should lean left {low:?}");
        assert!(high[7] > high[2], "brightness should lean right {high:?}");
    }

    /// A vowel is not a sine parked on a band center. Harmonics between the centers still have
    /// to raise both humps.
    #[test]
    fn a_vowel_forms_two_peaks() {
        let rate = 48_000;
        let n = HOP * 16;
        let mut raw = vec![0.0f32; n];
        for h in 1..28 {
            let freq = 137.0 * h as f32;
            if freq >= 8_000.0 {
                break;
            }
            let weight = 1.0 / h as f32;
            for (i, sample) in raw.iter_mut().enumerate() {
                *sample +=
                    weight * (2.0 * std::f32::consts::PI * freq * i as f32 / rate as f32).sin();
            }
        }
        let peak = raw.iter().fold(0.0f32, |m, s| m.max(s.abs()));
        for sample in &mut raw {
            *sample = *sample / peak * 0.16;
        }
        let mut wave = Waveform::new(rate);
        let meter = BarMeter::new();
        wave.push(&raw, &meter);
        let bars = meter.load();
        assert_two_peaks(&bars, "vowel");
        assert!(bars[2] > 0.35, "left hump {bars:?}");
    }

    /// Sibilants are noise, not a tone. They still draw two humps, with the right one taller.
    #[test]
    fn hiss_leans_on_the_right_peak() {
        let mut state = 0x1234_5678u32;
        let mut prev = 0.0f32;
        let mut raw = Vec::with_capacity(HOP * 20);
        for _ in 0..HOP * 20 {
            state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            let white = ((state >> 8) & 0x00ff_ffff) as f32 / 16_777_215.0 * 2.0 - 1.0;
            raw.push(white - prev);
            prev = white;
        }
        let peak = raw.iter().fold(0.0f32, |m, s| m.max(s.abs()));
        for sample in &mut raw {
            *sample = *sample / peak * 0.15;
        }
        let mut wave = Waveform::new(48_000);
        let meter = BarMeter::new();
        wave.push(&raw, &meter);
        let bars = meter.load();
        assert_two_peaks(&bars, "hiss");
        assert!(bars[7] > bars[2], "right hump should lead {bars:?}");
    }

    #[test]
    fn the_row_moves_as_one_curve() {
        let bars = tone(BAND_HZ[0], 0.5, 20, 48_000);
        let step = bars.windows(2).fold(0.0f32, |m, w| m.max((w[0] - w[1]).abs()));
        assert!(step < 0.28, "step {step} {bars:?}");
    }

    /// A steady hiss used to twitch, because each hop kept its loudest sample.
    #[test]
    fn steady_noise_holds_its_shape() {
        let mut state = 0x89ab_cdefu32;
        let mut prev = 0.0f32;
        let mut raw = Vec::with_capacity(HOP * 48);
        for _ in 0..HOP * 48 {
            state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            let white = ((state >> 8) & 0x00ff_ffff) as f32 / 16_777_215.0 * 2.0 - 1.0;
            let sample = (white - prev) * 0.08;
            prev = white;
            raw.push(sample);
        }
        let mut wave = Waveform::new(48_000);
        let meter = BarMeter::new();
        let mut worst = 0.0f32;
        let mut last = [0.0f32; BAR_COUNT];
        for hop in 0..48 {
            let start = hop * HOP;
            wave.push(&raw[start..start + HOP], &meter);
            let bars = meter.load();
            if hop >= 30 {
                for i in 0..BAR_COUNT {
                    worst = worst.max((bars[i] - last[i]).abs());
                }
            }
            last = bars;
        }
        assert!(worst < 0.05, "step {worst} {last:?}");
    }

    #[test]
    fn a_syllable_releases_instead_of_sticking() {
        let rate = 48_000;
        let freq = BAND_HZ[2];
        // Long enough for the eased attack to arrive, then a pause long enough to settle.
        let mut samples = tone_samples(freq, 0.5, 16, rate);
        let spoken = {
            let meter = BarMeter::new();
            let mut wave = Waveform::new(rate);
            wave.push(&samples, &meter);
            meter.load().into_iter().sum::<f32>()
        };
        samples.extend(std::iter::repeat_n(0.0, HOP * 80));
        let meter = BarMeter::new();
        let mut wave = Waveform::new(rate);
        wave.push(&samples, &meter);
        let after: f32 = meter.load().into_iter().sum();
        assert!(spoken > 2.0, "spoken {spoken}");
        assert!(after < spoken * 0.35, "after {after} spoken {spoken}");
    }

    fn tone_samples(freq: f32, amp: f32, hops: usize, rate: u32) -> Vec<f32> {
        let n = HOP * hops;
        (0..n)
            .map(|i| amp * (2.0 * std::f32::consts::PI * freq * i as f32 / rate as f32).sin())
            .collect()
    }

    #[test]
    fn steady_hiss_is_not_speech() {
        // 10 s of white noise at -38 dBFS (xorshift): scattered voice-like frames, never a run.
        let mut x = 0x0bad_cafe_1234_5678u64;
        let amp = 10f32.powf(-38.0 / 20.0) * 3f32.sqrt();
        let hiss: Vec<f32> = (0..48_000 * 10)
            .map(|_| {
                x ^= x << 13;
                x ^= x >> 7;
                x ^= x << 17;
                ((x >> 40) as f32 / (1u64 << 24) as f32 * 2.0 - 1.0) * amp
            })
            .collect();
        assert!(!denoise(&hiss, 48_000).1);
    }
}
