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
    /// "accessibility" | "microphone" | "no_microphone" | "hotkeys" | "stt" | "stt_backup_1"…"stt_backup_4" | "llm"
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
    /// Unsupported runtime/languages for a local live primary, if its model file is ready.
    pub stt_live_error: Option<&'a str>,
    pub llm_model: ModelFile,
}

/// Display names of the cloud presets that need an API key (Ollama, LM Studio and custom servers don't).
fn keyed_provider(id: &str) -> Option<&'static str> {
    Some(match id {
        "groq" => "Groq",
        "openai" => "OpenAI",
        "elevenlabs" => "ElevenLabs",
        "microsoft" => "Microsoft Azure",
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
    let backups = if config.uses_live_transcription() { config.live_backup_configs() } else { Vec::new() };
    let stt_level = if backups.iter().any(|backup| crate::realtime::validate_live_config(backup).is_ok()) {
        "warning"
    } else {
        "error"
    };

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
            // A cloud live backup can keep dictation available while this downloads.
            ModelFile::Downloading => Some("Downloading {model}…"),
            ModelFile::Missing => Some("Download the transcription model"),
        };
        if let Some(title) = title {
            add("stt", stt_level, title, "{model} isn't on this Mac yet.", &[("model", &model)], open_model);
        } else if config.uses_live_transcription() {
            if let Some(error) = c.stt_live_error {
                add("stt", stt_level, "Set up transcription", error, &[], open_model);
            }
        }
    } else if config.endpoint_url.trim().is_empty() {
        add("stt", stt_level, "Set up transcription", "Choose where your audio is transcribed.", &[], open_model);
    } else if let Some(provider) = keyed_provider(&config.stt_provider).filter(|_| config.api_key.trim().is_empty()) {
        add(
            "stt",
            stt_level,
            "Add your {provider} API key",
            "Transcription needs it. Paste it in Settings › Model.",
            &[("provider", provider)],
            open_model,
        );
    } else if config.uses_live_transcription() {
        if let Err(error) = crate::realtime::validate_live_config(config) {
            add("stt", stt_level, "Set up transcription", &error, &[], open_model);
        }
    }

    let backup_ids = config.live_backup_slots()
        .into_iter().filter(|(_, enabled)| *enabled).map(|(id, _)| id);
    for (id, backup) in backup_ids.zip(&backups) {
        if backup.endpoint_url.trim().is_empty() || !crate::realtime::is_live_model(&backup.model_name) {
            add(id, "warning", "Set up transcription", "Choose where your audio is transcribed.", &[], open_model);
        } else if let Some(provider) = keyed_provider(&backup.stt_provider).filter(|_| backup.api_key.trim().is_empty()) {
            add(
                id,
                "warning",
                "Add your {provider} API key",
                "Transcription needs it. Paste it in Settings › Model.",
                &[("provider", provider)],
                open_model,
            );
        } else if let Err(error) = crate::realtime::validate_live_config(backup) {
            add(id, "warning", "Set up transcription", &error, &[], open_model);
        }
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
    let stt_live_error = local.iter().find(|m| m.model.id == config.local_stt_model).and_then(|m| {
        if !m.runtime_supported {
            Some("Microsoft local models require an Apple Silicon Mac with macOS 14 or later.")
        } else if m.supported_languages.is_some_and(|supported| config.languages.iter()
            .map(|language| language.trim()).filter(|language| !language.is_empty()).any(|language| !supported.contains(&language))) {
            Some("This model does not support the selected dictation languages. Choose another model or change the languages in Settings.")
        } else {
            None
        }
    });
    compute(&Checks {
        config: &config,
        hotkeys: &hotkeys,
        mic: mic_permission(fresh_mic),
        has_microphone: audio::has_input_device(),
        stt_model: file(&config.local_stt_model),
        stt_live_error,
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
        Checks { config, hotkeys, mic: Mic::Granted, has_microphone: true, stt_model, stt_live_error: None, llm_model }
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

    fn live_config() -> TranscriptionConfig {
        TranscriptionConfig {
            stt_provider: "openai".into(),
            endpoint_url: "https://api.openai.com/v1/audio/transcriptions".into(),
            model_name: "gpt-live-transcribe".into(),
            api_key: "primary-key".into(),
            llm_source: "off".into(),
            stt_backup_1_enabled: true,
            stt_backup_1_provider: "elevenlabs".into(),
            stt_backup_1_endpoint_url: "https://api.elevenlabs.io/v1/speech-to-text".into(),
            stt_backup_1_model_name: "scribe_v2_realtime".into(),
            stt_backup_1_api_key: "second-key".into(),
            stt_backup_2_enabled: true,
            stt_backup_2_provider: "microsoft".into(),
            stt_backup_2_endpoint_url: "https://test.services.ai.azure.com".into(),
            stt_backup_2_model_name: crate::microsoft::LIVE_MODEL.into(),
            stt_backup_2_api_key: "third-key".into(),
            ..Default::default()
        }
    }

    #[test]
    fn a_valid_live_backup_makes_an_unconfigured_primary_nonblocking() {
        assert!(compute(&checks(&live_config(), &OK)).is_empty());
        for (key, endpoint) in [("", "https://api.openai.com/v1/audio/transcriptions"), ("primary-key", ""), ("primary-key", "invalid-url")] {
            let config = TranscriptionConfig { api_key: key.into(), endpoint_url: endpoint.into(), ..live_config() };
            let issues = compute(&checks(&config, &OK));
            assert_eq!(levels(&issues), [("stt", "warning")]);
            assert!(!issues.iter().any(|issue| blocks(issue, Purpose::Dictation) || blocks(issue, Purpose::Command)));
        }
        // The third provider remains usable when the first two have no key.
        let config = TranscriptionConfig { api_key: String::new(), stt_backup_1_api_key: String::new(), ..live_config() };
        let issues = compute(&checks(&config, &OK));
        assert_eq!(levels(&issues), [("stt", "warning"), ("stt_backup_1", "warning")]);
    }

    fn five_provider_config() -> TranscriptionConfig {
        TranscriptionConfig {
            stt_backup_3_enabled: true,
            stt_backup_3_provider: "elevenlabs".into(),
            stt_backup_3_endpoint_url: "https://api.elevenlabs.io/v1/speech-to-text".into(),
            stt_backup_3_model_name: "scribe_v2_realtime".into(),
            stt_backup_3_api_key: "fourth-key".into(),
            stt_backup_4_enabled: true,
            stt_backup_4_provider: "openai".into(),
            stt_backup_4_endpoint_url: "https://api.openai.com/v1/audio/transcriptions".into(),
            stt_backup_4_model_name: "gpt-live-transcribe".into(),
            stt_backup_4_api_key: "fifth-key".into(),
            ..live_config()
        }
    }

    #[test]
    fn a_working_fifth_provider_keeps_dictation_available_when_the_first_four_are_unusable() {
        let config = TranscriptionConfig {
            api_key: String::new(),
            stt_backup_1_api_key: String::new(),
            stt_backup_2_api_key: String::new(),
            stt_backup_3_endpoint_url: "invalid-url".into(),
            ..five_provider_config()
        };
        let issues = compute(&checks(&config, &OK));
        assert_eq!(levels(&issues), [("stt", "warning"), ("stt_backup_1", "warning"), ("stt_backup_2", "warning"), ("stt_backup_3", "warning")]);
        assert!(!issues.iter().any(|issue| blocks(issue, Purpose::Dictation) || blocks(issue, Purpose::Command)));
        let unusable = TranscriptionConfig { stt_backup_4_api_key: "invalid\nkey".into(), ..config.clone() };
        assert_eq!(levels(&compute(&checks(&unusable, &OK))), [
            ("stt", "error"), ("stt_backup_1", "warning"), ("stt_backup_2", "warning"), ("stt_backup_3", "warning"), ("stt_backup_4", "warning"),
        ]);
        let local = TranscriptionConfig { stt_source: "local".into(), local_stt_model: "vibevoice-asr-streaming-7b".into(), ..config };
        let issues = compute(&Checks { stt_model: ModelFile::Missing, ..checks(&local, &OK) });
        assert_eq!(issues[0].level, "warning");
        assert!(!issues.iter().any(|issue| blocks(issue, Purpose::Dictation)));
    }

    #[test]
    fn backup_warning_ids_keep_settings_order_with_nonconsecutive_enabled_slots() {
        let config = TranscriptionConfig {
            stt_backup_1_enabled: false,
            stt_backup_2_api_key: String::new(),
            stt_backup_3_enabled: false,
            stt_backup_4_api_key: String::new(),
            ..five_provider_config()
        };
        let issues = compute(&checks(&config, &OK));
        assert_eq!(levels(&issues), [("stt_backup_2", "warning"), ("stt_backup_4", "warning")]);
        assert_eq!(issues[0].vars["provider"], "Microsoft Azure");
        assert_eq!(issues[1].vars["provider"], "OpenAI");
        assert!(!issues.iter().any(|issue| blocks(issue, Purpose::Dictation)));
        let consecutive = TranscriptionConfig { stt_backup_3_enabled: true, stt_backup_3_api_key: String::new(), ..config };
        assert_eq!(levels(&compute(&checks(&consecutive, &OK))), [("stt_backup_2", "warning"), ("stt_backup_3", "warning"), ("stt_backup_4", "warning")]);
    }

    #[test]
    fn four_enabled_backups_are_inactive_for_primary_completed_recording_models() {
        let config = TranscriptionConfig {
            model_name: "gpt-transcribe".into(),
            stt_backup_1_api_key: String::new(),
            stt_backup_2_api_key: String::new(),
            stt_backup_3_model_name: "scribe_v2".into(),
            stt_backup_4_endpoint_url: "invalid-url".into(),
            ..five_provider_config()
        };
        assert!(compute(&checks(&config, &OK)).is_empty());
        let missing_primary = TranscriptionConfig { api_key: String::new(), ..config.clone() };
        assert_eq!(levels(&compute(&checks(&missing_primary, &OK))), [("stt", "error")]);
        let local = TranscriptionConfig { stt_source: "local".into(), local_stt_model: "whisper-tiny".into(), ..config };
        assert!(compute(&Checks { stt_live_error: Some("unavailable runtime"), ..checks(&local, &OK) }).is_empty());
    }

    #[test]
    fn invalid_live_backups_warn_without_blocking_a_working_primary() {
        let config = TranscriptionConfig {
            stt_backup_1_endpoint_url: "https://api.elevenlabs.io/v1/chat/completions".into(),
            stt_backup_2_deployment: "invalid\ndeployment".into(),
            ..live_config()
        };
        let issues = compute(&checks(&config, &OK));
        assert_eq!(levels(&issues), [("stt_backup_1", "warning"), ("stt_backup_2", "warning")]);
        assert!(!issues.iter().any(|issue| blocks(issue, Purpose::Dictation)));
        let disabled = TranscriptionConfig { stt_backup_1_enabled: false, stt_backup_2_enabled: false, ..config };
        assert!(compute(&checks(&disabled, &OK)).is_empty());
        let third_only = TranscriptionConfig { stt_backup_1_enabled: false, stt_backup_2_api_key: String::new(), ..live_config() };
        assert_eq!(levels(&compute(&checks(&third_only, &OK))), [("stt_backup_2", "warning")]);
    }

    #[test]
    fn only_valid_enabled_live_backups_can_satisfy_transcription_readiness() {
        let no_primary_key = TranscriptionConfig { api_key: String::new(), stt_backup_2_enabled: false, ..live_config() };
        for backup in [
            TranscriptionConfig { stt_backup_1_enabled: false, ..no_primary_key.clone() },
            TranscriptionConfig { stt_backup_1_model_name: "scribe_v2".into(), ..no_primary_key.clone() },
            TranscriptionConfig { stt_backup_1_endpoint_url: "not-a-url".into(), ..no_primary_key.clone() },
            TranscriptionConfig { stt_backup_1_endpoint_url: "http://example.com/v1/speech-to-text".into(), ..no_primary_key.clone() },
            TranscriptionConfig { stt_backup_1_endpoint_url: "https://user:password@example.com/v1/speech-to-text".into(), ..no_primary_key.clone() },
            TranscriptionConfig { stt_backup_1_api_key: String::new(), ..no_primary_key.clone() },
            TranscriptionConfig { stt_backup_1_api_key: "invalid\nkey".into(), ..no_primary_key.clone() },
        ] {
            let issues = compute(&checks(&backup, &OK));
            assert_eq!((issues[0].id.as_str(), issues[0].level.as_str()), ("stt", "error"));
            assert!(blocks(&issues[0], Purpose::Dictation));
        }
        let batch = TranscriptionConfig { model_name: "gpt-transcribe".into(), api_key: String::new(), ..live_config() };
        assert_eq!(levels(&compute(&checks(&batch, &OK))), [("stt", "error")]);
    }

    #[test]
    fn a_live_backup_keeps_local_live_dictation_available_while_the_primary_is_unavailable() {
        let local = TranscriptionConfig { stt_source: "local".into(), local_stt_model: "vibevoice-asr-streaming-7b".into(), ..live_config() };
        for stt_model in [ModelFile::Missing, ModelFile::Downloading] {
            let issues = compute(&Checks { stt_model, ..checks(&local, &OK) });
            assert_eq!(levels(&issues), [("stt", "warning")]);
            assert!(!blocks(&issues[0], Purpose::Dictation));
        }
        for error in [
            "Microsoft local models require an Apple Silicon Mac with macOS 14 or later.",
            "This model does not support the selected dictation languages. Choose another model or change the languages in Settings.",
        ] {
            let issues = compute(&Checks { stt_live_error: Some(error), ..checks(&local, &OK) });
            assert_eq!(levels(&issues), [("stt", "warning")]);
            assert_eq!(issues[0].detail, error);
            let no_backup = TranscriptionConfig { stt_backup_1_enabled: false, stt_backup_2_enabled: false, ..local.clone() };
            let issues = compute(&Checks { stt_live_error: Some(error), ..checks(&no_backup, &OK) });
            assert_eq!(levels(&issues), [("stt", "error")]);
        }
        let local_batch = TranscriptionConfig { local_stt_model: "whisper-tiny".into(), ..local };
        assert_eq!(levels(&compute(&Checks { stt_model: ModelFile::Missing, ..checks(&local_batch, &OK) })), [("stt", "error")]);
    }

    #[test]
    fn live_backups_never_bypass_permissions_devices_or_language_model_readiness() {
        let config = TranscriptionConfig { api_key: String::new(), llm_source: "cloud".into(), llm_api_key: String::new(), ..live_config() };
        let issues = compute(&Checks { mic: Mic::Denied, has_microphone: false, ..checks(&config, &OK) });
        assert_eq!(levels(&issues), [("microphone", "error"), ("no_microphone", "error"), ("stt", "warning"), ("llm", "warning")]);
        let only_models = compute(&checks(&config, &OK));
        assert!(!only_models.iter().any(|issue| blocks(issue, Purpose::Dictation)));
        assert_eq!(only_models.iter().filter(|issue| blocks(issue, Purpose::Command)).map(|issue| issue.id.as_str()).collect::<Vec<_>>(), ["llm"]);
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
