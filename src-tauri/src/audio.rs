use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use hound::{WavSpec, WavWriter};
use std::io::Cursor;
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::thread;

enum AudioCommand {
    Start(Sender<Result<(), String>>),
    Stop(Sender<Result<Vec<u8>, String>>),
    Cancel,
}

pub struct AudioRecorder {
    tx: Sender<AudioCommand>,
}

impl AudioRecorder {
    pub fn new() -> Self {
        let (tx, rx) = channel::<AudioCommand>();

        thread::spawn(move || {
            Self::event_loop(rx);
        });

        Self { tx }
    }

    fn event_loop(rx: Receiver<AudioCommand>) {
        let mut _current_stream: Option<cpal::Stream> = None;
        let samples = Arc::new(Mutex::new(Vec::new()));
        let mut sample_rate = 16000;

        while let Ok(cmd) = rx.recv() {
            match cmd {
                AudioCommand::Start(reply) => {
                    let host = cpal::default_host();
                    let device = match host.default_input_device() {
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

                    let channels = supported_config.channels();
                    sample_rate = supported_config.sample_rate().0;

                    {
                        let mut s = samples.lock().unwrap();
                        s.clear();
                    }

                    let err_fn = |err| eprintln!("Audio stream error: {}", err);
                    let samples_clone = Arc::clone(&samples);

                    let stream_res = match supported_config.sample_format() {
                        cpal::SampleFormat::F32 => device.build_input_stream(
                            &supported_config.into(),
                            move |data: &[f32], _| {
                                let mut s = samples_clone.lock().unwrap();
                                if channels > 1 {
                                    for chunk in data.chunks(channels as usize) {
                                        let mono: f32 = chunk.iter().sum::<f32>() / (channels as f32);
                                        s.push(mono);
                                    }
                                } else {
                                    s.extend_from_slice(data);
                                }
                            },
                            err_fn,
                            None,
                        ),
                        cpal::SampleFormat::I16 => {
                            let samples_clone = Arc::clone(&samples);
                            device.build_input_stream(
                                &supported_config.into(),
                                move |data: &[i16], _| {
                                    let mut s = samples_clone.lock().unwrap();
                                    if channels > 1 {
                                        for chunk in data.chunks(channels as usize) {
                                            let mono = chunk.iter().map(|&x| x as f32 / 32768.0).sum::<f32>()
                                                / (channels as f32);
                                            s.push(mono);
                                        }
                                    } else {
                                        s.extend(data.iter().map(|&x| x as f32 / 32768.0));
                                    }
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
                                let _ = reply.send(Err(format!("Failed to start stream: {}", e)));
                            } else {
                                _current_stream = Some(stream);
                                let _ = reply.send(Ok(()));
                            }
                        }
                        Err(e) => {
                            let _ = reply.send(Err(format!("Stream build error: {}", e)));
                        }
                    }
                }
                AudioCommand::Stop(reply) => {
                    _current_stream = None;
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
                    let mut s = samples.lock().unwrap();
                    s.clear();
                }
            }
        }
    }

    pub fn start_recording(&self) -> Result<(), String> {
        let (reply_tx, reply_rx) = channel();
        self.tx
            .send(AudioCommand::Start(reply_tx))
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
}
