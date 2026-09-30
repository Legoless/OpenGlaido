//! Local transcription with whisper.cpp (whisper-rs, Metal). macOS only; elsewhere every call fails.

use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use tauri::AppHandle;

pub use imp::transcribe_file;

/// Set on quit: nothing may load a model after `unload_now` (ggml-metal aborts at exit while one is loaded).
static QUITTING: AtomicBool = AtomicBool::new(false);

/// Transcribes our pipeline's 16 kHz mono 16-bit WAV with catalog model `model_id`.
/// `languages`: [] = auto-detect, [x] = pinned, ≥2 = the most likely of those. `prompt` biases vocabulary.
pub async fn transcribe(
    app: &AppHandle,
    model_id: &str,
    wav: Vec<u8>,
    languages: &[String],
    prompt: Option<String>,
) -> Result<String, String> {
    if !super::supported() {
        return Err(super::UNSUPPORTED.into());
    }
    let path = super::path_if_downloaded(app, model_id).ok_or("Download the transcription model in Settings › Model")?;
    let languages = languages.to_vec();
    tokio::task::spawn_blocking(move || transcribe_file(&path, &wav, &languages, prompt.as_deref()))
        .await
        .map_err(|e| format!("Transcription failed: {e}"))?
}

/// Loads the model in the background so the first dictation is fast (errors are only logged).
pub fn preload(app: &AppHandle, model_id: &str) {
    let Some(path) = super::path_if_downloaded(app, model_id) else { return };
    let (app, id) = (app.clone(), model_id.to_string());
    std::thread::spawn(move || {
        if let Err(e) = imp::load(&path, || super::selected(&app, &id)) {
            eprintln!("Couldn't preload {}: {}", path.display(), e);
        }
    });
}

/// Frees the loaded model (e.g. after it was deleted or the source changed).
pub fn unload() {
    imp::unload(None);
}

/// Frees the loaded model if it is `path`.
pub(super) fn unload_path(path: &Path) {
    imp::unload(Some(path.to_path_buf()));
}

/// Frees the loaded model now, waiting for a running transcription. Call before the process exits: ggml-metal's
/// exit-time destructor aborts while a model still holds GPU memory.
pub fn unload_now() {
    QUITTING.store(true, Ordering::Relaxed);
    imp::free(None);
}

#[cfg(not(target_os = "macos"))]
mod imp {
    use std::path::{Path, PathBuf};

    pub fn transcribe_file(_: &Path, _: &[u8], _: &[String], _: Option<&str>) -> Result<String, String> {
        Err(super::super::UNSUPPORTED.into())
    }

    pub fn load(_: &Path, _: impl FnOnce() -> bool) -> Result<(), String> {
        Err(super::super::UNSUPPORTED.into())
    }

    pub fn unload(_: Option<PathBuf>) {}

    pub fn free(_: Option<&Path>) {}
}

#[cfg(target_os = "macos")]
mod imp {
    use std::io::Cursor;
    use std::path::{Path, PathBuf};
    use std::sync::atomic::Ordering;
    use std::sync::{Mutex, MutexGuard, PoisonError};
    use whisper_rs::{FullParams, SamplingStrategy, WhisperContext, WhisperContextParameters};

    const RATE: u32 = 16_000;

    /// The loaded model and its path; the lock also serializes transcriptions.
    static CONTEXT: Mutex<Option<(PathBuf, WhisperContext)>> = Mutex::new(None);

    fn lock() -> MutexGuard<'static, Option<(PathBuf, WhisperContext)>> {
        CONTEXT.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn loaded<'a>(cached: &'a mut Option<(PathBuf, WhisperContext)>, path: &Path) -> Result<&'a WhisperContext, String> {
        if cached.as_ref().is_none_or(|(p, _)| p != path) {
            *cached = None; // free the old model before loading the new one
            whisper_rs::install_logging_hooks();
            let ctx = WhisperContext::new_with_params(path, WhisperContextParameters::default()).map_err(|e| {
                eprintln!("Couldn't load {}: {e}", path.display());
                "Couldn't load the transcription model. Delete and download it again in Settings › Model.".to_string()
            })?;
            *cached = Some((path.to_path_buf(), ctx));
        }
        Ok(&cached.as_ref().unwrap().1)
    }

    /// Loads `path` unless `wanted()`, asked once the lock is ours, says a save switched away meanwhile
    /// (a running transcription held the lock).
    pub fn load(path: &Path, wanted: impl FnOnce() -> bool) -> Result<(), String> {
        let mut cached = lock();
        if !wanted() || super::QUITTING.load(Ordering::Relaxed) {
            return Ok(());
        }
        loaded(&mut cached, path).map(|_| ())
    }

    pub fn unload(path: Option<PathBuf>) {
        // On a thread: a running transcription holds the lock, and callers may be on the main thread.
        std::thread::spawn(move || free(path.as_deref()));
    }

    /// Frees the model if it is `path` (None = any), waiting for a running transcription.
    pub fn free(path: Option<&Path>) {
        let mut cached = lock();
        if path.is_none() || cached.as_ref().map(|(p, _)| p.as_path()) == path {
            *cached = None;
        }
    }

    /// The core of `transcribe`, without a Tauri app: `wav` with the model file at `model_path`.
    pub fn transcribe_file(
        model_path: &Path,
        wav: &[u8],
        languages: &[String],
        prompt: Option<&str>,
    ) -> Result<String, String> {
        let fail = |e: whisper_rs::WhisperError| {
            eprintln!("Whisper failed: {e}");
            "Transcription failed: the local model couldn't process the audio".to_string()
        };
        let pcm = decode_wav(wav)?;
        if pcm.is_empty() {
            return Ok(String::new());
        }
        let mut cached = lock();
        if super::QUITTING.load(Ordering::Relaxed) {
            return Err("Cancelled".into());
        }
        let ctx = loaded(&mut cached, model_path)?;
        let mut state = ctx.create_state().map_err(fail)?;
        let threads = std::thread::available_parallelism().map_or(4, |n| n.get()).min(8);

        // Pre-v3 models know 99 languages, v3 100 ("yue"); an id past that is the translate token.
        let known = ctx.n_vocab() - 51766;
        let allowed: Vec<&str> = languages
            .iter()
            .map(|l| l.trim())
            .filter(|l| !l.contains('\0') && whisper_rs::get_lang_id(l).is_some_and(|id| id < known))
            .collect();
        let language = if !ctx.is_multilingual() {
            "en"
        } else if let [lang] = allowed.as_slice() {
            lang
        } else if allowed.len() > 1 {
            // No allow-list in whisper.cpp: detect once and take the most likely allowed language.
            state.pcm_to_mel(&pcm, threads).map_err(fail)?;
            let (_, probs) = state.lang_detect(0, threads).map_err(fail)?;
            most_likely(&allowed, &probs)
        } else {
            "auto"
        };

        let mut params = FullParams::new(SamplingStrategy::Greedy { best_of: 1 });
        params.set_n_threads(threads as i32);
        params.set_language(Some(language));
        params.set_suppress_nst(true);
        params.set_print_special(false);
        params.set_print_progress(false);
        params.set_print_realtime(false);
        params.set_print_timestamps(false);
        let prompt = prompt.unwrap_or_default().replace('\0', "");
        if !prompt.trim().is_empty() {
            params.set_initial_prompt(&prompt);
        }
        state.full(params, &pcm).map_err(fail)?;
        let text: String = state.as_iter().map(|s| strip_markers(&s.to_str_lossy().unwrap_or_default())).collect();
        Ok(text.split_whitespace().collect::<Vec<_>>().join(" "))
    }

    /// The allowed language with the highest detection probability (`probs` is indexed by whisper language id).
    fn most_likely<'a>(allowed: &[&'a str], probs: &[f32]) -> &'a str {
        let prob = |l: &str| whisper_rs::get_lang_id(l).and_then(|id| probs.get(id as usize).copied()).unwrap_or(0.0);
        allowed.iter().copied().max_by(|a, b| prob(a).total_cmp(&prob(b))).unwrap_or("auto")
    }

    /// Mono f32 samples at 16 kHz from any PCM WAV (downmixed and resampled when needed).
    fn decode_wav(wav: &[u8]) -> Result<Vec<f32>, String> {
        let bad = |e: hound::Error| format!("Transcription failed: unreadable audio ({e})");
        let mut reader = hound::WavReader::new(Cursor::new(wav)).map_err(bad)?;
        let spec = reader.spec();
        let samples: Vec<f32> = match spec.sample_format {
            hound::SampleFormat::Float => reader.samples::<f32>().collect::<Result<_, _>>().map_err(bad)?,
            hound::SampleFormat::Int => {
                let scale = (1u64 << (spec.bits_per_sample - 1)) as f32;
                reader.samples::<i32>().map(|s| s.map(|s| s as f32 / scale)).collect::<Result<_, _>>().map_err(bad)?
            }
        };
        let channels = spec.channels.max(1) as usize;
        let mono: Vec<f32> = samples.chunks(channels).map(|c| c.iter().sum::<f32>() / c.len() as f32).collect();
        Ok(crate::audio::resample(&mono, spec.sample_rate, RATE))
    }

    /// Drops whisper's non-speech markers from a segment: any `[...]` (e.g. `[BLANK_AUDIO]`, `[Music]`) and a
    /// segment that is only a `(...)` or `*...*` note (e.g. `(music)`, `*laughs*`). Dictated parentheses mid-text stay.
    fn strip_markers(segment: &str) -> String {
        let t = segment.trim();
        let note = |open: char, close: char| {
            t.len() > 1 && t.starts_with(open) && t.ends_with(close) && !t[1..t.len() - 1].contains([open, close])
        };
        if note('(', ')') || note('*', '*') {
            return String::new();
        }
        let mut out = String::with_capacity(segment.len());
        let mut rest = segment;
        while let Some(start) = rest.find('[') {
            let Some(end) = rest[start..].find(']') else { break };
            out.push_str(&rest[..start]);
            out.push(' ');
            rest = &rest[start + end + 1..];
        }
        out.push_str(rest);
        out
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn strips_non_speech_markers() {
            let clean = |segments: &[&str]| {
                let text: String = segments.iter().map(|s| strip_markers(s)).collect();
                text.split_whitespace().collect::<Vec<_>>().join(" ")
            };
            assert_eq!(clean(&[" [BLANK_AUDIO]"]), "");
            assert_eq!(clean(&[" Hello there.", " (music)", " [Music] How are you?"]), "Hello there. How are you?");
            assert_eq!(clean(&[" *laughs*", " Okay [BLANK_AUDIO] then."]), "Okay then.");
            assert_eq!(clean(&[" Call me (maybe) tomorrow."]), "Call me (maybe) tomorrow.");
            assert_eq!(clean(&[" (see above) and (below)"]), "(see above) and (below)");
            assert_eq!(clean(&[" An open [bracket"]), "An open [bracket");
        }

        #[test]
        fn decodes_and_resamples_wav() {
            let spec = hound::WavSpec { channels: 2, sample_rate: 8000, bits_per_sample: 16, sample_format: hound::SampleFormat::Int };
            let mut cursor = Cursor::new(Vec::new());
            let mut writer = hound::WavWriter::new(&mut cursor, spec).unwrap();
            for _ in 0..800 {
                writer.write_sample(16384i16).unwrap();
                writer.write_sample(0i16).unwrap();
            }
            writer.finalize().unwrap();
            let pcm = decode_wav(&cursor.into_inner()).unwrap();
            assert_eq!(pcm.len(), 1600);
            assert!(pcm.iter().all(|s| (s - 0.25).abs() < 1e-4));
            assert!(decode_wav(b"not a wav").is_err());
        }

        #[test]
        fn picks_the_most_likely_allowed_language() {
            let mut probs = vec![0.0; whisper_rs::get_lang_max_id() as usize + 1];
            probs[whisper_rs::get_lang_id("en").unwrap() as usize] = 0.7;
            probs[whisper_rs::get_lang_id("de").unwrap() as usize] = 0.2;
            probs[whisper_rs::get_lang_id("sl").unwrap() as usize] = 0.1;
            assert_eq!(most_likely(&["sl", "de"], &probs), "de");
            assert_eq!(most_likely(&["en", "sl"], &probs), "en");
            assert_eq!(most_likely(&[], &probs), "auto");
        }
    }
}
