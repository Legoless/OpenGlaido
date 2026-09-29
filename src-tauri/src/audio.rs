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
    Stop(Sender<Result<Vec<u8>, String>>),
    Cancel,
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
                AudioCommand::Stop(reply) => {
                    _current_stream = None;
                    level.store(0, Ordering::Relaxed);
                    let recorded = {
                        let mut s = samples.lock().unwrap();
                        std::mem::take(&mut *s)
                    };

                    if recorded.is_empty() {
                        let _ = reply.send(Err("No audio samples recorded".to_string()));
                        continue;
                    }

                    let spec = WavSpec {
                        channels: 1,
                        sample_rate,
                        bits_per_sample: 16,
                        sample_format: hound::SampleFormat::Int,
                    };

                    let mut cursor = Cursor::new(Vec::new());
                    let encode_res = (|| -> Result<Vec<u8>, String> {
                        let mut writer = WavWriter::new(&mut cursor, spec)
                            .map_err(|e| format!("WAV writer error: {}", e))?;
                        for &sample in &recorded {
                            let clamped = sample.clamp(-1.0, 1.0);
                            let val = (clamped * 32767.0) as i16;
                            writer.write_sample(val).map_err(|e| e.to_string())?;
                        }
                        writer.finalize().map_err(|e| e.to_string())?;
                        Ok(cursor.into_inner())
                    })();

                    let _ = reply.send(encode_res);
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

    pub fn stop_recording(&self) -> Result<Vec<u8>, String> {
        let (reply_tx, reply_rx) = channel();
        self.tx
            .send(AudioCommand::Stop(reply_tx))
            .map_err(|e| e.to_string())?;
        reply_rx.recv().map_err(|e| e.to_string())?
    }

    pub fn cancel(&self) {
        let _ = self.tx.send(AudioCommand::Cancel);
    }

    /// Current decaying input peak (0..1 linear); 0 when not recording.
    pub fn level(&self) -> f32 {
        f32::from_bits(self.level.load(Ordering::Relaxed))
    }
}
