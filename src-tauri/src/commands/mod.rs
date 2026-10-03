//! Commands: spoken instructions answered in the command window, with built-in tools and local
//! MCP servers ("custom tools"). Recording is shared with dictation (lib.rs routes the command
//! hotkeys through the same state machine); a finished command recording lands in `run_recorded`.

pub mod llm;
pub mod mcp;
pub mod tools;

use crate::audio::Recording;
use crate::db::{HistoryEntry, Source};
use crate::frontmost::AppContext;
use crate::hotkeys::{HotkeyEngine, HotkeyEvent};
use crate::processing::{Cancellation, Job, CANCELLED};
use crate::transcribe::{transcribe_raw_cancellable, TranscriptionConfig};
use crate::{history_updated, report_error, AppState, Session};
use llm::Llm;
use serde::Serialize;
use serde_json::{json, Value};
use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant};
use tauri::{AppHandle, Emitter, Manager, PhysicalPosition, State};
use tokio::sync::oneshot;

pub use mcp::{McpServer, McpTool};
pub use tools::BuiltinTool;

const MAX_STEPS: usize = 6;
const APPROVAL_TIMEOUT: Duration = Duration::from_secs(60);

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct ToolStatus {
    pub name: String,
    /// English label (MCP tools: "server: tool"); the UI builds a translated one from name/action/detail.
    pub label: String,
    /// files_apps action (find/read/open/open_app), empty otherwise.
    pub action: String,
    /// The query or thing the tool works on, for the translated label.
    pub detail: String,
    pub status: String, // "running" | "done" | "failed"
}

#[derive(Debug, Clone, Serialize)]
pub struct Approval {
    pub request_id: String,
    pub server: String,
    pub tool: String,
    pub summary: String,
    pub expires_in_s: u64,
    pub allow_always: bool,
}

/// What the command window shows ("command-state" payload).
#[derive(Debug, Clone, Serialize, Default)]
pub struct Run {
    pub id: String,
    pub phase: String, // listening | transcribing | thinking | tool | streaming | done | error | approval
    pub instruction: String,
    pub has_selection: bool,
    pub text: String,
    pub tools: Vec<ToolStatus>,
    pub approval: Option<Approval>,
    pub error: Option<String>,
    pub refine_keys: String,
    /// Enter pastes / ⌘C copies (a finished answer, not the idle prompt).
    pub answer_ready: bool,
    #[serde(skip)]
    messages: Vec<Value>,
    /// URLs the conversation may read (user, public), carried into follow-up turns.
    #[serde(skip)]
    urls: (HashSet<String>, HashSet<String>),
    #[serde(skip)]
    generation: u64,
}

pub struct Commands {
    run: Mutex<Run>,
    /// Command window shown. Its lock also serializes key arming with show/hide.
    open: Mutex<bool>,
    generation: AtomicU64,
    /// Closing the window revokes private reads, even if it is reopened before they finish.
    private_epoch: AtomicU64,
    approvals: Mutex<HashMap<String, (u64, oneshot::Sender<String>)>>,
    pub mcp: mcp::Manager,
}

/// The language model from Settings › Model (commands defaults: temperature 0.3, 1024 tokens).
pub fn llm_from(app: &AppHandle, config: &TranscriptionConfig) -> Result<Llm, String> {
    if config.llm_source == "local" {
        crate::models::path_if_downloaded(app, &config.local_llm_model).ok_or("Download the language model in Settings › Model")?;
        return Ok(Llm::Local { app: app.clone(), model_id: config.local_llm_model.clone(), temperature: 0.3, max_tokens: 1024 });
    }
    cloud_llm(config)
}

fn cloud_llm(config: &TranscriptionConfig) -> Result<Llm, String> {
    match (config.llm_source.as_str(), &config.llm_endpoint_url, &config.llm_model_name) {
        ("cloud", Some(url), Some(model)) if !url.trim().is_empty() && !model.trim().is_empty() => Ok(Llm::Remote {
            url: url.trim().into(),
            model: model.trim().into(),
            api_key: config.llm_api_key.clone(),
            temperature: 0.3,
        }),
        _ => Err("Set up a language model in Settings › Model to use commands".into()),
    }
}

pub fn notify(app: &AppHandle, title: &str, body: &str) {
    use tauri_plugin_notification::NotificationExt;
    if let Err(e) = app.notification().builder().title(title).body(body).show() {
        eprintln!("Notification failed: {e}");
    }
}

/// Voice activation: "Hey Glaido, make this formal" → Some("make this formal"); the name alone
/// → Some(""); no name within the first two words → None.
pub fn wake_command(text: &str) -> Option<String> {
    const NAMES: [&str; 8] = ["glaido", "glido", "gladio", "claido", "glado", "glydo", "openglaido", "gleido"];
    let norm = |w: &str| w.chars().filter(|c| c.is_alphanumeric()).collect::<String>().to_lowercase();
    let words: Vec<&str> = text.split_whitespace().collect();
    let is_name = |i: usize| words.get(i).is_some_and(|w| NAMES.contains(&norm(w).as_str()));
    let is_open = |i: usize| words.get(i).is_some_and(|w| norm(w) == "open");
    // Name as word 1 or 2 ("Hey Glaido"), or "Open Glaido" as words 1–2 / 2–3.
    // (Word 2 being the name also covers "Open Glaido" and "Hey Glaido".)
    let rest_from = if is_name(0) {
        1
    } else if is_name(1) {
        2
    } else if is_open(1) && is_name(2) {
        3
    } else {
        return None;
    };
    let rest = words[rest_from.min(words.len())..].join(" ");
    let rest = rest.trim_start_matches(|c: char| c.is_ascii_punctuation() || c.is_whitespace());
    let mut chars = rest.chars();
    Some(match chars.next() {
        Some(first) => first.to_uppercase().chain(chars).collect(),
        None => String::new(),
    })
}

/// `tools` = false for a local model, which gets none.
fn system_prompt(context: Option<&AppContext>, tools: bool) -> String {
    let now = chrono::Local::now();
    let app = context.map_or(String::new(), |c| format!(" The user is in {}.", c.name));
    let tools = if tools {
        "Use tools when they help (exact maths and dates, the web, the open page, files, the user's own dictation history, OpenGlaido's documentation)."
    } else {
        "You have no tools (no web, files or history): answer from what you know, and say so when a request needs them."
    };
    format!(
        "You are OpenGlaido's command assistant. The user spoke an instruction (speech recognition may contain small errors).{app} \
         If a <selection> is given, it is the text they had selected, and instructions like \"make this shorter\" refer to it. \
         When the instruction writes or transforms text, reply with ONLY the resulting text, ready to paste: no preamble, no quotes, no explanation. \
         When it's a question, answer concisely; markdown is fine. {tools} Today is {}.",
        now.format("%A %Y-%m-%d %H:%M (%:z)")
    )
}

// ---- state + window ----

fn commands(app: &AppHandle) -> State<'_, Commands> {
    app.state::<Commands>()
}

fn emit(app: &AppHandle, run: &Run) {
    let _ = app.emit("command-state", run);
}

/// Applies `f` to the shown run if it is still `generation`, then emits it.
fn update(app: &AppHandle, generation: u64, f: impl FnOnce(&mut Run)) {
    let c = commands(app);
    {
        let mut run = c.run.lock().unwrap();
        if run.generation != generation || c.generation.load(Ordering::SeqCst) != generation {
            return;
        }
        f(&mut run);
        run.answer_ready = run.phase == "done" && !run.text.is_empty() && !run.messages.is_empty();
        emit(app, &run);
    }
    arm_keys(app);
}

/// Esc while the window is shown; Enter only with an answer or an approval card; ⌘C with an answer.
fn arm_keys(app: &AppHandle) {
    let c = commands(app);
    let open = c.open.lock().unwrap();
    let (ready, approval) = {
        let run = c.run.lock().unwrap();
        (run.answer_ready, run.approval.is_some())
    };
    app.state::<HotkeyEngine>().set_command_keys(*open, ready || approval, ready);
}

fn position_window(app: &AppHandle, window: &tauri::WebviewWindow) {
    let (Ok(Some(monitor)), Ok(size)) = (window.primary_monitor(), window.outer_size()) else { return };
    let bar_location = app.state::<AppState>().config.lock().unwrap().bar_location.clone();
    let scale = monitor.scale_factor();
    let hud = crate::hud_origin(monitor.work_area(), tauri::PhysicalSize::new((420.0 * scale) as u32, (72.0 * scale) as u32), scale, &bar_location);
    let area = monitor.work_area();
    let x = area.position.x + (area.size.width as i32 - size.width as i32) / 2;
    // Bottom edge 12 px above the pill (which is vertically centred in the 72 px bar window).
    let y = hud.y + ((72.0 - 38.0) / 2.0 * scale) as i32 - (12.0 * scale) as i32 - size.height as i32;
    let _ = window.set_position(PhysicalPosition::new(x, y.max(area.position.y)));
}

fn show_window(app: &AppHandle, generation: u64, cancellation: &Cancellation) {
    let Some(window) = app.get_webview_window("command") else { return };
    position_window(app, &window);
    let c = commands(app);
    let mut open = c.open.lock().unwrap();
    let run = c.run.lock().unwrap();
    if cancellation.is_cancelled() || run.generation != generation || c.generation.load(Ordering::SeqCst) != generation {
        return;
    }
    let _ = window.set_ignore_cursor_events(false);
    let _ = crate::show_passive(&window);
    *open = true;
    drop(run);
    drop(open);
    let _ = app.emit("command-window", json!({ "open": true }));
    arm_keys(app);
}

fn hide_window(app: &AppHandle) {
    if let Some(window) = app.get_webview_window("command") {
        let _ = window.hide();
    }
    *commands(app).open.lock().unwrap() = false;
    commands(app).private_epoch.fetch_add(1, Ordering::SeqCst);
    let _ = app.emit("command-window", json!({ "open": false }));
    // Closing declines anything still waiting for approval (the run itself carries on).
    deny_pending(app);
    arm_keys(app);
}

fn deny_pending(app: &AppHandle) {
    for (_, (_, tx)) in commands(app).approvals.lock().unwrap().drain() {
        let _ = tx.send("deny".into());
    }
}

fn deny_older_pending(app: &AppHandle, generation: u64) {
    // Dropping the sender resolves its receiver as denied; keep newer cards untouched.
    commands(app).approvals.lock().unwrap().retain(|_, (owner, _)| *owner >= generation);
}

fn deny_generation(approvals: &mut HashMap<String, (u64, oneshot::Sender<String>)>, generation: u64) {
    approvals.retain(|_, (owner, _)| *owner != generation);
}

/// Called while holding the shown-run lock, before changing any visible state.
fn reserve_generation(generation: &AtomicU64, expected: u64) -> Result<u64, String> {
    generation.compare_exchange(expected, expected + 1, Ordering::SeqCst, Ordering::SeqCst)
        .map(|_| expected + 1).map_err(|_| CANCELLED.into())
}

/// Cancellation of an old response must not close a newer command or decline its approval.
fn clear_cancelled_run(run: &mut Run, generation: u64, current_generation: u64) -> bool {
    if run.generation != generation || current_generation != generation {
        return false;
    }
    *run = Run { generation: generation + 1, ..Default::default() };
    true
}

fn save_run_history(app: &AppHandle, generation: u64, row: &HistoryEntry) {
    let c = commands(app);
    let run = c.run.lock().unwrap();
    // Refine reuses the entry ID: an older result must never replace its newer turn.
    if run.id == row.id && (run.generation != generation || c.generation.load(Ordering::SeqCst) != generation) {
        return;
    }
    let _ = app.state::<AppState>().db.update_history(row);
    drop(run);
    history_updated(app);
}

fn cancel_run(app: &AppHandle, generation: u64, history_id: &str) {
    if let Ok(mut row) = app.state::<AppState>().db.get_history_entry(history_id) {
        row.status = "failed".into();
        row.error = Some(CANCELLED.into());
        save_run_history(app, generation, &row);
    }
    let c = commands(app);
    let mut open = c.open.lock().unwrap();
    let mut run = c.run.lock().unwrap();
    let current = c.generation.load(Ordering::SeqCst);
    // Reserve invalidation before touching the window; a concurrently admitted run wins.
    if run.generation == generation && current == generation &&
        c.generation.compare_exchange(generation, generation + 1, Ordering::SeqCst, Ordering::SeqCst).is_ok() &&
        clear_cancelled_run(&mut run, generation, current)
    {
        if let Some(window) = app.get_webview_window("command") { let _ = window.hide(); }
        *open = false;
        c.private_epoch.fetch_add(1, Ordering::SeqCst);
        emit(app, &run);
        let _ = app.emit("command-window", json!({ "open": false }));
    }
    let mut approvals = c.approvals.lock().unwrap();
    deny_generation(&mut approvals, generation);
    drop(approvals);
    drop(run);
    drop(open);
    arm_keys(app);
}

fn is_open(app: &AppHandle) -> bool {
    *commands(app).open.lock().unwrap()
}

fn access_valid(open: bool, generation: u64, current_generation: u64, epoch: Option<u64>, current_epoch: u64) -> bool {
    generation == current_generation && epoch.is_none_or(|epoch| open && epoch == current_epoch)
}

fn check_run_access(app: &AppHandle, generation: u64, private_epoch: Option<u64>) -> Result<(), String> {
    let c = commands(app);
    if access_valid(is_open(app), generation, c.generation.load(Ordering::SeqCst), private_epoch, c.private_epoch.load(Ordering::SeqCst)) {
        Ok(())
    } else {
        Err("The command's access was cancelled.".into())
    }
}

fn refine_keys(app: &AppHandle) -> String {
    app.state::<AppState>().config.lock().unwrap().commands_hold.clone()
}

/// Called once from setup, after AppState and the HotkeyEngine are managed.
pub fn init(app: &AppHandle) {
    let dir = app.state::<AppState>().db.app_dir.clone();
    app.manage(Commands {
        run: Mutex::default(),
        open: Mutex::new(false),
        generation: AtomicU64::new(0),
        private_epoch: AtomicU64::new(0),
        approvals: Mutex::default(),
        mcp: mcp::Manager::new(&dir),
    });
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        let prefs = app.state::<AppState>().config.lock().unwrap().mcp_servers.clone();
        for (name, pref) in prefs {
            if pref.enabled {
                commands(&app).mcp.start(&name).await;
            }
        }
    });
}

/// Stops MCP server processes (app exit).
pub fn shutdown(app: &AppHandle) {
    if let Some(c) = app.try_state::<Commands>() {
        c.mcp.stop_all();
    }
}

/// Enter / Esc / ⌘C while the command window is open (hotkey worker thread).
pub fn handle_hotkey(app: &AppHandle, event: HotkeyEvent) {
    match event {
        HotkeyEvent::CommandEnter => {
            // Only the card that is on screen; never a leftover from an older run.
            let shown = commands(app).run.lock().unwrap().approval.as_ref().map(|a| a.request_id.clone());
            match shown {
                Some(id) => decide(app, &id, "once"),
                None => {
                    let _ = paste_answer(app);
                }
            }
        }
        HotkeyEvent::CommandEscape => hide_window(app),
        HotkeyEvent::CommandCopy => {
            let _ = copy_answer(app);
        }
        _ => {}
    }
}

// ---- flows ----

/// A finished recording from the commands hotkey (lib.rs worker / async runtime).
pub async fn run_recorded(app: AppHandle, recording: Recording, duration_ms: i64, mut session: Session) {
    let job = session.processing.take().unwrap_or_else(|| crate::job_started(&app));
    let cancellation = job.cancellation();
    let expected_generation = commands(&app).generation.load(Ordering::SeqCst);
    // Keep the audio preparation alive to preserve Retry even if the user cancels during it.
    let (wav, has_speech) = match crate::finish_recording(recording).await {
        Ok(done) => done,
        Err(e) => {
            if !cancellation.is_cancelled() { report_error(&app, e); }
            return;
        }
    };
    if !has_speech { return; }
    let c = commands(&app);
    let admission = cancellation.run_if_active(|| {
        let open = c.open.lock().unwrap();
        let mut run = c.run.lock().unwrap();
        let generation = reserve_generation(&c.generation, expected_generation)?;
        let refine = *open && run.answer_ready;
        let history_id = if refine { run.id.clone() } else { uuid::Uuid::new_v4().to_string() };
        let (messages, urls) = if refine { (std::mem::take(&mut run.messages), std::mem::take(&mut run.urls)) } else { Default::default() };
        *run = Run { id: history_id.clone(), phase: "transcribing".into(), refine_keys: refine_keys(&app), messages, urls, generation, ..Default::default() };
        emit(&app, &run);
        Ok((generation, history_id, refine))
    });
    let (generation, history_id, refine) = match admission {
        Ok(admitted) => admitted,
        Err(_) => {
            // Cancellation while preparing an older recording must not replace the current command.
            let id = uuid::Uuid::new_v4().to_string();
            let filename = format!("{id}.wav");
            let state = app.state::<AppState>();
            let saved = std::fs::write(state.db.app_dir.join("audio").join(&filename), &wav).is_ok();
            let row = HistoryEntry {
                kind: "command".into(), status: "failed".into(), error: Some(CANCELLED.into()),
                duration_ms, created_at: session.recorded_at.clone(), audio_filename: saved.then_some(filename),
                app_name: session.context.as_ref().map(|c| c.name.clone()),
                app_bundle_id: session.context.as_ref().map(|c| c.bundle_id.clone()),
                website: crate::history_website(session.context.as_ref()), ..HistoryEntry::new(id)
            };
            let _ = state.db.insert_history(&row);
            history_updated(&app);
            return;
        }
    };
    deny_older_pending(&app, generation);
    let state = app.state::<AppState>();
    let mut row = {
        let shown_run = c.run.lock().unwrap();
        if shown_run.generation != generation || c.generation.load(Ordering::SeqCst) != generation { return; }
        let existing = state.db.get_history_entry(&history_id).ok();
        let mut row = existing.clone().unwrap_or_else(|| HistoryEntry {
            kind: "command".into(), created_at: session.recorded_at.clone(), ..HistoryEntry::new(history_id.clone())
        });
        row.status = "running".into();
        row.error = None;
        row.duration_ms = duration_ms;
        let audio_filename = format!("{history_id}.wav");
        match std::fs::write(state.db.app_dir.join("audio").join(&audio_filename), &wav) {
            Ok(()) => row.audio_filename = Some(audio_filename),
            Err(e) if !cancellation.is_cancelled() => report_error(&app, format!("Couldn't save the recording: {e}")),
            Err(_) => {}
        }
        if existing.is_some() { let _ = state.db.update_history(&row); }
        else { let _ = state.db.insert_history(&row); }
        row
    };
    history_updated(&app);
    if cancellation.run(session.resolve_context()).await.is_err() {
        cancel_run(&app, generation, &history_id);
        return;
    }
    row.app_name = session.context.as_ref().map(|c| c.name.clone());
    row.app_bundle_id = session.context.as_ref().map(|c| c.bundle_id.clone());
    row.website = crate::history_website(session.context.as_ref());
    save_run_history(&app, generation, &row);
    show_window(&app, generation, &cancellation);
    let config = session.config.clone();
    let transcription = match session.live.take() {
        Some(live) => cancellation.run(live.finish()).await.and_then(|result| result),
        None => transcribe_raw_cancellable(&app, wav, &config, Vec::new(), cancellation.clone()).await,
    };
    if cancellation.is_cancelled() {
        cancel_run(&app, generation, &history_id);
        return;
    }
    let instruction = match transcription {
        Ok(t) if t.chars().any(char::is_alphanumeric) => t,
        Ok(_) => {
            row.status = "failed".into(); row.error = Some("No speech detected".into());
            save_run_history(&app, generation, &row);
            return update(&app, generation, |r| fail(r, "No speech detected"));
        }
        Err(e) => {
            row.status = "failed".into(); row.error = Some(e.clone());
            save_run_history(&app, generation, &row);
            return update(&app, generation, |r| fail(r, &e));
        }
    };
    row.text = instruction.clone();
    row.raw_text = instruction.clone();
    save_run_history(&app, generation, &row);
    // Accessibility only: a synthetic copy would clobber non-text clipboards or interrupt terminals.
    let selection = if refine { None } else { session.selection.clone() };
    run_command(&app, generation, instruction, selection, session.context, history_id, &cancellation).await;
}

fn fail(run: &mut Run, error: &str) {
    run.phase = "error".into();
    run.error = Some(error.into());
}

/// Voice activation: `instruction` (after the name) runs as a command for dictation row
/// `history_id` (already kind "command"). `selection` was captured at dictation start.
pub fn start_from_dictation(
    app: &AppHandle,
    instruction: String,
    selection: Option<String>,
    context: Option<AppContext>,
    history_id: String,
    job: Job,
) {
    let activity = match app.state::<crate::updater::UpdateState>().activity() {
        Ok(activity) => activity,
        Err(error) => return report_error(app, error),
    };
    let expected_generation = commands(app).generation.load(Ordering::SeqCst);
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        let _activity = activity;
        let _job = job;
        let cancellation = _job.cancellation();
        let c = commands(&app);
        let waiting = instruction.trim().is_empty();
        let admission = cancellation.run_if_active(|| {
            let mut run = c.run.lock().unwrap();
            let generation = reserve_generation(&c.generation, expected_generation)?;
            *run = Run {
                id: history_id.clone(),
                phase: if waiting { "done" } else { "thinking" }.into(),
                text: if waiting { "What can I help you with?".into() } else { String::new() },
                refine_keys: refine_keys(&app),
                generation,
                ..Default::default()
            };
            emit(&app, &run);
            Ok(generation)
        });
        let generation = match admission {
            Ok(generation) => generation,
            Err(_) => {
                // Nothing was admitted, so update only the originating History entry.
                if let Ok(mut row) = app.state::<AppState>().db.get_history_entry(&history_id) {
                    row.status = "failed".into(); row.error = Some(CANCELLED.into());
                    let _ = app.state::<AppState>().db.update_history(&row);
                    history_updated(&app);
                }
                return;
            }
        };
        deny_older_pending(&app, generation);
        show_window(&app, generation, &cancellation);
        if cancellation.is_cancelled() { cancel_run(&app, generation, &history_id); return; }
        if waiting {
            // Nothing to do yet: the next commands-hotkey recording starts a fresh command.
            let state = app.state::<AppState>();
            if let Ok(mut row) = state.db.get_history_entry(&history_id) {
                row.status = "ok".into();
                row.answer = Some("What can I help you with?".into());
                save_run_history(&app, generation, &row);
            }
            return;
        }
        run_command(&app, generation, instruction, selection, context, history_id, &cancellation).await;
    });
}

async fn run_command(
    app: &AppHandle,
    generation: u64,
    instruction: String,
    selection: Option<String>,
    context: Option<AppContext>,
    history_id: String,
    cancellation: &Cancellation,
) {
    if cancellation.is_cancelled() { cancel_run(app, generation, &history_id); return; }
    let config = app.state::<AppState>().config.lock().unwrap().clone();
    let (mut messages, urls) = {
        let c = commands(app);
        let run = c.run.lock().unwrap();
        if run.generation == generation { (run.messages.clone(), run.urls.clone()) } else { Default::default() }
    };
    if messages.is_empty() {
        messages.push(json!({"role":"system","content": system_prompt(context.as_ref(), config.llm_source != "local")}));
    }
    let content = match &selection {
        Some(sel) => format!("{instruction}\n\n<selection>\n{sel}\n</selection>"),
        None => instruction.clone(),
    };
    messages.push(json!({"role":"user","content": content}));
    update(app, generation, |r| {
        r.id = history_id.clone();
        r.instruction = instruction.clone();
        r.has_selection = selection.is_some() || r.has_selection;
        r.phase = "thinking".into();
        r.text.clear();
        r.tools.clear();
        r.error = None;
    });

    let mut tool_ctx = tools::ToolContext::new(app.clone(), config.clone(), context.as_ref().and_then(|c| c.url.clone()), selection, &instruction);
    tool_ctx.inherit_urls(urls);
    let result = cancellation.run(agent_loop(app, generation, &config, &tool_ctx, &mut messages, cancellation)).await.and_then(|result| result);
    if cancellation.is_cancelled() { cancel_run(app, generation, &history_id); return; }

    // Delivering a late answer/error or notification must be atomic with the user's cancellation.
    let delivered = cancellation.run_if_active(|| {
        let state = app.state::<AppState>();
        let mut row = state.db.get_history_entry(&history_id).unwrap_or_else(|_| HistoryEntry {
            kind: "command".into(),
            text: instruction.clone(),
            raw_text: instruction.clone(),
            ..HistoryEntry::new(history_id.clone())
        });
        let still_shown = is_open(app) && commands(app).run.lock().unwrap().generation == generation;
        match result {
            Ok((answer, sources)) => {
                messages.push(json!({"role":"assistant","content": answer}));
                row.status = "ok".into();
                row.answer = Some(answer.clone());
                row.error = None;
                row.sources = sources;
                update(app, generation, |r| {
                    r.phase = "done".into();
                    r.text = answer;
                    r.messages = messages;
                    r.urls = tool_ctx.urls();
                });
            }
            Err(e) => {
                row.status = "failed".into();
                row.error = Some(e.clone());
                update(app, generation, |r| fail(r, &e));
            }
        }
        save_run_history(app, generation, &row);
        if !still_shown && commands(app).generation.load(Ordering::SeqCst) == generation {
            let title = if row.status == "ok" { "Command finished" } else { "Command failed" };
            notify(app, title, &instruction);
        }
        Ok(())
    });
    if delivered.is_err() { cancel_run(app, generation, &history_id); }

}

/// Streams completions, running tool calls between them, until the model answers.
async fn agent_loop(
    app: &AppHandle,
    generation: u64,
    config: &TranscriptionConfig,
    ctx: &tools::ToolContext,
    messages: &mut Vec<Value>,
    cancellation: &Cancellation,
) -> Result<(String, Vec<Source>), String> {
    let llm = llm_from(app, config)?;
    let mcp = &commands(app).mcp;
    // A local model can't call tools; system_prompt told it so.
    let mut schemas = Vec::new();
    if !llm.is_local() {
        schemas = tools::schemas(config);
        schemas.extend(mcp.schemas(&config.mcp_servers));
    }
    let mut sources = Vec::new();
    let mut last_emit = Instant::now();
    let mut private_epoch = None;
    for step in 0..MAX_STEPS {
        cancellation.check()?;
        check_run_access(app, generation, private_epoch)?;
        // The last step gets no tools, so the model has to answer.
        let offered: &[Value] = if step + 1 == MAX_STEPS { &[] } else { &schemas };
        update(app, generation, |r| r.text.clear());
        let completion = llm
            .stream(messages, offered, |delta| {
                let c = commands(app);
                let mut run = c.run.lock().unwrap();
                if cancellation.is_cancelled() || run.generation != generation || c.generation.load(Ordering::SeqCst) != generation {
                    return;
                }
                run.phase = "streaming".into();
                run.text.push_str(delta);
                if last_emit.elapsed() > Duration::from_millis(40) {
                    emit(app, &run);
                    last_emit = Instant::now();
                }
            })
            .await?;
        cancellation.check()?;
        check_run_access(app, generation, private_epoch)?;
        if completion.tool_calls.is_empty() {
            return Ok((completion.text.trim().to_string(), sources));
        }
        messages.push(llm::assistant_tool_message(&completion));
        for call in &completion.tool_calls {
            let args: Value = serde_json::from_str(&call.arguments).unwrap_or_else(|_| json!({}));
            if tools::requires_approval(&call.name, &args) {
                private_epoch.get_or_insert_with(|| commands(app).private_epoch.load(Ordering::SeqCst));
            }
            let output = run_tool(app, generation, ctx, &call.name, &args, &mut sources, cancellation).await;
            // Never pass a private result to the next model request after close/supersession.
            cancellation.check()?;
            check_run_access(app, generation, private_epoch)?;
            messages.push(json!({"role":"tool","tool_call_id": call.id, "content": output}));
        }
    }
    Err("The command took too many steps".into())
}

async fn run_tool(
    app: &AppHandle,
    generation: u64,
    ctx: &tools::ToolContext,
    name: &str,
    args: &Value,
    sources: &mut Vec<Source>,
    cancellation: &Cancellation,
) -> String {
    let config = &ctx.config;
    if cancellation.is_cancelled() { return CANCELLED.into(); }
    if let Err(e) = check_run_access(app, generation, None) {
        return format!("Error: {e}");
    }
    let mcp = &commands(app).mcp;
    let resolved = (!tools::is_builtin(name)).then(|| mcp.resolve(&config.mcp_servers, name)).flatten();
    let label = match &resolved {
        Some((server, tool, _)) => format!("{server}: {tool}"),
        None => tools::label(name, args),
    };
    update(app, generation, |r| {
        r.phase = "tool".into();
        let arg = |k: &str| args.get(k).and_then(Value::as_str).unwrap_or("").to_string();
        let detail = if arg("query").is_empty() { arg("question") } else { arg("query") };
        r.tools.push(ToolStatus { name: name.into(), label: label.clone(), action: arg("action"), detail, status: "running".into() });
    });
    let result: Result<String, String> = if tools::is_builtin(name) {
        let private_epoch = tools::requires_approval(name, args).then(|| commands(app).private_epoch.load(Ordering::SeqCst));
        let approved = if private_epoch.is_some() {
            ask_approval(app, generation, "OpenGlaido · this call only", name, args, false, cancellation).await == "once"
        } else {
            false
        };
        match cancellation.check().and_then(|()| check_run_access(app, generation, private_epoch)) {
            Err(e) => Err(e),
            Ok(()) => match tools::run(ctx, name, args, approved).await {
                Err(e) => Err(e),
                Ok(r) => check_run_access(app, generation, private_epoch).map(|()| {
                    sources.extend(r.sources);
                    r.content
                }),
            },
        }
    } else if let Some((server, tool, policy)) = resolved {
        let decision = if policy == "ask" { ask_approval(app, generation, &server, &tool, args, true, cancellation).await } else { policy.clone() };
        match decision.as_str() {
            "once" | "auto" if !cancellation.is_cancelled() && check_run_access(app, generation, None).is_ok() => mcp.call(&server, &tool, args.clone()).await,
            _ => Ok("The user declined this tool call.".into()),
        }
    } else {
        Err(format!("Unknown tool {name}"))
    };
    let status = if result.is_ok() { "done" } else { "failed" };
    update(app, generation, |r| {
        if let Some(t) = r.tools.iter_mut().rev().find(|t| t.name == name && t.status == "running") {
            t.status = status.into();
        }
    });
    result.unwrap_or_else(|e| format!("Error: {e}"))
}

fn approval_decision(decision: &str, persistent: bool, valid: bool) -> &'static str {
    match (decision, persistent, valid) {
        ("always", true, true) => "auto",
        ("once" | "always", _, true) => "once",
        _ => "deny",
    }
}

/// Enter = once; Esc/close/60 s = deny. Only MCP can persist "always" decisions.
async fn ask_approval(app: &AppHandle, generation: u64, server: &str, tool: &str, args: &Value, persistent: bool, cancellation: &Cancellation) -> String {
    let c = commands(app);
    let private_epoch = c.private_epoch.load(Ordering::SeqCst);
    let request_id = uuid::Uuid::new_v4().to_string();
    let (tx, rx) = oneshot::channel();
    // All of it: padding benign fields must not push a recipient or path out of view.
    let summary = serde_json::to_string_pretty(args).unwrap_or_else(|_| args.to_string());
    {
        // Register atomically with close/new-run cancellation: a hidden card must never wait.
        let open = c.open.lock().unwrap();
        let mut run = c.run.lock().unwrap();
        if cancellation.is_cancelled() || !*open || run.generation != generation || c.generation.load(Ordering::SeqCst) != generation {
            return "deny".into();
        }
        c.approvals.lock().unwrap().insert(request_id.clone(), (generation, tx));
        run.phase = "approval".into();
        run.approval = Some(Approval {
            request_id: request_id.clone(),
            server: server.into(),
            tool: tool.into(),
            summary,
            expires_in_s: APPROVAL_TIMEOUT.as_secs(),
            allow_always: persistent,
        });
        emit(app, &run);
    }
    arm_keys(app);
    let decision = match cancellation.run(tokio::time::timeout(APPROVAL_TIMEOUT, rx)).await {
        Ok(Ok(Ok(d))) => d,
        _ => "deny".into(),
    };
    c.approvals.lock().unwrap().remove(&request_id);
    update(app, generation, |r| {
        r.approval = None;
        r.phase = "tool".into();
    });
    let decision = approval_decision(&decision, persistent, !cancellation.is_cancelled() && check_run_access(app, generation, Some(private_epoch)).is_ok());
    if decision == "auto" {
        let (server, tool) = (server.to_string(), tool.to_string());
        if let Err(e) = crate::update_config(app, |c| {
            c.mcp_servers.entry(server).or_default().tool_policies.insert(tool, "auto".into());
        }) {
            eprintln!("{e}");
        }
    }
    decision.into()
}

fn decide(app: &AppHandle, request_id: &str, decision: &str) {
    let c = commands(app);
    let open = c.open.lock().unwrap();
    let run = c.run.lock().unwrap();
    if !*open || run.generation != c.generation.load(Ordering::SeqCst) || run.approval.as_ref().is_none_or(|a| a.request_id != request_id) {
        return;
    }
    if let Some((_, tx)) = c.approvals.lock().unwrap().remove(request_id) {
        let _ = tx.send(decision.into());
    };
}

fn current_answer(app: &AppHandle) -> Option<String> {
    let c = commands(app);
    let run = c.run.lock().unwrap();
    run.answer_ready.then(|| run.text.clone())
}

/// What gets pasted: the answer without markdown markup (**bold**, `code`, # headings, fences);
/// list bullets and line breaks stay.
pub fn plain_text(markdown: &str) -> String {
    let mut out = Vec::new();
    for line in markdown.lines() {
        if line.trim_start().starts_with("```") {
            continue;
        }
        // "## Title" → "Title" (a heading needs a space after its #s; "#hashtag" stays).
        let hashes = line.len() - line.trim_start_matches('#').len();
        let line = match line[hashes..].strip_prefix(' ') {
            Some(rest) if (1..=6).contains(&hashes) => rest,
            _ => line,
        };
        let line = line.replace("**", "").replace("__", "").replace('`', "");
        out.push(line);
    }
    out.join("\n").trim().to_string()
}

fn paste_answer(app: &AppHandle) -> Result<(), String> {
    let activity = app.state::<crate::updater::UpdateState>().activity()?;
    let answer = plain_text(&current_answer(app).ok_or("No answer to paste yet")?);
    hide_window(app);
    let keep = app.state::<AppState>().config.lock().unwrap().copy_to_clipboard;
    let app = app.clone();
    std::thread::spawn(move || {
        let _activity = activity;
        if let Err(e) = crate::paste::paste_text(&app, &answer, keep) {
            report_error(&app, format!("Couldn't paste: {e}"));
        }
    });
    Ok(())
}

fn copy_answer(app: &AppHandle) -> Result<(), String> {
    let answer = plain_text(&current_answer(app).ok_or("No answer to copy yet")?);
    arboard::Clipboard::new().and_then(|mut c| c.set_text(answer)).map_err(|e| e.to_string())
}

// ---- Tauri commands ----

#[tauri::command]
pub fn get_builtin_tools(state: State<'_, AppState>) -> Vec<BuiltinTool> {
    tools::builtin_tools(&state.config.lock().unwrap())
}

#[tauri::command]
pub async fn command_paste(app: AppHandle) -> Result<(), String> {
    paste_answer(&app)
}

#[tauri::command]
pub async fn command_copy(app: AppHandle) -> Result<(), String> {
    copy_answer(&app)
}

#[tauri::command]
pub async fn command_close(app: AppHandle) -> Result<(), String> {
    hide_window(&app);
    Ok(())
}

/// `decision`: "once" | "always" | "deny".
#[tauri::command]
pub async fn command_approve(app: AppHandle, request_id: String, decision: String) -> Result<(), String> {
    if !["once", "always", "deny"].contains(&decision.as_str()) {
        return Err(format!("Unknown decision {decision}"));
    }
    decide(&app, &request_id, &decision);
    Ok(())
}

#[tauri::command]
pub fn mcp_list_servers(app: AppHandle) -> Vec<McpServer> {
    let prefs = app.state::<AppState>().config.lock().unwrap().mcp_servers.clone();
    commands(&app).mcp.list(&prefs)
}

#[tauri::command]
pub async fn mcp_import_folder(app: AppHandle, path: String) -> Result<Vec<String>, String> {
    commands(&app).mcp.import_folder(std::path::Path::new(&path))
}

#[tauri::command]
pub async fn mcp_set_enabled(app: AppHandle, name: String, enabled: bool) -> Result<(), String> {
    let key = name.clone();
    crate::update_config(&app, |c| c.mcp_servers.entry(key).or_default().enabled = enabled)?;
    let mcp = &commands(&app).mcp;
    if enabled {
        mcp.start(&name).await;
    } else {
        mcp.stop(&name);
    }
    Ok(())
}

// Async: update_config takes config_write, which save_config holds while it waits on the main thread.
#[tauri::command]
pub async fn mcp_set_tool_policy(app: AppHandle, name: String, tool: String, policy: String) -> Result<(), String> {
    if !["auto", "ask", "deny"].contains(&policy.as_str()) {
        return Err(format!("Unknown policy {policy}"));
    }
    crate::update_config(&app, |c| {
        c.mcp_servers.entry(name).or_default().tool_policies.insert(tool, policy);
    })
}

#[tauri::command]
pub async fn mcp_refresh(app: AppHandle, name: String) -> Result<(), String> {
    commands(&app).mcp.start(&name).await;
    Ok(())
}

#[tauri::command]
pub async fn mcp_delete(app: AppHandle, name: String) -> Result<(), String> {
    commands(&app).mcp.delete(&name)?;
    crate::update_config(&app, |c| {
        c.mcp_servers.remove(&name);
    })
}

#[tauri::command]
pub fn mcp_open_config(app: AppHandle) -> Result<(), String> {
    let mcp = &commands(&app).mcp;
    if !mcp.config_path().exists() {
        mcp.write(&mcp.read())?;
    }
    open_path(mcp.config_path())
}

#[tauri::command]
pub fn mcp_reveal_folder(path: String) -> Result<(), String> {
    open_path(std::path::Path::new(&path))
}

/// Opens a file or folder with its default app (no shell; ShellExecute on Windows).
fn open_path(path: &std::path::Path) -> Result<(), String> {
    tauri_plugin_opener::open_path(path, None::<&str>).map_err(|e| format!("Couldn't open {}: {e}", path.display()))
}

#[derive(Debug, Clone, Serialize)]
pub struct RuntimeCheck {
    pub found: bool,
    pub path: Option<String>,
    pub install_command: String,
}

/// `language`: "python" | "typescript".
#[tauri::command]
pub fn mcp_check_runtime(language: String) -> Result<RuntimeCheck, String> {
    let path = mcp::find_executable(mcp::runtime_for(&language));
    Ok(RuntimeCheck {
        found: path.is_some(),
        path: path.map(|p| p.display().to_string()),
        install_command: mcp::install_command(&language).into(),
    })
}

#[derive(Debug, Clone, Serialize)]
pub struct NewServer {
    pub folder: String,
    pub prompt: String,
}

#[tauri::command]
pub async fn mcp_new_server(app: AppHandle, location: String, language: String, name: String) -> Result<NewServer, String> {
    let name = name.trim().to_string();
    if name.is_empty() {
        return Err("Give the server a name".into());
    }
    let home = std::env::var("HOME").or_else(|_| std::env::var("USERPROFILE")).unwrap_or_default();
    let location = location.trim();
    let location = match location.strip_prefix('~') {
        Some(rest) => format!("{home}{rest}"),
        None => location.to_string(),
    };
    let slug: String = name.chars().map(|c| if c.is_ascii_alphanumeric() { c.to_ascii_lowercase() } else { '-' }).collect();
    let mut folder = std::path::Path::new(&location).join(&slug);
    let mut n = 2;
    while folder.exists() {
        folder = std::path::Path::new(&location).join(format!("{slug}-{n}"));
        n += 1;
    }
    mcp::scaffold(&folder, &language, &name)?;
    commands(&app).mcp.import_folder(&folder)?;
    Ok(NewServer { folder: folder.display().to_string(), prompt: mcp::agent_prompt(&folder, &language, &name) })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn private_approval_cannot_persist_or_outlive_its_visible_run() {
        assert_eq!(approval_decision("once", false, true), "once");
        assert_eq!(approval_decision("always", false, true), "once");
        assert_eq!(approval_decision("always", true, true), "auto");
        for decision in ["deny", "", "auto", "forged"] {
            assert_eq!(approval_decision(decision, false, true), "deny");
        }
        for decision in ["once", "always"] {
            for (open, generation, epoch) in [(false, 4, 8), (true, 5, 8), (true, 4, 9)] {
                let valid = access_valid(open, 4, generation, Some(8), epoch);
                assert!(!valid);
                assert_eq!(approval_decision(decision, false, valid), "deny");
                assert_eq!(approval_decision(decision, true, valid), "deny");
            }
        }
        assert!(access_valid(true, 4, 4, Some(8), 8));
        // Background safe tools keep running; superseded runs do not.
        assert!(access_valid(false, 4, 4, None, 9));
        assert!(!access_valid(true, 4, 5, None, 9));
    }

    #[test]
    fn admission_serializes_with_cancellation_and_rejects_out_of_order_preparation() {
        use std::sync::{Arc, Barrier};
        let cancellation = Cancellation::default();
        let generation = Arc::new(AtomicU64::new(4));
        let barrier = Arc::new(Barrier::new(2));
        let preparation = std::thread::spawn({
            let cancellation = cancellation.clone();
            let generation = generation.clone();
            let barrier = barrier.clone();
            move || {
                let expected = generation.load(Ordering::SeqCst);
                barrier.wait(); // The old recording is still preparing.
                barrier.wait(); // Cancellation and a newer admission have won.
                cancellation.run_if_active(|| reserve_generation(&generation, expected))
            }
        });
        barrier.wait();
        cancellation.cancel();
        assert_eq!(reserve_generation(&generation, 4).unwrap(), 5);
        barrier.wait();
        assert_eq!(preparation.join().unwrap().unwrap_err(), CANCELLED);
        assert_eq!(generation.load(Ordering::SeqCst), 5);
        // Out-of-order preparation is refused even if its token was never canceled.
        let still_active = Cancellation::default();
        assert_eq!(still_active.run_if_active(|| reserve_generation(&generation, 4)).unwrap_err(), CANCELLED);
        assert_eq!(generation.load(Ordering::SeqCst), 5);
        assert_eq!(still_active.run_if_active(|| reserve_generation(&generation, 5)).unwrap(), 6);
    }

    #[tokio::test]
    async fn cancelled_command_cannot_clear_a_newer_turn_or_decline_its_approval() {
        let mut run = Run {
            id: "new-command".into(), generation: 5, phase: "streaming".into(), text: "New answer".into(),
            ..Default::default()
        };
        assert!(!clear_cancelled_run(&mut run, 4, 5));
        assert_eq!(run.id, "new-command");
        assert_eq!(run.text, "New answer");
        assert!(!clear_cancelled_run(&mut run, 5, 6));
        let (old_tx, old_rx) = oneshot::channel();
        let (new_tx, new_rx) = oneshot::channel();
        let mut approvals = HashMap::from([("old".into(), (4, old_tx)), ("new".into(), (5, new_tx))]);
        deny_generation(&mut approvals, 4);
        assert!(old_rx.await.is_err());
        approvals.remove("new").unwrap().1.send("once".into()).unwrap();
        assert_eq!(new_rx.await.unwrap(), "once");
        assert!(clear_cancelled_run(&mut run, 5, 5));
        assert_eq!(run.generation, 6);
        assert!(run.text.is_empty() && !run.answer_ready && run.approval.is_none());
    }

    #[test]
    fn wake_phrase() {
        assert_eq!(wake_command("Glaido, make this more formal.").as_deref(), Some("Make this more formal."));
        assert_eq!(wake_command("Hey Glaido, what's a better word for important?").as_deref(), Some("What's a better word for important?"));
        assert_eq!(wake_command("hey gladio translate this to French").as_deref(), Some("Translate this to French"));
        assert_eq!(wake_command("Open Glaido, summarize this").as_deref(), Some("Summarize this"));
        assert_eq!(wake_command("Hey Glaido.").as_deref(), Some(""));
        assert_eq!(wake_command("Glaido").as_deref(), Some(""));
        // Only within the first two words; otherwise it's ordinary dictation.
        assert_eq!(wake_command("I told my friend about Glaido yesterday"), None);
        assert_eq!(wake_command("Send the report to Glaido"), None);
        assert_eq!(wake_command(""), None);
    }

    #[test]
    fn answers_paste_as_plain_text() {
        assert_eq!(plain_text("The answer is **148.575**."), "The answer is 148.575.");
        assert_eq!(plain_text("#hashtag stays"), "#hashtag stays");
        assert_eq!(plain_text("## Title\n\n- one\n- `two`\n```rust\nfn x() {}\n```"), "Title\n\n- one\n- two\nfn x() {}");
    }

    #[test]
    fn cloud_llm_needs_source_endpoint_and_model() {
        let mut cfg = TranscriptionConfig { api_key: "stt".into(), llm_api_key: "chat".into(), ..Default::default() };
        match cloud_llm(&cfg) {
            Ok(Llm::Remote { api_key, temperature, .. }) => assert_eq!((api_key.as_str(), temperature), ("chat", 0.3)),
            _ => panic!("expected a cloud model"),
        }
        cfg.llm_source = "off".into();
        assert!(matches!(cloud_llm(&cfg), Err(e) if e.starts_with("Set up a language model")));
        cfg.llm_source = "cloud".into();
        cfg.llm_model_name = Some(" ".into());
        assert!(cloud_llm(&cfg).is_err());
    }

    #[test]
    fn prompt_mentions_app_and_date() {
        let ctx = AppContext { bundle_id: "com.apple.mail".into(), name: "Mail".into(), url: None };
        let p = system_prompt(Some(&ctx), true);
        assert!(p.contains("in Mail") && p.contains(&chrono::Local::now().format("%Y-%m-%d").to_string()));
        assert!(p.contains("Use tools") && system_prompt(None, false).contains("You have no tools"));
    }
}
