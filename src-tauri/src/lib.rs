pub mod audio;
pub mod db;
pub mod paste;
pub mod sound;
pub mod transcribe;

use audio::AudioRecorder;
use db::{Database, DictionaryEntry, HistoryEntry, SnippetEntry};
use transcribe::{transcribe_audio, TranscriptionConfig};

use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;
use tauri::menu::{Menu, MenuItem};
use tauri::tray::TrayIconBuilder;
use tauri::{AppHandle, Emitter, Manager, PhysicalPosition, State, WindowEvent};
use tauri_plugin_global_shortcut::{Code, GlobalShortcutExt, Modifiers, Shortcut, ShortcutState};

pub struct AppState {
    pub recorder: AudioRecorder,
    pub is_recording: AtomicBool,
    pub recording_start: Mutex<Option<Instant>>,
    pub config: Mutex<TranscriptionConfig>,
    pub db: Arc<Database>,
}

// Registered only while recording so Esc cancels, like Glaido.
fn esc_shortcut() -> Shortcut {
    Shortcut::new(None, Code::Escape)
}

// ponytail: primary monitor only; follow the cursor's monitor if multi-display users ask.
fn show_hud(app: &AppHandle) {
    let Some(hud) = app.get_webview_window("hud") else { return };
    if let (Ok(Some(monitor)), Ok(size)) = (hud.primary_monitor(), hud.outer_size()) {
        let area = monitor.work_area();
        let x = area.position.x + (area.size.width as i32 - size.width as i32) / 2;
        let y = area.position.y + area.size.height as i32 - size.height as i32;
        let _ = hud.set_position(PhysicalPosition::new(x, y));
    }
    let _ = hud.show();
}

fn hide_hud(app: &AppHandle) {
    if let Some(hud) = app.get_webview_window("hud") {
        let _ = hud.hide();
    }
}

#[tauri::command]
fn get_recording_state(state: State<'_, AppState>) -> bool {
    state.is_recording.load(Ordering::SeqCst)
}

#[tauri::command]
fn start_recording(app: AppHandle, state: State<'_, AppState>) -> Result<(), String> {
    if state.is_recording.load(Ordering::SeqCst) {
        return Ok(());
    }

    state.recorder.start_recording()?;
    state.is_recording.store(true, Ordering::SeqCst);
    *state.recording_start.lock().unwrap() = Some(Instant::now());

    let sound_on = state.config.lock().map(|c| c.sound_feedback).unwrap_or(false);
    if sound_on {
        sound::play_start_tone();
    }

    let _ = app.global_shortcut().register(esc_shortcut());
    show_hud(&app);

    let _ = app.emit("recording-status", true);
    Ok(())
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

#[tauri::command]
async fn stop_recording_and_process(
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<String, String> {
    if !state.is_recording.load(Ordering::SeqCst) {
        return Err("Not recording".to_string());
    }

    let sound_on = state.config.lock().map(|c| c.sound_feedback).unwrap_or(false);
    if sound_on {
        sound::play_stop_tone();
    }

    let duration_ms = {
        let mut start = state.recording_start.lock().unwrap();
        start.take().map(|s| s.elapsed().as_millis() as i64).unwrap_or(0)
    };

    let wav_result = state.recorder.stop_recording();
    state.is_recording.store(false, Ordering::SeqCst);
    let _ = app.global_shortcut().unregister(esc_shortcut());
    let _ = app.emit("processing-status", true);
    let _ = app.emit("recording-status", false);
    let wav_bytes = match wav_result {
        Ok(bytes) => bytes,
        Err(e) => {
            let _ = app.emit("processing-status", false);
            hide_hud(&app);
            return Err(e);
        }
    };

    let id = uuid::Uuid::new_v4().to_string();
    let audio_filename = format!("{}.wav", id);

    // Save audio file locally for history playback & retranscription
    let audio_file_path = state.db.app_dir.join("audio").join(&audio_filename);
    let _ = fs::write(&audio_file_path, &wav_bytes);

    let config = {
        let c = state.config.lock().map_err(|e| e.to_string())?;
        c.clone()
    };

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
            let _ = app.emit("processing-status", false);
            hide_hud(&app);
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

    // Auto-paste text into current active application
    let _ = paste::paste_text(&final_text);

    // Save to history
    let _ = state.db.insert_history(&id, &final_text, &transcribed_text, duration_ms, Some(&audio_filename));

    let _ = app.emit("processing-status", false);
    hide_hud(&app);
    let _ = app.emit("transcription-completed", &final_text);

    Ok(final_text)
}

#[tauri::command]
fn cancel_recording(app: AppHandle, state: State<'_, AppState>) -> Result<(), String> {
    if !state.is_recording.load(Ordering::SeqCst) {
        return Ok(());
    }

    state.recorder.cancel();
    state.is_recording.store(false, Ordering::SeqCst);
    *state.recording_start.lock().unwrap() = None;
    let _ = app.global_shortcut().unregister(esc_shortcut());
    hide_hud(&app);

    let _ = app.emit("recording-status", false);
    Ok(())
}

#[tauri::command]
fn get_config(state: State<'_, AppState>) -> Result<TranscriptionConfig, String> {
    let cfg = state.config.lock().map_err(|e| e.to_string())?;
    Ok(cfg.clone())
}

#[tauri::command]
fn save_config(new_config: TranscriptionConfig, state: State<'_, AppState>) -> Result<(), String> {
    let mut cfg = state.config.lock().map_err(|e| e.to_string())?;
    *cfg = new_config;
    Ok(())
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
        .plugin(
            tauri_plugin_global_shortcut::Builder::new()
                .with_handler(|app, shortcut, event| {
                    if event.state() != ShortcutState::Pressed {
                        return;
                    }
                    let is_esc = shortcut.matches(Modifiers::empty(), Code::Escape);
                    let app = app.clone();
                    // Off the handler stack: the plugin holds its shortcut lock while calling us,
                    // and start/stop/cancel (un)register Esc.
                    tauri::async_runtime::spawn(async move {
                        let state = app.state::<AppState>();
                        if is_esc {
                            let _ = cancel_recording(app.clone(), state);
                        } else if state.is_recording.load(Ordering::SeqCst) {
                            let _ = stop_recording_and_process(app.clone(), state).await;
                        } else {
                            let _ = start_recording(app.clone(), state);
                        }
                    });
                })
                .build(),
        )
        .setup(|app| {
            let app_handle = app.handle();
            let app_data_dir = app_handle
                .path()
                .app_data_dir()
                .unwrap_or_else(|_| PathBuf::from("./data"));

            let db = Database::new(app_data_dir).expect("Failed to initialize database");

            app.manage(AppState {
                recorder: AudioRecorder::new(),
                is_recording: AtomicBool::new(false),
                recording_start: Mutex::new(None),
                config: Mutex::new(TranscriptionConfig::default()),
                db: Arc::new(db),
            });

            // Register default global hotkey (CommandOrControl+Shift+Space)
            if let Ok(shortcut) = "CommandOrControl+Shift+Space".parse::<Shortcut>() {
                let _ = app.global_shortcut().register(shortcut);
            }

            // Create System Tray Menu
            let toggle_i = MenuItem::with_id(app, "toggle", "Toggle Dictation", true, None::<&str>)?;
            let dashboard_i = MenuItem::with_id(app, "dashboard", "Dashboard / Settings", true, None::<&str>)?;
            let quit_i = MenuItem::with_id(app, "quit", "Quit OpenGlaido", true, None::<&str>)?;
            let menu = Menu::with_items(app, &[&toggle_i, &dashboard_i, &quit_i])?;

            if let Some(hud) = app.get_webview_window("hud") {
                let _ = hud.set_ignore_cursor_events(true);
            }

            let _tray = TrayIconBuilder::new()
                .icon(app.default_window_icon().unwrap().clone())
                .tooltip("OpenGlaido")
                .menu(&menu)
                .on_menu_event(|app, event| {
                    match event.id().as_ref() {
                        "toggle" => {
                            let state = app.state::<AppState>();
                            if state.is_recording.load(Ordering::SeqCst) {
                                let app_c = app.clone();
                                tauri::async_runtime::spawn(async move {
                                    let st = app_c.state::<AppState>();
                                    let _ = stop_recording_and_process(app_c.clone(), st).await;
                                });
                            } else {
                                let _ = start_recording(app.clone(), state);
                            }
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

            Ok(())
        })
        .on_window_event(|window, event| {
            if let WindowEvent::CloseRequested { api, .. } = event {
                if window.label() == "main" {
                    api.prevent_close();
                    let _ = window.hide();
                }
            }
        })
        .invoke_handler(tauri::generate_handler![
            get_recording_state,
            start_recording,
            stop_recording_and_process,
            cancel_recording,
            get_config,
            save_config,
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
