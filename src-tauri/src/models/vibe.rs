//! Microsoft BitNet ASR runs in its own warm native process; it never links a third ggml into Tauri.
use crate::processing::Cancellation;
use std::path::PathBuf;
use tauri::{AppHandle, Manager};

pub(super) const LANGUAGES: &[&str] = &["en", "zh", "fr", "it", "ko", "pt", "vi"];
const VAE_FILE: &str = "vibeasr-vae-encoder-i8_s.gguf";

pub async fn transcribe_cancellable(app: &AppHandle, id: &str, wav: Vec<u8>, terms: &[String], languages: &[String], cancellation: Cancellation) -> Result<String, String> {
    cancellation.check()?;
    if languages.iter().any(|language| !LANGUAGES.contains(&language.trim())) {
        return Err("This model does not support the selected dictation languages. Choose another model or change the languages in Settings.".into());
    }
    let lm = super::path_if_downloaded(app, id).ok_or("Download the transcription model in Settings › Model")?;
    let vae = lm.with_file_name(VAE_FILE);
    if !vae.is_file() { return Err("Download the complete VibeVoice BitNet model in Settings › Model".into()); }
    let hints = dictionary_context(terms);
    let activity = app.state::<crate::updater::UpdateState>().activity()?;
    tokio::task::spawn_blocking(move || {
        // Includes cancellation's kill + wait: a dropped caller cannot let an update interrupt
        // a still-running helper or remove the temporary input while it is being read.
        let _activity = activity;
        imp::transcribe(&lm, &vae, &wav, &hints, &cancellation)
    }).await.map_err(|error| format!("Transcription failed: {error}"))?
}

fn dictionary_context(terms: &[String]) -> String {
    let mut accepted: Vec<&str> = Vec::new();
    let mut bytes = 0;
    for term in terms {
        if term.contains(['\n', '\r', '\0']) { continue; }
        let term = term.trim();
        if term.is_empty() || accepted.contains(&term) { continue; }
        // ponytail: upstream's command buffer is 4096 bytes; retain complete phrases up to 3800
        // and expand the binary request protocol if a larger recognition glossary is needed.
        let extra = term.len() + if accepted.is_empty() { 0 } else { 2 };
        if bytes + extra > 3800 { continue; }
        bytes += extra;
        accepted.push(term);
    }
    accepted.join(", ")
}

pub fn preload(app: &AppHandle, id: &str) {
    let Some(lm) = super::path_if_downloaded(app, id) else { return };
    let vae = lm.with_file_name(VAE_FILE);
    if !vae.is_file() { return; }
    let Ok(activity) = app.state::<crate::updater::UpdateState>().activity() else { return };
    let (app, id) = (app.clone(), id.to_string());
    std::thread::spawn(move || {
        let _activity = activity;
        if let Err(error) = imp::load(&lm, &vae, || super::selected(&app, &id)) {
            eprintln!("Couldn't preload Microsoft transcription model: {error}");
        }
    });
}

pub fn unload(path: Option<PathBuf>) { std::thread::spawn(move || imp::free(path.as_deref())); }
pub fn unload_now() { imp::quit(); }

#[cfg(not(target_os = "macos"))]
mod imp {
    use std::path::Path;
    pub fn transcribe(_: &Path, _: &Path, _: &[u8], _: &str, _: &super::Cancellation) -> Result<String, String> { Err(super::super::UNSUPPORTED.into()) }
    pub fn load(_: &Path, _: &Path, _: impl FnOnce() -> bool) -> Result<(), String> { Err(super::super::UNSUPPORTED.into()) }
    pub fn free(_: Option<&Path>) {}
    pub fn quit() {}
}

#[cfg(target_os = "macos")]
mod imp {
    use crate::processing::{Cancellation, CANCELLED};
    use std::fs::OpenOptions;
    use std::io::{BufWriter, Read, Write};
    use std::os::unix::fs::OpenOptionsExt;
    use std::path::{Path, PathBuf};
    use std::process::{Child, Command, Stdio};
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{mpsc, Mutex, MutexGuard, PoisonError, TryLockError};
    use std::time::{Duration, Instant};

    static QUITTING: AtomicBool = AtomicBool::new(false);
    static CONTEXT: Mutex<Option<Helper>> = Mutex::new(None);
    const MAX_REPLY: usize = 4 * 1024 * 1024;

    struct Helper {
        lm: PathBuf,
        vae: PathBuf,
        child: Child,
        requests: mpsc::SyncSender<(String, String)>,
        replies: mpsc::Receiver<Result<String, String>>,
    }

    impl Drop for Helper {
        fn drop(&mut self) { let _ = self.child.kill(); let _ = self.child.wait(); }
    }

    fn helper_path() -> Result<PathBuf, String> {
        let executable = std::env::current_exe().map_err(|error| error.to_string())?;
        let folder = executable.parent().ok_or("Couldn't locate the local transcription engine")?;
        let folder = if folder.file_name().is_some_and(|name| name == "deps") { folder.parent().unwrap_or(folder) } else { folder };
        let path = folder.join("openglaido-vibe");
        if !path.is_file() { return Err("The local transcription engine is missing. Reinstall OpenGlaido.".into()); }
        Ok(path)
    }

    fn read_reply(reader: &mut impl Read) -> Result<String, String> {
        let mut header = [0; 8];
        reader.read_exact(&mut header).map_err(|_| "The Microsoft transcription engine stopped unexpectedly. Try again.".to_string())?;
        let status = u32::from_le_bytes(header[..4].try_into().unwrap());
        let count = u32::from_le_bytes(header[4..].try_into().unwrap()) as usize;
        if status > 1 || count > MAX_REPLY { return Err("The Microsoft transcription engine sent an invalid response".into()); }
        let mut bytes = vec![0; count];
        reader.read_exact(&mut bytes).map_err(|_| "The Microsoft transcription engine returned incomplete text".to_string())?;
        let text = String::from_utf8(bytes).map_err(|_| "The Microsoft transcription engine returned invalid text".to_string())?;
        if status == 1 { return Err(format!("The Microsoft transcription model failed: {text}")); }
        Ok(text)
    }

    fn wait_reply(replies: &mpsc::Receiver<Result<String, String>>, timeout: Duration, quitting: &AtomicBool, cancellation: &Cancellation) -> Result<String, String> {
        let deadline = Instant::now() + timeout;
        loop {
            cancellation.check()?;
            if quitting.load(Ordering::Relaxed) { return Err(CANCELLED.into()); }
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() { return Err("The local transcription model took too long. Try a shorter recording.".into()); }
            match replies.recv_timeout(remaining.min(Duration::from_millis(50))) {
                Ok(result) => { cancellation.check()?; return result; }
                Err(mpsc::RecvTimeoutError::Timeout) => {}
                Err(mpsc::RecvTimeoutError::Disconnected) => return Err("The Microsoft transcription engine stopped unexpectedly. Try again.".into()),
            }
        }
    }

    fn lock(cancellation: &Cancellation) -> Result<MutexGuard<'static, Option<Helper>>, String> {
        loop {
            cancellation.check()?;
            if QUITTING.load(Ordering::Relaxed) { return Err(CANCELLED.into()); }
            match CONTEXT.try_lock() {
                Ok(guard) => return Ok(guard),
                Err(TryLockError::Poisoned(error)) => return Ok(error.into_inner()),
                Err(TryLockError::WouldBlock) => std::thread::sleep(Duration::from_millis(25)),
            }
        }
    }

    impl Helper {
        fn start(lm: &Path, vae: &Path, cancellation: &Cancellation) -> Result<Self, String> {
            cancellation.check()?;
            let mut child = Command::new(helper_path()?).args(["--lm-model"]).arg(lm).arg("--vae-model").arg(vae)
                .args(["--greedy", "--no-token-stream", "-c", "32768", "--max-tokens", "16384"])
                .stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::null())
                .spawn().map_err(|error| format!("Couldn't start Microsoft transcription: {error}"))?;
            let mut input = BufWriter::new(child.stdin.take().unwrap());
            let mut output = child.stdout.take().unwrap();
            let (sender, replies) = mpsc::sync_channel(1);
            let (requests, receiver) = mpsc::sync_channel::<(String, String)>(1);
            std::thread::spawn(move || {
                let ready = read_reply(&mut output);
                let failed = ready.is_err();
                if sender.send(ready).is_err() || failed { return; }
                while let Ok((audio, hints)) = receiver.recv() {
                    let result = (|| {
                        writeln!(input, "CONTEXT:{hints}").and_then(|()| input.flush()).map_err(|error| error.to_string())?;
                        if !read_reply(&mut output)?.is_empty() { return Err("Unexpected Microsoft context response".into()); }
                        writeln!(input, "{audio}").and_then(|()| input.flush()).map_err(|error| error.to_string())?;
                        read_reply(&mut output)
                    })();
                    let failed = result.is_err();
                    if sender.send(result).is_err() || failed { break; }
                }
            });
            let helper = Self { lm: lm.to_path_buf(), vae: vae.to_path_buf(), child, requests, replies };
            if !wait_reply(&helper.replies, Duration::from_secs(180), &QUITTING, cancellation)?.is_empty() {
                return Err("Microsoft transcription could not initialize".into());
            }
            Ok(helper)
        }
    }

    fn loaded<'a>(cached: &'a mut Option<Helper>, lm: &Path, vae: &Path, cancellation: &Cancellation) -> Result<&'a mut Helper, String> {
        if cached.as_ref().is_none_or(|helper| helper.lm != lm || helper.vae != vae) {
            *cached = None;
            *cached = Some(Helper::start(lm, vae, cancellation)?);
        }
        Ok(cached.as_mut().unwrap())
    }

    pub fn load(lm: &Path, vae: &Path, wanted: impl FnOnce() -> bool) -> Result<(), String> {
        let cancellation = Cancellation::default();
        let mut cached = lock(&cancellation)?;
        if !wanted() { return Ok(()); }
        loaded(&mut cached, lm, vae, &cancellation).map(|_| ())
    }

    pub fn free(path: Option<&Path>) {
        let mut cached = CONTEXT.lock().unwrap_or_else(PoisonError::into_inner);
        if path.is_none_or(|path| cached.as_ref().is_some_and(|helper| helper.lm == path || helper.vae == path)) { *cached = None; }
    }
    pub fn quit() { QUITTING.store(true, Ordering::Relaxed); free(None); }

    struct AudioFile(PathBuf);
    impl Drop for AudioFile { fn drop(&mut self) { let _ = std::fs::remove_file(&self.0); } }

    fn audio_file(samples: &[f32]) -> Result<AudioFile, String> {
        let path = std::env::temp_dir().join(format!("openglaido-vibe-{}.wav", uuid::Uuid::new_v4()));
        let audio = AudioFile(path);
        let file = OpenOptions::new().create_new(true).write(true).mode(0o600).open(&audio.0).map_err(|error| error.to_string())?;
        let spec = hound::WavSpec { channels: 1, sample_rate: 16000, bits_per_sample: 16, sample_format: hound::SampleFormat::Int };
        let mut writer = hound::WavWriter::new(file, spec).map_err(|error| error.to_string())?;
        for sample in samples { writer.write_sample((sample.clamp(-1.0, 1.0) * i16::MAX as f32) as i16).map_err(|error| error.to_string())?; }
        writer.finalize().map_err(|error| error.to_string())?;
        Ok(audio)
    }

    pub fn transcribe(lm: &Path, vae: &Path, wav: &[u8], hints: &str, cancellation: &Cancellation) -> Result<String, String> {
        cancellation.check()?;
        let samples = super::super::stt::decode_wav(wav, Some(1800))?;
        if samples.is_empty() { return Ok(String::new()); }
        let audio = audio_file(&samples)?;
        let path = audio.0.to_str().ok_or("Recorded audio path must be valid UTF-8")?;
        if path.len() >= 4000 || path.contains(['\n', '\r', '\0']) { return Err("Recorded audio path is not supported".into()); }
        let mut cached = lock(cancellation)?;
        let result = (|| {
            let helper = loaded(&mut cached, lm, vae, cancellation)?;
            helper.requests.try_send((path.to_string(), hints.to_string())).map_err(|_| "Microsoft transcription is unavailable. Try again.".to_string())?;
            wait_reply(&helper.replies, Duration::from_secs(180 + samples.len() as u64 / 16000 * 3), &QUITTING, cancellation)
        })();
        if result.is_err() { *cached = None; }
        result.map(|text| text.trim().to_string())
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use std::io::Cursor;
        fn reply(status: u32, text: &[u8]) -> Vec<u8> {
            [status.to_le_bytes().as_slice(), (text.len() as u32).to_le_bytes().as_slice(), text].concat()
        }
        #[test]
        fn replies_preserve_utf8_and_control_markers_and_reject_partial_outputs() {
            let text = "---END---\nété prochain\n你好";
            assert_eq!(read_reply(&mut Cursor::new(reply(0, text.as_bytes()))).unwrap(), text);
            assert!(read_reply(&mut Cursor::new(reply(1, b"output limit reached"))).unwrap_err().contains("output limit"));
            assert!(read_reply(&mut Cursor::new(reply(0, &[255]))).is_err());
            assert!(read_reply(&mut Cursor::new(reply(4, b"bad"))).is_err());
            assert!(read_reply(&mut Cursor::new([0u32.to_le_bytes(), u32::MAX.to_le_bytes()].concat())).is_err());
            let mut bytes = reply(0, b"incomplete"); bytes.pop();
            assert!(read_reply(&mut Cursor::new(bytes)).is_err());
        }
        #[test]
        fn cancellation_and_quit_interrupt_stalled_model_replies() {
            let (_sender, receiver) = mpsc::channel();
            let cancellation = Cancellation::default();
            let signal = cancellation.clone();
            let worker = std::thread::spawn(move || { std::thread::sleep(Duration::from_millis(10)); signal.cancel(); });
            let started = Instant::now();
            assert_eq!(wait_reply(&receiver, Duration::from_secs(180), &AtomicBool::new(false), &cancellation).unwrap_err(), CANCELLED);
            assert!(started.elapsed() < Duration::from_secs(2));
            worker.join().unwrap();
            assert_eq!(wait_reply(&receiver, Duration::from_secs(180), &AtomicBool::new(true), &Cancellation::default()).unwrap_err(), CANCELLED);
        }
        #[test]
        fn cancelled_helper_is_physically_reaped_before_its_guard_is_released() {
            let child = Command::new("/bin/sleep").arg("30").spawn().unwrap();
            let pid = child.id();
            let (requests, _receiver) = mpsc::sync_channel(1);
            let (_sender, replies) = mpsc::channel();
            let helper = Helper { lm: PathBuf::new(), vae: PathBuf::new(), child, requests, replies };
            let started = Instant::now();
            drop(helper);
            assert!(started.elapsed() < Duration::from_secs(2));
            // kill -0 checks process existence without signaling or launching model inference.
            assert!(!Command::new("/bin/kill").args(["-0", &pid.to_string()]).stderr(Stdio::null()).status().unwrap().success());
        }
        #[test]
        fn temporary_audio_is_private_and_removed_after_use() {
            let audio = audio_file(&[0.0, 0.5, -0.5]).unwrap();
            let path = audio.0.clone();
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(std::fs::metadata(&path).unwrap().permissions().mode() & 0o777, 0o600);
            assert_eq!(hound::WavReader::open(&path).unwrap().duration(), 3);
            drop(audio);
            assert!(!path.exists());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn dictionary_context_cannot_inject_native_commands_or_exceed_the_line_buffer() {
        let terms = vec![" OpenGlaido ".into(), "OpenGlaido".into(), "été".into(), "\nEXIT\n".into(), "bad\0value".into(), "x".repeat(5000)];
        assert_eq!(dictionary_context(&terms), "OpenGlaido, été");
        let context = dictionary_context(&["é".repeat(1900), "tail".into()]);
        assert_eq!(context.len(), 3800);
        assert!(!LANGUAGES.contains(&"sl"));
    }
}
