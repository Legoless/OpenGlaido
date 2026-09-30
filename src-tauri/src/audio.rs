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
fn resample(samples: &[f32], from: u32, to: u32) -> Vec<f32> {
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

pub struct AudioRecorder {
    tx: Sender<AudioCommand>,
    /// Decaying peak of the input (f32 bits), written by the audio callback.
    level: Arc<AtomicU32>,
}

/// Names of the available input devices (cpal names, as stored in `input_device`).
pub fn list_input_devices() -> Vec<String> {
    cpal::default_host()
        .input_devices()
        .map(|devices| devices.filter_map(|d| d.name().ok()).collect())
        .unwrap_or_default()
}

// Downmixes one callback chunk to mono, appends it and updates the decaying peak level.
fn record_chunk<T: Copy>(
    data: &[T],
    channels: usize,
    to_f32: fn(T) -> f32,
    samples: &Mutex<Vec<f32>>,
    level: &AtomicU32,
) {
    let mut s = samples.lock().unwrap();
    let start = s.len();
    s.extend(
        data.chunks(channels)
            .map(|frame| frame.iter().map(|&x| to_f32(x)).sum::<f32>() / channels as f32),
    );
    let peak = s[start..].iter().fold(0.0f32, |m, x| m.max(x.abs()));
    let prev = f32::from_bits(level.load(Ordering::Relaxed));
    level.store(peak.max(prev * 0.85).to_bits(), Ordering::Relaxed);
}

impl AudioRecorder {
    #[allow(clippy::new_without_default)] // spawns the audio thread; not a plain default value
    pub fn new() -> Self {
        let (tx, rx) = channel::<AudioCommand>();
        let level = Arc::new(AtomicU32::new(0));
        let loop_level = Arc::clone(&level);

        thread::spawn(move || {
            Self::event_loop(rx, loop_level);
        });

        Self { tx, level }
    }

    fn event_loop(rx: Receiver<AudioCommand>, level: Arc<AtomicU32>) {
        let mut _current_stream: Option<cpal::Stream> = None;
        let samples = Arc::new(Mutex::new(Vec::new()));
        let mut sample_rate = 16000;

        while let Ok(cmd) = rx.recv() {
            match cmd {
                AudioCommand::Start(device_name, reply) => {
                    let host = cpal::default_host();
                    let named = device_name.as_deref().and_then(|name| {
                        host.input_devices()
                            .ok()?
                            .find(|d| d.name().is_ok_and(|n| n == name))
                    });
                    let fell_back = device_name.is_some() && named.is_none();
                    let device = match named.or_else(|| host.default_input_device()) {
                        Some(d) => d,
                        None => {
                            let _ = reply.send(Err("No audio input device found".to_string()));
                            continue;
                        }
                    };

                    let supported_config = match device.default_input_config() {
                        Ok(c) => c,
                        Err(e) => {
                            let _ = reply.send(Err(format!("Input config error: {}", e)));
                            continue;
                        }
                    };

                    let channels = supported_config.channels() as usize;
                    sample_rate = supported_config.sample_rate().0;

                    samples.lock().unwrap().clear();
                    level.store(0, Ordering::Relaxed);

                    let err_fn = |err| eprintln!("Audio stream error: {}", err);
                    let samples_clone = Arc::clone(&samples);
                    let level_clone = Arc::clone(&level);

                    let stream_res = match supported_config.sample_format() {
                        cpal::SampleFormat::F32 => device.build_input_stream(
                            &supported_config.into(),
                            move |data: &[f32], _| {
                                record_chunk(data, channels, |x| x, &samples_clone, &level_clone)
                            },
                            err_fn,
                            None,
                        ),
                        cpal::SampleFormat::I16 => device.build_input_stream(
                            &supported_config.into(),
                            move |data: &[i16], _| {
                                record_chunk(data, channels, |x| x as f32 / 32768.0, &samples_clone, &level_clone)
                            },
                            err_fn,
                            None,
                        ),
                        _ => {
                            let _ = reply.send(Err("Unsupported sample format".to_string()));
                            continue;
                        }
                    };

                    match stream_res {
                        Ok(stream) => {
                            if let Err(e) = stream.play() {
                                let _ = reply.send(Err(format!("Failed to start stream: {}", e)));
                            } else {
                                _current_stream = Some(stream);
                                let _ = reply.send(Ok(fell_back));
                            }
                        }
                        Err(e) => {
                            let _ = reply.send(Err(format!("Stream build error: {}", e)));
                        }
                    }
                }
                AudioCommand::Stop(skip_ms, reply) => {
                    _current_stream = None;
                    level.store(0, Ordering::Relaxed);
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
                    let _ = reply.send(Ok(Recording { samples, rate: sample_rate }));
                }
                AudioCommand::Cancel => {
                    _current_stream = None;
                    level.store(0, Ordering::Relaxed);
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
            .send(AudioCommand::Start(device_name.map(str::to_string), reply_tx))
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
            let samples: Vec<f32> = reader.samples::<i16>().filter_map(Result::ok).step_by(spec.channels as usize).map(|s| s as f32 / 32768.0).collect();
            return Ok(Recording { samples, rate: spec.sample_rate });
        }
        Ok(recording)
    }

    pub fn cancel(&self) {
        let _ = self.tx.send(AudioCommand::Cancel);
    }

    /// Current decaying input peak (0..1 linear); 0 when not recording.
    pub fn level(&self) -> f32 {
        f32::from_bits(self.level.load(Ordering::Relaxed))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
