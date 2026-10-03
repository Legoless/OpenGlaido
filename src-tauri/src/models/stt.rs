//! Dispatch local speech models without loading another ggml runtime into the app process.

use std::path::Path;
use crate::processing::Cancellation;
use tauri::AppHandle;

/// Shared PCM conversion for local engines. Native helper input is bounded before allocating it;
/// Whisper keeps its existing duration support. Display-only waveform gain never affects this data.
#[cfg(target_os = "macos")]
pub(super) fn decode_wav(wav: &[u8], max_seconds: Option<u64>) -> Result<Vec<f32>, String> {
    let mut reader = hound::WavReader::new(std::io::Cursor::new(wav)).map_err(|e| format!("Couldn't read recorded audio: {e}"))?;
    let spec = reader.spec();
    if spec.channels == 0 || spec.sample_rate == 0 || !(1..=32).contains(&spec.bits_per_sample) {
        return Err("The recording has an unsupported audio format".into());
    }
    if max_seconds.is_some_and(|seconds| reader.duration() as u64 > seconds * spec.sample_rate as u64) {
        return Err("Local transcription supports recordings up to 30 minutes".into());
    }
    let samples: Vec<f32> = match spec.sample_format {
        hound::SampleFormat::Float => reader.samples::<f32>().collect::<Result<_, _>>(),
        hound::SampleFormat::Int => {
            let scale = (1u64 << (spec.bits_per_sample - 1)) as f32;
            reader.samples::<i32>().map(|sample| sample.map(|sample| sample as f32 / scale)).collect::<Result<_, _>>()
        }
    }.map_err(|e| format!("Couldn't read recorded audio: {e}"))?;
    if samples.iter().any(|sample| !sample.is_finite()) { return Err("The recording contains invalid audio samples".into()); }
    let mono: Vec<f32> = samples.chunks(spec.channels as usize).map(|c| c.iter().sum::<f32>() / c.len() as f32).collect();
    Ok(crate::audio::resample(&mono, spec.sample_rate, 16_000))
}

pub async fn transcribe(
    app: &AppHandle,
    id: &str,
    wav: Vec<u8>,
    languages: &[String],
    prompt: Option<String>,
) -> Result<String, String> {
    transcribe_cancellable(app, id, wav, languages, prompt, Cancellation::default()).await
}

pub async fn transcribe_cancellable(
    app: &AppHandle,
    id: &str,
    wav: Vec<u8>,
    languages: &[String],
    prompt: Option<String>,
    cancellation: Cancellation,
) -> Result<String, String> {
    cancellation.check()?;
    let model = super::find(id).filter(|m| m.kind == "stt").ok_or("Unknown transcription model")?;
    match model.backend {
        "whisper" => super::whisper::transcribe_cancellable(app, id, wav, languages, prompt, cancellation).await,
        "native" => super::native::transcribe_cancellable(app, id, wav, languages, cancellation).await,
        _ => Err("Unsupported transcription engine".into()),
    }
}

pub fn preload(app: &AppHandle, id: &str) {
    match super::find(id).filter(|m| m.kind == "stt").map(|m| m.backend) {
        Some("native") => {
            super::whisper::unload();
            super::native::preload(app, id);
        }
        Some("whisper") => {
            super::native::unload(None);
            super::whisper::preload(app, id);
        }
        _ => {}
    }
}

pub fn unload() {
    super::whisper::unload();
    super::native::unload(None);
}

pub(super) fn unload_path(path: &Path) {
    super::whisper::unload_path(path);
    super::native::unload(Some(path.to_path_buf()));
}

pub fn unload_now() {
    super::native::unload_now();
    super::whisper::unload_now();
}
