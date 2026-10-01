//! What still keeps dictation from working (or working well): the Home setup card and the check
//! before a recording starts.

use crate::hotkeys::{HotkeyEngine, HotkeyStatus};
use crate::transcribe::TranscriptionConfig;
use crate::{audio, models, AppState, Purpose};
use serde::Serialize;
use std::collections::BTreeMap;
use tauri::{AppHandle, Manager};

#[derive(Serialize, Clone, Debug, PartialEq)]
pub struct SetupIssue {
    /// "accessibility" | "microphone" | "no_microphone" | "hotkeys" | "stt" | "llm"
    pub id: String,
    /// "error" (dictation can't work) | "warning" (works, but degraded)
    pub level: String,
    /// English UI text; the frontend translates it with `vars`.
    pub title: String,
    pub detail: String,
    pub vars: BTreeMap<String, String>,
    /// "open_accessibility" | "open_microphone" | "request_microphone" | "open_model" | "open_hotkeys"
    pub action: Option<String>,
}

/// macOS microphone permission (always `Granted` elsewhere).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mic {
    NotAsked,
    Denied,
    Granted,
}

/// A selected local model's file.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ModelFile {
    Ready,
    Downloading,
    Missing,
}

/// Everything the issues depend on.
pub struct Checks<'a> {
    pub config: &'a TranscriptionConfig,
    pub hotkeys: &'a HotkeyStatus,
    pub mic: Mic,
    pub has_microphone: bool,
    pub stt_model: ModelFile,
    pub llm_model: ModelFile,
}

/// Display names of the cloud presets that need an API key (Ollama, LM Studio and custom servers don't).
fn keyed_provider(id: &str) -> Option<&'static str> {
    Some(match id {
        "groq" => "Groq",
        "openai" => "OpenAI",
        "elevenlabs" => "ElevenLabs",
        "openrouter" => "OpenRouter",
        "mistral" => "Mistral",
        "gemini" => "Google Gemini",
        _ => return None,
    })
}

/// Errors first, otherwise in check order.
pub fn compute(c: &Checks) -> Vec<SetupIssue> {
    let mut issues = Vec::new();
    let mut add = |id: &str, level: &str, title: &str, detail: &str, vars: &[(&str, &str)], action: Option<&str>| {
        issues.push(SetupIssue {
            id: id.into(),
            level: level.into(),
            title: title.into(),
            detail: detail.into(),
            vars: vars.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect(),
            action: action.map(String::from),
        })
    };
    let config = c.config;
    let model_name = |id: &str| models::find(id).map_or(id.to_string(), |m| m.name.to_string());
    let open_model = Some("open_model");

    if !c.hotkeys.permission_granted {
        add(
            "accessibility",
            "error",
            "Allow Accessibility access",
            "OpenGlaido needs it to notice your hotkeys and to paste text.",
            &[],
            Some("open_accessibility"),
        );
    }
    match c.mic {
        Mic::Denied => add(
            "microphone",
            "error",
            "Allow microphone access",
            "OpenGlaido can't hear you. Turn it on in System Settings › Privacy & Security › Microphone.",
            &[],
            Some("open_microphone"),
        ),
        Mic::NotAsked => add(
            "microphone",
            "warning",
            "Allow microphone access",
            "macOS asks the first time you dictate.",
            &[],
            Some("request_microphone"),
        ),
        Mic::Granted => {}
    }
    if !c.has_microphone {
        add("no_microphone", "error", "No microphone found", "Connect a microphone to dictate.", &[], None);
    }
    // Without Accessibility the error is the Accessibility message, already covered above.
    if let Some(error) = c.hotkeys.error.as_deref().filter(|_| c.hotkeys.permission_granted) {
        add("hotkeys", "warning", "A hotkey isn't working", error, &[], Some("open_hotkeys"));
    }

    if config.stt_source == "local" {
        let model = model_name(&config.local_stt_model);
        let title = match c.stt_model {
            ModelFile::Ready => None,
            // Still an error: dictation can't work until it's done.
            ModelFile::Downloading => Some("Downloading {model}…"),
            ModelFile::Missing => Some("Download the transcription model"),
        };
        if let Some(title) = title {
            add("stt", "error", title, "{model} isn't on this Mac yet.", &[("model", &model)], open_model);
        }
    } else if config.endpoint_url.trim().is_empty() {
        add("stt", "error", "Set up transcription", "Choose where your audio is transcribed.", &[], open_model);
    } else if let Some(provider) = keyed_provider(&config.stt_provider).filter(|_| config.api_key.trim().is_empty()) {
        add(
            "stt",
            "error",
            "Add your {provider} API key",
            "Transcription needs it. Paste it in Settings › Model.",
            &[("provider", provider)],
            open_model,
        );
    }

    match config.llm_source.as_str() {
        "local" => {
            let model = model_name(&config.local_llm_model);
            let title = match c.llm_model {
                ModelFile::Ready => None,
                ModelFile::Downloading => Some("Downloading {model}…"),
                ModelFile::Missing => Some("Download the language model"),
            };
            if let Some(title) = title {
                let detail = "{model} isn't on this Mac yet, so dictations aren't cleaned up.";
                add("llm", "warning", title, detail, &[("model", &model)], open_model);
            }
        }
        "cloud" => {
            if let Some(provider) = keyed_provider(&config.llm_provider).filter(|_| config.llm_api_key.trim().is_empty()) {
                add(
                    "llm",
                    "warning",
                    "Add your {provider} API key",
                    "The language model needs it to clean up dictations and answer commands.",
                    &[("provider", provider)],
                    open_model,
                );
            }
        }
        _ => {}
    }
    issues.sort_by_key(|i| i.level != "error");
    issues
}

fn collect(app: &AppHandle, fresh_mic: bool) -> Vec<SetupIssue> {
    let config = app.state::<AppState>().config.lock().unwrap().clone();
    let hotkeys = app.state::<HotkeyEngine>().status();
    let local = models::list_local_models(app.clone());
    let file = |id: &str| match local.iter().find(|m| m.model.id == id) {
        Some(m) if m.downloaded => ModelFile::Ready,
        Some(m) if m.download.as_ref().is_some_and(|d| matches!(d.state.as_str(), "downloading" | "verifying")) => {
            ModelFile::Downloading
        }
        _ => ModelFile::Missing,
    };
    compute(&Checks {
        config: &config,
        hotkeys: &hotkeys,
        mic: mic_permission(fresh_mic),
        has_microphone: audio::has_input_device(),
        stt_model: file(&config.local_stt_model),
        llm_model: file(&config.local_llm_model),
    })
}

/// Current issues for the Home card. Re-checks the microphone permission.
pub fn issues(app: &AppHandle) -> Vec<SetupIssue> {
    collect(app, true)
}

/// Whether `issue` keeps a recording for `purpose` from starting: errors, and for a command (which
/// needs the language model) any language model issue.
fn blocks(issue: &SetupIssue, purpose: Purpose) -> bool {
    issue.level == "error" || (issue.id == "llm" && purpose == Purpose::Command)
}

/// The first issue that blocks this recording, title still a translation key.
/// Uses the cached microphone grant so a recording doesn't wait on tccd.
pub fn blocker(app: &AppHandle, purpose: Purpose) -> Option<SetupIssue> {
    collect(app, false).into_iter().find(|i| blocks(i, purpose))
}

#[tauri::command]
pub async fn get_setup_issues(app: AppHandle) -> Vec<SetupIssue> {
    issues(&app)
}

#[tauri::command]
pub fn open_microphone_settings() {
    #[cfg(target_os = "macos")]
    if let Err(e) = std::process::Command::new("open")
        .arg("x-apple.systempreferences:com.apple.preference.security?Privacy_Microphone")
        .status()
    {
        eprintln!("Failed to open Microphone settings: {}", e);
    }
}

/// Shows macOS's microphone prompt (only the first time); resolves with whether access is granted.
#[tauri::command]
pub async fn request_microphone_access() -> bool {
    #[cfg(target_os = "macos")]
    {
        let (tx, rx) = tokio::sync::oneshot::channel();
        av::request_access(move |granted| {
            let _ = tx.send(granted);
        });
        rx.await.unwrap_or(false)
    }
    #[cfg(not(target_os = "macos"))]
    true
}

// A granted microphone stays cached so a recording doesn't wait on tccd (~15 ms). The Home card
// passes fresh=true and always re-queries. Revoking access while running then shows on Home
// immediately, and on the next recording only after a restart once the cache says granted.
#[cfg(target_os = "macos")]
fn mic_permission(fresh: bool) -> Mic {
    use std::sync::atomic::{AtomicBool, Ordering};
    static GRANTED: AtomicBool = AtomicBool::new(false);
    if !fresh && GRANTED.load(Ordering::Relaxed) {
        return Mic::Granted;
    }
    // AVAuthorizationStatus: 0 not determined, 1 restricted, 2 denied, 3 authorized.
    let mic = match av::authorization_status() {
        0 => Mic::NotAsked,
        1 | 2 => Mic::Denied,
        _ => Mic::Granted,
    };
    GRANTED.store(mic == Mic::Granted, Ordering::Relaxed);
    mic
}

#[cfg(not(target_os = "macos"))]
fn mic_permission(_fresh: bool) -> Mic {
    Mic::Granted
}

#[cfg(target_os = "macos")]
mod av {
    use block2::RcBlock;
    use objc2::runtime::Bool;
    use objc2::{class, msg_send};
    use objc2_foundation::NSString;
    use std::sync::Mutex;

    #[link(name = "AVFoundation", kind = "framework")]
    extern "C" {
        static AVMediaTypeAudio: &'static NSString;
    }

    pub fn authorization_status() -> isize {
        unsafe { msg_send![class!(AVCaptureDevice), authorizationStatusForMediaType: AVMediaTypeAudio] }
    }

    /// `done` runs once, on an arbitrary thread (right away when already decided).
    pub fn request_access(done: impl FnOnce(bool) + Send + 'static) {
        let done = Mutex::new(Some(done));
        let block = RcBlock::new(move |granted: Bool| {
            if let Some(done) = done.lock().unwrap().take() {
                done(granted.as_bool());
            }
        });
        unsafe {
            let _: () = msg_send![
                class!(AVCaptureDevice),
                requestAccessForMediaType: AVMediaTypeAudio,
                completionHandler: &*block
            ];
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const OK: HotkeyStatus = HotkeyStatus { engine: String::new(), permission_granted: true, error: None };

    /// Everything granted and on disk.
    fn checks<'a>(config: &'a TranscriptionConfig, hotkeys: &'a HotkeyStatus) -> Checks<'a> {
        let (stt_model, llm_model) = (ModelFile::Ready, ModelFile::Ready);
        Checks { config, hotkeys, mic: Mic::Granted, has_microphone: true, stt_model, llm_model }
    }

    /// Cloud Groq with both keys.
    fn configured() -> TranscriptionConfig {
        TranscriptionConfig { api_key: "gsk".into(), llm_api_key: "gsk".into(), ..Default::default() }
    }

    fn levels(issues: &[SetupIssue]) -> Vec<(&str, &str)> {
        issues.iter().map(|i| (i.id.as_str(), i.level.as_str())).collect()
    }

    #[test]
    fn permissions_devices_and_hotkeys_errors_first() {
        let config = configured();
        assert!(compute(&checks(&config, &OK)).is_empty());
        let no_access = HotkeyStatus { permission_granted: false, error: Some("needs Accessibility".into()), ..OK };
        let issues = compute(&Checks { mic: Mic::Denied, has_microphone: false, ..checks(&config, &no_access) });
        assert_eq!(levels(&issues), [("accessibility", "error"), ("microphone", "error"), ("no_microphone", "error")]);

        let broken = HotkeyStatus { error: Some("Couldn't start the keyboard listener".into()), ..OK };
        let no_llm_key = TranscriptionConfig { llm_api_key: String::new(), ..configured() };
        let issues = compute(&Checks { mic: Mic::NotAsked, ..checks(&no_llm_key, &broken) });
        assert_eq!(levels(&issues), [("microphone", "warning"), ("hotkeys", "warning"), ("llm", "warning")]);
        assert_eq!(issues[0].action.as_deref(), Some("request_microphone"));
        assert_eq!((issues[1].detail.as_str(), issues[1].action.as_deref()), ("Couldn't start the keyboard listener", Some("open_hotkeys")));
    }

    #[test]
    fn cloud_keys() {
        let default = TranscriptionConfig::default();
        let issues = compute(&checks(&default, &OK));
        assert_eq!(levels(&issues), [("stt", "error"), ("llm", "warning")]);
        assert_eq!((issues[0].title.as_str(), issues[0].vars["provider"].as_str()), ("Add your {provider} API key", "Groq"));
        // Local servers and custom endpoints need no key; an empty endpoint needs setting up.
        let keyless = TranscriptionConfig { stt_provider: "custom".into(), llm_provider: "ollama".into(), ..Default::default() };
        assert!(compute(&checks(&keyless, &OK)).is_empty());
        let no_url = TranscriptionConfig { endpoint_url: " ".into(), llm_source: "off".into(), ..Default::default() };
        assert_eq!(compute(&checks(&no_url, &OK))[0].title, "Set up transcription");
        let gemini = TranscriptionConfig { llm_provider: "gemini".into(), llm_api_key: String::new(), ..configured() };
        assert_eq!(compute(&checks(&gemini, &OK))[0].vars["provider"], "Google Gemini");
        let elevenlabs = TranscriptionConfig {
            stt_provider: "elevenlabs".into(), endpoint_url: "https://api.elevenlabs.io/v1/speech-to-text".into(),
            api_key: String::new(), llm_source: "off".into(), ..configured()
        };
        assert_eq!(compute(&checks(&elevenlabs, &OK))[0].vars["provider"], "ElevenLabs");
    }

    #[test]
    fn local_models() {
        let local = TranscriptionConfig {
            stt_source: "local".into(),
            local_stt_model: "whisper-tiny".into(),
            llm_source: "local".into(),
            local_llm_model: "gemma-4-e2b-it-q4km".into(),
            ..Default::default()
        };
        assert!(compute(&checks(&local, &OK)).is_empty());
        let issues = compute(&Checks { stt_model: ModelFile::Missing, llm_model: ModelFile::Downloading, ..checks(&local, &OK) });
        let summary: Vec<_> = issues.iter().map(|i| (i.id.as_str(), i.level.as_str(), i.title.as_str(), i.vars["model"].as_str())).collect();
        assert_eq!(
            summary,
            [
                ("stt", "error", "Download the transcription model", "Tiny"),
                ("llm", "warning", "Downloading {model}…", "Gemma 4 E2B Instruct"),
            ]
        );
        let issues = compute(&Checks { stt_model: ModelFile::Downloading, llm_model: ModelFile::Missing, ..checks(&local, &OK) });
        assert_eq!((issues[0].title.as_str(), issues[1].title.as_str()), ("Downloading {model}…", "Download the language model"));
        // Language model off: nothing to download.
        let off = TranscriptionConfig { llm_source: "off".into(), ..local.clone() };
        assert!(compute(&Checks { llm_model: ModelFile::Missing, ..checks(&off, &OK) }).is_empty());
    }

    #[test]
    fn what_blocks_a_recording() {
        let config = TranscriptionConfig { llm_api_key: String::new(), ..configured() };
        let broken = HotkeyStatus { error: Some("x".into()), ..OK };
        let issues = compute(&Checks { mic: Mic::NotAsked, ..checks(&config, &broken) });
        let blocking = |purpose| issues.iter().filter(|i| blocks(i, purpose)).map(|i| i.id.as_str()).collect::<Vec<_>>();
        // Microphone not asked yet, a hotkey warning and a missing language model key don't stop dictation…
        assert!(blocking(Purpose::Dictation).is_empty());
        // …but commands need the language model.
        assert_eq!(blocking(Purpose::Command), ["llm"]);
        let llm = issues.last().unwrap();
        assert_eq!(llm.title, "Add your {provider} API key");
        assert_eq!(llm.vars["provider"], "Groq");
        let denied = compute(&Checks { mic: Mic::Denied, ..checks(&config, &OK) });
        assert!(blocks(&denied[0], Purpose::Dictation));
    }
}
