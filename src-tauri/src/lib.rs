pub mod audio;
pub mod commands;
pub mod db;
pub mod frontmost;
pub mod hotkeys;
pub mod output;
pub mod paste;
pub mod secrets;
pub mod sound;
pub mod transcribe;

use audio::{AudioRecorder, Recording};
use db::{Database, DictionaryEntry, HistoryEntry, SnippetEntry};
use frontmost::{AppContext, AppInfo};
use hotkeys::{HotkeyEngine, HotkeyEvent, HotkeyStatus};
use transcribe::{apply_formatting, transcribe_raw, TranscriptionConfig};

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc::{channel, Sender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tauri::menu::{Menu, MenuItem};
use tauri::Wry;
use tauri::tray::TrayIconBuilder;
use tauri::{
    AppHandle, Emitter, Manager, PhysicalPosition, PhysicalRect, PhysicalSize, State, WindowEvent,
};
use tauri_plugin_autostart::{MacosLauncher, ManagerExt};

const TRAY_ID: &str = "main";
const CONFIG_FILE: &str = "config.json";
/// Hold-to-talk presses shorter than this are treated as accidental taps and cancelled.
const MIN_HOLD: Duration = Duration::from_millis(300);
/// A modifier-only hold only starts the mic (chime, mute, Accessibility lookups) once the key has
/// been held alone this long, so ⌥←, ⌥e or fn+arrows never flash a recording.
const HOLD_START_DELAY: Duration = Duration::from_millis(150);

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

/// Recording state machine. `held` = time since the recording started.
fn transition(mode: Mode, event: HotkeyEvent, held: Duration) -> (Mode, Effect) {
    use HotkeyEvent::*;
    use Mode::*;
    match (mode, event) {
        (Idle, HoldPressed) => (Hold, Effect::Start),
        (Idle, TogglePressed) => (HandsFree, Effect::Start),
        (Hold, HoldReleased) if held < MIN_HOLD => (Idle, Effect::Cancel),
        (Hold, HoldReleased) => (Idle, Effect::Stop),
        (Hold, HoldAborted) => (Idle, Effect::Cancel),
        (Hold, TogglePressed) => (HandsFree, Effect::None),
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
    /// Transcriptions in flight: the bar shows "processing" while > 0.
    pub jobs: AtomicUsize,
    /// The dictation bar stays visible until then to show an error (when idle-hiding is on).
    pub error_until: Mutex<Option<Instant>>,
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
#[derive(Debug, Clone)]
pub struct Session {
    pub purpose: Purpose,
    pub context: Option<AppContext>,
    pub selection: Option<String>,
    /// The start chime played (its first ~150 ms are cut from the audio).
    pub chime: bool,
}

/// Tray menu items, kept so their labels can be localized.
pub struct TrayItems(pub [MenuItem<Wry>; 3]);

const CHIME_TRIM_MS: u32 = 150;

/// Logs `msg` and shows it in the dictation bar.
pub fn report_error(app: &AppHandle, msg: impl AsRef<str>) {
    let msg = msg.as_ref();
    eprintln!("{}", msg);
    if let Some(state) = app.try_state::<AppState>() {
        *state.error_until.lock().unwrap() = Some(Instant::now() + Duration::from_millis(3500));
        update_hud(app);
        let app = app.clone();
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(3600));
            update_hud(&app);
        });
    }
    if let Err(e) = app.emit("dictation-error", msg) {
        eprintln!("Failed to emit dictation-error: {}", e);
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

/// Shows the dictation bar while idle only when `show_bar_when_idle`; always while busy.
pub fn update_hud(app: &AppHandle) {
    let (Some(state), Some(hud)) = (app.try_state::<AppState>(), app.get_webview_window("hud")) else { return };
    let showing_error = state.error_until.lock().unwrap().is_some_and(|t| Instant::now() < t);
    let busy = state.recording.load(Ordering::SeqCst) || state.jobs.load(Ordering::SeqCst) > 0;
    let show = busy || showing_error || state.config.lock().unwrap().show_bar_when_idle;
    let result = if show { show_passive(&hud) } else { hud.hide().map_err(|e| e.to_string()) };
    if let Err(e) = result {
        eprintln!("Failed to update the dictation bar: {}", e);
    }
}

/// Counts a transcription in flight (the bar shows "processing" while any runs).
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
    // Keys live in the keychain. A plaintext key from an older config.json moves there once.
    let plaintext = !config.api_key.is_empty() || !config.search_api_key.is_empty();
    for (account, value) in [("api_key", &mut config.api_key), ("search_api_key", &mut config.search_api_key)] {
        if value.is_empty() {
            *value = secrets::get(account);
        } else if let Err(e) = secrets::set(account, value) {
            eprintln!("{e}");
        }
    }
    // config_json keeps the keys on disk if the keychain failed, so this never loses one.
    if plaintext || migrated {
        if let Err(e) = write_config(path, &config) {
            eprintln!("Couldn't rewrite {} without keys: {}", path.display(), e);
        }
    }
    config
}

fn read_config(path: &Path) -> TranscriptionConfig {
    match fs::read_to_string(path) {
        Ok(json) => TranscriptionConfig::from_json(&json).unwrap_or_else(|e| {
            eprintln!("Invalid {}, using defaults: {}", path.display(), e);
            TranscriptionConfig::default()
        }),
        Err(e) => {
            eprintln!("No config at {} ({}), using defaults", path.display(), e);
            TranscriptionConfig::default()
        }
    }
}

/// The on-disk form: everything except the keys (those are in the keychain) — unless the
/// keychain is unusable, in which case dropping them would lose them.
fn config_json(config: &TranscriptionConfig) -> Result<String, String> {
    let on_disk = if secrets::available() {
        TranscriptionConfig { api_key: String::new(), search_api_key: String::new(), ..config.clone() }
    } else {
        config.clone()
    };
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
    let state = app.state::<AppState>();
    let _serialized = state.config_write.lock().unwrap();
    let updated = {
        let mut config = state.config.lock().unwrap();
        f(&mut config);
        config.clone()
    };
    write_config(&state.db.app_dir.join(CONFIG_FILE), &updated)?;
    let _ = app.emit("config-changed", &updated);
    Ok(())
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

// ponytail: primary monitor only, placed at startup/save; follow the cursor's monitor or
// re-place on display changes if multi-display users ask.
fn position_hud(app: &AppHandle, bar_location: &str) {
    let Some(hud) = app.get_webview_window("hud") else { return };
    match (hud.primary_monitor(), hud.outer_size()) {
        (Ok(Some(monitor)), Ok(size)) => {
            let origin = hud_origin(monitor.work_area(), size, monitor.scale_factor(), bar_location);
            if let Err(e) = hud.set_position(origin) {
                eprintln!("Failed to position the dictation bar: {}", e);
            }
        }
        _ => eprintln!("Failed to position the dictation bar: no primary monitor"),
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

/// Maps a linear peak (0..1) to a perceptual bar level: -50 dBFS → 0, 0 dBFS → 1.
fn level_to_ui(peak: f32) -> f32 {
    if peak <= 0.0 {
        return 0.0;
    }
    ((20.0 * peak.log10() + 50.0) / 50.0).clamp(0.0, 1.0)
}

// Emits "mic-level" to the HUD at ~30 Hz until the recording that started at `started` ends.
fn spawn_level_meter(app: &AppHandle, started: Instant) {
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        let state = app.state::<AppState>();
        while *state.recording_start.lock().unwrap() == Some(started) {
            let _ = app.emit_to("hud", "mic-level", level_to_ui(state.recorder.level()));
            tokio::time::sleep(Duration::from_millis(33)).await;
        }
    });
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
fn begin_capture(app: &AppHandle, state: &AppState, started: Instant, purpose: Purpose) -> Result<(), String> {
    let (device, sound_on, mute, beta) = {
        let c = state.config.lock().unwrap();
        (c.input_device.clone(), c.sound_feedback, c.mute_background, c.beta_features)
    };
    let fell_back = state
        .recorder
        .start_recording(device.as_deref())
        .map_err(|e| format!("Couldn't start recording: {}", e))?;
    if fell_back {
        report_error(
            app,
            format!("Microphone “{}” not found, using the system default", device.unwrap_or_default()),
        );
    }

    *state.recording_start.lock().unwrap() = Some(started);
    state.recording.store(true, Ordering::SeqCst);
    if sound_on {
        sound::play_start_tone();
    }
    if mute {
        // After the start chime, so the user still hears it.
        output::mute_after(&state.db.app_dir, Duration::from_millis(if sound_on { 250 } else { 0 }));
    }
    spawn_level_meter(app, started);
    let _ = app.emit("recording-status", true);
    update_hud(app);
    // The mic is already running; Accessibility lookups may take a moment.
    let context = frontmost::current_context();
    // Only commands (and voice activation, beta) use the selection.
    let selection = if purpose == Purpose::Command || beta { frontmost::selected_text() } else { None };
    *state.session.lock().unwrap() = Some(Session { purpose, context, selection, chime: sound_on });
    Ok(())
}

// Caller holds the mode lock. Returns the recording, its duration and its session.
fn end_capture(app: &AppHandle, state: &AppState) -> Result<(Recording, i64, Session), String> {
    let session = state.session.lock().unwrap().take().unwrap_or(Session {
        purpose: Purpose::Dictation,
        context: None,
        selection: None,
        chime: false,
    });
    let duration_ms = state
        .recording_start
        .lock()
        .unwrap()
        .take()
        .map(|s| s.elapsed().as_millis() as i64)
        .unwrap_or(0);
    output::restore(&state.db.app_dir);
    let recording = state.recorder.stop_recording(if session.chime { CHIME_TRIM_MS } else { 0 });
    // Only now: the stream is stopped, so the stop chime never lands in the audio.
    if session.chime {
        sound::play_stop_tone();
    }
    // processing before !recording, so the bar never flashes idle in between.
    if recording.is_ok() {
        job_started(app);
    }
    state.recording.store(false, Ordering::SeqCst);
    let _ = app.emit("recording-status", false);
    update_hud(app);
    recording
        .map(|r| (r, duration_ms, session))
        .map_err(|e| format!("Recording failed: {}", e))
}

// Caller holds the mode lock.
fn cancel_capture(app: &AppHandle, state: &AppState) {
    state.recorder.cancel();
    *state.recording_start.lock().unwrap() = None;
    *state.session.lock().unwrap() = None;
    output::restore(&state.db.app_dir);
    state.recording.store(false, Ordering::SeqCst);
    let _ = app.emit("recording-status", false);
    update_hud(app);
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
    // A dictation hotkey never stops a command recording (and vice versa); Esc/Enter apply to both.
    let current = state.session.lock().unwrap().as_ref().map(|s| s.purpose);
    let shared = matches!(event, HotkeyEvent::Cancel | HotkeyEvent::Submit);
    if *mode != Mode::Idle && !shared && current.is_some_and(|p| p != purpose) {
        return;
    }
    // Event timestamps, so a slow mic start doesn't shorten the measured hold.
    let held = state
        .recording_start
        .lock()
        .unwrap()
        .map_or(Duration::ZERO, |t| at.saturating_duration_since(t));
    let (next, effect) = transition(*mode, event, held);
    let not_started = state.pending_start.lock().unwrap().take().is_some();
    match effect {
        Effect::None => {
            // Still waiting to start (e.g. hold → hands-free upgrade): keep waiting.
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
            let app = app.clone();
            std::thread::spawn(move || {
                std::thread::sleep(HOLD_START_DELAY);
                let state = app.state::<AppState>();
                let mut mode = state.mode.lock().unwrap();
                if *state.pending_start.lock().unwrap() != Some(token) || *mode == Mode::Idle {
                    return;
                }
                *state.pending_start.lock().unwrap() = None;
                if let Err(e) = begin_capture(&app, &state, at, purpose) {
                    report_error(&app, e);
                    set_mode(&app, &state, &mut mode, Mode::Idle);
                }
            });
        }
        Effect::Start => {
            if let Err(e) = begin_capture(app, &state, at, purpose) {
                state.recording.store(false, Ordering::SeqCst);
                report_error(app, e);
                return;
            }
        }
        // Released/aborted/cancelled before the mic ever started: nothing to stop.
        Effect::Stop | Effect::Cancel if not_started => {
            *state.recording_start.lock().unwrap() = None;
        }
        Effect::Stop => match end_capture(app, &state) {
            Ok((recording, duration_ms, session)) => match session.purpose {
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

/// Dictionary words bias Whisper towards the right spelling.
fn vocab_prompt(db: &Database) -> Option<String> {
    let words: Vec<String> = db.get_dictionary().unwrap_or_default().into_iter().map(|d| d.phrase).collect();
    (!words.is_empty()).then(|| words.join(", "))
}

/// Punctuation or whitespace only (Whisper's output for silence).
fn is_blank(text: &str) -> bool {
    !text.chars().any(char::is_alphanumeric)
}

/// Snippet (whole dictation = trigger) or the formatting rule's cleanup + dictionary corrections.
async fn finish_text(db: &Database, raw: &str, config: &TranscriptionConfig, context: Option<&AppContext>) -> String {
    if let Some(snippet_content) = db.match_snippet(raw) {
        return expand_dynamic_tags(&snippet_content);
    }
    let mut text = apply_formatting(raw, config, &config.formatting_for(context)).await;
    for entry in db.get_dictionary().unwrap_or_default() {
        if !entry.phrase.trim().is_empty() && !entry.replacement.trim().is_empty() {
            text = text.replace(&entry.phrase, &expand_dynamic_tags(&entry.replacement));
        }
    }
    text
}

/// Transcribes, pastes and saves a dictation from `end_capture` (which counted the job);
/// failures are reported to the dictation bar.
async fn process_recording(app: AppHandle, recording: Recording, duration_ms: i64, session: Session) {
    match transcribe_and_paste(&app, recording, duration_ms, session).await {
        Ok(Some(text)) => {
            let _ = app.emit("transcription-completed", text);
        }
        Ok(None) => {}
        Err(e) => report_error(&app, e),
    }
    job_finished(&app);
}

/// Ok(None) = handed over to a command (voice activation), nothing pasted.
async fn transcribe_and_paste(
    app: &AppHandle,
    recording: Recording,
    duration_ms: i64,
    session: Session,
) -> Result<Option<String>, String> {
    let state = app.state::<AppState>();
    let (wav, has_speech) = finish_recording(recording).await?;
    if !has_speech {
        return Err("No speech detected".to_string());
    }
    let id = uuid::Uuid::new_v4().to_string();
    let audio_filename = format!("{}.wav", id);

    // Save audio file locally for history playback & retranscription
    let audio_file_path = state.db.app_dir.join("audio").join(&audio_filename);
    let audio_saved = match fs::write(&audio_file_path, &wav) {
        Ok(()) => true,
        Err(e) => {
            report_error(app, format!("Couldn't save the recording: {}", e));
            false
        }
    };

    let config = state.config.lock().unwrap().clone();
    let context = session.context.clone();
    let row = HistoryEntry {
        duration_ms,
        audio_filename: audio_saved.then_some(audio_filename),
        app_name: context.as_ref().map(|c| c.name.clone()),
        app_bundle_id: context.as_ref().map(|c| c.bundle_id.clone()),
        ..HistoryEntry::new(id.clone())
    };
    // Saved as "running" first: quitting mid-transcription leaves a record with Retry, not a lost dictation.
    let saved_early = audio_saved && state.db.insert_history(&HistoryEntry { status: "running".into(), ..row.clone() }).is_ok();

    let raw = match transcribe_raw(wav, &config, vocab_prompt(&state.db)).await {
        Ok(t) => t,
        Err(e) => {
            // Keep the audio: the record shows "Transcription failed" with Retry.
            if saved_early {
                let _ = state.db.update_history(&HistoryEntry { status: "failed".into(), error: Some(e.clone()), ..row });
                history_updated(app);
            }
            return Err(e);
        }
    };
    if is_blank(&raw) {
        if saved_early {
            let _ = state.db.delete_history_entry(&id);
        }
        if audio_saved {
            let _ = fs::remove_file(&audio_file_path);
        }
        return Err("No speech detected".to_string());
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

    let final_text = finish_text(&state.db, &raw, &config, context.as_ref()).await;

    // Auto-paste into the active app. Without Accessibility access macOS silently drops the
    // synthetic Cmd+V, so leave the text on the clipboard and tell the user.
    let can_paste = app.state::<HotkeyEngine>().status().permission_granted;
    let keep_in_clipboard = config.copy_to_clipboard || !can_paste;
    let text = final_text.clone();
    let paste_app = app.clone();
    match tauri::async_runtime::spawn_blocking(move || paste::paste_text(&paste_app, &text, keep_in_clipboard)).await {
        Ok(Ok(())) if !can_paste => report_error(
            app,
            "Copied to clipboard: allow OpenGlaido in Privacy & Security › Accessibility to paste automatically",
        ),
        Ok(Ok(())) => {}
        Ok(Err(e)) => report_error(app, format!("Couldn't paste: {}", e)),
        Err(e) => report_error(app, format!("Couldn't paste: {}", e)),
    }

    let entry = HistoryEntry { text: final_text.clone(), raw_text: raw, ..row };
    let saved = if saved_early { state.db.update_history(&entry) } else { state.db.insert_history(&entry) };
    if let Err(e) = saved {
        report_error(app, format!("Couldn't save to history: {}", e));
        if audio_saved {
            let _ = fs::remove_file(&audio_file_path);
        }
    }
    history_updated(app);

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
    begin_capture(&app, &state, Instant::now(), Purpose::Dictation).inspect_err(|e| report_error(&app, e))?;
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
    state: State<'_, AppState>,
) -> Result<TranscriptionConfig, String> {
    if !["bottom", "raised", "high"].contains(&new_config.bar_location.as_str()) {
        return Err(format!("Unknown dictation bar location “{}”", new_config.bar_location));
    }
    new_config.validate_formatting()?;
    // MCP prefs only change through the mcp_* commands; never let a stale UI copy overwrite them.
    let _serialized = state.config_write.lock().unwrap();
    let mut new_config = new_config;
    new_config.mcp_servers = state.config.lock().unwrap().mcp_servers.clone();

    let old = state.config.lock().unwrap().clone();
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
    for (account, new, old) in [
        ("api_key", &new_config.api_key, &old.api_key),
        ("search_api_key", &new_config.search_api_key, &old.search_api_key),
    ] {
        if new != old {
            if let Err(e) = secrets::set(account, new) {
                report_error(&app, e);
            }
        }
    }
    apply_ui_settings(&app, &new_config, Some(&old));

    let mode = state.mode.lock().unwrap();
    *state.config.lock().unwrap() = new_config.clone();
    engine.set_submit_enabled(submit_enabled(*mode, &new_config));
    drop(mode);
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
fn delete_history_entry(id: String, state: State<'_, AppState>) -> Result<(), String> {
    if let Ok(entry) = state.db.get_history_entry(&id) {
        if let Some(filename) = entry.audio_filename {
            let path = state.db.app_dir.join("audio").join(filename);
            let _ = fs::remove_file(path);
        }
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

    let raw = transcribe_raw(bytes, &config, vocab_prompt(&state.db)).await.and_then(|raw| {
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
    // Same rule as the original dictation (the URL isn't stored, so app rules only).
    let context = entry.app_bundle_id.clone().map(|bundle_id| AppContext {
        name: entry.app_name.clone().unwrap_or_else(|| bundle_id.clone()),
        bundle_id,
        url: None,
    });
    entry.text = finish_text(&state.db, &raw, &config, context.as_ref()).await;
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
fn set_tray_labels(toggle: String, dashboard: String, quit: String, items: State<'_, TrayItems>) -> Result<(), String> {
    for (item, text) in items.0.iter().zip([toggle, dashboard, quit]) {
        item.set_text(text).map_err(|e| e.to_string())?;
    }
    Ok(())
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
                config_write: Mutex::new(()),
                pending_start: Mutex::new(None),
                start_token: std::sync::atomic::AtomicU64::new(0),
            });

            // Create System Tray Menu
            let toggle_i = MenuItem::with_id(app, "toggle", "Toggle Dictation", true, None::<&str>)?;
            let dashboard_i = MenuItem::with_id(app, "dashboard", "Dashboard / Settings", true, None::<&str>)?;
            let quit_i = MenuItem::with_id(app, "quit", "Quit OpenGlaido", true, None::<&str>)?;
            let menu = Menu::with_items(app, &[&toggle_i, &dashboard_i, &quit_i])?;
            app.manage(TrayItems([toggle_i.clone(), dashboard_i.clone(), quit_i.clone()]));

            let _tray = TrayIconBuilder::with_id(TRAY_ID)
                .icon(app.default_window_icon().unwrap().clone())
                .tooltip("OpenGlaido")
                .menu(&menu)
                .on_menu_event(|app, event| {
                    match event.id().as_ref() {
                        "toggle" => {
                            let event = (HotkeyEvent::TogglePressed, Instant::now());
                            let _ = app.state::<AppState>().events.send(event);
                        }
                        "dashboard" => {
                            if let Some(main) = app.get_webview_window("main") {
                                let _ = main.show();
                                let _ = main.set_focus();
                            }
                        }
                        "quit" => {
                            app.exit(0);
                        }
                        _ => {}
                    }
                })
                .build(app)?;

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

            // The dictation bar (idle pill unless disabled) never takes clicks.
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
            get_history,
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
        ])
        .build(tauri::generate_context!())
        .expect("error while building tauri application")
        .run(|_app, _event| {
            if let tauri::RunEvent::Exit = _event {
                commands::shutdown(_app);
                if let Some(state) = _app.try_state::<AppState>() {
                    output::restore(&state.db.app_dir);
                }
            }
            #[cfg(target_os = "macos")]
            if let tauri::RunEvent::Reopen { .. } = _event {
                if let Some(main) = _app.get_webview_window("main") {
                    let _ = main.show();
                    let _ = main.set_focus();
                }
            }
        });
}

#[cfg(test)]
mod tests {
    use super::*;
    use HotkeyEvent::*;

    const SHORT: Duration = Duration::from_millis(100);
    const LONG: Duration = Duration::from_millis(900);

    #[test]
    fn hold_to_talk() {
        assert_eq!(transition(Mode::Idle, HoldPressed, Duration::ZERO), (Mode::Hold, Effect::Start));
        assert_eq!(transition(Mode::Hold, HoldReleased, LONG), (Mode::Idle, Effect::Stop));
        assert_eq!(transition(Mode::Hold, HoldReleased, SHORT), (Mode::Idle, Effect::Cancel));
        assert_eq!(transition(Mode::Hold, HoldAborted, LONG), (Mode::Idle, Effect::Cancel));
        assert_eq!(transition(Mode::Hold, Cancel, LONG), (Mode::Idle, Effect::Cancel));
        assert_eq!(transition(Mode::Hold, Submit, LONG), (Mode::Hold, Effect::None));
    }

    #[test]
    fn hands_free() {
        assert_eq!(transition(Mode::Idle, TogglePressed, Duration::ZERO), (Mode::HandsFree, Effect::Start));
        // Hold → hands-free keeps recording; the later hold release is ignored.
        assert_eq!(transition(Mode::Hold, TogglePressed, SHORT), (Mode::HandsFree, Effect::None));
        assert_eq!(transition(Mode::HandsFree, HoldReleased, LONG), (Mode::HandsFree, Effect::None));
        assert_eq!(transition(Mode::HandsFree, TogglePressed, LONG), (Mode::Idle, Effect::Stop));
        assert_eq!(transition(Mode::HandsFree, Submit, LONG), (Mode::Idle, Effect::Stop));
        assert_eq!(transition(Mode::HandsFree, Cancel, LONG), (Mode::Idle, Effect::Cancel));
    }

    #[test]
    fn hold_key_stops_hands_free_on_release_only() {
        // Fn alone: stop on release.
        assert_eq!(transition(Mode::HandsFree, HoldPressed, LONG), (Mode::HandsFreeHoldDown, Effect::None));
        assert_eq!(transition(Mode::HandsFreeHoldDown, HoldReleased, LONG), (Mode::Idle, Effect::Stop));
        // Fn+Space: stops once on the toggle; the trailing Fn release is a no-op when idle.
        assert_eq!(transition(Mode::HandsFreeHoldDown, TogglePressed, LONG), (Mode::Idle, Effect::Stop));
        assert_eq!(transition(Mode::Idle, HoldReleased, LONG), (Mode::Idle, Effect::None));
        // Fn + another key: keep dictating.
        assert_eq!(transition(Mode::HandsFreeHoldDown, HoldAborted, LONG), (Mode::HandsFree, Effect::None));
        assert_eq!(transition(Mode::HandsFreeHoldDown, Cancel, LONG), (Mode::Idle, Effect::Cancel));
    }

    #[test]
    fn idle_ignores_everything_but_start() {
        for ev in [HoldReleased, HoldAborted, Cancel, Submit] {
            assert_eq!(transition(Mode::Idle, ev, LONG), (Mode::Idle, Effect::None));
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
    fn level_mapping() {
        assert_eq!(level_to_ui(0.0), 0.0);
        assert_eq!(level_to_ui(-1.0), 0.0);
        assert_eq!(level_to_ui(1.0), 1.0);
        assert_eq!(level_to_ui(2.0), 1.0);
        assert_eq!(level_to_ui(0.001), 0.0); // -60 dBFS
        assert!((level_to_ui(0.1) - 0.6).abs() < 1e-5); // -20 dBFS
        assert!((level_to_ui(0.01) - 0.2).abs() < 1e-5); // -40 dBFS
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
