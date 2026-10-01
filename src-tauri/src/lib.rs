pub mod audio;
pub mod commands;
pub mod db;
pub mod frontmost;
pub mod hotkeys;
pub mod models;
pub mod output;
pub mod paste;
pub mod realtime;
pub mod secrets;
pub mod setup;
pub mod sound;
pub mod transcribe;
mod updater;

use audio::{AudioRecorder, Recording};
use db::{Database, DictionaryEntry, HistoryEntry, SnippetEntry};
use frontmost::{AppContext, AppInfo};
use hotkeys::{HotkeyEngine, HotkeyEvent, HotkeyStatus};
use transcribe::{apply_formatting, transcribe_raw, TranscriptionConfig};

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc::{channel, Sender};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};
use tauri::menu::{CheckMenuItem, Menu, MenuItem, PredefinedMenuItem, Submenu};
use tauri::tray::TrayIconBuilder;
use tauri::{
    AppHandle, Emitter, Manager, PhysicalPosition, PhysicalRect, PhysicalSize, State, WindowEvent,
};
use tauri_plugin_autostart::{MacosLauncher, ManagerExt};

const TRAY_ID: &str = "main";
const CONFIG_FILE: &str = "config.json";
/// Hold-to-talk presses shorter than this are treated as accidental taps and cancelled.
const MIN_HOLD: Duration = Duration::from_millis(300);
/// Give another key time to complete a modifier shortcut. Fn is the primary dictation key,
/// so it uses a shorter grace period; ordinary editing modifiers retain the longer one.
const HOLD_START_DELAY: Duration = Duration::from_millis(150);

fn hold_start_delay(binding: &str) -> Duration {
    if binding == "Fn" { Duration::from_millis(60) } else { HOLD_START_DELAY }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Idle,
    /// Push-to-talk: releasing the hold binding stops.
    Hold,
    HandsFree,
    /// Hands-free with the hold binding pressed: its release stops, so a Fn+Space stop chord
    /// doesn't stop on Fn-down and restart on Space.
    HandsFreeHoldDown,
}

#[derive(Debug, PartialEq, Eq)]
enum Effect {
    None,
    Start,
    Stop,
    Cancel,
}

/// Recording state machine. `held` = time since the press; `pending` = mic start still delayed.
fn transition(mode: Mode, event: HotkeyEvent, held: Duration, pending: bool) -> (Mode, Effect) {
    use HotkeyEvent::*;
    use Mode::*;
    match (mode, event) {
        (Idle, HoldPressed) => (Hold, Effect::Start),
        (Idle, TogglePressed) => (HandsFree, Effect::Start),
        (Hold, HoldReleased) if held < MIN_HOLD => (Idle, Effect::Cancel),
        (Hold, HoldReleased) => (Idle, Effect::Stop),
        (Hold, HoldAborted) => (Idle, Effect::Cancel),
        (Hold, TogglePressed) => (HandsFree, if pending { Effect::Start } else { Effect::None }),
        (HandsFree, HoldPressed) => (HandsFreeHoldDown, Effect::None),
        (HandsFreeHoldDown, HoldAborted) => (HandsFree, Effect::None),
        (HandsFreeHoldDown, HoldReleased) => (Idle, Effect::Stop),
        (HandsFree | HandsFreeHoldDown, TogglePressed | Submit) => (Idle, Effect::Stop),
        (Hold | HandsFree | HandsFreeHoldDown, Cancel) => (Idle, Effect::Cancel),
        _ => (mode, Effect::None),
    }
}

fn submit_enabled(mode: Mode, config: &TranscriptionConfig) -> bool {
    config.enter_to_stop && matches!(mode, Mode::HandsFree | Mode::HandsFreeHoldDown)
}

pub struct AppState {
    pub recorder: AudioRecorder,
    /// Also serializes start/stop/cancel. Never lock it on the main thread: the engine and the
    /// global-shortcut plugin (called while it is held) dispatch to the main thread.
    pub mode: Mutex<Mode>,
    /// Mirror of `mode != Idle` that can be read without the mode lock (dictation bar).
    pub recording: AtomicBool,
    pub recording_start: Mutex<Option<Instant>>,
    /// What the current recording is for and what was captured when it started.
    pub session: Mutex<Option<Session>>,
    pub config: Mutex<TranscriptionConfig>,
    pub db: Arc<Database>,
    /// Hotkey and tray events with the time they happened, handled in order on one worker thread.
    pub events: Sender<(HotkeyEvent, Instant)>,
    /// Transcriptions in flight; processing must not keep the recording bar visible.
    pub jobs: AtomicUsize,
    /// The dictation bar stays visible until then to show a message.
    pub error_until: Mutex<Option<Instant>>,
    pub hud: Mutex<HudVisibility>,
    /// Serializes settings changes (save_config vs backend update_config).
    pub config_write: Mutex<()>,
    /// A hold whose delayed start hasn't fired yet (token of that press).
    pub pending_start: Mutex<Option<u64>>,
    pub start_token: std::sync::atomic::AtomicU64,
}

/// Command hotkeys only exist while beta features are on (the Commands screen is beta).
fn command_bindings(config: &TranscriptionConfig) -> (&str, &str) {
    if config.beta_features {
        (&config.commands_hold, &config.commands_toggle)
    } else {
        ("", "")
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Purpose {
    Dictation,
    Command,
}

/// Captured when a recording starts: which app/URL it is for and the selected text.
pub struct Session {
    pub purpose: Purpose,
    pub context: Option<AppContext>,
    pub selection: Option<String>,
    /// The start chime played (its first ~150 ms are cut from the audio).
    pub chime: bool,
    /// Model and formatting choices for this recording, even if settings change meanwhile.
    pub config: TranscriptionConfig,
    pub live: Option<realtime::LiveTranscription>,
    recorded_at: String,
    context_task: Option<tokio::sync::oneshot::Receiver<(AppContext, Option<String>)>>,
    delivery: Option<paste::DeliveryTicket>,
}

impl Session {
    /// Metadata never blocks capture or release. A late/failed lookup leaves the original app snapshot intact.
    pub async fn resolve_context(&mut self) {
        let Some(task) = self.context_task.take() else { return };
        if let Ok(Ok((context, selection))) = tokio::time::timeout(Duration::from_millis(300), task).await {
            if self.context.as_ref().is_some_and(|original| original.bundle_id == context.bundle_id) {
                self.context = Some(context);
                self.selection = selection;
            }
        }
    }
}

/// Tray menu labels (Microphone, System Default, Show OpenGlaido, Quit); the frontend sends translations.
pub struct TrayLabels(Mutex<[String; 4]>);

/// Glaido's menu bar menu: Microphone ▸ (System Default, the input devices), Show OpenGlaido, Quit.
/// Rebuilt when the pointer reaches the icon, so the device list is current.
fn refresh_tray(app: &AppHandle) {
    let result = (|| -> tauri::Result<()> {
        let Some(tray) = app.tray_by_id(TRAY_ID) else { return Ok(()) };
        let [microphone, system_default, show, quit] = app.state::<TrayLabels>().0.lock().unwrap().clone();
        let selected = app.state::<AppState>().config.lock().unwrap().input_device.clone();
        let mics = Submenu::with_id(app, "mic", &microphone, true)?;
        mics.append(&CheckMenuItem::with_id(app, "mic-default", &system_default, true, selected.is_none(), None::<&str>)?)?;
        mics.append(&PredefinedMenuItem::separator(app)?)?;
        for name in audio::list_input_devices() {
            let checked = selected.as_deref() == Some(name.as_str());
            mics.append(&CheckMenuItem::with_id(app, format!("mic:{name}"), &name, true, checked, None::<&str>)?)?;
        }
        let menu = Menu::with_items(
            app,
            &[
                &mics,
                &PredefinedMenuItem::separator(app)?,
                &MenuItem::with_id(app, "show", &show, true, None::<&str>)?,
                &PredefinedMenuItem::separator(app)?,
                &MenuItem::with_id(app, "quit", &quit, true, None::<&str>)?,
            ],
        )?;
        tray.set_menu(Some(menu))
    })();
    if let Err(e) = result {
        eprintln!("Failed to build the menu bar menu: {e}");
    }
}

const CHIME_TRIM_MS: u32 = 150;

/// A bar/toast message. `message` stays an English translation key; `vars` fills `{placeholders}`.
struct Notice {
    message: String,
    level: &'static str,
    vars: BTreeMap<String, String>,
    model_failure: bool,
}

impl Notice {
    fn error(message: impl Into<String>) -> Self {
        Self { message: message.into(), level: "error", vars: BTreeMap::new(), model_failure: false }
    }

    fn model_error(message: impl Into<String>) -> Self {
        Self { model_failure: true, ..Self::error(message) }
    }

    fn warning(message: impl Into<String>) -> Self {
        Self { level: "warning", ..Self::error(message) }
    }

    /// Only model setup failures reopen the bar; other blockers stay in the main window.
    fn from_issue(issue: setup::SetupIssue) -> Self {
        let model_failure = matches!(issue.id.as_str(), "stt" | "llm");
        Self { message: issue.title, level: "error", vars: issue.vars, model_failure }
    }

    fn filled(&self) -> String {
        fill(&self.message, &self.vars)
    }
}

fn fill(msg: &str, vars: &BTreeMap<String, String>) -> String {
    vars.iter().fold(msg.to_string(), |text, (k, v)| text.replace(&format!("{{{k}}}"), v))
}

/// Logs `msg` and shows it in the main window without reopening the dictation bar.
pub fn report_error(app: &AppHandle, msg: impl AsRef<str>) {
    report(app, msg.as_ref(), "error", Duration::from_millis(4500), &BTreeMap::new(), false);
}

/// A model failure with a usable fallback, shown in the bar and the main window.
pub fn report_model_warning(app: &AppHandle, msg: impl AsRef<str>) {
    report(app, msg.as_ref(), "warning", Duration::from_millis(3500), &BTreeMap::new(), true);
}

fn report_notice(app: &AppHandle, notice: &Notice) {
    let duration = if notice.level == "warning" { 3500 } else { 4500 };
    report(app, &notice.message, notice.level, Duration::from_millis(duration), &notice.vars, notice.model_failure);
}

fn report(app: &AppHandle, msg: &str, level: &str, duration: Duration, vars: &BTreeMap<String, String>, model_failure: bool) {
    eprintln!("{}", fill(msg, vars));
    let payload = serde_json::json!({ "message": msg, "level": level, "vars": vars });
    // Only model failures reach the HUD listener. Other actionable issues remain in the main
    // window, without restarting the bar's message animation or visibility timer.
    let emitted = if model_failure {
        app.emit("dictation-error", payload)
    } else {
        app.emit_to("main", "dictation-error", payload)
    };
    if let Err(e) = emitted {
        eprintln!("Failed to emit dictation-error: {}", e);
    }
    if !model_failure {
        return;
    }
    if let Some(state) = app.try_state::<AppState>() {
        *state.error_until.lock().unwrap() = Some(Instant::now() + duration);
        update_hud(app);
        let app = app.clone();
        std::thread::spawn(move || {
            std::thread::sleep(duration + Duration::from_millis(100));
            update_hud(&app);
        });
    }
}

/// Shows an overlay (dictation bar, command window) without activating OpenGlaido, so the app
/// the user is typing in stays frontmost (and receives the paste).
pub fn show_passive(window: &tauri::WebviewWindow) -> Result<(), String> {
    #[cfg(target_os = "macos")]
    {
        // ns_window() off the main thread blocks on it; inside the closure it doesn't.
        let w = window.clone();
        window
            .run_on_main_thread(move || match w.ns_window() {
                Ok(ptr) => unsafe {
                    let _: () = objc2::msg_send![ptr as *mut objc2::runtime::AnyObject, orderFrontRegardless];
                },
                Err(e) => eprintln!("Failed to show {}: {}", w.label(), e),
            })
            .map_err(|e| e.to_string())
    }
    #[cfg(not(target_os = "macos"))]
    window.show().map_err(|e| e.to_string())
}

/// The dictation bar's visibility; `changes` invalidates queued shows after a release.
pub struct HudVisibility {
    shown: bool,
    hidden: bool,
    changes: u64,
}

impl HudVisibility {
    fn update(&mut self, recording: bool, showing_error: bool) -> bool {
        let show = recording || showing_error;
        if self.shown == show {
            return false;
        }
        self.shown = show;
        self.changes += 1;
        true
    }

    fn can_show(&self, change: u64) -> bool {
        self.shown && self.changes == change
    }
}

/// Shows the dictation bar while recording or showing a message, on the display with the
/// pointer. Release hides it immediately; transcription continues without keeping it visible.
pub fn update_hud(app: &AppHandle) {
    let (Some(state), Some(hud)) = (app.try_state::<AppState>(), app.get_webview_window("hud")) else { return };
    let mut bar = state.hud.lock().unwrap();
    let showing_error = state.error_until.lock().unwrap().is_some_and(|t| Instant::now() < t);
    let changed = bar.update(state.recording.load(Ordering::SeqCst), showing_error);
    if changed {
        // Under the lock, so the HUD hears the changes in order.
        let _ = app.emit("hud-visibility", bar.shown);
    }
    if bar.shown {
        let appear = std::mem::take(&mut bar.hidden);
        let change = bar.changes;
        drop(bar);
        let location = appear.then(|| state.config.lock().unwrap().bar_location.clone());
        let app = app.clone();
        let window = hud.clone();
        // One main-thread dispatch: positioning must not hold up microphone startup with
        // separate blocking round trips for each monitor/window query.
        if let Err(e) = hud.run_on_main_thread(move || {
            let state = app.state::<AppState>();
            let bar = state.hud.lock().unwrap();
            if !bar.can_show(change) {
                return;
            }
            if let Some(location) = location {
                position_hud_now(&app, &location);
            }
            if let Err(e) = show_passive(&window) {
                eprintln!("Failed to show the dictation bar: {}", e);
            }
        }) {
            eprintln!("Failed to show the dictation bar: {}", e);
        }
    } else if changed {
        bar.hidden = true;
        if let Err(e) = hud.hide() {
            eprintln!("Failed to hide the dictation bar: {}", e);
        }
    }
}

/// Counts transcriptions in flight independently of recording/HUD visibility.
pub fn job_started(app: &AppHandle) {
    let state = app.state::<AppState>();
    if state.jobs.fetch_add(1, Ordering::SeqCst) == 0 {
        let _ = app.emit("processing-status", true);
    }
}

pub fn job_finished(app: &AppHandle) {
    let state = app.state::<AppState>();
    if state.jobs.fetch_sub(1, Ordering::SeqCst) == 1 {
        let _ = app.emit("processing-status", false);
    }
    update_hud(app);
}

pub fn history_updated(app: &AppHandle) {
    let _ = app.emit("history-updated", ());
}

// Missing/invalid file → defaults (logged); missing fields → defaults via #[serde(default)];
// fields from older versions are migrated.
fn load_config(path: &Path) -> TranscriptionConfig {
    let mut config = read_config(path);
    // Written before the model settings, when api_key served both endpoints.
    let shared_key = fs::read_to_string(path)
        .is_ok_and(|json| serde_json::from_str::<serde_json::Value>(&json).is_ok_and(|v| v.get("llm_source").is_none()));
    // A hotkey an older version accepted but this platform now rejects (Windows Ctrl+Alt = AltGr)
    // falls back to its default instead of leaving dictation without a hotkey.
    let mut migrated = false;
    for (binding, default) in [
        (&mut config.hotkey_hold, hotkeys::default_hold()),
        (&mut config.hotkey_toggle, hotkeys::default_toggle()),
        (&mut config.commands_hold, hotkeys::default_commands_hold()),
        (&mut config.commands_toggle, hotkeys::default_commands_toggle()),
    ] {
        if let Err(e) = hotkeys::validate_binding(binding) {
            eprintln!("Hotkey {binding} no longer works here ({e}); using {default}");
            *binding = default.to_string();
            migrated = true;
        }
    }
    // Each credential family migrates independently; failed migration must not rewrite plaintext keys.
    let plaintext = !config.api_key.is_empty() || !config.search_api_key.is_empty() || !config.llm_api_key.is_empty();
    let search_loaded = if config.search_keys_migrated {
        config.search_api_key.clear();
        Ok(())
    } else {
        hydrate_legacy_search_key(&mut config)
    };
    let mut search_migrated = false;
    match search_loaded.and_then(|()| secrets::load_search_key(&mut config)) {
        Ok(changed) => search_migrated = changed,
        Err(e) => eprintln!("Couldn't load search provider keys: {e}"),
    }
    let keys_loaded = if config.provider_keys_migrated {
        config.api_key.clear();
        config.llm_api_key.clear();
        Ok(())
    } else {
        hydrate_legacy_model_keys(&mut config).map(|()| {
            if shared_key && config.llm_api_key.is_empty() && config.llm_source == "cloud" {
                config.llm_api_key = config.api_key.clone();
            }
        })
    };
    let mut keys_migrated = false;
    match keys_loaded.and_then(|()| secrets::load_model_keys(&mut config)) {
        Ok(changed) => keys_migrated = changed,
        Err(e) => eprintln!("Couldn't load provider keys: {e}"),
    }
    // Only remove legacy slots after both their scoped copies and migration metadata are saved.
    if plaintext || migrated || keys_migrated || search_migrated {
        match write_config(path, &config) {
            Ok(()) => {
                if keys_migrated { remove_legacy_model_keys(); }
                if search_migrated { remove_legacy_search_key(); }
            },
            Err(e) => eprintln!("Couldn't rewrite {} without keys: {}", path.display(), e),
        }
    }
    config
}

fn hydrate_legacy_model_keys(config: &mut TranscriptionConfig) -> Result<(), String> {
    let load = |account, value: &String| {
        if value.is_empty() { secrets::read(account).map(Option::unwrap_or_default) } else { Ok(value.clone()) }
    };
    let stt = load("api_key", &config.api_key)?;
    let llm = load("llm_api_key", &config.llm_api_key)?;
    config.api_key = stt;
    config.llm_api_key = llm;
    Ok(())
}

fn hydrate_legacy_search_key(config: &mut TranscriptionConfig) -> Result<(), String> {
    if config.search_api_key.is_empty() {
        config.search_api_key = secrets::read("search_api_key")?.unwrap_or_default();
    }
    Ok(())
}

fn remove_legacy_search_key() {
    if let Err(e) = secrets::set("search_api_key", "") {
        eprintln!("Couldn't remove migrated search key slot: {e}");
    }
}

fn remove_legacy_model_keys() {
    for account in ["api_key", "llm_api_key"] {
        if let Err(e) = secrets::set(account, "") {
            eprintln!("Couldn't remove migrated key slot: {e}");
        }
    }
}

fn read_config(path: &Path) -> TranscriptionConfig {
    // Without a valid config we cannot know which provider owns a legacy unscoped key.
    let defaults = || TranscriptionConfig { provider_keys_migrated: true, search_keys_migrated: true, ..Default::default() };
    match fs::read_to_string(path) {
        Ok(json) => TranscriptionConfig::from_json(&json).unwrap_or_else(|e| {
            eprintln!("Invalid {}, using defaults: {}", path.display(), e);
            defaults()
        }),
        Err(e) => {
            eprintln!("No config at {} ({}), using defaults", path.display(), e);
            defaults()
        }
    }
}

/// Never write credentials to config.json. Failed legacy migration leaves the original file intact.
fn config_json(config: &TranscriptionConfig) -> Result<String, String> {
    if (!config.provider_keys_migrated && (!config.api_key.is_empty() || !config.llm_api_key.is_empty()))
        || (!config.search_keys_migrated && !config.search_api_key.is_empty()) {
        return Err("Couldn't save settings until API keys are secured in the system keychain".into());
    }
    let mut on_disk = config.clone();
    on_disk.api_key.clear();
    on_disk.llm_api_key.clear();
    on_disk.search_api_key.clear();
    serde_json::to_string_pretty(&on_disk).map_err(|e| e.to_string())
}

/// Temp file + rename, so a crash mid-write never leaves a truncated config.json.
fn write_config(path: &Path, config: &TranscriptionConfig) -> Result<(), String> {
    let tmp = path.with_extension("json.tmp");
    fs::write(&tmp, config_json(config)?).map_err(|e| e.to_string())?;
    fs::rename(&tmp, path).map_err(|e| e.to_string())
}

/// Changes settings from the backend (MCP prefs, "always allow"), persists them and tells the UI.
pub fn update_config(app: &AppHandle, f: impl FnOnce(&mut TranscriptionConfig)) -> Result<(), String> {
    let _activity = app.state::<updater::UpdateState>().activity()?;
    let state = app.state::<AppState>();
    let _serialized = state.config_write.lock().unwrap();
    let mut updated = state.config.lock().unwrap().clone();
    f(&mut updated);
    write_config(&state.db.app_dir.join(CONFIG_FILE), &updated)?;
    *state.config.lock().unwrap() = updated.clone();
    let _ = app.emit("config-changed", &updated);
    Ok(())
}

/// Loads the local models the settings now use in the background, and frees the ones they left
/// (`old` None = startup or a finished download). Never blocks: a transcription may be holding the model.
pub(crate) fn load_local_models(app: &AppHandle, config: &TranscriptionConfig, old: Option<&TranscriptionConfig>) {
    let selected = |c: &TranscriptionConfig| {
        let local = |source: &str, id: &String| (source == "local").then(|| id.clone());
        (local(&c.stt_source, &c.local_stt_model), local(&c.llm_source, &c.local_llm_model))
    };
    let (stt, llm) = selected(config);
    let (old_stt, old_llm) = old.map(selected).unzip();
    let app = app.clone();
    std::thread::spawn(move || {
        if old_stt.as_ref() != Some(&stt) {
            match stt {
                Some(id) => models::whisper::preload(&app, &id),
                None => models::whisper::unload(),
            }
        }
        if old_llm.as_ref() != Some(&llm) {
            match llm {
                Some(id) => models::llama::preload(&app, &id),
                None => models::llama::unload(),
            }
        }
    });
}

fn set_launch_at_login(app: &AppHandle, enabled: bool) -> Result<(), String> {
    let autolaunch = app.autolaunch();
    let result = match autolaunch.is_enabled() {
        Ok(current) if current == enabled => Ok(()),
        _ if enabled => autolaunch.enable(),
        _ => autolaunch.disable(),
    };
    result.map_err(|e| format!("Couldn't change Launch at login: {}", e))
}

/// Top-left of the dictation bar: centred in the work area, bottom edge raised by the
/// `bar_location` offset (logical px × scale, or 30% of the work area for "high").
pub(crate) fn hud_origin(
    area: &PhysicalRect<i32, u32>,
    window: PhysicalSize<u32>,
    scale: f64,
    bar_location: &str,
) -> PhysicalPosition<i32> {
    let offset = match bar_location {
        "raised" => 136.0 * scale,
        "high" => area.size.height as f64 * 0.3,
        _ => 16.0 * scale,
    }
    .round() as i32;
    PhysicalPosition::new(
        area.position.x + (area.size.width as i32 - window.width as i32) / 2,
        area.position.y + area.size.height as i32 - window.height as i32 - offset,
    )
}

/// The thread that runs `.setup`. Monitor queries wait on it and deadlock if called there.
static MAIN_THREAD: OnceLock<std::thread::ThreadId> = OnceLock::new();

fn on_main_thread() -> bool {
    MAIN_THREAD.get().is_none_or(|id| *id == std::thread::current().id())
}

/// Pointer in the space `monitor_from_point` uses: points, origin at the top-left of the main
/// display, y down. Tauri's cursor position flips y with the pixel height, which misses every
/// display on a Retina screen.
#[cfg(target_os = "macos")]
fn display_point_under_cursor() -> Option<(f64, f64)> {
    use objc2_app_kit::NSEvent;
    #[repr(C)]
    struct CGPoint {
        x: f64,
        y: f64,
    }
    #[repr(C)]
    struct CGSize {
        width: f64,
        height: f64,
    }
    #[repr(C)]
    struct CGRect {
        origin: CGPoint,
        size: CGSize,
    }
    #[link(name = "CoreGraphics", kind = "framework")]
    extern "C" {
        fn CGMainDisplayID() -> u32;
        fn CGDisplayBounds(display: u32) -> CGRect;
    }
    let mouse = NSEvent::mouseLocation();
    let bounds = unsafe { CGDisplayBounds(CGMainDisplayID()) };
    Some((mouse.x, bounds.size.height - mouse.y))
}

/// Places the dictation bar on the display under the pointer (else the primary one).
fn position_hud(app: &AppHandle, bar_location: &str) {
    // Don't wait: setup is the main thread, and the event loop answers only after it returns.
    if on_main_thread() {
        let app = app.clone();
        let bar_location = bar_location.to_string();
        std::thread::spawn(move || position_hud_now(&app, &bar_location));
    } else {
        position_hud_now(app, bar_location);
    }
}

fn position_hud_now(app: &AppHandle, bar_location: &str) {
    let Some(hud) = app.get_webview_window("hud") else { return };
    let primary = app.primary_monitor().ok().flatten();
    #[cfg(target_os = "macos")]
    let pointer = display_point_under_cursor();
    #[cfg(not(target_os = "macos"))]
    let pointer = app.cursor_position().ok().map(|p| (p.x, p.y));
    let monitor = pointer.and_then(|(x, y)| app.monitor_from_point(x, y).ok().flatten()).or(primary);
    let (Some(monitor), Ok(size), Ok(scale)) = (monitor, hud.outer_size(), hud.scale_factor()) else {
        return eprintln!("Failed to position the dictation bar: no display");
    };
    // Each display measures in its own pixels, so the bar's size is converted to the target display's.
    let size = size.to_logical::<f64>(scale).to_physical(monitor.scale_factor());
    let origin = hud_origin(monitor.work_area(), size, monitor.scale_factor(), bar_location);
    // A macOS window reads a physical position with the scale of the display it is on now: move it in points.
    #[cfg(target_os = "macos")]
    let origin = origin.to_logical::<f64>(monitor.scale_factor());
    if let Err(e) = hud.set_position(origin) {
        eprintln!("Failed to position the dictation bar: {}", e);
    }
}

/// Tray, Dock and dictation bar placement. `old` = None applies everything (startup).
fn apply_ui_settings(app: &AppHandle, config: &TranscriptionConfig, old: Option<&TranscriptionConfig>) {
    if old.is_none_or(|o| o.show_in_menu_bar != config.show_in_menu_bar) {
        if let Some(tray) = app.tray_by_id(TRAY_ID) {
            if let Err(e) = tray.set_visible(config.show_in_menu_bar) {
                eprintln!("Failed to set menu bar icon visibility: {}", e);
            }
        }
    }
    // Only on change: tao ignores a Dock hide within 1 s of a Dock show.
    #[cfg(target_os = "macos")]
    if old.is_none_or(|o| o.show_in_dock != config.show_in_dock) {
        if let Err(e) = app.set_dock_visibility(config.show_in_dock) {
            eprintln!("Failed to set Dock visibility: {}", e);
        }
    }
    if old.is_none_or(|o| o.bar_location != config.bar_location) {
        position_hud(app, &config.bar_location);
    }
}

// Emits "mic-level" (ten bar levels, 0..1) to the HUD at ~60 Hz until that recording ends.
fn spawn_level_meter(app: &AppHandle, started: Instant) {
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        let state = app.state::<AppState>();
        while *state.recording_start.lock().unwrap() == Some(started) {
            if state.recorder.interruption().is_some() {
                let app = app.clone();
                tauri::async_runtime::spawn_blocking(move || interrupt_capture(&app, started));
                break;
            }
            let _ = app.emit_to("hud", "mic-level", state.recorder.bars());
            tokio::time::sleep(Duration::from_millis(16)).await;
        }
    });
}

fn interrupt_capture(app: &AppHandle, started: Instant) {
    let state = app.state::<AppState>();
    let capture = {
        let mut mode = state.mode.lock().unwrap();
        if *mode == Mode::Idle || *state.recording_start.lock().unwrap() != Some(started) {
            return;
        }
        let capture = end_capture(app, &state);
        set_mode(app, &state, &mut mode, Mode::Idle);
        capture
    };
    match capture {
        Ok((recording, duration, session)) => {
            tauri::async_runtime::spawn(process_recording(app.clone(), recording, duration, session));
        }
        Err(error) => report_error(app, error),
    }
}

// Caller holds the mode lock. Keeps the engine's Esc/Enter arming in sync with `mode`.
fn set_mode(app: &AppHandle, state: &AppState, mode: &mut Mode, next: Mode) {
    *mode = next;
    state.recording.store(next != Mode::Idle, Ordering::SeqCst);
    let engine = app.state::<HotkeyEngine>();
    engine.set_recording(next != Mode::Idle);
    let submit = submit_enabled(next, &state.config.lock().unwrap());
    engine.set_submit_enabled(submit);
}

// Caller holds the mode lock. `started` = when the user asked (opening the mic can take a while).
// Err (reported once by the caller) leaves the mode Idle, also when setup keeps it from starting.
fn begin_capture(app: &AppHandle, state: &AppState, started: Instant, purpose: Purpose) -> Result<(), Notice> {
    if app.state::<updater::UpdateState>().is_installing() {
        return Err(Notice::warning("OpenGlaido is installing an update."));
    }
    if let Some(issue) = setup::blocker(app, purpose) {
        return Err(Notice::from_issue(issue));
    }
    let config = state.config.lock().unwrap().clone();
    let (device, sound_on, mute, beta) = (
        config.input_device.clone(), config.sound_feedback, config.mute_background, config.beta_features,
    );
    let target = frontmost::capture_context();
    let context = target.as_ref().map(|target| target.context.clone());
    let context_task = target.map(|target| {
        let (tx, rx) = tokio::sync::oneshot::channel();
        std::thread::spawn(move || {
            let result = frontmost::resolve_context(target, purpose == Purpose::Command || beta);
            let _ = tx.send(result);
        });
        rx
    });
    let (live, input) = if config.stt_source == "cloud" && realtime::is_live_model(&config.model_name) {
        let vocabulary = if purpose == Purpose::Dictation { vocab_terms(&state.db) } else { Vec::new() };
        let (live, input) = realtime::LiveTranscription::start(&config, vocabulary).map_err(Notice::model_error)?;
        (Some(live), Some(input))
    } else {
        (None, None)
    };
    // Acknowledge the press while the device opens. Bars stay at rest until real mic levels
    // arrive; the start chime still means the microphone is ready.
    *state.recording_start.lock().unwrap() = Some(started);
    state.recording.store(true, Ordering::SeqCst);
    let _ = app.emit("recording-status", true);
    update_hud(app);
    let fell_back = match state.recorder.start_recording_streamed(
        device.as_deref(), input, if sound_on { CHIME_TRIM_MS } else { 0 },
    ) {
        Ok(fell_back) => fell_back,
        Err(e) => {
            *state.recording_start.lock().unwrap() = None;
            end_recording_feedback(app, state);
            return Err(Notice::error(format!("Couldn't start recording: {}", e)));
        }
    };
    if fell_back {
        let mut vars = BTreeMap::new();
        vars.insert("name".into(), device.unwrap_or_default());
        report(
            app,
            "Microphone “{name}” not found, using the system default",
            "warning",
            Duration::from_millis(3500),
            &vars,
            false,
        );
    }

    if sound_on {
        sound::play_start_tone();
    }
    if mute {
        // After the start chime, so the user still hears it.
        output::mute_after(&state.db.app_dir, Duration::from_millis(if sound_on { 250 } else { 0 }));
    }
    *state.session.lock().unwrap() = Some(Session {
        purpose, context, selection: None, chime: sound_on, config, live, context_task, delivery: None,
        recorded_at: (chrono::Utc::now() - chrono::Duration::from_std(started.elapsed()).unwrap_or_default()).to_rfc3339(),
    });
    spawn_level_meter(app, started);
    Ok(())
}

// Hide before microphone teardown or output restoration can block the hotkey worker.
fn end_recording_feedback(app: &AppHandle, state: &AppState) {
    state.recording.store(false, Ordering::SeqCst);
    let _ = app.emit("recording-status", false);
    update_hud(app);
}

// Caller holds the mode lock. Returns the recording, its duration and its session.
fn end_capture(app: &AppHandle, state: &AppState) -> Result<(Recording, i64, Session), String> {
    end_recording_feedback(app, state);
    let mut session = state.session.lock().unwrap().take().unwrap_or_else(|| Session {
        purpose: Purpose::Dictation,
        context: None,
        selection: None,
        chime: false,
        config: state.config.lock().unwrap().clone(),
        live: None,
        recorded_at: chrono::Utc::now().to_rfc3339(),
        context_task: None,
        delivery: None,
    });
    let duration_ms = state
        .recording_start
        .lock()
        .unwrap()
        .take()
        .map(|s| s.elapsed().as_millis() as i64)
        .unwrap_or(0);
    let recording = state.recorder.stop_recording(if session.chime { CHIME_TRIM_MS } else { 0 });
    // Microphone callbacks have stopped. Drain their queued audio and commit the live turn
    // immediately, in parallel with local denoising/history work.
    if recording.as_ref().is_ok_and(|recording| recording.interruption.is_none()) {
        if let Some(live) = session.live.as_mut() {
            live.finish_input();
        }
    } else {
        // Never commit a partial live turn after the input device fails.
        session.live = None;
    }
    output::restore(&state.db.app_dir);
    // Only now: the stream is stopped, so the stop chime never lands in the audio.
    if session.chime {
        sound::play_stop_tone();
    }
    if recording.is_ok() {
        if session.purpose == Purpose::Dictation {
            session.delivery = Some(paste::reserve_delivery());
        }
        job_started(app);
    }
    recording
        .map(|r| (r, duration_ms, session))
        .map_err(|e| format!("Recording failed: {}", e))
}

// Caller holds the mode lock.
fn cancel_capture(app: &AppHandle, state: &AppState) {
    end_recording_feedback(app, state);
    state.recorder.cancel();
    *state.recording_start.lock().unwrap() = None;
    *state.session.lock().unwrap() = None;
    output::restore(&state.db.app_dir);
}

/// Command hotkeys drive the same state machine as dictation; the purpose decides what a
/// finished recording is used for. Enter/Esc/⌘C for the command window go to the commands module.
fn split_event(event: HotkeyEvent) -> Option<(HotkeyEvent, Purpose)> {
    use HotkeyEvent::*;
    Some(match event {
        CommandHoldPressed => (HoldPressed, Purpose::Command),
        CommandHoldReleased => (HoldReleased, Purpose::Command),
        CommandHoldAborted => (HoldAborted, Purpose::Command),
        CommandTogglePressed => (TogglePressed, Purpose::Command),
        CommandEnter | CommandEscape | CommandCopy => return None,
        other => (other, Purpose::Dictation),
    })
}

// Runs on the hotkey worker thread, so blocking on the audio thread is fine here.
fn handle_event(app: &AppHandle, event: HotkeyEvent, at: Instant) {
    let Some((event, purpose)) = split_event(event) else {
        commands::handle_hotkey(app, event);
        return;
    };
    let state = app.state::<AppState>();
    let mut mode = state.mode.lock().unwrap();
    if app.state::<updater::UpdateState>().is_installing() {
        return;
    }
    // A dictation hotkey never stops a command recording (and vice versa); Esc/Enter apply to both.
    let current = state.session.lock().unwrap().as_ref().map(|s| s.purpose);
    let shared = matches!(event, HotkeyEvent::Cancel | HotkeyEvent::Submit);
    if *mode != Mode::Idle && !shared && current.is_some_and(|p| p != purpose) {
        return;
    }
    // Event timestamps, so a slow mic start doesn't shorten the measured hold.
    let started = *state.recording_start.lock().unwrap();
    let held = started.map_or(Duration::ZERO, |t| at.saturating_duration_since(t));
    let not_started = state.pending_start.lock().unwrap().take().is_some();
    let (next, effect) = transition(*mode, event, held, not_started);
    match effect {
        Effect::None => {
            // An ignored event keeps the original delayed start armed.
            if not_started {
                *state.pending_start.lock().unwrap() = Some(state.start_token.load(Ordering::SeqCst));
            }
        }
        Effect::Start
            if event == HotkeyEvent::HoldPressed
                && app.state::<HotkeyEngine>().hold_is_modifier_only((purpose == Purpose::Command) as usize) =>
        {
            let token = state.start_token.fetch_add(1, Ordering::SeqCst) + 1;
            *state.pending_start.lock().unwrap() = Some(token);
            *state.recording_start.lock().unwrap() = Some(at);
            let delay = {
                let c = state.config.lock().unwrap();
                hold_start_delay(if purpose == Purpose::Command { &c.commands_hold } else { &c.hotkey_hold })
            };
            let app = app.clone();
            std::thread::spawn(move || {
                std::thread::sleep(delay.saturating_sub(at.elapsed()));
                let state = app.state::<AppState>();
                let mut mode = state.mode.lock().unwrap();
                if *state.pending_start.lock().unwrap() != Some(token) || *mode == Mode::Idle {
                    return;
                }
                *state.pending_start.lock().unwrap() = None;
                if let Err(e) = begin_capture(&app, &state, at, purpose) {
                    report_notice(&app, &e);
                    set_mode(&app, &state, &mut mode, Mode::Idle);
                }
            });
        }
        Effect::Start => {
            // A confirmed toggle chord needn't wait; taking its token above disarms the timer.
            let started = if not_started { started.unwrap_or(at) } else { at };
            if let Err(e) = begin_capture(app, &state, started, purpose) {
                set_mode(app, &state, &mut mode, Mode::Idle);
                report_notice(app, &e);
                return;
            }
        }
        // Released/aborted/cancelled before the mic ever started: nothing to stop.
        Effect::Stop | Effect::Cancel if not_started => {
            *state.recording_start.lock().unwrap() = None;
            end_recording_feedback(app, &state);
        }
        Effect::Stop => match end_capture(app, &state) {
            Ok((recording, duration_ms, session)) => match session.purpose {
                _ if recording.interruption.is_some() => {
                    tauri::async_runtime::spawn(process_recording(app.clone(), recording, duration_ms, session));
                }
                Purpose::Dictation => {
                    tauri::async_runtime::spawn(process_recording(app.clone(), recording, duration_ms, session));
                }
                Purpose::Command => {
                    let app = app.clone();
                    tauri::async_runtime::spawn(async move {
                        commands::run_recorded(app.clone(), recording, duration_ms, session).await;
                        job_finished(&app);
                    });
                }
            },
            Err(e) => report_error(app, e),
        },
        Effect::Cancel => cancel_capture(app, &state),
    }
    if next != *mode {
        set_mode(app, &state, &mut mode, next);
    }
}

fn expand_dynamic_tags(template: &str) -> String {
    let mut result = template.to_string();
    if result.contains("{date}") {
        let now = chrono::Local::now().format("%Y-%m-%d").to_string();
        result = result.replace("{date}", &now);
    }
    if result.contains("{time}") {
        let now = chrono::Local::now().format("%H:%M").to_string();
        result = result.replace("{time}", &now);
    }
    if result.contains("{clipboard}") {
        if let Ok(mut clip) = arboard::Clipboard::new() {
            if let Ok(text) = clip.get_text() {
                result = result.replace("{clipboard}", &text);
            }
        }
    }
    result
}

/// Denoise + speech check + WAV encode, off the async runtime and outside every lock.
pub async fn finish_recording(recording: Recording) -> Result<(Vec<u8>, bool), String> {
    tauri::async_runtime::spawn_blocking(move || recording.process())
        .await
        .map_err(|e| format!("Recording failed: {e}"))?
}

/// Keep phrases intact: providers with keyterm lists must not split names containing commas.
fn vocab_terms(db: &Database) -> Vec<String> {
    db.get_dictionary().unwrap_or_default().into_iter().map(|d| d.phrase).collect()
}

/// Punctuation or whitespace only (Whisper's output for silence).
fn is_blank(text: &str) -> bool {
    !text.chars().any(char::is_alphanumeric)
}

/// Snippet (whole dictation = trigger) or the formatting rule's cleanup + dictionary corrections.
async fn finish_text(app: &AppHandle, db: &Database, raw: &str, config: &TranscriptionConfig, context: Option<&AppContext>) -> String {
    if let Some(snippet_content) = db.match_snippet(raw) {
        return expand_dynamic_tags(&snippet_content);
    }
    let mut text = apply_formatting(app, raw, config, &config.formatting_for(context)).await;
    for entry in db.get_dictionary().unwrap_or_default() {
        if !entry.phrase.trim().is_empty() && !entry.replacement.trim().is_empty() {
            text = text.replace(&entry.phrase, &expand_dynamic_tags(&entry.replacement));
        }
    }
    text
}

fn history_website(context: Option<&AppContext>) -> Option<String> {
    context.and_then(|c| c.url.as_deref()).map(transcribe::host_of).filter(|host| !host.is_empty())
}

fn history_context(entry: &HistoryEntry) -> Option<AppContext> {
    entry.app_bundle_id.clone().map(|bundle_id| AppContext {
        name: entry.app_name.clone().unwrap_or_else(|| bundle_id.clone()),
        bundle_id,
        url: entry.website.clone(),
    })
}

/// Transcribes, pastes and saves a dictation from `end_capture` (which counted the job);
/// model failures are reported to the dictation bar; other failures stay in the main window.
async fn process_recording(app: AppHandle, recording: Recording, duration_ms: i64, session: Session) {
    match transcribe_and_paste(&app, recording, duration_ms, session).await {
        Ok(Some(text)) => {
            let _ = app.emit("transcription-completed", text);
        }
        Ok(None) => {}
        Err(e) => report_notice(&app, &e),
    }
    job_finished(&app);
}

/// Ok(None) = nothing pasted: no speech (silent) or handed over to a command (voice activation).
async fn transcribe_and_paste(
    app: &AppHandle,
    recording: Recording,
    duration_ms: i64,
    mut session: Session,
) -> Result<Option<String>, Notice> {
    let state = app.state::<AppState>();
    let interruption = recording.interruption.clone();
    let had_samples = !recording.samples.is_empty();
    if interruption.is_some() {
        session.live = None;
        session.delivery = None;
    }
    let (wav, has_speech) = finish_recording(recording).await.map_err(Notice::error)?;
    session.resolve_context().await;
    eprintln!("Dictation prepared: provider={} model={} duration_ms={} speech={} wav_bytes={}", session.config.stt_provider, session.config.model_name, duration_ms, has_speech, wav.len());
    if !has_speech && interruption.is_none() {
        return Ok(None);
    }
    let id = uuid::Uuid::new_v4().to_string();
    let audio_filename = format!("{}.wav", id);

    // Save audio file locally for history playback & retranscription
    let audio_file_path = state.db.app_dir.join("audio").join(&audio_filename);
    let audio_saved = had_samples && match fs::write(&audio_file_path, &wav) {
        Ok(()) => true,
        Err(e) => {
            report_error(app, format!("Couldn't save the recording: {}", e));
            false
        }
    };

    let config = session.config.clone();
    let context = session.context.clone();
    let row = HistoryEntry {
        kind: if session.purpose == Purpose::Command { "command" } else { "dictation" }.into(),
        created_at: session.recorded_at.clone(),
        duration_ms,
        audio_filename: audio_saved.then_some(audio_filename),
        app_name: context.as_ref().map(|c| c.name.clone()),
        app_bundle_id: context.as_ref().map(|c| c.bundle_id.clone()),
        website: history_website(context.as_ref()),
        ..HistoryEntry::new(id.clone())
    };
    if let Some(error) = interruption {
        const MESSAGE: &str = "Recording was interrupted. Check your microphone.";
        eprintln!("Recording interrupted: {error}");
        if let Err(error) = state.db.insert_history(&HistoryEntry {
            status: "failed".into(), error: Some(MESSAGE.into()), ..row
        }) {
            report_error(app, format!("Couldn't save to history: {error}"));
        }
        history_updated(app);
        return Err(Notice::warning(MESSAGE));
    }
    // Saved as "running" first: quitting mid-transcription leaves a record with Retry, not a lost dictation.
    let saved_early = audio_saved && state.db.insert_history(&HistoryEntry { status: "running".into(), ..row.clone() }).is_ok();

    let transcription = match session.live.take() {
        Some(live) => live.finish().await,
        None => transcribe_raw(app, wav, &config, vocab_terms(&state.db)).await,
    };
    let raw = match transcription {
        Ok(t) => t,
        Err(e) => {
            // Keep the audio: the record shows "Transcription failed" with Retry.
            if saved_early {
                let _ = state.db.update_history(&HistoryEntry { status: "failed".into(), error: Some(e.clone()), ..row });
                history_updated(app);
            }
            return Err(Notice::model_error(e));
        }
    };
    eprintln!("Dictation transcribed: chars={}", raw.chars().count());
    if is_blank(&raw) {
        if saved_early {
            let _ = state.db.delete_history_entry(&id);
        }
        if audio_saved {
            let _ = fs::remove_file(&audio_file_path);
        }
        return Ok(None);
    }

    // Voice activation: "Glaido, …" turns the dictation into a command; nothing is pasted.
    if let Some(instruction) = config.beta_features.then(|| commands::wake_command(&raw)).flatten() {
        let command_row = HistoryEntry { kind: "command".into(), status: "running".into(), text: raw.clone(), raw_text: raw, ..row };
        let saved = if saved_early { state.db.update_history(&command_row) } else { state.db.insert_history(&command_row) };
        if let Err(e) = saved {
            eprintln!("Couldn't save the command record: {}", e);
        }
        history_updated(app);
        commands::start_from_dictation(app, instruction, session.selection, context, id);
        return Ok(None);
    }

    let final_text = finish_text(app, &state.db, &raw, &config, context.as_ref()).await;

    // Make a completed transcript recoverable before waiting for another delivery or touching the clipboard.
    let entry = HistoryEntry { text: final_text.clone(), raw_text: raw, ..row };
    let saved = if saved_early { state.db.update_history(&entry) } else { state.db.insert_history(&entry) };
    if let Err(e) = saved {
        // Keep the recording for recovery even if the database is unavailable.
        report_error(app, format!("Couldn't save to history: {e}"));
    }
    history_updated(app);

    // Auto-paste into the active app. Without Accessibility access macOS silently drops the
    // synthetic Cmd+V, so leave the text on the clipboard and tell the user.
    let can_paste = app.state::<HotkeyEngine>().status().permission_granted;
    let keep_in_clipboard = config.copy_to_clipboard || !can_paste;
    let text = final_text.clone();
    let paste_app = app.clone();
    let target_app_id = context.as_ref().map(|c| c.bundle_id.clone());
    let delivery = session.delivery.take();
    if let Some(ticket) = delivery.as_ref() {
        ticket.wait().await;
    }
    eprintln!("Dictation paste: target={:?} accessibility={} chars={}", context.as_ref().map(|c| c.bundle_id.as_str()), can_paste, final_text.chars().count());
    match tauri::async_runtime::spawn_blocking(move || {
        // The native operation owns its turn even if the awaiting task is cancelled.
        let _delivery = delivery;
        paste::paste_text_to(&paste_app, &text, keep_in_clipboard, target_app_id.as_deref())
    }).await {
        Ok(Ok(())) if !can_paste => report_error(
            app,
            "Copied to clipboard: allow OpenGlaido in Privacy & Security › Accessibility to paste automatically",
        ),
        Ok(Ok(())) => eprintln!("Dictation paste shortcut posted"),
        Ok(Err(e)) => report_error(app, format!("Couldn't paste: {}", e)),
        Err(e) => report_error(app, format!("Couldn't paste: {}", e)),
    }

    Ok(Some(final_text))
}

#[tauri::command]
async fn get_recording_state(state: State<'_, AppState>) -> Result<bool, String> {
    Ok(*state.mode.lock().unwrap() != Mode::Idle)
}

// Async commands below run off the main thread; see `AppState::mode`.
#[tauri::command]
async fn start_recording(app: AppHandle, state: State<'_, AppState>) -> Result<(), String> {
    let mut mode = state.mode.lock().unwrap();
    if *mode != Mode::Idle {
        return Ok(());
    }
    begin_capture(&app, &state, Instant::now(), Purpose::Dictation)
        .inspect_err(|e| report_notice(&app, e))
        .map_err(|e| e.filled())?;
    set_mode(&app, &state, &mut mode, Mode::HandsFree);
    Ok(())
}

#[tauri::command]
async fn stop_recording_and_process(app: AppHandle, state: State<'_, AppState>) -> Result<(), String> {
    let (recording, duration_ms, session) = {
        let mut mode = state.mode.lock().unwrap();
        if *mode == Mode::Idle {
            return Err("Not recording".to_string());
        }
        let capture = end_capture(&app, &state);
        set_mode(&app, &state, &mut mode, Mode::Idle);
        capture.inspect_err(|e| report_error(&app, e))?
    };
    match session.purpose {
        _ if recording.interruption.is_some() => process_recording(app, recording, duration_ms, session).await,
        Purpose::Dictation => process_recording(app, recording, duration_ms, session).await,
        Purpose::Command => {
            commands::run_recorded(app.clone(), recording, duration_ms, session).await;
            job_finished(&app);
        }
    }
    Ok(())
}

#[tauri::command]
async fn cancel_recording(app: AppHandle, state: State<'_, AppState>) -> Result<(), String> {
    let mut mode = state.mode.lock().unwrap();
    if *mode != Mode::Idle {
        cancel_capture(&app, &state);
        set_mode(&app, &state, &mut mode, Mode::Idle);
    }
    Ok(())
}

#[tauri::command]
fn get_config(state: State<'_, AppState>) -> Result<TranscriptionConfig, String> {
    let cfg = state.config.lock().map_err(|e| e.to_string())?;
    Ok(cfg.clone())
}

/// Validates and applies `new_config`, then persists it. On Err nothing is saved or kept applied.
#[tauri::command]
async fn save_config(
    app: AppHandle,
    new_config: TranscriptionConfig,
    edited_model_keys: Option<Vec<String>>,
    edited_search_provider: Option<String>,
    state: State<'_, AppState>,
) -> Result<TranscriptionConfig, String> {
    let _activity = app.state::<updater::UpdateState>().activity()?;
    if !["bottom", "raised", "high"].contains(&new_config.bar_location.as_str()) {
        return Err(format!("Unknown dictation bar location “{}”", new_config.bar_location));
    }
    new_config.validate_formatting()?;
    new_config.validate_models()?;
    // MCP prefs only change through the mcp_* commands; never let a stale UI copy overwrite them.
    let _serialized = state.config_write.lock().unwrap();
    let mut new_config = new_config;
    new_config.mcp_servers = state.config.lock().unwrap().mcp_servers.clone();

    let mut old = state.config.lock().unwrap().clone();
    if !old.provider_keys_migrated {
        hydrate_legacy_model_keys(&mut old)?;
    }
    if !old.search_keys_migrated {
        hydrate_legacy_search_key(&mut old)?;
    }
    let edited = edited_model_keys.unwrap_or_default();
    let edited: Vec<&str> = edited.iter().map(String::as_str).collect();
    let model_keys = secrets::save_model_keys(&old, &mut new_config, &edited)?;
    let search_key = secrets::save_search_key(&old, &mut new_config, edited_search_provider.as_deref())?;
    let engine = app.state::<HotkeyEngine>();
    // Only hotkeys that changed are (re)applied: one that failed to register at startup must not
    // make every unrelated setting fail to save.
    let dictation_changed = (&new_config.hotkey_hold, &new_config.hotkey_toggle) != (&old.hotkey_hold, &old.hotkey_toggle);
    if dictation_changed {
        engine.set_bindings(&new_config.hotkey_hold, &new_config.hotkey_toggle)?;
    }
    let restore_hotkeys = || {
        if dictation_changed {
            if let Err(e) = engine.set_bindings(&old.hotkey_hold, &old.hotkey_toggle) {
                eprintln!("Failed to restore previous hotkeys: {}", e);
            }
        }
        let (hold, toggle) = command_bindings(&old);
        if let Err(e) = engine.set_command_bindings(hold, toggle) {
            eprintln!("Failed to restore previous command hotkeys: {}", e);
        }
    };
    if command_bindings(&new_config) != command_bindings(&old) {
        let (hold, toggle) = command_bindings(&new_config);
        if let Err(e) = engine.set_command_bindings(hold, toggle) {
            let beta_turned_on = new_config.beta_features && !old.beta_features;
            if !beta_turned_on {
                restore_hotkeys();
                return Err(e);
            }
            // Turning beta on: a command hotkey (hidden until now) that clashes with dictation is
            // cleared rather than blocking the switch; the user picks a new one in Settings.
            let (hold, toggle) = [(hold, ""), ("", toggle), ("", "")]
                .into_iter()
                .find(|(h, t)| engine.set_command_bindings(h, t).is_ok())
                .unwrap_or(("", ""));
            let (hold, toggle) = (hold.to_string(), toggle.to_string());
            new_config.commands_hold = hold;
            new_config.commands_toggle = toggle;
            report_error(&app, format!("{e}. Choose a new commands hotkey in Settings › Hotkeys."));
        }
    }
    if let Err(e) = set_launch_at_login(&app, new_config.launch_at_login) {
        restore_hotkeys();
        return Err(e);
    }
    if let Err(e) = write_config(&state.db.app_dir.join(CONFIG_FILE), &new_config) {
        restore_hotkeys();
        let _ = set_launch_at_login(&app, old.launch_at_login);
        return Err(format!("Couldn't save settings: {}", e));
    }
    model_keys.commit();
    search_key.commit();
    if !old.provider_keys_migrated {
        remove_legacy_model_keys();
    }
    if !old.search_keys_migrated { remove_legacy_search_key(); }
    apply_ui_settings(&app, &new_config, Some(&old));

    let mode = state.mode.lock().unwrap();
    *state.config.lock().unwrap() = new_config.clone();
    engine.set_submit_enabled(submit_enabled(*mode, &new_config));
    drop(mode);
    load_local_models(&app, &new_config, Some(&old));
    update_hud(&app);
    // The dictation bar and command window follow language changes.
    let _ = app.emit("config-changed", &new_config);
    Ok(new_config)
}

#[tauri::command]
fn get_hotkey_status(engine: State<'_, HotkeyEngine>) -> HotkeyStatus {
    engine.status()
}

/// Err = invalid on this platform; Ok(Some) = conflict warning.
/// Sync on purpose: the macOS conflict probe uses Carbon, which wants the main thread.
#[tauri::command]
fn check_hotkey(binding: String) -> Result<Option<String>, String> {
    hotkeys::validate_binding(&binding)?;
    Ok(hotkeys::conflict_warning(&binding))
}

// Async: the plugin backend waits on the main thread while holding the engine lock, which the
// hotkey worker may hold too; blocking the main thread here could deadlock.
#[tauri::command]
async fn start_hotkey_capture(app: AppHandle) -> bool {
    app.state::<HotkeyEngine>().start_capture()
}

#[tauri::command]
async fn stop_hotkey_capture(app: AppHandle) {
    app.state::<HotkeyEngine>().stop_capture()
}

#[tauri::command]
async fn list_input_devices() -> Vec<String> {
    audio::list_input_devices()
}

#[tauri::command]
fn open_accessibility_settings() {
    #[cfg(target_os = "macos")]
    if let Err(e) = std::process::Command::new("open")
        .arg("x-apple.systempreferences:com.apple.preference.security?Privacy_Accessibility")
        .status()
    {
        eprintln!("Failed to open Accessibility settings: {}", e);
    }
}

#[tauri::command]
fn get_history(limit: Option<usize>, state: State<'_, AppState>) -> Result<Vec<HistoryEntry>, String> {
    state.db.get_history(limit.unwrap_or(50))
}

#[tauri::command]
async fn get_history_page(query: String, before: Option<db::HistoryCursor>, limit: Option<usize>, state: State<'_, AppState>) -> Result<Vec<HistoryEntry>, String> {
    let db = state.db.clone();
    tauri::async_runtime::spawn_blocking(move || db.get_history_page(&query, before.as_ref(), limit.unwrap_or(100)))
        .await.map_err(|e| e.to_string())?
}

#[tauri::command]
async fn get_history_stats(state: State<'_, AppState>) -> Result<db::HistoryStats, String> {
    let db = state.db.clone();
    tauri::async_runtime::spawn_blocking(move || db.history_stats()).await.map_err(|e| e.to_string())?
}

#[tauri::command]
async fn get_search_provider_key(provider: String) -> Result<String, String> {
    secrets::get_search_provider_key(&provider)
}

#[tauri::command]
fn delete_history_entry(id: String, state: State<'_, AppState>) -> Result<(), String> {
    if state.db.get_history_entry(&id)?.status == "running" {
        return Err("This recording is already being processed".into());
    }
    state.db.delete_history_entry(&id)
}

#[tauri::command]
fn get_audio_base64(id: String, state: State<'_, AppState>) -> Result<String, String> {
    let entry = state.db.get_history_entry(&id)?;
    let filename = entry
        .audio_filename
        .ok_or_else(|| "No audio recording found for this entry".to_string())?;
    let path = state.db.app_dir.join("audio").join(filename);
    let bytes = fs::read(path).map_err(|e| e.to_string())?;
    use base64::Engine;
    Ok(base64::engine::general_purpose::STANDARD.encode(bytes))
}

/// Transcribes a record's audio again with the current settings and replaces its text.
#[tauri::command]
async fn retranscribe(app: AppHandle, id: String, state: State<'_, AppState>) -> Result<HistoryEntry, String> {
    let _activity = app.state::<updater::UpdateState>().activity()?;
    let _retry = state.db.begin_retry(&id)?;
    job_started(&app);
    let result = retranscribe_entry(&app, &id, &state).await;
    job_finished(&app);
    result
}

async fn retranscribe_entry(app: &AppHandle, id: &str, state: &AppState) -> Result<HistoryEntry, String> {
    let app = app.clone();
    let mut entry = state.db.get_history_entry(id)?;
    let filename = entry
        .audio_filename
        .clone()
        .ok_or_else(|| "No audio available to retranscribe".to_string())?;
    let bytes = fs::read(state.db.app_dir.join("audio").join(filename)).map_err(|e| e.to_string())?;
    let config = state.config.lock().unwrap().clone();

    let raw = transcribe_raw(&app, bytes, &config, vocab_terms(&state.db)).await.and_then(|raw| {
        if is_blank(&raw) {
            Err("No speech detected".to_string())
        } else {
            Ok(raw)
        }
    });
    let raw = match raw {
        Ok(raw) => raw,
        Err(e) => {
            entry.status = "failed".into();
            entry.error = Some(e.clone());
            state.db.update_history(&entry)?;
            history_updated(&app);
            return Err(e);
        }
    };
    // Retain the original website while using the user's current model and formatting settings.
    let context = history_context(&entry);
    entry.text = finish_text(&app, &state.db, &raw, &config, context.as_ref()).await;
    entry.raw_text = raw;
    entry.status = "ok".into();
    entry.error = None;
    state.db.update_history(&entry)?;
    history_updated(&app);
    Ok(entry)
}

/// One record (the ⌘K palette opens results older than the loaded page).
#[tauri::command]
fn get_history_item(id: String, state: State<'_, AppState>) -> Result<HistoryEntry, String> {
    state.db.get_history_entry(&id)
}

#[tauri::command]
fn search_history(query: String, limit: Option<usize>, state: State<'_, AppState>) -> Result<Vec<HistoryEntry>, String> {
    state.db.search_history(&query, limit.unwrap_or(15))
}

/// Installed apps (no browsers, no OpenGlaido), for the formatting rule picker.
#[tauri::command]
async fn list_apps() -> Vec<AppInfo> {
    frontmost::list_apps()
}

/// Up to 30 apps dictated into during the last 90 days that are still installed.
#[tauri::command]
async fn recent_apps(state: State<'_, AppState>) -> Result<Vec<AppInfo>, String> {
    let since = (chrono::Utc::now() - chrono::Duration::days(90)).to_rfc3339();
    let installed = frontmost::list_apps();
    let recent = state.db.recent_apps(&since, 60)?;
    Ok(recent
        .into_iter()
        .filter_map(|(bundle_id, name)| {
            let found = installed.iter().find(|a| a.bundle_id == bundle_id);
            found.cloned().or_else(|| {
                (!frontmost::is_browser(&bundle_id) && frontmost::is_installed(&bundle_id))
                    .then(|| AppInfo { bundle_id, name, path: String::new() })
            })
        })
        .take(30)
        .collect())
}

/// App icon as a 64 px PNG data URL (macOS); None elsewhere.
#[tauri::command]
async fn get_app_icon(bundle_id: String) -> Option<String> {
    frontmost::app_icon(&bundle_id)
}

/// Localized tray menu labels (the frontend owns the translations).
#[tauri::command]
fn set_tray_labels(app: AppHandle, microphone: String, system_default: String, show: String, quit: String) {
    *app.state::<TrayLabels>().0.lock().unwrap() = [microphone, system_default, show, quit];
    refresh_tray(&app);
}

#[tauri::command]
fn get_dictionary(state: State<'_, AppState>) -> Result<Vec<DictionaryEntry>, String> {
    state.db.get_dictionary()
}

#[tauri::command]
fn add_dictionary_entry(
    phrase: String,
    replacement: String,
    state: State<'_, AppState>,
) -> Result<String, String> {
    state.db.add_dictionary_entry(&phrase, &replacement)
}

#[tauri::command]
fn delete_dictionary_entry(id: String, state: State<'_, AppState>) -> Result<(), String> {
    state.db.delete_dictionary_entry(&id)
}

#[tauri::command]
fn get_snippets(state: State<'_, AppState>) -> Result<Vec<SnippetEntry>, String> {
    state.db.get_snippets()
}

#[tauri::command]
fn add_snippet(trigger: String, content: String, state: State<'_, AppState>) -> Result<String, String> {
    state.db.add_snippet(&trigger, &content)
}

#[tauri::command]
fn delete_snippet(id: String, state: State<'_, AppState>) -> Result<(), String> {
    state.db.delete_snippet(&id)
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_notification::init())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .plugin(tauri_plugin_autostart::init(MacosLauncher::LaunchAgent, None))
        .plugin(
            tauri_plugin_global_shortcut::Builder::new()
                .with_handler(|app, shortcut, event| {
                    if let Some(engine) = app.try_state::<HotkeyEngine>() {
                        engine.handle_plugin_event(shortcut, event.state());
                    }
                })
                .build(),
        )
        .setup(|app| {
            let _ = MAIN_THREAD.set(std::thread::current().id());
            let app_data_dir = app
                .path()
                .app_data_dir()
                .unwrap_or_else(|_| PathBuf::from("./data"));

            let config = load_config(&app_data_dir.join(CONFIG_FILE));
            output::recover(&app_data_dir);
            let db = Database::new(app_data_dir).expect("Failed to initialize database");
            if let Err(e) = db.mark_interrupted() {
                eprintln!("Couldn't mark interrupted records: {e}");
            }
            app.manage(updater::UpdateState::new(app.package_info().version.to_string()));

            // The engine's callback must not block: queue events for one worker thread, which
            // handles them in order (and may block on the audio thread / main thread).
            let (events, rx) = channel::<(HotkeyEvent, Instant)>();
            let worker_app = app.handle().clone();
            std::thread::spawn(move || {
                for (event, at) in rx {
                    // === commands wiring: begin ===
                    // Command hotkeys share the recording state machine; handle_event routes them.
                    // === commands wiring: end ===
                    handle_event(&worker_app, event, at);
                }
            });
            let engine_events = events.clone();
            let engine = HotkeyEngine::start(app.handle(), move |event| {
                let _ = engine_events.send((event, Instant::now()));
            });
            app.manage(engine.clone());

            app.manage(AppState {
                recorder: AudioRecorder::new(),
                mode: Mutex::new(Mode::Idle),
                recording: AtomicBool::new(false),
                recording_start: Mutex::new(None),
                session: Mutex::new(None),
                config: Mutex::new(config.clone()),
                db: Arc::new(db),
                events,
                jobs: AtomicUsize::new(0),
                error_until: Mutex::new(None),
                hud: Mutex::new(HudVisibility { shown: false, hidden: true, changes: 0 }),
                config_write: Mutex::new(()),
                pending_start: Mutex::new(None),
                start_token: std::sync::atomic::AtomicU64::new(0),
            });

            // Menu bar icon; its menu is built by refresh_tray (English until the frontend sends translations).
            app.manage(TrayLabels(Mutex::new(["Microphone", "System Default", "Show OpenGlaido", "Quit"].map(String::from))));
            let _tray = TrayIconBuilder::with_id(TRAY_ID)
                .icon(app.default_window_icon().unwrap().clone())
                .tooltip("OpenGlaido")
                .on_tray_icon_event(|tray, event| {
                    if let tauri::tray::TrayIconEvent::Enter { .. } = event {
                        refresh_tray(tray.app_handle());
                    }
                })
                .on_menu_event(|app, event| match event.id().as_ref() {
                    "show" => {
                        let Ok(_activity) = app.state::<updater::UpdateState>().activity() else { return; };
                        if let Some(main) = app.get_webview_window("main") {
                            let _ = main.show();
                            let _ = main.set_focus();
                        }
                    }
                    "quit" => app.exit(0),
                    id => {
                        let device = match id.strip_prefix("mic:") {
                            Some(name) => Some(name.to_string()),
                            None if id == "mic-default" => None,
                            None => return,
                        };
                        // Off the main thread: update_config takes config_write, which save_config holds while
                        // it waits on the main thread.
                        let app = app.clone();
                        std::thread::spawn(move || {
                            if let Err(e) = update_config(&app, |c| c.input_device = device) {
                                report_error(&app, e);
                            }
                            refresh_tray(&app);
                        });
                    }
                })
                .build(app)?;
            refresh_tray(app.handle());

            // Apply the persisted settings. Failures are reported but never stop the app.
            if let Err(e) = engine.set_bindings(&config.hotkey_hold, &config.hotkey_toggle) {
                report_error(app.handle(), format!("Couldn't register hotkeys: {}", e));
            }
            // === commands wiring: begin ===
            let (hold, toggle) = command_bindings(&config);
            if let Err(e) = engine.set_command_bindings(hold, toggle) {
                report_error(app.handle(), format!("Couldn't register command hotkeys: {}", e));
            }
            commands::init(app.handle());
            // === commands wiring: end ===
            if let Err(e) = set_launch_at_login(app.handle(), config.launch_at_login) {
                report_error(app.handle(), e);
            }
            apply_ui_settings(app.handle(), &config, None);
            load_local_models(app.handle(), &config, None);

            // The dictation bar never takes clicks.
            if let Some(hud) = app.get_webview_window("hud") {
                if let Err(e) = hud.set_ignore_cursor_events(true) {
                    eprintln!("Failed to set up the dictation bar: {}", e);
                }
            }
            // Hiding OpenGlaido (⌘H, Hide Others) must not hide the bar or an armed command window.
            #[cfg(target_os = "macos")]
            for label in ["hud", "command"] {
                if let Some(ptr) = app.get_webview_window(label).and_then(|w| w.ns_window().ok()) {
                    let window = ptr as *mut objc2::runtime::AnyObject;
                    unsafe {
                        let _: () = objc2::msg_send![window, setCanHide: false];
                    }
                }
            }
            update_hud(app.handle());
            updater::start(app.handle());

            Ok(())
        })
        .on_window_event(|window, event| match event {
            WindowEvent::CloseRequested { api, .. } if window.label() == "main" => {
                api.prevent_close();
                let _ = window.hide();
            }
            // The hotkey recorder only lives while the main window has focus.
            WindowEvent::Focused(false) if window.label() == "main" => {
                // Off the main thread: on the plugin backend the engine may be waiting on it.
                let engine = window.state::<HotkeyEngine>().inner().clone();
                std::thread::spawn(move || engine.stop_capture());
            }
            _ => {}
        })
        .invoke_handler(tauri::generate_handler![
            updater::get_update_status,
            updater::check_for_updates,
            get_recording_state,
            start_recording,
            stop_recording_and_process,
            cancel_recording,
            get_config,
            save_config,
            get_hotkey_status,
            check_hotkey,
            start_hotkey_capture,
            stop_hotkey_capture,
            list_input_devices,
            open_accessibility_settings,
            setup::get_setup_issues,
            setup::open_microphone_settings,
            setup::request_microphone_access,
            get_history,
            get_history_page,
            get_history_stats,
            get_search_provider_key,
            delete_history_entry,
            get_audio_base64,
            retranscribe,
            get_dictionary,
            add_dictionary_entry,
            delete_dictionary_entry,
            get_snippets,
            add_snippet,
            delete_snippet,
            search_history,
            get_history_item,
            list_apps,
            recent_apps,
            get_app_icon,
            set_tray_labels,
            // === commands wiring: begin ===
            commands::get_builtin_tools,
            commands::command_paste,
            commands::command_copy,
            commands::command_close,
            commands::command_approve,
            commands::mcp_list_servers,
            commands::mcp_import_folder,
            commands::mcp_set_enabled,
            commands::mcp_set_tool_policy,
            commands::mcp_refresh,
            commands::mcp_delete,
            commands::mcp_open_config,
            commands::mcp_check_runtime,
            commands::mcp_new_server,
            commands::mcp_reveal_folder,
            // === commands wiring: end ===
            models::list_local_models,
            models::local_models_supported,
            models::download_model,
            models::cancel_model_download,
            models::delete_model,
            models::list_provider_models,
            models::validate_provider_key,
            models::reveal_models_folder,
        ])
        .build(tauri::generate_context!())
        .expect("error while building tauri application")
        .run(|_app, _event| {
            if let tauri::RunEvent::Exit = _event { prepare_exit(_app); }
            #[cfg(target_os = "macos")]
            if let tauri::RunEvent::Reopen { .. } = _event {
                let updates = _app.state::<updater::UpdateState>();
                if updates.suppress_initial_reopen() { return; }
                let Ok(_activity) = updates.activity() else { return; };
                if let Some(main) = _app.get_webview_window("main") {
                    let _ = main.show();
                    let _ = main.set_focus();
                }
            }
        });
}

fn prepare_exit(app: &AppHandle) {
    commands::shutdown(app);
    // ponytail: waits for a running local transcription; whisper's abort callback if that grows.
    models::whisper::unload_now();
    models::llama::unload_now();
    if let Some(state) = app.try_state::<AppState>() { output::restore(&state.db.app_dir); }
}

#[cfg(test)]
mod tests {
    use super::*;
    use HotkeyEvent::*;

    const SHORT: Duration = Duration::from_millis(100);
    const LONG: Duration = Duration::from_millis(900);

    #[test]
    fn releasing_recording_hides_hud_and_invalidates_queued_shows() {
        let mut hud = HudVisibility { shown: false, hidden: true, changes: 0 };
        assert!(hud.update(true, false));
        let first_recording = hud.changes;
        assert!(hud.can_show(first_recording));
        assert!(hud.update(false, false));
        assert!(!hud.shown);
        assert!(!hud.can_show(first_recording));
        // Background job completions cannot make a stopped recording visible again.
        assert!(!hud.update(false, false));
        assert!(!hud.shown);
        // A quick new recording has its own show; the old queued one stays invalid.
        assert!(hud.update(true, false));
        assert!(!hud.can_show(first_recording));
        assert!(hud.can_show(hud.changes));
        // Errors still appear after release, and disappear when their lifetime ends.
        assert!(!hud.update(false, true));
        assert!(hud.shown);
        assert!(hud.update(false, false));
        assert!(!hud.shown);
    }

    #[test]
    fn fn_hold_responds_promptly_without_speeding_up_editing_modifiers() {
        assert_eq!(hold_start_delay("Fn"), Duration::from_millis(60));
        for binding in ["AltRight", "ControlLeft", "ShiftLeft", "MetaRight", "Fn+AltRight"] {
            assert_eq!(hold_start_delay(binding), Duration::from_millis(150));
        }
        // Releasing or completing another shortcut during the grace period still cancels.
        for event in [HoldReleased, HoldAborted, Cancel] {
            assert_eq!(transition(Mode::Hold, event, Duration::from_millis(30), true), (Mode::Idle, Effect::Cancel));
        }
    }

    #[test]
    fn notice_keeps_placeholders_until_filled() {
        let mut vars = BTreeMap::new();
        vars.insert("name".into(), "MacBook Pro Microphone".into());
        let text = fill("Microphone “{name}” not found, using the system default", &vars);
        assert_eq!(text, "Microphone “MacBook Pro Microphone” not found, using the system default");
        let notice = Notice::error("Couldn't start recording: busy");
        assert_eq!(notice.filled(), "Couldn't start recording: busy");
    }

    #[test]
    fn only_model_failures_reopen_the_dictation_bar() {
        assert!(Notice::model_error("Transcription failed: couldn't load the model").model_failure);
        for message in ["Recording failed", "Couldn't paste", "Couldn't save to history"] {
            assert!(!Notice::error(message).model_failure);
        }
        for id in ["stt", "llm", "accessibility", "microphone", "no_microphone", "hotkeys"] {
            let notice = Notice::from_issue(setup::SetupIssue {
                id: id.into(),
                level: "error".into(),
                title: "Setup issue".into(),
                detail: String::new(),
                vars: BTreeMap::new(),
                action: None,
            });
            assert_eq!(notice.model_failure, matches!(id, "stt" | "llm"), "{id}");
        }
        for text in ["", "  ", "…", ". ! ?"] {
            assert!(is_blank(text));
        }
        assert!(!is_blank("Hello"));
    }

    #[test]
    fn hold_to_talk() {
        assert_eq!(transition(Mode::Idle, HoldPressed, Duration::ZERO, false), (Mode::Hold, Effect::Start));
        assert_eq!(transition(Mode::Hold, HoldReleased, LONG, false), (Mode::Idle, Effect::Stop));
        assert_eq!(transition(Mode::Hold, HoldReleased, SHORT, false), (Mode::Idle, Effect::Cancel));
        assert_eq!(transition(Mode::Hold, HoldAborted, LONG, false), (Mode::Idle, Effect::Cancel));
        assert_eq!(transition(Mode::Hold, Cancel, LONG, false), (Mode::Idle, Effect::Cancel));
        assert_eq!(transition(Mode::Hold, Submit, LONG, false), (Mode::Hold, Effect::None));
    }

    #[test]
    fn hands_free() {
        assert_eq!(transition(Mode::Idle, TogglePressed, Duration::ZERO, false), (Mode::HandsFree, Effect::Start));
        // Hold → hands-free keeps recording; the later hold release is ignored.
        assert_eq!(transition(Mode::Hold, TogglePressed, SHORT, false), (Mode::HandsFree, Effect::None));
        assert_eq!(transition(Mode::HandsFree, HoldReleased, LONG, false), (Mode::HandsFree, Effect::None));
        assert_eq!(transition(Mode::HandsFree, TogglePressed, LONG, false), (Mode::Idle, Effect::Stop));
        assert_eq!(transition(Mode::HandsFree, Submit, LONG, false), (Mode::Idle, Effect::Stop));
        assert_eq!(transition(Mode::HandsFree, Cancel, LONG, false), (Mode::Idle, Effect::Cancel));
    }

    #[test]
    fn pending_hold_starts_on_toggle_and_still_cancels() {
        // Fn then Space confirms intent before the modifier-only guard expires.
        assert_eq!(transition(Mode::Hold, TogglePressed, SHORT, true), (Mode::HandsFree, Effect::Start));
        assert_eq!(transition(Mode::Hold, TogglePressed, SHORT, false), (Mode::HandsFree, Effect::None));
        // A short tap, another shortcut, or Esc must cancel even before capture starts.
        for event in [HoldReleased, HoldAborted, Cancel] {
            assert_eq!(transition(Mode::Hold, event, SHORT, true), (Mode::Idle, Effect::Cancel));
        }
        assert_eq!(transition(Mode::Hold, HoldPressed, SHORT, true), (Mode::Hold, Effect::None));
    }

    #[test]
    fn hold_key_stops_hands_free_on_release_only() {
        // Fn alone: stop on release.
        assert_eq!(transition(Mode::HandsFree, HoldPressed, LONG, false), (Mode::HandsFreeHoldDown, Effect::None));
        assert_eq!(transition(Mode::HandsFreeHoldDown, HoldReleased, LONG, false), (Mode::Idle, Effect::Stop));
        // Fn+Space: stops once on the toggle; the trailing Fn release is a no-op when idle.
        assert_eq!(transition(Mode::HandsFreeHoldDown, TogglePressed, LONG, false), (Mode::Idle, Effect::Stop));
        assert_eq!(transition(Mode::Idle, HoldReleased, LONG, false), (Mode::Idle, Effect::None));
        // Fn + another key: keep dictating.
        assert_eq!(transition(Mode::HandsFreeHoldDown, HoldAborted, LONG, false), (Mode::HandsFree, Effect::None));
        assert_eq!(transition(Mode::HandsFreeHoldDown, Cancel, LONG, false), (Mode::Idle, Effect::Cancel));
    }

    #[test]
    fn idle_ignores_everything_but_start() {
        for ev in [HoldReleased, HoldAborted, Cancel, Submit] {
            assert_eq!(transition(Mode::Idle, ev, LONG, false), (Mode::Idle, Effect::None));
        }
    }

    #[test]
    fn submit_only_in_hands_free_with_enter_to_stop() {
        let on = TranscriptionConfig { enter_to_stop: true, ..Default::default() };
        assert!(submit_enabled(Mode::HandsFree, &on));
        assert!(submit_enabled(Mode::HandsFreeHoldDown, &on));
        assert!(!submit_enabled(Mode::Hold, &on));
        assert!(!submit_enabled(Mode::HandsFree, &TranscriptionConfig::default()));
    }

    #[test]
    fn old_config_file_migrates() {
        secrets::use_mock_store();
        let dir = std::env::temp_dir().join(format!("og-config-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join(CONFIG_FILE);
        // Written before the model settings by a version that kept the key on disk: a local transcription
        // server (no key needed) and Groq cleanup with a Llama model Groq has since retired.
        let old = r#"{
          "endpoint_url": "http://localhost:8000/v1/audio/transcriptions", "api_key": "gsk_old",
          "model_name": "Systran/faster-whisper-small", "temperature": 0.0,
          "llm_endpoint_url": "https://api.groq.com/openai/v1/chat/completions", "llm_model_name": "llama-3.3-70b-versatile",
          "style": "casual", "raw_text": false, "languages": ["de"], "search_api_key": ""
        }"#;
        fs::write(&path, old).unwrap();
        let cfg = load_config(&path);
        assert!(cfg.provider_keys_migrated);
        assert_eq!((cfg.stt_source.as_str(), cfg.stt_provider.as_str()), ("cloud", "custom"));
        assert_eq!((cfg.llm_source.as_str(), cfg.llm_provider.as_str()), ("cloud", "groq"));
        assert_eq!(cfg.llm_model_name.as_deref(), Some("openai/gpt-oss-20b"));
        assert_eq!((cfg.model_name.as_str(), cfg.style.as_str()), ("Systran/faster-whisper-small", "casual"));
        // The language model keeps using the shared key…
        assert_eq!((cfg.api_key.as_str(), cfg.llm_api_key.as_str()), ("gsk_old", "gsk_old"));
        let saved = fs::read_to_string(&path).unwrap();
        assert!(saved.contains(r#""llm_source": "cloud""#) && !saved.contains("gsk_old"), "{saved}");
        // …once: a config from this version with no language model key keeps it empty.
        fs::write(&path, r#"{"api_key": "gsk_old", "llm_source": "cloud"}"#).unwrap();
        assert!(load_config(&path).llm_api_key.is_empty());
        // No config yet: the defaults.
        fs::remove_file(&path).unwrap();
        assert_eq!(load_config(&path), TranscriptionConfig { provider_keys_migrated: true, search_keys_migrated: true, ..Default::default() });
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn credentials_never_fall_back_to_plaintext() {
        let mut config = TranscriptionConfig {
            provider_keys_migrated: true,
            search_keys_migrated: true,
            api_key: "stt-secret".into(),
            llm_api_key: "llm-secret".into(),
            search_api_key: "search-secret".into(),
            ..Default::default()
        };
        let saved: TranscriptionConfig = serde_json::from_str(&config_json(&config).unwrap()).unwrap();
        assert!(saved.api_key.is_empty() && saved.llm_api_key.is_empty() && saved.search_api_key.is_empty());
        assert!(saved.provider_keys_migrated && saved.search_keys_migrated);
        // Failed first-time migration cannot rewrite or lose the original plaintext file.
        config.provider_keys_migrated = false;
        assert!(config_json(&config).is_err());
        config.provider_keys_migrated = true;
        config.search_keys_migrated = false;
        assert!(config_json(&config).is_err());
        let dir = std::env::temp_dir().join(format!("og-key-migration-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join(CONFIG_FILE);
        let original = serde_json::to_string(&config).unwrap();
        fs::write(&path, &original).unwrap();
        assert!(write_config(&path, &config).is_err());
        assert_eq!(fs::read_to_string(&path).unwrap(), original);
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn retry_retains_the_site_rule_without_storing_private_url_details() {
        let config = TranscriptionConfig::default();
        let original = AppContext { bundle_id: "com.google.Chrome".into(), name: "Chrome".into(), url: Some("https://mail.google.com/mail/u/0?secret=example#inbox".into()) };
        let entry = HistoryEntry {
            app_bundle_id: Some(original.bundle_id.clone()), app_name: Some(original.name.clone()),
            website: history_website(Some(&original)), ..HistoryEntry::new("retry")
        };
        assert_eq!(entry.website.as_deref(), Some("mail.google.com"));
        assert_eq!(config.formatting_for(Some(&original)), config.formatting_for(history_context(&entry).as_ref()));
        let old = HistoryEntry { website: None, ..entry };
        assert!(history_context(&old).unwrap().url.is_none());
    }

    #[tokio::test]
    async fn session_context_is_bound_to_its_original_target_and_survives_cancelled_lookup() {
        let original = AppContext { bundle_id: "com.google.Chrome".into(), name: "Chrome".into(), url: None };
        let mut session = Session {
            purpose: Purpose::Dictation, context: Some(original.clone()), selection: None,
            chime: false, config: TranscriptionConfig::default(), live: None, context_task: None, delivery: None,
            recorded_at: chrono::Utc::now().to_rfc3339(),
        };
        let (tx, rx) = tokio::sync::oneshot::channel();
        session.context_task = Some(rx);
        tx.send((AppContext { bundle_id: "other.app".into(), name: "Other".into(), url: None }, Some("unrelated text".into()))).unwrap();
        session.resolve_context().await;
        assert_eq!(session.context, Some(original.clone()));
        assert!(session.selection.is_none());
        let (tx, rx) = tokio::sync::oneshot::channel();
        session.context_task = Some(rx);
        drop(tx);
        session.resolve_context().await;
        assert_eq!(session.context, Some(original.clone()));
        let (tx, rx) = tokio::sync::oneshot::channel();
        session.context_task = Some(rx);
        let resolved = AppContext { url: Some("https://example.com".into()), ..original };
        tx.send((resolved.clone(), Some("selected".into()))).unwrap();
        session.resolve_context().await;
        assert_eq!(session.context, Some(resolved));
        assert_eq!(session.selection.as_deref(), Some("selected"));
    }

    #[test]
    fn bar_offsets() {
        let area = PhysicalRect {
            position: PhysicalPosition::new(0, 50),
            size: PhysicalSize::new(2000, 1000),
        };
        let window = PhysicalSize::new(840, 144);
        assert_eq!(hud_origin(&area, window, 2.0, "bottom"), PhysicalPosition::new(580, 874));
        assert_eq!(hud_origin(&area, window, 2.0, "raised"), PhysicalPosition::new(580, 634));
        assert_eq!(hud_origin(&area, window, 2.0, "high"), PhysicalPosition::new(580, 606));
        assert_eq!(hud_origin(&area, window, 1.0, "unknown"), PhysicalPosition::new(580, 890));
    }
}
