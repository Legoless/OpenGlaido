//! Offline, bundled Microsoft speech runtime. The Python/Torch process is isolated from Tauri.
use crate::audio::StreamInput;
use crate::processing::Cancellation;
use std::path::PathBuf;
use std::sync::{atomic::AtomicBool, Arc};
use tauri::{AppHandle, Manager};
use tokio::sync::{mpsc, oneshot};

pub const UNSUPPORTED: &str = "Microsoft local models require an Apple Silicon Mac with macOS 14 or later.";
#[cfg(target_os = "macos")]
const RATE: u32 = 16_000;
#[cfg(target_os = "macos")]
const MAX_SAMPLES: usize = RATE as usize * 30 * 60;
const MAX_METADATA: usize = 64 * 1024;

pub fn supported() -> bool {
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    {
        static SUPPORTED: std::sync::LazyLock<bool> = std::sync::LazyLock::new(|| std::process::Command::new("/usr/bin/sw_vers").arg("-productVersion")
            .output().ok().and_then(|output| String::from_utf8(output.stdout).ok())
            .and_then(|version| version.trim().split('.').next()?.parse::<u32>().ok()).is_some_and(|major| major >= 14));
        *SUPPORTED
    }
    #[cfg(not(all(target_os = "macos", target_arch = "aarch64")))]
    { false }
}

pub fn supported_languages(id: &str) -> Option<&'static [&'static str]> {
    Some(match id {
        "vibevoice-asr" => &["en", "zh", "es", "pt", "de", "ja", "ko", "fr", "ru", "id", "sv", "it", "he", "nl", "pl", "no", "tr", "th", "ar", "hu", "ca", "cs", "da", "fa", "af", "hi", "fi", "et", "aa", "el", "ro", "vi", "bg", "is", "sl", "sk", "lt", "sw", "uk", "kl", "lv", "hr", "ne", "sr", "tl", "yi", "ms", "ur", "mn", "hy", "jv"],
        "vibevoice-asr-streaming-7b" => &["en", "zh", "es", "pt", "de", "ja", "ko", "fr", "ru", "it"],
        // Phi's 24 text languages must not be advertised as audio recognition capabilities.
        "phi-4-multimodal" => &["en", "de", "es", "fr", "it", "ja", "pt", "zh"],
        _ => return None,
    })
}

fn family(id: &str) -> Result<&'static str, String> {
    match id {
        "vibevoice-asr" => Ok("vibevoice-asr"),
        "vibevoice-asr-streaming-7b" => Ok("vibevoice-asr-streaming"),
        "phi-4-multimodal" => Ok("phi4-multimodal"),
        _ => Err("Unknown Microsoft transcription model".into()),
    }
}

fn language_for(id: &str, languages: &[String]) -> Result<String, String> {
    let supported = supported_languages(id).ok_or("Unknown Microsoft transcription model")?;
    let languages: Vec<_> = languages.iter().map(|language| language.trim()).filter(|language| !language.is_empty()).collect();
    if languages.iter().any(|language| !supported.contains(language)) {
        return Err("This model does not support the selected dictation languages. Choose another model or change the languages in Settings.".into());
    }
    Ok(if languages.len() == 1 { languages[0].to_string() } else { String::new() })
}

fn hints_json(terms: &[String]) -> Result<Vec<u8>, String> {
    let mut hints = Vec::new();
    for term in terms {
        let term = term.trim();
        if !term.is_empty() && !term.chars().any(char::is_control) && !hints.contains(&term) { hints.push(term); }
    }
    let bytes = serde_json::to_vec(&hints).map_err(|error| error.to_string())?;
    if bytes.len() > MAX_METADATA - 32 { return Err("Dictionary is too large for Microsoft local transcription".into()); }
    Ok(bytes)
}

fn paths(app: &AppHandle, id: &str) -> Result<(PathBuf, PathBuf), String> {
    if !supported() { return Err(UNSUPPORTED.into()); }
    family(id)?;
    let model = super::path_if_downloaded(app, id).ok_or("Download the transcription model in Settings › Model")?;
    let model = model.parent().ok_or("Couldn't locate the Microsoft transcription model")?.to_path_buf();
    let resource = app.path().resource_dir().ok().map(|path| path.join("microsoft-runtime/openglaido-microsoft"));
    if let Some(runtime) = resource.filter(|path| path.is_file()) { return Ok((runtime, model)); }
    let executable = std::env::current_exe().map_err(|error| error.to_string())?;
    let folder = executable.parent().ok_or("Couldn't locate Microsoft local transcription")?;
    let folder = if folder.file_name().is_some_and(|name| name == "deps") { folder.parent().unwrap_or(folder) } else { folder };
    let runtime = folder.join("microsoft-runtime/openglaido-microsoft");
    if !runtime.is_file() { return Err("The local Microsoft transcription runtime is missing. Reinstall OpenGlaido.".into()); }
    Ok((runtime, model))
}

pub async fn transcribe_cancellable(app: &AppHandle, id: &str, wav: Vec<u8>, terms: &[String], languages: &[String], cancellation: Cancellation) -> Result<String, String> {
    cancellation.check()?;
    let family = family(id)?;
    let language = language_for(id, languages)?;
    let hints = hints_json(terms)?;
    let (runtime, model) = paths(app, id)?;
    let activity = app.state::<crate::updater::UpdateState>().activity()?;
    tokio::task::spawn_blocking(move || {
        let _activity = activity;
        imp::transcribe(&runtime, &model, family, &wav, &language, hints, &cancellation)
    }).await.map_err(|error| format!("Transcription failed: {error}"))?
}

pub fn start_live(app: &AppHandle, id: &str, terms: Vec<String>, languages: Vec<String>) -> Result<(crate::realtime::LiveTranscription, StreamInput), String> {
    if id != "vibevoice-asr-streaming-7b" { return Err("This Microsoft local model does not support live transcription".into()); }
    let family = family(id)?;
    let language = language_for(id, &languages)?;
    let hints = hints_json(&terms)?;
    let (runtime, model) = paths(app, id)?;
    let activity = app.state::<crate::updater::UpdateState>().activity()?;
    let (tx, rx) = mpsc::channel(2048);
    let overflowed = Arc::new(AtomicBool::new(false));
    let overflow_check = overflowed.clone();
    let (finish_tx, finish_rx) = oneshot::channel();
    let cancellation = Cancellation::default();
    let cancel_worker = cancellation.clone();
    let task = tauri::async_runtime::spawn_blocking(move || {
        // A blocking task continues after abort. Drop cancels its token; retain admission until
        // inference stops, the child is killed/reaped, and its pipe worker exits.
        let _activity = activity;
        imp::stream(&runtime, &model, family, &language, hints, rx, finish_rx, &overflow_check, &cancel_worker)
    });
    Ok((crate::realtime::LiveTranscription::from_local(task, finish_tx, cancellation), StreamInput { tx, overflowed }))
}

pub fn preload(app: &AppHandle, id: &str) {
    let Ok((runtime, model)) = paths(app, id) else { return };
    let Ok(family) = family(id) else { return };
    let Ok(activity) = app.state::<crate::updater::UpdateState>().activity() else { return };
    let (app, id) = (app.clone(), id.to_string());
    std::thread::spawn(move || {
        let _activity = activity;
        if let Err(error) = imp::load(&runtime, &model, family, || super::selected(&app, &id)) {
            eprintln!("Couldn't preload Microsoft transcription model: {error}");
        }
    });
}

pub fn unload(path: Option<PathBuf>) { std::thread::spawn(move || imp::free(path.as_deref())); }
pub fn unload_now() { imp::quit(); }

#[cfg(not(target_os = "macos"))]
mod imp {
    use std::path::Path;
    use std::sync::atomic::AtomicBool;
    use crate::processing::Cancellation;
    use crate::audio::AudioChunk;
    use tokio::sync::{mpsc, oneshot};
    pub fn transcribe(_: &Path, _: &Path, _: &str, _: &[u8], _: &str, _: Vec<u8>, _: &Cancellation) -> Result<String, String> { Err(super::UNSUPPORTED.into()) }
    #[allow(clippy::too_many_arguments)]
    pub fn stream(_: &Path, _: &Path, _: &str, _: &str, _: Vec<u8>, _: mpsc::Receiver<AudioChunk>, _: oneshot::Receiver<()>, _: &AtomicBool, _: &Cancellation) -> Result<String, String> { Err(super::UNSUPPORTED.into()) }
    pub fn load(_: &Path, _: &Path, _: &str, _: impl FnOnce() -> bool) -> Result<(), String> { Err(super::UNSUPPORTED.into()) }
    pub fn free(_: Option<&Path>) {}
    pub fn quit() {}
}

#[cfg(target_os = "macos")]
mod imp {
    use super::{MAX_METADATA, MAX_SAMPLES, RATE};
    use crate::audio::AudioChunk;
    use crate::processing::{Cancellation, CANCELLED};
    use std::io::{BufWriter, Read, Write};
    use std::path::{Path, PathBuf};
    use std::process::{Child, Command, Stdio};
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{mpsc, Mutex, MutexGuard, PoisonError, TryLockError};
    use std::time::{Duration, Instant};
    use tokio::sync::{mpsc as audio_channel, oneshot};

    static QUITTING: AtomicBool = AtomicBool::new(false);
    static CONTEXT: Mutex<Option<Helper>> = Mutex::new(None);
    const MAX_REPLY: usize = 4 * 1024 * 1024;

    struct Request { operation: u32, samples: Vec<f32>, language: String, hints: Vec<u8> }
    struct Helper {
        model: PathBuf,
        family: String,
        child: Child,
        requests: Option<mpsc::SyncSender<Request>>,
        replies: mpsc::Receiver<Result<String, String>>,
        worker: Option<std::thread::JoinHandle<()>>,
    }
    impl Drop for Helper {
        fn drop(&mut self) {
            self.requests.take();
            let _ = self.child.kill();
            let _ = self.child.wait();
            if let Some(worker) = self.worker.take() { let _ = worker.join(); }
        }
    }

    fn read_reply(reader: &mut impl Read) -> Result<String, String> {
        let mut header = [0; 8];
        reader.read_exact(&mut header).map_err(|_| "The Microsoft transcription runtime stopped unexpectedly. Try again.".to_string())?;
        let status = u32::from_le_bytes(header[..4].try_into().unwrap());
        let length = u32::from_le_bytes(header[4..].try_into().unwrap()) as usize;
        if status > 1 || length > MAX_REPLY { return Err("The Microsoft transcription runtime sent an invalid response".into()); }
        let mut bytes = vec![0; length];
        reader.read_exact(&mut bytes).map_err(|_| "The Microsoft transcription runtime returned incomplete text".to_string())?;
        let text = String::from_utf8(bytes).map_err(|_| "The Microsoft transcription runtime returned invalid text".to_string())?;
        if status == 1 { return Err(format!("The Microsoft transcription model failed: {text}")); }
        Ok(text)
    }

    fn write_request(input: &mut impl Write, request: &Request) -> Result<(), String> {
        if request.operation > 3 || request.samples.len() > MAX_SAMPLES || request.language.len() > 32 || request.hints.len() + request.language.len() > MAX_METADATA ||
            request.samples.iter().any(|sample| !sample.is_finite() || sample.abs() > 1.0) {
            return Err("Invalid Microsoft local audio request".into());
        }
        (|| -> std::io::Result<()> {
            for value in [request.operation, request.samples.len() as u32, request.language.len() as u32, request.hints.len() as u32] { input.write_all(&value.to_le_bytes())?; }
            input.write_all(request.language.as_bytes())?;
            input.write_all(&request.hints)?;
            for sample in &request.samples { input.write_all(&sample.to_le_bytes())?; }
            input.flush()
        })().map_err(|error| format!("Couldn't send audio to Microsoft local transcription: {error}"))
    }

    fn wait_reply(replies: &mpsc::Receiver<Result<String, String>>, timeout: Duration, quitting: &AtomicBool, cancellation: &Cancellation) -> Result<String, String> {
        let deadline = Instant::now() + timeout;
        loop {
            cancellation.check()?;
            if quitting.load(Ordering::Relaxed) { return Err(CANCELLED.into()); }
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() { return Err("The Microsoft local transcription model took too long. Try a shorter recording.".into()); }
            match replies.recv_timeout(remaining.min(Duration::from_millis(50))) {
                Ok(reply) => { cancellation.check()?; return reply; }
                Err(mpsc::RecvTimeoutError::Timeout) => {}
                Err(mpsc::RecvTimeoutError::Disconnected) => return Err("The Microsoft transcription runtime stopped unexpectedly. Try again.".into()),
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
        fn start(runtime: &Path, model: &Path, family: &str, cancellation: &Cancellation) -> Result<Self, String> {
            cancellation.check()?;
            let mut child = Command::new(runtime).arg(family).arg(model)
                .env("HF_HUB_OFFLINE", "1").env("TRANSFORMERS_OFFLINE", "1").env("HF_HUB_DISABLE_TELEMETRY", "1")
                .stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::null())
                .spawn().map_err(|error| format!("Couldn't start Microsoft local transcription: {error}"))?;
            let mut input = BufWriter::new(child.stdin.take().unwrap());
            let mut output = child.stdout.take().unwrap();
            let (sender, replies) = mpsc::channel();
            let (requests, receiver) = mpsc::sync_channel::<Request>(1);
            let worker = std::thread::spawn(move || {
                let ready = read_reply(&mut output);
                let failed = ready.is_err();
                if sender.send(ready).is_err() || failed { return; }
                while let Ok(request) = receiver.recv() {
                    let reply = write_request(&mut input, &request).and_then(|()| read_reply(&mut output));
                    let failed = reply.is_err();
                    if sender.send(reply).is_err() || failed { break; }
                }
            });
            let helper = Self { model: model.to_path_buf(), family: family.into(), child, requests: Some(requests), replies, worker: Some(worker) };
            if !wait_reply(&helper.replies, Duration::from_secs(600), &QUITTING, cancellation)?.is_empty() { return Err("Microsoft local transcription could not initialize".into()); }
            Ok(helper)
        }
        fn run(&self, operation: u32, samples: Vec<f32>, language: &str, hints: Vec<u8>, cancellation: &Cancellation, timeout: Duration) -> Result<String, String> {
            cancellation.check()?;
            self.requests.as_ref().ok_or("Microsoft local transcription is shutting down")?.try_send(Request { operation, samples, language: language.into(), hints })
                .map_err(|_| "Microsoft local transcription is unavailable. Try again.".to_string())?;
            wait_reply(&self.replies, timeout, &QUITTING, cancellation)
        }
    }

    fn loaded<'a>(cached: &'a mut Option<Helper>, runtime: &Path, model: &Path, family: &str, cancellation: &Cancellation) -> Result<&'a mut Helper, String> {
        if cached.as_ref().is_none_or(|helper| helper.model != model || helper.family != family) {
            *cached = None;
            *cached = Some(Helper::start(runtime, model, family, cancellation)?);
        }
        Ok(cached.as_mut().unwrap())
    }

    pub fn load(runtime: &Path, model: &Path, family: &str, wanted: impl FnOnce() -> bool) -> Result<(), String> {
        let cancellation = Cancellation::default();
        let mut cached = lock(&cancellation)?;
        if !wanted() { return Ok(()); }
        loaded(&mut cached, runtime, model, family, &cancellation).map(|_| ())
    }
    pub fn free(path: Option<&Path>) {
        let mut cached = CONTEXT.lock().unwrap_or_else(PoisonError::into_inner);
        if path.is_none_or(|path| cached.as_ref().is_some_and(|helper| path == helper.model || path.starts_with(&helper.model))) { *cached = None; }
    }
    pub fn quit() { QUITTING.store(true, Ordering::Relaxed); free(None); }

    pub fn transcribe(runtime: &Path, model: &Path, family: &str, wav: &[u8], language: &str, hints: Vec<u8>, cancellation: &Cancellation) -> Result<String, String> {
        cancellation.check()?;
        let samples = super::super::stt::decode_wav(wav, Some(1800))?.into_iter().map(|sample| sample.clamp(-1.0, 1.0)).collect::<Vec<_>>();
        if samples.is_empty() { return Ok(String::new()); }
        let timeout = Duration::from_secs(120 + samples.len() as u64 / RATE as u64 * 4);
        let mut cached = lock(cancellation)?;
        let result = loaded(&mut cached, runtime, model, family, cancellation).and_then(|helper| helper.run(0, samples, language, hints, cancellation, timeout));
        if result.is_err() { *cached = None; }
        result.map(|text| text.trim().to_string())
    }

    #[derive(Default)]
    struct Resampler { rate: u32, input: u64, output: u64, last: f32 }
    impl Resampler {
        fn push(&mut self, chunk: AudioChunk, out: &mut Vec<f32>) -> Result<(), String> {
            if chunk.rate == 0 || (self.rate != 0 && chunk.rate != self.rate) { return Err("Transcription failed: invalid audio recording".into()); }
            self.rate = chunk.rate;
            for sample in chunk.samples {
                if !sample.is_finite() { return Err("Transcription failed: invalid audio recording".into()); }
                let sample = sample.clamp(-1.0, 1.0);
                while self.output * self.rate as u64 <= self.input * RATE as u64 {
                    let position = self.output * self.rate as u64;
                    let fraction = if self.input == 0 { 1.0 } else { (position - (self.input - 1) * RATE as u64) as f32 / RATE as f32 };
                    out.push(self.last + (sample - self.last) * fraction);
                    self.output += 1;
                    if self.output > MAX_SAMPLES as u64 { return Err("Local live transcription is limited to 30 minutes".into()); }
                }
                self.last = sample;
                self.input += 1;
            }
            Ok(())
        }
        fn finish(&mut self, out: &mut Vec<f32>) {
            if self.rate == 0 { return; }
            let target = self.input * RATE as u64 / self.rate as u64;
            while self.output < target { out.push(self.last); self.output += 1; }
        }
    }

    fn collect_audio(rx: &mut audio_channel::Receiver<AudioChunk>, finish: &mut oneshot::Receiver<()>, finishing: &mut bool) -> Result<Option<AudioChunk>, String> {
        if !*finishing {
            match finish.try_recv() {
                Ok(()) | Err(oneshot::error::TryRecvError::Closed) => { *finishing = true; rx.close(); }
                Err(oneshot::error::TryRecvError::Empty) => {}
            }
        }
        match rx.try_recv() {
            Ok(chunk) => Ok(Some(chunk)),
            Err(audio_channel::error::TryRecvError::Disconnected) => { *finishing = true; Ok(None) }
            Err(audio_channel::error::TryRecvError::Empty) => Ok(None),
        }
    }

    const LIVE_FRAME: usize = RATE as usize / 10;
    const LIVE_BUFFER_FRAMES: usize = 2048;

    /// Drain device callbacks before waiting for the model/cache: a cold 7B load takes much
    /// longer than a callback-count queue can hold. Every queued frame is exactly 100 ms.
    fn relay_audio(mut rx: audio_channel::Receiver<AudioChunk>, mut finish: oneshot::Receiver<()>, tx: audio_channel::Sender<AudioChunk>, ready: oneshot::Sender<()>, overflowed: &AtomicBool, cancellation: &Cancellation) -> Result<(), String> {
        let mut resampler = Resampler::default();
        let mut pending = Vec::new();
        let mut finishing = false;
        loop {
            cancellation.check()?;
            if QUITTING.load(Ordering::Relaxed) { return Err(CANCELLED.into()); }
            if tx.is_closed() { return Ok(()); } // the model worker already failed/stopped
            if overflowed.load(Ordering::Acquire) { return Err("Transcription failed: the connection was too slow for live audio; retry from History".into()); }
            if let Some(chunk) = collect_audio(&mut rx, &mut finish, &mut finishing)? {
                resampler.push(chunk, &mut pending)?;
                let complete = pending.len() / LIVE_FRAME * LIVE_FRAME;
                for samples in pending[..complete].chunks_exact(LIVE_FRAME) {
                    relay_frame(&tx, samples.to_vec(), overflowed)?;
                }
                pending.drain(..complete);
                continue;
            }
            if finishing { break; }
            std::thread::sleep(Duration::from_millis(2));
        }
        resampler.finish(&mut pending);
        if !pending.is_empty() { relay_frame(&tx, pending, overflowed)?; }
        // The receiver only finalizes after all queued audio, including its short tail, exists.
        drop(tx);
        let _ = ready.send(());
        Ok(())
    }

    fn relay_frame(tx: &audio_channel::Sender<AudioChunk>, samples: Vec<f32>, overflowed: &AtomicBool) -> Result<(), String> {
        match tx.try_send(AudioChunk { samples, rate: RATE }) {
            Ok(()) => Ok(()),
            Err(audio_channel::error::TrySendError::Closed(_)) => Err(CANCELLED.into()),
            Err(audio_channel::error::TrySendError::Full(_)) => {
                overflowed.store(true, Ordering::Release);
                Err("Transcription failed: the connection was too slow for live audio; retry from History".into())
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub fn stream(runtime: &Path, model: &Path, family: &str, language: &str, hints: Vec<u8>, rx: audio_channel::Receiver<AudioChunk>, finish: oneshot::Receiver<()>, overflowed: &AtomicBool, cancellation: &Cancellation) -> Result<String, String> {
        std::thread::scope(|scope| {
            // ponytail: ~205 seconds / 13 MiB of startup/backpressure buffer; bounded by audio
            // time rather than device callback count. Expand only if cold loads exceed this.
            let (tx, normalized) = audio_channel::channel(LIVE_BUFFER_FRAMES);
            let (ready_tx, ready) = oneshot::channel();
            let relay = scope.spawn(move || {
                let result = relay_audio(rx, finish, tx, ready_tx, overflowed, cancellation);
                if result.is_err() { cancellation.cancel(); } // interrupt a model still loading
                result
            });
            let result = stream_normalized(runtime, model, family, language, hints, normalized, ready, overflowed, cancellation);
            if result.is_err() { cancellation.cancel(); }
            let relay_result = relay.join().map_err(|_| "Microsoft live audio collection stopped unexpectedly".to_string())?;
            match (result, relay_result) {
                (Err(error), Err(relay_error)) if error == CANCELLED => Err(relay_error),
                (Err(error), _) => Err(error),
                (Ok(text), Ok(())) => Ok(text),
                (Ok(_), Err(error)) => Err(error),
            }
        })
    }

    #[allow(clippy::too_many_arguments)]
    fn stream_normalized(runtime: &Path, model: &Path, family: &str, language: &str, hints: Vec<u8>, mut rx: audio_channel::Receiver<AudioChunk>, mut finish: oneshot::Receiver<()>, overflowed: &AtomicBool, cancellation: &Cancellation) -> Result<String, String> {
        let mut cached = lock(cancellation)?;
        let result = (|| {
            let helper = loaded(&mut cached, runtime, model, family, cancellation)?;
            helper.run(1, vec![], language, hints, cancellation, Duration::from_secs(240))?;
            let mut resampler = Resampler::default();
            let mut pending = Vec::new();
            let mut finishing = false;
            loop {
                cancellation.check()?;
                if QUITTING.load(Ordering::Relaxed) { return Err(CANCELLED.into()); }
                if overflowed.load(Ordering::Acquire) { return Err("Transcription failed: the connection was too slow for live audio; retry from History".into()); }
                if let Some(chunk) = collect_audio(&mut rx, &mut finish, &mut finishing)? {
                    resampler.push(chunk, &mut pending)?;
                    if pending.len() >= RATE as usize / 10 { helper.run(2, std::mem::take(&mut pending), language, vec![], cancellation, Duration::from_secs(240))?; }
                    continue;
                }
                if finishing { break; }
                std::thread::sleep(Duration::from_millis(10));
            }
            resampler.finish(&mut pending);
            if !pending.is_empty() { helper.run(2, pending, language, vec![], cancellation, Duration::from_secs(240))?; }
            helper.run(3, vec![], language, vec![], cancellation, Duration::from_secs(240))
        })();
        if result.is_err() { *cached = None; }
        result.map(|text| text.trim().to_string())
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use std::io::Cursor;
        static LIVE_TESTS: Mutex<()> = Mutex::new(());
        #[test]
        fn framed_requests_and_replies_preserve_audio_and_unicode_and_fail_closed() {
            let request = Request { operation: 0, samples: vec![-1.0, 0.5, 1.0], language: "sl".into(), hints: super::super::hints_json(&["Živjo".into()]).unwrap() };
            let mut bytes = vec![];
            write_request(&mut bytes, &request).unwrap();
            assert_eq!(&bytes[..16], [0u32, 3, 2, request.hints.len() as u32].into_iter().flat_map(u32::to_le_bytes).collect::<Vec<_>>());
            assert_eq!(&bytes[16..18], b"sl");
            assert_eq!(&bytes[bytes.len() - 12..], [-1.0f32, 0.5, 1.0].into_iter().flat_map(f32::to_le_bytes).collect::<Vec<_>>());
            let text = "---END---\nŽivjo svet";
            let frame = [0u32.to_le_bytes().as_slice(), (text.len() as u32).to_le_bytes().as_slice(), text.as_bytes()].concat();
            assert_eq!(read_reply(&mut Cursor::new(frame.clone())).unwrap(), text);
            assert!(read_reply(&mut Cursor::new(&frame[..frame.len() - 1])).is_err());
            assert!(read_reply(&mut Cursor::new([0u32.to_le_bytes(), u32::MAX.to_le_bytes()].concat())).is_err());
            let request = Request { samples: vec![f32::NAN], ..request };
            assert!(write_request(&mut vec![], &request).is_err());
        }
        #[test]
        fn resampling_remains_continuous_across_every_microphone_callback() {
            for rate in [16000, 44100, 48000, 96000] {
                let samples: Vec<_> = (0..rate / 10).map(|i| ((i as f32 * 0.07).sin()) * 0.3).collect();
                let mut whole = Resampler::default();
                let mut expected = vec![];
                whole.push(AudioChunk { samples: samples.clone(), rate }, &mut expected).unwrap();
                whole.finish(&mut expected);
                let mut divided = Resampler::default();
                let mut actual = vec![];
                for chunk in samples.chunks(127) { divided.push(AudioChunk { samples: chunk.to_vec(), rate }, &mut actual).unwrap(); }
                divided.finish(&mut actual);
                assert_eq!(actual.len(), 1600);
                assert_eq!(actual, expected);
            }
            let mut resampler = Resampler::default();
            resampler.push(AudioChunk { samples: vec![0.0], rate: 16000 }, &mut vec![]).unwrap();
            assert!(resampler.push(AudioChunk { samples: vec![0.0], rate: 48000 }, &mut vec![]).is_err());
        }
        #[test]
        fn finish_drains_audio_even_when_the_microphone_still_owns_its_sender() {
            let (tx, mut rx) = audio_channel::channel(4);
            let (finish_tx, mut finish) = oneshot::channel();
            tx.try_send(AudioChunk { samples: vec![0.1, 0.2], rate: 16000 }).unwrap();
            tx.try_send(AudioChunk { samples: vec![0.3], rate: 16000 }).unwrap();
            finish_tx.send(()).unwrap();
            let mut finishing = false;
            assert_eq!(collect_audio(&mut rx, &mut finish, &mut finishing).unwrap().unwrap().samples, [0.1, 0.2]);
            assert!(finishing && tx.is_closed());
            assert_eq!(collect_audio(&mut rx, &mut finish, &mut finishing).unwrap().unwrap().samples, [0.3]);
            assert!(collect_audio(&mut rx, &mut finish, &mut finishing).unwrap().is_none());
        }
        #[test]
        fn live_protocol_streams_before_release_then_drains_the_tail_and_reuses_warm_models() {
            let _serial = LIVE_TESTS.lock().unwrap();
            use std::os::unix::fs::PermissionsExt;
            let directory = std::env::temp_dir().join(format!("openglaido-microsoft-protocol-{}", uuid::Uuid::new_v4()));
            std::fs::create_dir(&directory).unwrap();
            let runtime = directory.join("fixture.py");
            std::fs::write(&runtime, r#"#!/usr/bin/python3
import pathlib, struct, sys
path = pathlib.Path(sys.argv[2])
(path / 'loads').open('a').write('loaded\n')
def reply(text):
    data = text.encode()
    sys.stdout.buffer.write(struct.pack('<2I', 0, len(data)) + data)
    sys.stdout.buffer.flush()
reply('')
total = 0
while True:
    header = sys.stdin.buffer.read(16)
    if not header: break
    op, count, language, hints = struct.unpack('<4I', header)
    sys.stdin.buffer.read(language + hints + count * 4)
    if op == 1: total = 0
    if op == 2:
        total += count
        (path / 'appends').open('a').write(str(count) + '\n')
    reply(str(total) if op == 3 else '')
"#).unwrap();
            std::fs::set_permissions(&runtime, std::fs::Permissions::from_mode(0o700)).unwrap();
            let (tx, rx) = audio_channel::channel(4);
            let (finish_tx, finish) = oneshot::channel();
            tx.try_send(AudioChunk { samples: vec![0.2; 8000], rate: 16000 }).unwrap();
            let worker_runtime = runtime.clone();
            let worker_model = directory.clone();
            let worker = std::thread::spawn(move || stream(&worker_runtime, &worker_model, "vibevoice-asr-streaming", "", b"[]".to_vec(), rx, finish, &AtomicBool::new(false), &Cancellation::default()));
            let started = Instant::now();
            while !directory.join("appends").exists() {
                assert!(started.elapsed() < Duration::from_secs(5), "Audio never reached the live helper before release");
                std::thread::sleep(Duration::from_millis(10));
            }
            tx.try_send(AudioChunk { samples: vec![0.3; 1600], rate: 16000 }).unwrap();
            finish_tx.send(()).unwrap();
            assert_eq!(worker.join().unwrap().unwrap(), "9600");
            assert!(tx.is_closed());
            // Keep the same process/model warm, but reset the next recording's internal state.
            let (tx, rx) = audio_channel::channel(4);
            let (finish_tx, finish) = oneshot::channel();
            tx.try_send(AudioChunk { samples: vec![0.1; 320], rate: 16000 }).unwrap();
            finish_tx.send(()).unwrap();
            assert_eq!(stream(&runtime, &directory, "vibevoice-asr-streaming", "", b"[]".to_vec(), rx, finish, &AtomicBool::new(false), &Cancellation::default()).unwrap(), "320");
            assert_eq!(std::fs::read_to_string(directory.join("loads")).unwrap(), "loaded\n");
            free(Some(&directory));
            std::fs::remove_dir_all(directory).unwrap();
        }
        #[test]
        fn cold_loading_drains_small_callbacks_before_ready_and_preserves_the_final_tail() {
            let _serial = LIVE_TESTS.lock().unwrap();
            use std::os::unix::fs::PermissionsExt;
            let directory = std::env::temp_dir().join(format!("openglaido-microsoft-cold-{}", uuid::Uuid::new_v4()));
            std::fs::create_dir(&directory).unwrap();
            let runtime = directory.join("fixture.py");
            std::fs::write(&runtime, r#"#!/usr/bin/python3
import pathlib, struct, sys, time
path = pathlib.Path(sys.argv[2])
(path / 'started').write_text(str(__import__('os').getpid()))
while not (path / 'ready').exists(): time.sleep(0.01)
def reply(text):
    data = text.encode()
    sys.stdout.buffer.write(struct.pack('<2I', 0, len(data)) + data)
    sys.stdout.buffer.flush()
reply('')
total, first, last = 0, None, None
while True:
    header = sys.stdin.buffer.read(16)
    if not header: break
    op, count, language, hints = struct.unpack('<4I', header)
    sys.stdin.buffer.read(language + hints)
    samples = sys.stdin.buffer.read(count * 4)
    if op == 1: total, first, last = 0, None, None
    if op == 2:
        total += count
        if first is None: first = struct.unpack_from('<f', samples)[0]
        last = struct.unpack_from('<f', samples, len(samples) - 4)[0]
    reply(str(total) + ':' + str(round(first, 1)) + ':' + str(round(last, 1)) if op == 3 else '')
"#).unwrap();
            std::fs::set_permissions(&runtime, std::fs::Permissions::from_mode(0o700)).unwrap();
            let (tx, rx) = audio_channel::channel(2048);
            let (finish_tx, finish) = oneshot::channel();
            let cancellation = Cancellation::default();
            let worker_cancel = cancellation.clone();
            let worker_runtime = runtime.clone();
            let worker_model = directory.clone();
            let overflowed = std::sync::Arc::new(AtomicBool::new(false));
            let check = overflowed.clone();
            let worker = std::thread::spawn(move || stream(&worker_runtime, &worker_model, "vibevoice-asr-streaming", "", b"[]".to_vec(), rx, finish, &check, &worker_cancel));
            // 9000 small callbacks represent 48 seconds: over four times the raw queue capacity.
            // The model cannot signal readiness until every callback was accepted and release sent.
            let started = Instant::now();
            for _ in 0..9000 {
                while tx.capacity() == 0 {
                    assert!(started.elapsed() < Duration::from_secs(10), "Cold model blocked microphone collection");
                    std::thread::sleep(Duration::from_millis(1));
                }
                tx.try_send(AudioChunk { samples: vec![0.1; 256], rate: 48000 }).unwrap();
            }
            while tx.capacity() == 0 { std::thread::sleep(Duration::from_millis(1)); }
            tx.try_send(AudioChunk { samples: vec![0.9; 192], rate: 48000 }).unwrap();
            finish_tx.send(()).unwrap();
            assert!(!overflowed.load(Ordering::Acquire));
            std::fs::write(directory.join("ready"), "ready").unwrap();
            assert_eq!(worker.join().unwrap().unwrap(), "768064:0.1:0.9");
            assert!(tx.is_closed());
            free(Some(&directory));
            // Cancellation while waiting for cold readiness must stop both collection and child.
            std::fs::remove_file(directory.join("ready")).unwrap();
            std::fs::remove_file(directory.join("started")).unwrap();
            let (tx, rx) = audio_channel::channel(2048);
            let (_finish_tx, finish) = oneshot::channel();
            let cancellation = Cancellation::default();
            let worker_cancel = cancellation.clone();
            let worker_runtime = runtime.clone();
            let worker_model = directory.clone();
            let worker = std::thread::spawn(move || stream(&worker_runtime, &worker_model, "vibevoice-asr-streaming", "", b"[]".to_vec(), rx, finish, &AtomicBool::new(false), &worker_cancel));
            let started = Instant::now();
            while !directory.join("started").exists() {
                assert!(started.elapsed() < Duration::from_secs(5));
                std::thread::sleep(Duration::from_millis(10));
            }
            let pid = std::fs::read_to_string(directory.join("started")).unwrap();
            tx.try_send(AudioChunk { samples: vec![0.2; 256], rate: 48000 }).unwrap();
            let cancel_start = Instant::now();
            cancellation.cancel();
            assert_eq!(worker.join().unwrap().unwrap_err(), CANCELLED);
            assert!(cancel_start.elapsed() < Duration::from_secs(2));
            assert!(tx.is_closed());
            assert!(!Command::new("/bin/kill").args(["-0", pid.trim()]).stderr(Stdio::null()).status().unwrap().success());
            std::fs::remove_dir_all(directory).unwrap();
        }
        #[test]
        fn quit_deadline_and_closed_pipes_cannot_leave_the_runtime_waiting_forever() {
            let (_sender, replies) = mpsc::channel();
            assert_eq!(wait_reply(&replies, Duration::from_secs(600), &AtomicBool::new(true), &Cancellation::default()).unwrap_err(), CANCELLED);
            assert!(wait_reply(&replies, Duration::from_millis(1), &AtomicBool::new(false), &Cancellation::default()).unwrap_err().contains("too long"));
            let (sender, replies) = mpsc::channel();
            drop(sender);
            assert!(wait_reply(&replies, Duration::from_secs(600), &AtomicBool::new(false), &Cancellation::default()).unwrap_err().contains("stopped unexpectedly"));
        }
        #[test]
        fn cancelled_reply_wait_reaps_a_real_process_before_the_worker_returns() {
            let (_sender, replies) = mpsc::channel();
            let cancellation = Cancellation::default();
            let signal = cancellation.clone();
            let cancel = std::thread::spawn(move || { std::thread::sleep(Duration::from_millis(10)); signal.cancel(); });
            let started = Instant::now();
            assert_eq!(wait_reply(&replies, Duration::from_secs(600), &AtomicBool::new(false), &cancellation).unwrap_err(), CANCELLED);
            cancel.join().unwrap();
            assert!(started.elapsed() < Duration::from_secs(2));
            let child = Command::new("/bin/sleep").arg("30").spawn().unwrap();
            let pid = child.id();
            let (requests, _receiver) = mpsc::sync_channel(1);
            let helper = Helper { model: PathBuf::new(), family: "test".into(), child, requests: Some(requests), replies, worker: None };
            drop(helper);
            assert!(!Command::new("/bin/kill").args(["-0", &pid.to_string()]).stderr(Stdio::null()).status().unwrap().success());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn speech_languages_differ_from_phi_text_languages_and_hints_remain_data() {
        assert_eq!(supported_languages("vibevoice-asr").unwrap().len(), 51);
        assert!(language_for("vibevoice-asr", &["sl".into()]).is_ok());
        assert!(language_for("vibevoice-asr-streaming-7b", &["sl".into()]).is_err());
        assert!(language_for("phi-4-multimodal", &["sl".into()]).is_err());
        assert_eq!(supported_languages("phi-4-multimodal").unwrap().len(), 8);
        assert_eq!(family("vibevoice-asr-streaming-7b").unwrap(), "vibevoice-asr-streaming");
        let hints = hints_json(&[" Živjo ".into(), "Živjo".into(), "bad\ncommand".into(), "quoted\"word".into()]).unwrap();
        assert_eq!(serde_json::from_slice::<Vec<String>>(&hints).unwrap(), ["Živjo", "quoted\"word"]);
        assert!(hints_json(&["x".repeat(MAX_METADATA)]).is_err());
    }
}
