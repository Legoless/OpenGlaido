pub mod audio;
pub mod db;
pub mod hotkeys;
pub mod paste;
pub mod sound;
pub mod transcribe;

use audio::AudioRecorder;
use db::{Database, DictionaryEntry, HistoryEntry, SnippetEntry};
use hotkeys::{HotkeyEngine, HotkeyEvent, HotkeyStatus};
use transcribe::{transcribe_audio, TranscriptionConfig};

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{channel, Sender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tauri::menu::{Menu, MenuItem};
use tauri::tray::TrayIconBuilder;
use tauri::{
    AppHandle, Emitter, Manager, PhysicalPosition, PhysicalRect, PhysicalSize, State, WindowEvent,
};
use tauri_plugin_autostart::{MacosLauncher, ManagerExt};

const TRAY_ID: &str = "main";
const CONFIG_FILE: &str = "config.json";
/// Hold-to-talk presses shorter than this are treated as accidental taps and cancelled.
const MIN_HOLD: Duration = Duration::from_millis(300);

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

/// Recording state machine (CONTRACT.md). `held` = time since the recording started.
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
    pub recording_start: Mutex<Option<Instant>>,
    pub config: Mutex<TranscriptionConfig>,
    pub db: Arc<Database>,
    /// Hotkey and tray events with the time they happened, handled in order on one worker thread.
    pub events: Sender<(HotkeyEvent, Instant)>,
}

/// Logs `msg` and shows it in the dictation bar.
fn report_error(app: &AppHandle, msg: impl AsRef<str>) {
    let msg = msg.as_ref();
    eprintln!("{}", msg);
    if let Err(e) = app.emit("dictation-error", msg) {
        eprintln!("Failed to emit dictation-error: {}", e);
    }
}

// Missing/invalid file → defaults (logged); missing fields → defaults via #[serde(default)].
fn load_config(path: &Path) -> TranscriptionConfig {
    match fs::read_to_string(path) {
        Ok(json) => serde_json::from_str(&json).unwrap_or_else(|e| {
            eprintln!("Invalid {}, using defaults: {}", path.display(), e);
            TranscriptionConfig::default()
        }),
        Err(e) => {
            eprintln!("No config at {} ({}), using defaults", path.display(), e);
            TranscriptionConfig::default()
        }
    }
}

fn write_config(path: &Path, config: &TranscriptionConfig) -> Result<(), String> {
    let json = serde_json::to_string_pretty(config).map_err(|e| e.to_string())?;
    fs::write(path, json).map_err(|e| e.to_string())
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
fn hud_origin(
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
    let engine = app.state::<HotkeyEngine>();
    engine.set_recording(next != Mode::Idle);
    let submit = submit_enabled(next, &state.config.lock().unwrap());
    engine.set_submit_enabled(submit);
}

// Caller holds the mode lock. `started` = when the user asked (opening the mic can take a while).
fn begin_capture(app: &AppHandle, state: &AppState, started: Instant) -> Result<(), String> {
    let (device, sound_on) = {
        let c = state.config.lock().unwrap();
        (c.input_device.clone(), c.sound_feedback)
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
    if sound_on {
        sound::play_start_tone();
    }
    spawn_level_meter(app, started);
    let _ = app.emit("recording-status", true);
    Ok(())
}

// Caller holds the mode lock. Returns the WAV and its duration.
fn end_capture(app: &AppHandle, state: &AppState) -> Result<(Vec<u8>, i64), String> {
    let sound_on = state.config.lock().map(|c| c.sound_feedback).unwrap_or(false);
    if sound_on {
        sound::play_stop_tone();
    }
    let duration_ms = state
        .recording_start
        .lock()
        .unwrap()
        .take()
        .map(|s| s.elapsed().as_millis() as i64)
        .unwrap_or(0);
    let wav = state.recorder.stop_recording();
    // processing before !recording, so the bar never flashes idle in between.
    if wav.is_ok() {
        let _ = app.emit("processing-status", true);
    }
    let _ = app.emit("recording-status", false);
    wav.map(|w| (w, duration_ms))
        .map_err(|e| format!("Recording failed: {}", e))
}

// Caller holds the mode lock.
fn cancel_capture(app: &AppHandle, state: &AppState) {
    state.recorder.cancel();
    *state.recording_start.lock().unwrap() = None;
    let _ = app.emit("recording-status", false);
}

// Runs on the hotkey worker thread, so blocking on the audio thread is fine here.
fn handle_event(app: &AppHandle, event: HotkeyEvent, at: Instant) {
    let state = app.state::<AppState>();
    let mut mode = state.mode.lock().unwrap();
    // Event timestamps, so a slow mic start doesn't shorten the measured hold.
    let held = state
        .recording_start
        .lock()
        .unwrap()
        .map_or(Duration::ZERO, |t| at.saturating_duration_since(t));
    let (next, effect) = transition(*mode, event, held);
    match effect {
        Effect::None => {}
        Effect::Start => {
            if let Err(e) = begin_capture(app, &state, at) {
                report_error(app, e);
                return;
            }
        }
        Effect::Stop => match end_capture(app, &state) {
            Ok((wav, duration_ms)) => {
                tauri::async_runtime::spawn(process_recording(app.clone(), wav, duration_ms));
            }
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

/// Transcribes, pastes and saves a recording from `end_capture` (which set processing-status);
/// failures are reported to the dictation bar.
async fn process_recording(app: AppHandle, wav_bytes: Vec<u8>, duration_ms: i64) -> Result<String, String> {
    let result = transcribe_and_paste(&app, wav_bytes, duration_ms).await;
    let _ = app.emit("processing-status", false);
    match &result {
        Ok(text) => {
            let _ = app.emit("transcription-completed", text);
        }
        Err(e) => report_error(&app, e),
    }
    result
}

async fn transcribe_and_paste(app: &AppHandle, wav_bytes: Vec<u8>, duration_ms: i64) -> Result<String, String> {
    let state = app.state::<AppState>();
    let id = uuid::Uuid::new_v4().to_string();
    let audio_filename = format!("{}.wav", id);

    // Save audio file locally for history playback & retranscription
    let audio_file_path = state.db.app_dir.join("audio").join(&audio_filename);
    let audio_saved = match fs::write(&audio_file_path, &wav_bytes) {
        Ok(()) => true,
        Err(e) => {
            report_error(app, format!("Couldn't save the recording: {}", e));
            false
        }
    };

    let config = state.config.lock().unwrap().clone();

    // Extract dictionary vocabulary words to bias Whisper recognition
    let dictionary_entries = state.db.get_dictionary().unwrap_or_default();
    let vocab_prompt = if !dictionary_entries.is_empty() {
        let words: Vec<String> = dictionary_entries.iter().map(|d| d.phrase.clone()).collect();
        Some(words.join(", "))
    } else {
        None
    };

    let transcribed_text = match transcribe_audio(wav_bytes, &config, vocab_prompt).await {
        Ok(t) => t,
        Err(e) => {
            // No history row for failed transcriptions, so don't leave the WAV behind.
            if audio_saved {
                let _ = fs::remove_file(&audio_file_path);
            }
            return Err(e);
        }
    };

    // 1. Check if whole phrase matches a snippet trigger (e.g. "my email" -> "john@example.com")
    let final_text = if let Some(snippet_content) = state.db.match_snippet(&transcribed_text) {
        expand_dynamic_tags(&snippet_content)
    } else {
        // 2. Otherwise apply dictionary replacements and expand dynamic tags
        let mut text = transcribed_text.clone();
        for entry in dictionary_entries {
            if !entry.phrase.trim().is_empty() && !entry.replacement.trim().is_empty() {
                let expanded_replacement = expand_dynamic_tags(&entry.replacement);
                text = text.replace(&entry.phrase, &expanded_replacement);
            }
        }
        text
    };

    // Auto-paste into the active app. Without Accessibility access macOS silently drops the
    // synthetic Cmd+V, so leave the text on the clipboard and tell the user.
    let can_paste = app.state::<HotkeyEngine>().status().permission_granted;
    let keep_in_clipboard = config.copy_to_clipboard || !can_paste;
    let text = final_text.clone();
    match tauri::async_runtime::spawn_blocking(move || paste::paste_text(&text, keep_in_clipboard)).await {
        Ok(Ok(())) if !can_paste => report_error(
            app,
            "Copied to clipboard: allow OpenGlaido in Privacy & Security › Accessibility to paste automatically",
        ),
        Ok(Ok(())) => {}
        Ok(Err(e)) => report_error(app, format!("Couldn't paste: {}", e)),
        Err(e) => report_error(app, format!("Couldn't paste: {}", e)),
    }

    // Save to history
    let audio = audio_saved.then_some(audio_filename.as_str());
    if let Err(e) = state.db.insert_history(&id, &final_text, &transcribed_text, duration_ms, audio) {
        report_error(app, format!("Couldn't save to history: {}", e));
        if audio_saved {
            let _ = fs::remove_file(&audio_file_path);
        }
    }

    Ok(final_text)
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
    begin_capture(&app, &state, Instant::now()).inspect_err(|e| report_error(&app, e))?;
    set_mode(&app, &state, &mut mode, Mode::HandsFree);
    Ok(())
}

#[tauri::command]
async fn stop_recording_and_process(
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<String, String> {
    let (wav_bytes, duration_ms) = {
        let mut mode = state.mode.lock().unwrap();
        if *mode == Mode::Idle {
            return Err("Not recording".to_string());
        }
        let capture = end_capture(&app, &state);
        set_mode(&app, &state, &mut mode, Mode::Idle);
        capture.inspect_err(|e| report_error(&app, e))?
    };
    process_recording(app, wav_bytes, duration_ms).await
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

    let old = state.config.lock().unwrap().clone();
    let engine = app.state::<HotkeyEngine>();
    engine.set_bindings(&new_config.hotkey_hold, &new_config.hotkey_toggle)?;
    let restore_hotkeys = || {
        if let Err(e) = engine.set_bindings(&old.hotkey_hold, &old.hotkey_toggle) {
            eprintln!("Failed to restore previous hotkeys: {}", e);
        }
    };
    if let Err(e) = set_launch_at_login(&app, new_config.launch_at_login) {
        restore_hotkeys();
        return Err(e);
    }
    if let Err(e) = write_config(&state.db.app_dir.join(CONFIG_FILE), &new_config) {
        restore_hotkeys();
        let _ = set_launch_at_login(&app, old.launch_at_login);
        return Err(format!("Couldn't save settings: {}", e));
    }
    apply_ui_settings(&app, &new_config, Some(&old));

    let mode = state.mode.lock().unwrap();
    *state.config.lock().unwrap() = new_config.clone();
    engine.set_submit_enabled(submit_enabled(*mode, &new_config));
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

#[tauri::command]
async fn retranscribe(id: String, state: State<'_, AppState>) -> Result<String, String> {
    let entry = state.db.get_history_entry(&id)?;
    let filename = entry
        .audio_filename
        .ok_or_else(|| "No audio available to retranscribe".to_string())?;
    let path = state.db.app_dir.join("audio").join(filename);
    let bytes = fs::read(path).map_err(|e| e.to_string())?;

    let config = {
        let c = state.config.lock().map_err(|e| e.to_string())?;
        c.clone()
    };

    let vocab_prompt = state.db.get_dictionary().ok().map(|entries| {
        entries.iter().map(|d| d.phrase.clone()).collect::<Vec<_>>().join(", ")
    });

    let new_text = transcribe_audio(bytes, &config, vocab_prompt).await?;
    Ok(new_text)
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
            let db = Database::new(app_data_dir).expect("Failed to initialize database");

            // The engine's callback must not block: queue events for one worker thread, which
            // handles them in order (and may block on the audio thread / main thread).
            let (events, rx) = channel::<(HotkeyEvent, Instant)>();
            let worker_app = app.handle().clone();
            std::thread::spawn(move || {
                for (event, at) in rx {
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
                recording_start: Mutex::new(None),
                config: Mutex::new(config.clone()),
                db: Arc::new(db),
                events,
            });

            // Create System Tray Menu
            let toggle_i = MenuItem::with_id(app, "toggle", "Toggle Dictation", true, None::<&str>)?;
            let dashboard_i = MenuItem::with_id(app, "dashboard", "Dashboard / Settings", true, None::<&str>)?;
            let quit_i = MenuItem::with_id(app, "quit", "Quit OpenGlaido", true, None::<&str>)?;
            let menu = Menu::with_items(app, &[&toggle_i, &dashboard_i, &quit_i])?;

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
            if let Err(e) = set_launch_at_login(app.handle(), config.launch_at_login) {
                report_error(app.handle(), e);
            }
            apply_ui_settings(app.handle(), &config, None);

            // The dictation bar is always visible (idle pill) and never takes clicks.
            if let Some(hud) = app.get_webview_window("hud") {
                if let Err(e) = hud.set_ignore_cursor_events(true).and_then(|_| hud.show()) {
                    eprintln!("Failed to show the dictation bar: {}", e);
                }
            }

            Ok(())
        })
        .on_window_event(|window, event| match event {
            WindowEvent::CloseRequested { api, .. } if window.label() == "main" => {
                api.prevent_close();
                let _ = window.hide();
            }
            // The hotkey recorder only lives while the main window has focus.
            WindowEvent::Focused(false) if window.label() == "main" => {
                window.state::<HotkeyEngine>().stop_capture();
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
            delete_snippet
        ])
        .build(tauri::generate_context!())
        .expect("error while building tauri application")
        .run(|_app, _event| {
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
