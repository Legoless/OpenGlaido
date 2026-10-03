//! Persistent, bundled speech helper. Its ggml/Metal runtime is isolated from Whisper and llama.cpp.
//! New models currently transcribe completed recordings; their dictionary replacements still run
//! in the shared pipeline. This pinned engine does not implement vocabulary prompts for them.

use std::path::PathBuf;
use crate::processing::Cancellation;
use tauri::{AppHandle, Manager};

pub(super) fn supported_languages(id: &str) -> Option<&'static [&'static str]> {
    Some(match id {
        "parakeet-ultra-q8" => &["bg", "hr", "cs", "da", "nl", "en", "et", "fi", "fr", "de", "el", "hu", "it", "lv", "lt", "mt", "pl", "pt", "ro", "ru", "sk", "sl", "es", "sv", "uk"],
        "ark-asr-3b-q8" => &["zh", "en", "de", "ja", "fr", "ko", "es", "pl", "it", "ro", "hu", "cs", "nl", "fi", "hr", "sk", "sl", "et", "lt"],
        "qwen3-asr-1.7b-q8" => &["zh", "en", "yue", "ar", "de", "fr", "es", "pt", "id", "it", "ko", "ru", "th", "vi", "ja", "tr", "hi", "ms", "nl", "sv", "da", "fi", "pl", "cs", "tl", "fa", "el", "hu", "mk", "ro"],
        "cohere-transcribe-2b-q8" => &["en", "fr", "de", "it", "es", "pt", "el", "nl", "pl", "zh", "ja", "ko", "vi", "ar"],
        "voxtral-realtime-4b-q8" => &["en", "zh", "hi", "es", "ar", "fr", "pt", "ru", "de", "ja", "ko", "it", "nl"],
        _ => return None,
    })
}

pub(super) fn requires_language(id: &str) -> bool {
    id == "cohere-transcribe-2b-q8"
}

/// Language restrictions are checked before loading a model. No unsupported language is silently
/// changed to English. Auto-detection engines cannot constrain detection to a multi-language list.
fn language_for(id: &str, languages: &[String]) -> Result<String, String> {
    let supported = supported_languages(id).ok_or("Unknown transcription model")?;
    let languages: Vec<&str> = languages.iter().map(|s| s.trim()).filter(|s| !s.is_empty()).collect();
    if requires_language(id) && languages.len() != 1 {
        return Err("Cohere Transcribe needs one language. Choose it in Settings › Model.".into());
    }
    if languages.iter().any(|lang| !supported.contains(lang)) {
        return Err("This model does not support the selected dictation languages. Choose another model or change the languages in Settings.".into());
    }
    // Only Qwen and Cohere accept a forced language in the pinned native engine. The other models
    // detect it from audio; passing a language to Voxtral is explicitly rejected by its engine.
    if matches!(id, "qwen3-asr-1.7b-q8" | "cohere-transcribe-2b-q8") {
        if let [language] = languages.as_slice() {
            return Ok(if *language == "tl" { "fil" } else { language }.to_string());
        }
    }
    Ok(String::new())
}

pub async fn transcribe_cancellable(app: &AppHandle, id: &str, wav: Vec<u8>, languages: &[String], cancellation: Cancellation) -> Result<String, String> {
    cancellation.check()?;
    if !super::supported() {
        return Err(super::UNSUPPORTED.into());
    }
    let language = language_for(id, languages)?;
    let path = super::path_if_downloaded(app, id).ok_or("Download the transcription model in Settings › Model")?;
    let activity = app.state::<crate::updater::UpdateState>().activity()?;
    tokio::task::spawn_blocking(move || {
        // A dropped awaiting future cannot release update admission while the helper is running.
        let _activity = activity;
        imp::transcribe(&path, &wav, &language, &cancellation)
    })
        .await.map_err(|e| format!("Transcription failed: {e}"))?
}

pub fn preload(app: &AppHandle, id: &str) {
    let Some(path) = super::path_if_downloaded(app, id) else { return };
    let (app, id) = (app.clone(), id.to_string());
    std::thread::spawn(move || {
        if let Err(error) = imp::load(&path, || super::selected(&app, &id)) {
            eprintln!("Couldn't preload {}: {error}", path.display());
        }
    });
}

pub fn unload(path: Option<PathBuf>) {
    std::thread::spawn(move || imp::free(path.as_deref()));
}

pub fn unload_now() {
    imp::quit();
}

#[cfg(not(target_os = "macos"))]
mod imp {
    use std::path::Path;
    pub fn transcribe(_: &Path, _: &[u8], _: &str, _: &super::Cancellation) -> Result<String, String> { Err(super::super::UNSUPPORTED.into()) }
    pub fn load(_: &Path, _: impl FnOnce() -> bool) -> Result<(), String> { Err(super::super::UNSUPPORTED.into()) }
    pub fn free(_: Option<&Path>) {}
    pub fn quit() {}
}

#[cfg(target_os = "macos")]
mod imp {
    use crate::processing::{Cancellation, CANCELLED};
    use std::io::{BufWriter, Read, Write};
    use std::path::{Path, PathBuf};
    use std::process::{Child, ChildStdin, Command, Stdio};
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{mpsc, Mutex, MutexGuard, PoisonError};
    use std::time::{Duration, Instant};

    const RATE: usize = 16_000;
    const MAX_RESPONSE: usize = 4 * 1024 * 1024;
    static QUITTING: AtomicBool = AtomicBool::new(false);
    static CONTEXT: Mutex<Option<Helper>> = Mutex::new(None);

    struct Helper {
        path: PathBuf,
        child: Child,
        requests: mpsc::SyncSender<(Vec<f32>, String)>,
        replies: mpsc::Receiver<Result<String, String>>,
    }

    impl Drop for Helper {
        fn drop(&mut self) {
            // Includes timeout/crash recovery, model switches and quit. Never leave an orphan model
            // holding GPU memory; closing stdin also terminates the helper when the parent exits.
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
    }

    fn lock() -> MutexGuard<'static, Option<Helper>> { CONTEXT.lock().unwrap_or_else(PoisonError::into_inner) }

    fn helper_path() -> Result<PathBuf, String> {
        let executable = std::env::current_exe().map_err(|e| e.to_string())?;
        let folder = executable.parent().ok_or("Couldn't locate the local transcription engine")?;
        // Cargo test executables live in deps; the build stages the helper beside the app binary.
        let folder = if folder.file_name().is_some_and(|name| name == "deps") { folder.parent().unwrap_or(folder) } else { folder };
        let path = folder.join("openglaido-stt");
        if !path.is_file() { return Err("The local transcription engine is missing. Reinstall OpenGlaido.".into()); }
        Ok(path)
    }

    fn read_reply(reader: &mut impl Read) -> Result<String, String> {
        let mut header = [0; 8];
        reader.read_exact(&mut header).map_err(|_| "The local transcription engine stopped unexpectedly. Try again.".to_string())?;
        let status = u32::from_le_bytes(header[..4].try_into().unwrap());
        let length = u32::from_le_bytes(header[4..].try_into().unwrap()) as usize;
        if status > 1 || length > MAX_RESPONSE { return Err("The local transcription engine sent an invalid response".into()); }
        let mut bytes = vec![0; length];
        reader.read_exact(&mut bytes).map_err(|_| "The local transcription engine returned an incomplete response".to_string())?;
        let text = String::from_utf8(bytes).map_err(|_| "The local transcription engine returned invalid text".to_string())?;
        if status != 0 { return Err(format!("The local transcription model failed: {text}")); }
        Ok(text)
    }

    impl Helper {
        fn start(path: &Path, cancellation: &Cancellation) -> Result<Self, String> {
            cancellation.check()?;
            let mut child = Command::new(helper_path()?).arg(path).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::inherit())
                .spawn().map_err(|e| format!("Couldn't start the local transcription engine: {e}"))?;
            let mut input = BufWriter::new(child.stdin.take().unwrap());
            let mut output = child.stdout.take().unwrap();
            let (sender, replies) = mpsc::sync_channel(1);
            let (requests, receiver) = mpsc::sync_channel::<(Vec<f32>, String)>(1);
            std::thread::spawn(move || {
                let ready = read_reply(&mut output);
                let failed = ready.is_err();
                if sender.send(ready).is_err() || failed { return; }
                while let Ok((samples, language)) = receiver.recv() {
                    let reply = write_request(&mut input, &samples, &language)
                        .and_then(|()| read_reply(&mut output));
                    let failed = reply.is_err();
                    if sender.send(reply).is_err() || failed { break; }
                }
            });
            let helper = Self { path: path.to_path_buf(), child, requests, replies };
            if !helper.reply(Duration::from_secs(180), cancellation)?.is_empty() { return Err("The local transcription engine could not initialize".into()); }
            Ok(helper)
        }

        fn reply(&self, timeout: Duration, cancellation: &Cancellation) -> Result<String, String> {
            wait_reply(&self.replies, timeout, &QUITTING, cancellation)
        }

        fn run(&mut self, samples: Vec<f32>, language: &str, cancellation: &Cancellation) -> Result<String, String> {
            cancellation.check()?;
            let timeout = Duration::from_secs(120 + (samples.len() / RATE) as u64 * 2);
            self.requests.try_send((samples, language.to_string()))
                .map_err(|_| "The local transcription engine is unavailable. Try again.".to_string())?;
            // Covers BOTH writing a large recording and waiting for inference. A stuck child can
            // never block the main process indefinitely while its stdin pipe is full.
            self.reply(timeout, cancellation)
        }
    }

    fn wait_reply(replies: &mpsc::Receiver<Result<String, String>>, timeout: Duration, quitting: &AtomicBool, cancellation: &Cancellation) -> Result<String, String> {
        let deadline = Instant::now() + timeout;
        loop {
            if quitting.load(Ordering::Relaxed) { return Err(CANCELLED.into()); }
            cancellation.check()?;
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() { return Err("The local transcription model took too long. Try a smaller model.".into()); }
            match replies.recv_timeout(remaining.min(Duration::from_millis(100))) {
                Ok(result) => { cancellation.check()?; return result; }
                Err(mpsc::RecvTimeoutError::Timeout) => continue,
                Err(mpsc::RecvTimeoutError::Disconnected) => return Err("The local transcription engine stopped unexpectedly. Try again.".into()),
            }
        }
    }

    fn write_request(input: &mut BufWriter<ChildStdin>, samples: &[f32], language: &str) -> Result<(), String> {
        (|| -> std::io::Result<()> {
            input.write_all(&(samples.len() as u32).to_le_bytes())?;
            input.write_all(&(language.len() as u32).to_le_bytes())?;
            input.write_all(language.as_bytes())?;
            for sample in samples { input.write_all(&sample.to_le_bytes())?; }
            input.flush()
        })().map_err(|e| format!("Couldn't send audio to the local transcription engine: {e}"))
    }

    fn loaded<'a>(cached: &'a mut Option<Helper>, path: &Path, cancellation: &Cancellation) -> Result<&'a mut Helper, String> {
        if cached.as_ref().is_none_or(|helper| helper.path != path) {
            *cached = None;
            *cached = Some(Helper::start(path, cancellation)?);
        }
        Ok(cached.as_mut().unwrap())
    }

    pub fn load(path: &Path, wanted: impl FnOnce() -> bool) -> Result<(), String> {
        let mut cached = lock();
        if QUITTING.load(Ordering::Relaxed) || !wanted() { return Ok(()); }
        loaded(&mut cached, path, &Cancellation::default()).map(|_| ())
    }

    pub fn free(path: Option<&Path>) {
        let mut cached = lock();
        if path.is_none() || cached.as_ref().map(|helper| helper.path.as_path()) == path { *cached = None; }
    }

    pub fn quit() { QUITTING.store(true, Ordering::Relaxed); free(None); }


    pub fn transcribe(path: &Path, wav: &[u8], language: &str, cancellation: &Cancellation) -> Result<String, String> {
        cancellation.check()?;
        let samples = super::super::stt::decode_wav(wav, Some(1800))?;
        if samples.is_empty() { return Ok(String::new()); }
        let mut cached = lock();
        cancellation.check()?;
        if QUITTING.load(Ordering::Relaxed) { return Err(CANCELLED.into()); }
        let result = loaded(&mut cached, path, cancellation).and_then(|helper| helper.run(samples, language, cancellation));
        if result.is_err() { *cached = None; } // next attempt starts a fresh engine after a crash/error
        result.map(|text| text.trim().to_string())
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use std::io::Cursor;
        fn decode_wav(wav: &[u8]) -> Result<Vec<f32>, String> { super::super::super::stt::decode_wav(wav, Some(1800)) }
        #[test]
        fn quit_interrupts_a_stuck_engine_without_waiting_for_inference_deadline() {
            let (_sender, receiver) = mpsc::channel();
            let quitting = std::sync::Arc::new(AtomicBool::new(false));
            let flag = quitting.clone();
            let signal = std::thread::spawn(move || {
                std::thread::sleep(Duration::from_millis(10));
                flag.store(true, Ordering::Relaxed);
            });
            let start = Instant::now();
            assert_eq!(wait_reply(&receiver, Duration::from_secs(180), &quitting, &Cancellation::default()).unwrap_err(), "Cancelled");
            assert!(start.elapsed() < Duration::from_secs(5));
            signal.join().unwrap();
            assert!(wait_reply(&receiver, Duration::from_millis(1), &AtomicBool::new(false), &Cancellation::default()).unwrap_err().contains("too long"));
        }
        #[test]
        fn cancellation_interrupts_a_stalled_reply_without_shutting_down_the_engine_globally() {
            let (_sender, receiver) = mpsc::channel();
            let cancellation = Cancellation::default();
            let signal = cancellation.clone();
            let thread = std::thread::spawn(move || {
                std::thread::sleep(Duration::from_millis(10));
                signal.cancel();
            });
            let quitting = AtomicBool::new(false);
            let start = Instant::now();
            assert_eq!(wait_reply(&receiver, Duration::from_secs(180), &quitting, &cancellation).unwrap_err(), CANCELLED);
            assert!(start.elapsed() < Duration::from_secs(2));
            assert!(!quitting.load(Ordering::Relaxed));
            thread.join().unwrap();
            let (sender, receiver) = mpsc::channel();
            sender.send(Ok("next request".into())).unwrap();
            assert_eq!(wait_reply(&receiver, Duration::from_secs(1), &quitting, &Cancellation::default()).unwrap(), "next request");
        }

        #[test]
        fn invalid_or_failed_helper_responses_cannot_be_transcripts() {
            let response = |status: u32, text: &[u8]| { let mut r = status.to_le_bytes().to_vec(); r.extend((text.len() as u32).to_le_bytes()); r.extend(text); r };
            assert_eq!(read_reply(&mut Cursor::new(response(0, "Živjo".as_bytes()))).unwrap(), "Živjo");
            for bytes in [response(1, b"out of memory"), response(2, b"text"), response(0, &[255]), vec![0; 3], response(0, b"x")[..8].to_vec()] {
                assert!(read_reply(&mut Cursor::new(bytes)).is_err());
            }
            let mut huge = 0u32.to_le_bytes().to_vec(); huge.extend(((MAX_RESPONSE + 1) as u32).to_le_bytes());
            assert!(read_reply(&mut Cursor::new(huge)).is_err());
        }

        #[test]
        fn audio_decode_rejects_nan_and_downmixes_resamples_pcm() {
            let encode = |samples: &[f32]| {
                let mut data = Cursor::new(Vec::new());
                let spec = hound::WavSpec { channels: 2, sample_rate: 8000, bits_per_sample: 32, sample_format: hound::SampleFormat::Float };
                let mut writer = hound::WavWriter::new(&mut data, spec).unwrap();
                for sample in samples { writer.write_sample(*sample).unwrap(); }
                writer.finalize().unwrap(); data.into_inner()
            };
            let pcm = decode_wav(&encode(&[0.5, 0.0, 0.5, 0.0])).unwrap();
            assert_eq!(pcm.len(), 4); assert!(pcm.iter().all(|s| (s - 0.25).abs() < 0.0001));
            assert!(decode_wav(&encode(&[f32::NAN, 0.0])).is_err());
            assert!(decode_wav(b"broken wav").is_err());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn native_languages_are_explicit_and_do_not_silently_fallback() {
        let langs = |items: &[&str]| items.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        let cohere = "cohere-transcribe-2b-q8";
        for input in [vec![], langs(&["en", "de"]), langs(&["sl"]), langs(&["en\0"])] { assert!(language_for(cohere, &input).is_err()); }
        assert_eq!(language_for(cohere, &langs(&["de"])).unwrap(), "de");
        assert_eq!(language_for("qwen3-asr-1.7b-q8", &langs(&["tl"])).unwrap(), "fil");
        assert_eq!(language_for("qwen3-asr-1.7b-q8", &[]).unwrap(), "");
        assert_eq!(language_for("voxtral-realtime-4b-q8", &langs(&["en"])).unwrap(), "");
        assert!(language_for("voxtral-realtime-4b-q8", &langs(&["sl"])).is_err());
        assert_eq!(language_for("parakeet-ultra-q8", &langs(&["sl"])).unwrap(), "");
        assert_eq!(language_for("ark-asr-3b-q8", &langs(&["sl"])).unwrap(), "");
        assert!(language_for("not-a-model", &[]).is_err());
    }
}
