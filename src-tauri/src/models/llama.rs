//! Local language model with llama.cpp (llama-cpp-2, Metal). macOS only; elsewhere every call fails.

use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use tauri::AppHandle;

pub use imp::chat_file;

const MISSING: &str = "Download the language model in Settings › Model";

/// Set on app exit: a running generation stops at its next token.
static QUITTING: AtomicBool = AtomicBool::new(false);

/// One chat turn with catalog model `model_id`. `messages` are (role, content) with role
/// "system" | "user" | "assistant". Streams text pieces through `on_text`; returns the whole answer.
pub async fn chat(
    app: &AppHandle,
    model_id: &str,
    messages: Vec<(String, String)>,
    temperature: f32,
    max_tokens: u32,
    on_text: impl FnMut(&str) + Send + 'static,
) -> Result<String, String> {
    if !super::supported() {
        return Err(super::UNSUPPORTED.into());
    }
    let template = super::find(model_id).filter(|m| m.kind == "llm").ok_or(MISSING)?.template;
    let path = super::path_if_downloaded(app, model_id).ok_or(MISSING)?;
    // Dropping this future (a timeout) stops the generation at its next token.
    let stop = StopOnDrop(Arc::default());
    let cancel = stop.0.clone();
    tokio::task::spawn_blocking(move || chat_file(&path, template, &messages, temperature, max_tokens, &cancel, on_text))
        .await
        .map_err(|e| format!("The local language model failed: {e}"))?
}

struct StopOnDrop(Arc<AtomicBool>);

impl Drop for StopOnDrop {
    fn drop(&mut self) {
        self.0.store(true, Ordering::Relaxed);
    }
}

/// Loads the model in the background (errors are only logged).
pub fn preload(app: &AppHandle, model_id: &str) {
    let Some(path) = super::path_if_downloaded(app, model_id) else { return };
    let (app, id) = (app.clone(), model_id.to_string());
    std::thread::spawn(move || {
        if let Err(e) = imp::load(&path, || super::selected(&app, &id)) {
            eprintln!("Couldn't preload {}: {}", path.display(), e);
        }
    });
}

/// Frees the loaded model.
pub fn unload() {
    imp::unload(None);
}

/// Frees the loaded model if it is `path`.
pub(super) fn unload_path(path: &Path) {
    imp::unload(Some(path.to_path_buf()));
}

/// Stops a running generation and frees the loaded model now. Call before the process exits: ggml-metal's
/// exit-time destructor aborts while a model still holds GPU memory.
pub fn unload_now() {
    QUITTING.store(true, Ordering::Relaxed);
    imp::free(None);
}

#[cfg(not(target_os = "macos"))]
mod imp {
    use std::path::{Path, PathBuf};
    use std::sync::atomic::AtomicBool;

    pub fn chat_file(
        _: &Path,
        _: &str,
        _: &[(String, String)],
        _: f32,
        _: u32,
        _: &AtomicBool,
        _: impl FnMut(&str),
    ) -> Result<String, String> {
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
    use llama_cpp_2::context::params::LlamaContextParams;
    use llama_cpp_2::llama_backend::LlamaBackend;
    use llama_cpp_2::llama_batch::LlamaBatch;
    use llama_cpp_2::model::params::LlamaModelParams;
    use llama_cpp_2::model::{AddBos, LlamaModel};
    use llama_cpp_2::sampling::LlamaSampler;
    use std::ffi::{c_char, c_void};
    use std::fmt::Write;
    use std::num::NonZeroU32;
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{Mutex, MutexGuard, OnceLock, PoisonError};

    const N_CTX: u32 = 8192;

    /// llama.cpp's backend may only be initialized once per process.
    static BACKEND: OnceLock<Result<LlamaBackend, String>> = OnceLock::new();
    /// The loaded model and its path; the lock also serializes requests.
    static MODEL: Mutex<Option<(PathBuf, LlamaModel)>> = Mutex::new(None);

    fn backend() -> Result<&'static LlamaBackend, String> {
        BACKEND
            .get_or_init(|| {
                // Silence llama.cpp before init, which already sets up Metal (`LlamaBackend::void_logs` comes too late).
                unsafe extern "C" fn quiet(_: llama_cpp_sys_2::ggml_log_level, _: *const c_char, _: *mut c_void) {}
                unsafe { llama_cpp_sys_2::llama_log_set(Some(quiet), std::ptr::null_mut()) };
                LlamaBackend::init().map_err(|e| e.to_string())
            })
            .as_ref()
            .map_err(|e| format!("Couldn't start the local language model: {e}"))
    }

    fn lock() -> MutexGuard<'static, Option<(PathBuf, LlamaModel)>> {
        MODEL.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn loaded<'a>(cached: &'a mut Option<(PathBuf, LlamaModel)>, path: &Path) -> Result<&'a LlamaModel, String> {
        if cached.as_ref().is_none_or(|(p, _)| p != path) {
            *cached = None; // free the old model before loading the new one
            if !path.exists() {
                return Err(super::MISSING.into());
            }
            let params = LlamaModelParams::default().with_n_gpu_layers(999);
            let model = LlamaModel::load_from_file(backend()?, path, &params).map_err(|e| {
                eprintln!("Couldn't load {}: {e}", path.display());
                "Couldn't load the language model. Delete and download it again in Settings › Model.".to_string()
            })?;
            *cached = Some((path.to_path_buf(), model));
        }
        Ok(&cached.as_ref().unwrap().1)
    }

    /// Loads `path` unless `wanted()`, asked once the lock is ours, says a save switched away meanwhile
    /// (a running request held the lock).
    pub fn load(path: &Path, wanted: impl FnOnce() -> bool) -> Result<(), String> {
        let mut cached = lock();
        if !wanted() || super::QUITTING.load(Ordering::Relaxed) {
            return Ok(());
        }
        loaded(&mut cached, path).map(|_| ())
    }

    pub fn unload(path: Option<PathBuf>) {
        // On a thread: a running request holds the lock, and callers may be on the main thread.
        std::thread::spawn(move || free(path.as_deref()));
    }

    /// Frees the model if it is `path` (None = any), waiting for a running request.
    pub fn free(path: Option<&Path>) {
        let mut cached = lock();
        if path.is_none() || cached.as_ref().map(|(p, _)| p.as_path()) == path {
            *cached = None;
        }
    }

    fn fail(e: impl std::fmt::Display) -> String {
        eprintln!("llama.cpp failed: {e}");
        "The local language model failed".to_string()
    }

    /// The core of `chat`, without a Tauri app: the GGUF at `model_path` with catalog prompt `template`.
    /// Setting `cancel` stops it at the next token.
    pub fn chat_file(
        model_path: &Path,
        template: &str,
        messages: &[(String, String)],
        temperature: f32,
        max_tokens: u32,
        cancel: &AtomicBool,
        mut on_text: impl FnMut(&str),
    ) -> Result<String, String> {
        let stopped = || cancel.load(Ordering::Relaxed) || super::QUITTING.load(Ordering::Relaxed);
        let backend = backend()?;
        let mut cached = lock();
        if stopped() {
            return Err("Cancelled".into()); // while waiting for the lock: don't (re)load the model
        }
        let model = loaded(&mut cached, model_path)?;

        let reserve = max_tokens.clamp(1, N_CTX / 2);
        let count = |prompt: &str| model.str_to_token(prompt, AddBos::Always).map_or(usize::MAX, |t| t.len());
        let messages = fit_messages(template, messages, (N_CTX - reserve) as usize, count)?;
        let tokens = model.str_to_token(&build_prompt(template, &messages), AddBos::Always).map_err(fail)?;

        // A fresh context per request: no state leaks between requests.
        let params = LlamaContextParams::default().with_n_ctx(NonZeroU32::new(N_CTX)).with_n_batch(N_CTX);
        let mut ctx = model.new_context(backend, params).map_err(fail)?;
        let mut batch = LlamaBatch::new(N_CTX as usize, 1);
        for (i, token) in tokens.iter().enumerate() {
            batch.add(*token, i as i32, &[0], i + 1 == tokens.len()).map_err(fail)?;
        }
        ctx.decode(&mut batch).map_err(fail)?;

        let mut sampler = if temperature <= 0.15 {
            LlamaSampler::greedy()
        } else {
            // Seed u32::MAX = LLAMA_DEFAULT_SEED: a random seed per request.
            LlamaSampler::chain_simple([LlamaSampler::temp(temperature), LlamaSampler::dist(u32::MAX)])
        };
        let mut decoder = encoding_rs::UTF_8.new_decoder();
        let mut out = String::new();
        let end = tokens.len() + (max_tokens as usize).min(N_CTX as usize - tokens.len());
        let mut complete = false;
        for pos in tokens.len() as i32..end as i32 {
            if stopped() {
                return Err("Cancelled".into());
            }
            let token = sampler.sample(&ctx, batch.n_tokens() - 1);
            if model.is_eog_token(token) {
                complete = true;
                break;
            }
            // `special: false` hides control tokens; think/transcript tags are plain text and get stripped below.
            let piece = model.token_to_piece(token, &mut decoder, false, None).map_err(fail)?;
            if !piece.is_empty() {
                on_text(&piece);
                out.push_str(&piece);
            }
            batch.clear();
            batch.add(token, pos, &[0], true).map_err(fail)?;
            ctx.decode(&mut batch).map_err(fail)?;
        }
        finish_output(&out, complete)
    }

    fn finish_output(text: &str, complete: bool) -> Result<String, String> {
        if !complete {
            return Err("The local language model reached its output limit before finishing".into());
        }
        Ok(clean_output(text))
    }

    /// The prompt for `messages` in the catalog `template` format, ending where the reply starts. The tokenizer
    /// adds BOS. Same text as llama.cpp's built-in chatml / mistral-v7-tekken formats; Gemma 4 isn't built in.
    fn build_prompt(template: &str, messages: &[(String, String)]) -> String {
        let mut prompt = String::new();
        for (role, content) in messages {
            let content = content.replace('\0', "");
            let _ = match (template, role.as_str()) {
                ("gemma4", "assistant") => write!(prompt, "<|turn>model\n{content}<turn|>\n"),
                ("gemma4", role) => write!(prompt, "<|turn>{role}\n{content}<turn|>\n"),
                ("mistral-v7-tekken", "system") => write!(prompt, "[SYSTEM_PROMPT]{content}[/SYSTEM_PROMPT]"),
                ("mistral-v7-tekken", "user") => write!(prompt, "[INST]{content}[/INST]"),
                ("mistral-v7-tekken", _) => write!(prompt, "{content}</s>"),
                (_, role) => write!(prompt, "<|im_start|>{role}\n{content}<|im_end|>\n"),
            };
        }
        prompt.push_str(match template {
            "gemma4" => "<|turn>model\n",
            "mistral-v7-tekken" => "",
            _ => "<|im_start|>assistant\n",
        });
        prompt
    }

    /// Makes the prompt fit `budget` tokens (`count` tokenizes): drops the oldest non-system messages (never the
    /// last one). The remaining prompt must fit intact: truncating a transcript silently loses words.
    fn fit_messages(
        template: &str,
        messages: &[(String, String)],
        budget: usize,
        count: impl Fn(&str) -> usize,
    ) -> Result<Vec<(String, String)>, String> {
        let mut messages = messages.to_vec();
        loop {
            let used = count(&build_prompt(template, &messages));
            if used <= budget {
                return Ok(messages);
            }
            let last = messages.len().saturating_sub(1);
            if let Some(i) = messages[..last].iter().position(|(role, _)| role != "system") {
                messages.remove(i);
                continue;
            }
            return Err("The conversation is too long for the local language model".into());
        }
    }

    /// The reply without `<think>…</think>` blocks and `<transcript>` tags, trimmed.
    fn clean_output(text: &str) -> String {
        let mut text = text.to_string();
        while let Some(start) = text.find("<think>") {
            match text[start..].find("</think>") {
                Some(end) => text.replace_range(start..start + end + "</think>".len(), ""),
                None => text.truncate(start), // still thinking when it hit max_tokens
            }
        }
        text.replace("<transcript>", "").replace("</transcript>", "").trim().to_string()
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        fn msgs(list: &[(&str, &str)]) -> Vec<(String, String)> {
            list.iter().map(|(r, c)| (r.to_string(), c.to_string())).collect()
        }

        #[test]
        fn builds_prompts_per_template() {
            let chat = msgs(&[("system", "Be brief."), ("user", "Hi"), ("assistant", "Hello!"), ("user", "Bye\0")]);
            assert_eq!(
                build_prompt("chatml", &chat),
                "<|im_start|>system\nBe brief.<|im_end|>\n<|im_start|>user\nHi<|im_end|>\n<|im_start|>assistant\n\
                 Hello!<|im_end|>\n<|im_start|>user\nBye<|im_end|>\n<|im_start|>assistant\n"
            );
            assert_eq!(
                build_prompt("gemma4", &chat),
                "<|turn>system\nBe brief.<turn|>\n<|turn>user\nHi<turn|>\n<|turn>model\nHello!<turn|>\n\
                 <|turn>user\nBye<turn|>\n<|turn>model\n"
            );
            assert_eq!(
                build_prompt("mistral-v7-tekken", &chat),
                "[SYSTEM_PROMPT]Be brief.[/SYSTEM_PROMPT][INST]Hi[/INST]Hello!</s>[INST]Bye[/INST]"
            );
            assert_eq!(build_prompt("", &chat[1..2]), "<|im_start|>user\nHi<|im_end|>\n<|im_start|>assistant\n");
        }

        #[test]
        fn fits_long_conversations() {
            let chars = |p: &str| p.chars().count();
            let chat = msgs(&[("system", "S"), ("user", "old question"), ("assistant", "old answer"), ("user", "new")]);
            let full = chars(&build_prompt("chatml", &chat));
            assert_eq!(fit_messages("chatml", &chat, full, chars).unwrap(), chat);

            // Drops the oldest non-system messages first, keeping the system prompt and the last message.
            let fitted = fit_messages("chatml", &chat, full - 30, chars).unwrap();
            assert_eq!(fitted, msgs(&[("system", "S"), ("assistant", "old answer"), ("user", "new")]));
            let fitted = fit_messages("chatml", &chat, full - 41, chars).unwrap();
            assert_eq!(fitted, msgs(&[("system", "S"), ("user", "new")]));

            // Neither the latest transcript nor the system instructions may be silently shortened.
            let long = msgs(&[("system", "S"), ("user", &"ü".repeat(500))]);
            assert!(fit_messages("chatml", &long, 200, chars).is_err());
            assert!(fit_messages("chatml", &long, 10, chars).is_err());
            assert!(fit_messages("chatml", &msgs(&[("system", &"S".repeat(500)), ("user", "new")]), 200, chars).is_err());
            let minimum = msgs(&[("system", "S"), ("user", "new")]);
            let exact = chars(&build_prompt("chatml", &minimum));
            assert_eq!(fit_messages("chatml", &chat, exact, chars).unwrap(), minimum);
            assert!(fit_messages("chatml", &chat, exact - 1, chars).is_err());
        }

        #[test]
        fn output_limit_is_an_error_even_with_usable_partial_text() {
            assert!(finish_output("An incomplete transcript", false).is_err());
            assert!(finish_output("", false).is_err());
            assert_eq!(finish_output("<transcript>Complete.</transcript>", true).unwrap(), "Complete.");
        }

        #[test]
        fn cleans_output() {
            assert_eq!(clean_output("<think>\nhmm\n</think>\n\nHello."), "Hello.");
            assert_eq!(clean_output("<transcript>\nHi there.\n</transcript>"), "Hi there.");
            assert_eq!(clean_output("A<think>x</think>B<think>unfinished"), "AB");
            assert_eq!(clean_output("  plain  "), "plain");
        }
    }
}
