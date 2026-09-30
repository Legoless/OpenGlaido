use crate::frontmost::AppContext;
use crate::hotkeys;
use reqwest::multipart::{Form, Part};
use reqwest::Client;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::time::Duration;

// Persisted as <app_data_dir>/config.json; missing fields fall back to Default.
#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
#[serde(default)]
pub struct TranscriptionConfig {
    pub endpoint_url: String,       // e.g. "https://api.groq.com/openai/v1/audio/transcriptions" or custom URL
    pub api_key: String,
    pub model_name: String,         // e.g. "whisper-large-v3-turbo" or "whisper-1"
    pub temperature: Option<f32>,
    pub llm_endpoint_url: Option<String>, // e.g. "https://api.groq.com/openai/v1/chat/completions" or Ollama
    pub llm_model_name: Option<String>,
    pub sound_feedback: bool,       // play audio chimes on start/stop
    pub hotkey_hold: String,        // push-to-talk binding, see hotkeys.rs ("" = disabled)
    pub hotkey_toggle: String,      // hands-free binding
    pub enter_to_stop: bool,        // Enter stops & pastes while dictating hands-free
    pub input_device: Option<String>, // cpal device name; None = system default
    pub copy_to_clipboard: bool,    // false = restore the previous clipboard text after pasting
    pub bar_location: String,       // "bottom" | "raised" | "high"
    pub launch_at_login: bool,
    pub show_in_menu_bar: bool,
    pub show_in_dock: bool,
    // --- Formatting ("All apps" + rules) ---
    pub style: String,              // "standard" | "casual" | "lowercase"
    pub raw_text: bool,             // true = no LLM pass
    pub custom_prompt: String,      // ≤ CUSTOM_PROMPT_MAX chars
    pub email_rule: FormattingRule,
    pub custom_rules: Vec<FormattingRule>,
    pub languages: Vec<String>,     // Whisper ISO-639-1 codes; [] = auto, 1 = pinned, ≥2 = auto
    pub mute_background: bool,
    pub show_bar_when_idle: bool,
    pub theme: String,              // "dark" | "light" | "system"
    pub app_language: String,       // "system" or a UI locale ("en", "de", "sr-Latn", ...)
    pub beta_features: bool,
    // --- Commands ---
    pub commands_hold: String,
    pub commands_toggle: String,
    pub builtin_tools: BTreeMap<String, bool>, // BUILTIN_TOOL_IDS -> enabled
    pub search_provider: String,    // "none" | "brave" | "tavily" | "searxng"
    pub search_api_key: String,
    pub searxng_url: String,
    pub mcp_servers: BTreeMap<String, McpServerPrefs>,
}

pub const CUSTOM_PROMPT_MAX: usize = 500;

pub const BUILTIN_TOOL_IDS: [&str; 8] = [
    "web_search",
    "read_page",
    "youtube",
    "deep_research",
    "math_dates",
    "files_apps",
    "history_search",
    "docs_search",
];

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq, Default)]
#[serde(default)]
pub struct AppRef {
    pub bundle_id: String,
    pub name: String,
}

/// A per-app/website formatting rule (the built-in Email rule or a custom one).
#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
#[serde(default)]
pub struct FormattingRule {
    pub id: String,
    pub name: String,
    pub enabled: bool,
    pub apps: Vec<AppRef>,
    pub websites: Vec<String>, // hosts, suffix-matched against the browser URL
    pub style: String,
    pub raw_text: bool,
    pub custom_prompt: String,
}

impl Default for FormattingRule {
    fn default() -> Self {
        Self {
            id: String::new(),
            name: String::new(),
            enabled: true,
            apps: Vec::new(),
            websites: Vec::new(),
            style: "standard".to_string(),
            raw_text: false,
            custom_prompt: String::new(),
        }
    }
}

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq, Default)]
#[serde(default)]
pub struct McpServerPrefs {
    pub enabled: bool,
    pub tool_policies: BTreeMap<String, String>, // tool -> "auto" | "ask" | "deny"
}

impl Default for TranscriptionConfig {
    fn default() -> Self {
        Self {
            endpoint_url: "https://api.groq.com/openai/v1/audio/transcriptions".to_string(),
            api_key: "".to_string(),
            model_name: "whisper-large-v3-turbo".to_string(),
            temperature: Some(0.0),
            llm_endpoint_url: Some("https://api.groq.com/openai/v1/chat/completions".to_string()),
            llm_model_name: Some("llama-3.3-70b-versatile".to_string()),
            sound_feedback: true,
            hotkey_hold: hotkeys::default_hold().to_string(),
            hotkey_toggle: hotkeys::default_toggle().to_string(),
            enter_to_stop: false,
            input_device: None,
            copy_to_clipboard: false,
            bar_location: "bottom".to_string(),
            launch_at_login: false,
            show_in_menu_bar: true,
            show_in_dock: true,
            style: "standard".to_string(),
            raw_text: false,
            custom_prompt: String::new(),
            email_rule: FormattingRule {
                id: "email".to_string(),
                name: "Email".to_string(),
                websites: vec!["mail.google.com".to_string(), "outlook.live.com".to_string()],
                ..Default::default()
            },
            custom_rules: Vec::new(),
            languages: Vec::new(),
            mute_background: cfg!(target_os = "macos"),
            show_bar_when_idle: true,
            theme: "dark".to_string(),
            app_language: "system".to_string(),
            beta_features: false,
            commands_hold: hotkeys::default_commands_hold().to_string(),
            commands_toggle: hotkeys::default_commands_toggle().to_string(),
            builtin_tools: BUILTIN_TOOL_IDS.iter().map(|id| (id.to_string(), true)).collect(),
            search_provider: "none".to_string(),
            search_api_key: String::new(),
            searxng_url: String::new(),
            mcp_servers: BTreeMap::new(),
        }
    }
}

const CODE_PROMPT: &str = "Write spoken programming terms as code syntax (e.g. 'open paren' → '(').";

const STANDARD_PROMPT: &str = "You are a voice dictation text cleanup assistant. Clean up this raw transcription: fix capitalization and punctuation, and remove conversational filler sounds (um, uh). Keep exact words and meaning. Return ONLY the cleaned text with no introductory or meta commentary.";

impl TranscriptionConfig {
    /// Parses config.json, migrating the fields removed in batch 2 when the file still has them:
    /// `mode`, `enable_llm_formatting`, `custom_formatting_prompt` and `language`.
    pub fn from_json(json: &str) -> serde_json::Result<Self> {
        let old: serde_json::Value = serde_json::from_str(json)?;
        let mut config = Self::deserialize(&old)?;
        let old_str = |key: &str| old.get(key).and_then(|v| v.as_str()).map(str::trim).filter(|s| !s.is_empty());

        if let Some(mode) = old_str("mode") {
            config.style = if mode == "chat" { "casual" } else { "standard" }.to_string();
            match mode {
                "code" => config.custom_prompt = CODE_PROMPT.to_string(),
                "raw" => config.raw_text = true,
                _ => {}
            }
        }
        if old.get("enable_llm_formatting").and_then(|v| v.as_bool()) == Some(false) {
            config.raw_text = true;
        }
        // Old default prompt = today's "standard" style, not a custom instruction. Code mode ignored it.
        if let Some(prompt) = old_str("custom_formatting_prompt") {
            if prompt != STANDARD_PROMPT && old_str("mode") != Some("code") {
                config.custom_prompt = prompt.chars().take(CUSTOM_PROMPT_MAX).collect();
            }
        }
        if let Some(language) = old_str("language") {
            config.languages = vec![language.to_string()];
        }
        Ok(config)
    }
}

const EMAIL_GUIDANCE: &str = "The user is writing an email. Lay it out as an email: a greeting on its own line when one is dictated, short paragraphs, and a polite, professional tone. Keep their words and meaning; don't invent content or sign-offs.";

fn style_prompt(style: &str) -> &'static str {
    match style {
        "casual" => "You are a voice dictation text cleanup assistant. Clean up this raw transcription in a casual style: keep capitals, use lighter punctuation and leave out the full stop at the end of a message, and remove conversational filler sounds (um, uh). Keep exact words and meaning. Return ONLY the cleaned text with no introductory or meta commentary.",
        "lowercase" => "You are a voice dictation text cleanup assistant. Clean up this raw transcription: write everything in lowercase with minimal punctuation, and remove conversational filler sounds (um, uh). Keep exact words and meaning. Return ONLY the cleaned text with no introductory or meta commentary.",
        _ => STANDARD_PROMPT,
    }
}

/// System prompt for the LLM cleanup pass: the style's instruction, then the user's prompts in order.
pub fn formatting_prompt(style: &str, custom_prompt: &str) -> String {
    build_prompt(style, None, &[custom_prompt])
}

fn build_prompt(style: &str, guidance: Option<&str>, custom: &[&str]) -> String {
    let mut prompt = style_prompt(style).to_string();
    if let Some(g) = guidance {
        prompt = format!("{prompt}\n\n{g}");
    }
    let custom: Vec<&str> = custom.iter().map(|c| c.trim()).filter(|c| !c.is_empty()).collect();
    if !custom.is_empty() {
        prompt = format!("{prompt}\n\nAlso follow these instructions from the user:\n{}", custom.join("\n"));
    }
    prompt
}

/// What happens to one dictation's text: resolved from All apps and the rule that owns the app.
#[derive(Debug, Clone, PartialEq)]
pub struct Formatting {
    /// The matching rule's id (None = All apps).
    pub rule_id: Option<String>,
    pub raw_text: bool,
    /// Also applied locally when `raw_text` is on ("your style still applies").
    pub style: String,
    pub system_prompt: String,
}

/// The style without a model: lowercase lowercases, casual drops the final full stop.
pub fn apply_style_locally(text: &str, style: &str) -> String {
    match style {
        "lowercase" => text.to_lowercase(),
        "casual" => text.strip_suffix('.').filter(|t| !t.ends_with('.')).unwrap_or(text).to_string(),
        _ => text.to_string(),
    }
}

/// "https://www.Mail.Google.com:443/x" → "mail.google.com" (also used to normalise rule websites).
pub fn host_of(url: &str) -> String {
    let rest = url.trim().split_once("://").map_or(url.trim(), |(_, r)| r);
    let host = rest.split(['/', '?', '#']).next().unwrap_or("");
    let host = host.rsplit_once('@').map_or(host, |(_, h)| h);
    let host = host.split(':').next().unwrap_or("").to_ascii_lowercase();
    host.strip_prefix("www.").map(str::to_string).unwrap_or(host)
}

impl TranscriptionConfig {
    /// Email first, then custom rules in order; disabled rules never match.
    pub fn rules(&self) -> impl Iterator<Item = &FormattingRule> {
        std::iter::once(&self.email_rule).chain(&self.custom_rules)
    }

    /// The enabled rule that owns this app or website (website rules win for browsers).
    pub fn matching_rule(&self, ctx: Option<&AppContext>) -> Option<&FormattingRule> {
        let ctx = ctx?;
        let host = ctx.url.as_deref().map(host_of).filter(|h| !h.is_empty());
        let site_match = |rule: &&FormattingRule| {
            host.as_deref().is_some_and(|h| {
                rule.websites.iter().map(|w| host_of(w)).any(|w| !w.is_empty() && (h == w || h.ends_with(&format!(".{w}"))))
            })
        };
        let app_match = |rule: &&FormattingRule| rule.apps.iter().any(|a| a.bundle_id == ctx.bundle_id);
        let enabled = || self.rules().filter(|r| r.enabled);
        enabled().find(site_match).or_else(|| enabled().find(app_match))
    }

    pub fn formatting_for(&self, ctx: Option<&AppContext>) -> Formatting {
        match self.matching_rule(ctx) {
            Some(rule) => Formatting {
                rule_id: Some(rule.id.clone()),
                raw_text: rule.raw_text,
                style: rule.style.clone(),
                system_prompt: build_prompt(
                    &rule.style,
                    (rule.id == "email").then_some(EMAIL_GUIDANCE),
                    &[&self.custom_prompt, &rule.custom_prompt],
                ),
            },
            None => Formatting {
                rule_id: None,
                raw_text: self.raw_text,
                style: self.style.clone(),
                system_prompt: build_prompt(&self.style, None, &[&self.custom_prompt]),
            },
        }
    }

    /// save_config checks: prompt lengths, styles, rule names/ids, one rule per app/website.
    pub fn validate_formatting(&self) -> Result<(), String> {
        const STYLES: [&str; 3] = ["standard", "casual", "lowercase"];
        let too_long = |p: &str| p.chars().count() > CUSTOM_PROMPT_MAX;
        if too_long(&self.custom_prompt) {
            return Err(format!("The All apps prompt is longer than {CUSTOM_PROMPT_MAX} characters"));
        }
        if !STYLES.contains(&self.style.as_str()) {
            return Err(format!("Unknown style “{}”", self.style));
        }
        if self.email_rule.id != "email" {
            return Err("The Email rule can't be replaced".into());
        }
        let mut ids = std::collections::HashSet::new();
        let mut owner: std::collections::HashMap<String, &FormattingRule> = std::collections::HashMap::new();
        for rule in self.rules() {
            if rule.id.trim().is_empty() || !ids.insert(rule.id.as_str()) {
                return Err(format!("Rule “{}” needs a unique id", rule.name));
            }
            if rule.name.trim().is_empty() {
                return Err("Every rule needs a name".into());
            }
            if too_long(&rule.custom_prompt) {
                return Err(format!("The prompt of “{}” is longer than {CUSTOM_PROMPT_MAX} characters", rule.name));
            }
            if !STYLES.contains(&rule.style.as_str()) {
                return Err(format!("Unknown style “{}” in “{}”", rule.style, rule.name));
            }
            let places = rule.apps.iter().map(|a| (format!("app:{}", a.bundle_id), a.name.clone()))
                .chain(rule.websites.iter().map(|w| (format!("site:{}", host_of(w)), host_of(w))));
            for (key, label) in places {
                if let Some(other) = owner.insert(key, rule) {
                    // Same rule listing a place twice is harmless; another rule owning it is not.
                    if other.id != rule.id {
                        return Err(format!("{label} is already in your “{}” rule", other.name));
                    }
                }
            }
        }
        Ok(())
    }
}

#[derive(Deserialize)]
struct WhisperResponse {
    text: String,
}

#[derive(Serialize, Deserialize)]
struct ChatMessage {
    role: String,
    content: String,
}

#[derive(Serialize)]
struct ChatCompletionRequest {
    model: String,
    messages: Vec<ChatMessage>,
    temperature: f32,
}

#[derive(Deserialize)]
struct ChatCompletionChoice {
    message: ChatMessage,
}

#[derive(Deserialize)]
struct ChatCompletionResponse {
    choices: Vec<ChatCompletionChoice>,
}

fn http_client() -> Result<Client, String> {
    Client::builder().timeout(Duration::from_secs(45)).build().map_err(|e| e.to_string())
}

/// STT only: the raw transcript (trimmed).
pub async fn transcribe_raw(
    wav_bytes: Vec<u8>,
    config: &TranscriptionConfig,
    initial_prompt: Option<String>,
) -> Result<String, String> {
    let client = http_client()?;

    let part = Part::bytes(wav_bytes)
        .file_name("audio.wav")
        .mime_str("audio/wav")
        .map_err(|e| e.to_string())?;

    let mut form = Form::new()
        .text("model", config.model_name.clone())
        .part("file", part);

    if let Some(prompt) = initial_prompt {
        if !prompt.trim().is_empty() {
            form = form.text("prompt", prompt);
        }
    }

    // Exactly one language is pinned; none or several means Whisper auto-detects.
    if let [lang] = config.languages.as_slice() {
        if !lang.trim().is_empty() {
            form = form.text("language", lang.clone());
        }
    }

    if let Some(temp) = config.temperature {
        form = form.text("temperature", temp.to_string());
    }

    let mut request = client.post(&config.endpoint_url).multipart(form);
    if !config.api_key.trim().is_empty() {
        request = request.header("Authorization", format!("Bearer {}", config.api_key.trim()));
    }

    // Errors below are user-facing (shown in the dictation bar); details go to the log.
    let response = request.send().await.map_err(|e| {
        eprintln!("STT request failed: {}", e);
        "Transcription failed: couldn't reach the speech-to-text endpoint".to_string()
    })?;

    if !response.status().is_success() {
        let status = response.status();
        let body = response
            .text()
            .await
            .unwrap_or_else(|_| "Unknown error".to_string());
        eprintln!("STT API returned error {}: {}", status, body);
        return Err(match status.as_u16() {
            401 | 403 => "Transcription failed: check the API key in Settings › Model".to_string(),
            404 => "Transcription failed: check the endpoint URL in Settings › Model".to_string(),
            _ => format!("Transcription failed: the server returned {}", status),
        });
    }

    let whisper_res: WhisperResponse = response.json().await.map_err(|e| {
        eprintln!("Failed to parse STT response JSON: {}", e);
        "Transcription failed: unexpected response from the speech-to-text endpoint".to_string()
    })?;

    Ok(whisper_res.text.trim().to_string())
}

/// The LLM cleanup pass for `formatting`; falls back to the raw text on any failure.
pub async fn apply_formatting(raw_text: &str, config: &TranscriptionConfig, formatting: &Formatting) -> String {
    if formatting.raw_text || raw_text.is_empty() {
        return apply_style_locally(raw_text, &formatting.style);
    }
    let (Some(llm_url), Some(llm_model)) = (&config.llm_endpoint_url, &config.llm_model_name) else {
        return raw_text.to_string();
    };
    if llm_url.trim().is_empty() || llm_model.trim().is_empty() {
        return raw_text.to_string();
    }
    let result = match http_client() {
        Ok(client) => {
            format_transcript_with_llm(&client, raw_text, llm_url, llm_model, &config.api_key, &formatting.system_prompt).await
        }
        Err(e) => Err(e),
    };
    result.unwrap_or_else(|err| {
        eprintln!("Formatting pass failed, falling back to raw text: {}", err);
        raw_text.to_string()
    })
}

async fn format_transcript_with_llm(
    client: &Client,
    raw_text: &str,
    llm_url: &str,
    llm_model: &str,
    api_key: &str,
    system_prompt: &str,
) -> Result<String, String> {
    let chat_req = ChatCompletionRequest {
        model: llm_model.to_string(),
        messages: vec![
            ChatMessage {
                role: "system".to_string(),
                content: system_prompt.to_string(),
            },
            ChatMessage {
                role: "user".to_string(),
                content: raw_text.to_string(),
            },
        ],
        temperature: 0.1,
    };

    let mut req = client.post(llm_url).json(&chat_req);
    if !api_key.trim().is_empty() {
        req = req.header("Authorization", format!("Bearer {}", api_key.trim()));
    }

    let resp = req.send().await.map_err(|e| e.to_string())?;
    if !resp.status().is_success() {
        return Err(format!("LLM status: {}", resp.status()));
    }

    let completion: ChatCompletionResponse = resp.json().await.map_err(|e| e.to_string())?;
    if let Some(choice) = completion.choices.into_iter().next() {
        let cleaned = choice.message.content.trim().to_string();
        if !cleaned.is_empty() {
            return Ok(cleaned);
        }
    }

    Ok(raw_text.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_json_round_trip() {
        let cfg = TranscriptionConfig {
            input_device: Some("USB Mic".into()),
            bar_location: "high".into(),
            ..Default::default()
        };
        let json = serde_json::to_string(&cfg).unwrap();
        assert_eq!(serde_json::from_str::<TranscriptionConfig>(&json).unwrap(), cfg);
    }

    #[test]
    fn missing_fields_use_defaults_and_old_hotkey_is_ignored() {
        let cfg: TranscriptionConfig =
            serde_json::from_str(r#"{"api_key":"k","hotkey":"CommandOrControl+Shift+Space"}"#).unwrap();
        assert_eq!(cfg.api_key, "k");
        assert_eq!(cfg.hotkey_hold, hotkeys::default_hold());
        assert_eq!(cfg.hotkey_toggle, hotkeys::default_toggle());
        assert_eq!(cfg.bar_location, "bottom");
        assert!(cfg.show_in_menu_bar && cfg.show_in_dock && !cfg.copy_to_clipboard);
        assert_eq!(serde_json::from_str::<TranscriptionConfig>("{}").unwrap(), TranscriptionConfig::default());
    }

    #[test]
    fn new_defaults() {
        let cfg = TranscriptionConfig::default();
        assert_eq!((cfg.style.as_str(), cfg.raw_text, cfg.custom_prompt.as_str()), ("standard", false, ""));
        assert_eq!(cfg.email_rule.id, "email");
        assert!(cfg.email_rule.enabled && cfg.email_rule.apps.is_empty());
        assert_eq!(cfg.email_rule.websites, ["mail.google.com", "outlook.live.com"]);
        assert_eq!(cfg.builtin_tools.len(), 8);
        assert!(cfg.builtin_tools.values().all(|on| *on));
        assert_eq!(cfg.mute_background, cfg!(target_os = "macos"));
        assert!(cfg.show_bar_when_idle && !cfg.beta_features);
        assert_eq!((cfg.theme.as_str(), cfg.app_language.as_str()), ("dark", "system"));
        assert_eq!(cfg.search_provider, "none");
        assert_eq!(TranscriptionConfig::from_json("{}").unwrap(), cfg);
    }

    #[test]
    fn legacy_fields_migrate() {
        let cfg = TranscriptionConfig::from_json(
            r#"{"mode":"chat","enable_llm_formatting":true,"custom_formatting_prompt":"Use British spelling.","language":"de"}"#,
        )
        .unwrap();
        assert_eq!((cfg.style.as_str(), cfg.raw_text), ("casual", false));
        assert_eq!(cfg.custom_prompt, "Use British spelling.");
        assert_eq!(cfg.languages, ["de"]);

        let cfg = TranscriptionConfig::from_json(r#"{"mode":"code","custom_formatting_prompt":"ignored in code mode"}"#).unwrap();
        assert_eq!((cfg.style.as_str(), cfg.custom_prompt.as_str()), ("standard", CODE_PROMPT));

        let cfg = TranscriptionConfig::from_json(r#"{"mode":"raw"}"#).unwrap();
        assert!(cfg.raw_text);

        // Email mode, formatting off, the old default prompt and auto-detect.
        let json = serde_json::json!({
            "mode": "email", "enable_llm_formatting": false,
            "custom_formatting_prompt": STANDARD_PROMPT, "language": null,
        });
        let cfg = TranscriptionConfig::from_json(&json.to_string()).unwrap();
        assert_eq!((cfg.style.as_str(), cfg.raw_text, cfg.custom_prompt.as_str()), ("standard", true, ""));
        assert!(cfg.languages.is_empty());

        let long = "é".repeat(600);
        let cfg = TranscriptionConfig::from_json(&serde_json::json!({ "custom_formatting_prompt": long }).to_string()).unwrap();
        assert_eq!(cfg.custom_prompt.chars().count(), CUSTOM_PROMPT_MAX);

        // Files written by this version have no legacy keys and load as-is.
        let cfg = TranscriptionConfig { style: "lowercase".into(), raw_text: true, custom_prompt: "x".into(), ..Default::default() };
        let json = serde_json::to_string(&cfg).unwrap();
        assert!(!json.contains("\"mode\"") && !json.contains("\"language\""));
        assert_eq!(TranscriptionConfig::from_json(&json).unwrap(), cfg);
    }

    fn ctx(bundle_id: &str, url: Option<&str>) -> AppContext {
        AppContext { bundle_id: bundle_id.into(), name: bundle_id.into(), url: url.map(Into::into) }
    }

    fn rule(id: &str, apps: &[&str], sites: &[&str]) -> FormattingRule {
        FormattingRule {
            id: id.into(),
            name: id.into(),
            apps: apps.iter().map(|a| AppRef { bundle_id: a.to_string(), name: a.to_string() }).collect(),
            websites: sites.iter().map(|s| s.to_string()).collect(),
            ..Default::default()
        }
    }

    #[test]
    fn raw_text_keeps_the_style() {
        assert_eq!(apply_style_locally("Hello There.", "lowercase"), "hello there.");
        assert_eq!(apply_style_locally("See you soon.", "casual"), "See you soon");
        assert_eq!(apply_style_locally("Wait...", "casual"), "Wait...");
        assert_eq!(apply_style_locally("Hello.", "standard"), "Hello.");
    }

    #[test]
    fn hosts_normalise() {
        assert_eq!(host_of("https://www.Mail.Google.com:443/mail/u/0#inbox"), "mail.google.com");
        assert_eq!(host_of("github.com/foo"), "github.com");
        assert_eq!(host_of("  outlook.live.com "), "outlook.live.com");
        assert_eq!(host_of("https://user@example.org?q=1"), "example.org");
    }

    #[test]
    fn rules_resolve_by_website_then_app() {
        let mut cfg = TranscriptionConfig { custom_prompt: "British spelling.".into(), ..Default::default() };
        cfg.custom_rules = vec![
            FormattingRule { style: "casual".into(), custom_prompt: "No emoji.".into(), ..rule("chat", &["com.tinyspeck.slackmacgap"], &["app.slack.com"]) },
            FormattingRule { raw_text: true, ..rule("code", &["com.microsoft.VSCode"], &["github.com"]) },
        ];
        // No context / unknown app → All apps.
        assert_eq!(cfg.formatting_for(None).rule_id, None);
        assert_eq!(cfg.formatting_for(Some(&ctx("com.apple.Notes", None))).rule_id, None);
        // Gmail in any browser → Email rule, with its guidance and both prompts in order.
        let f = cfg.formatting_for(Some(&ctx("com.google.Chrome", Some("https://mail.google.com/mail/u/0"))));
        assert_eq!(f.rule_id.as_deref(), Some("email"));
        assert!(f.system_prompt.contains("writing an email") && f.system_prompt.ends_with("British spelling."));
        // App rule, subdomain website match, raw text.
        let f = cfg.formatting_for(Some(&ctx("com.tinyspeck.slackmacgap", None)));
        assert_eq!(f.rule_id.as_deref(), Some("chat"));
        assert!(f.system_prompt.contains("casual") && f.system_prompt.ends_with("British spelling.\nNo emoji."));
        assert!(cfg.formatting_for(Some(&ctx("com.google.Chrome", Some("https://gist.github.com/x")))).raw_text);
        // A browser on an unknown site falls back to All apps; "notgithub.com" is not github.com.
        assert_eq!(cfg.formatting_for(Some(&ctx("com.google.Chrome", Some("https://notgithub.com")))).rule_id, None);
        // Disabled rules never match.
        cfg.email_rule.enabled = false;
        assert_eq!(cfg.formatting_for(Some(&ctx("x", Some("https://mail.google.com")))).rule_id, None);
    }

    #[test]
    fn formatting_validation() {
        let mut cfg = TranscriptionConfig::default();
        assert!(cfg.validate_formatting().is_ok());
        cfg.custom_prompt = "x".repeat(CUSTOM_PROMPT_MAX + 1);
        assert!(cfg.validate_formatting().unwrap_err().contains("longer than 500"));
        cfg.custom_prompt.clear();
        cfg.custom_rules = vec![rule("a", &["com.x"], &[]), rule("b", &["com.x"], &[])];
        assert_eq!(cfg.validate_formatting().unwrap_err(), "com.x is already in your “a” rule");
        cfg.custom_rules = vec![rule("a", &[], &["https://mail.google.com/"])];
        assert!(cfg.validate_formatting().unwrap_err().contains("“Email”"));
        cfg.custom_rules = vec![rule("a", &[], &[]), rule("a", &[], &[])];
        assert!(cfg.validate_formatting().unwrap_err().contains("unique id"));
        cfg.custom_rules = vec![FormattingRule { style: "shouty".into(), ..rule("a", &[], &[]) }];
        assert!(cfg.validate_formatting().is_err());
    }

    #[test]
    fn prompt_is_style_then_custom() {
        // The default config keeps the pre-batch-2 default prompt.
        assert_eq!(formatting_prompt("standard", "  "), STANDARD_PROMPT);
        assert_eq!(formatting_prompt("unknown", ""), STANDARD_PROMPT);
        let p = formatting_prompt("lowercase", "Keep sentences short.");
        assert!(p.contains("lowercase") && p.ends_with("\nKeep sentences short."));
    }
}
