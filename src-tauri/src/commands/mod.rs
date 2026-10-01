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
use crate::transcribe::{transcribe_raw, TranscriptionConfig};
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
    approvals: Mutex<HashMap<String, oneshot::Sender<String>>>,
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
        if run.generation != generation {
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

fn show_window(app: &AppHandle) {
    let Some(window) = app.get_webview_window("command") else { return };
    position_window(app, &window);
    let _ = window.set_ignore_cursor_events(false);
    let _ = crate::show_passive(&window);
    *commands(app).open.lock().unwrap() = true;
    let _ = app.emit("command-window", json!({ "open": true }));
    arm_keys(app);
}

fn hide_window(app: &AppHandle) {
    if let Some(window) = app.get_webview_window("command") {
        let _ = window.hide();
    }
    *commands(app).open.lock().unwrap() = false;
    let _ = app.emit("command-window", json!({ "open": false }));
    // Closing declines anything still waiting for approval (the run itself carries on).
    deny_pending(app);
    arm_keys(app);
}

fn deny_pending(app: &AppHandle) {
    for (_, tx) in commands(app).approvals.lock().unwrap().drain() {
        let _ = tx.send("deny".into());
    }
}

fn is_open(app: &AppHandle) -> bool {
    *commands(app).open.lock().unwrap()
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
pub async fn run_recorded(app: AppHandle, recording: Recording, _duration_ms: i64, mut session: Session) {
    let c = commands(&app);
    // Holding the hotkey again while an answer is shown refines it.
    let open = is_open(&app);
    let (refine, previous_id) = {
        let run = c.run.lock().unwrap();
        (open && run.answer_ready, run.id.clone())
    };
    let (wav, has_speech) = match crate::finish_recording(recording).await {
        Ok(done) => done,
        Err(e) => return report_error(&app, e),
    };
    if !has_speech {
        if open {
            let generation = c.run.lock().unwrap().generation;
            update(&app, generation, |r| r.error = Some("No speech detected".into()));
        }
        return;
    }
    let generation = c.generation.fetch_add(1, Ordering::SeqCst) + 1;
    deny_pending(&app);
    {
        let mut run = c.run.lock().unwrap();
        let (messages, urls) = if refine { (std::mem::take(&mut run.messages), std::mem::take(&mut run.urls)) } else { Default::default() };
        *run = Run { phase: "transcribing".into(), refine_keys: refine_keys(&app), messages, urls, generation, ..Default::default() };
        emit(&app, &run);
    }
    show_window(&app);

    let config = session.config.clone();
    let transcription = match session.live.take() {
        Some(live) => live.finish().await,
        None => transcribe_raw(&app, wav, &config, Vec::new()).await,
    };
    let instruction = match transcription {
        Ok(t) if t.chars().any(char::is_alphanumeric) => t,
        Ok(_) => return update(&app, generation, |r| fail(r, "No speech detected")),
        Err(e) => return update(&app, generation, |r| fail(r, &e)),
    };
    // Accessibility only: a synthetic ⌘C would clobber non-text clipboards and, on Windows,
    // interrupt terminals (Ctrl+C). ponytail: apps without AX selection get no context.
    let selection = if refine { None } else { session.selection.clone() };
    let history_id = if refine {
        previous_id
    } else {
        let row = HistoryEntry {
            kind: "command".into(),
            status: "running".into(),
            text: instruction.clone(),
            raw_text: instruction.clone(),
            app_name: session.context.as_ref().map(|c| c.name.clone()),
            app_bundle_id: session.context.as_ref().map(|c| c.bundle_id.clone()),
            ..HistoryEntry::new(uuid::Uuid::new_v4().to_string())
        };
        let _ = app.state::<AppState>().db.insert_history(&row);
        history_updated(&app);
        row.id
    };
    run_command(&app, generation, instruction, selection, session.context, history_id).await;
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
) {
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        let c = commands(&app);
        let generation = c.generation.fetch_add(1, Ordering::SeqCst) + 1;
        deny_pending(&app);
        let waiting = instruction.trim().is_empty();
        {
            let mut run = c.run.lock().unwrap();
            *run = Run {
                id: history_id.clone(),
                phase: if waiting { "done" } else { "thinking" }.into(),
                text: if waiting { "What can I help you with?".into() } else { String::new() },
                refine_keys: refine_keys(&app),
                generation,
                ..Default::default()
            };
            emit(&app, &run);
        }
        show_window(&app);
        if waiting {
            // Nothing to do yet: the next commands-hotkey recording starts a fresh command.
            let state = app.state::<AppState>();
            if let Ok(mut row) = state.db.get_history_entry(&history_id) {
                row.status = "ok".into();
                row.answer = Some("What can I help you with?".into());
                let _ = state.db.update_history(&row);
                history_updated(&app);
            }
            return;
        }
        run_command(&app, generation, instruction, selection, context, history_id).await;
    });
}

async fn run_command(
    app: &AppHandle,
    generation: u64,
    instruction: String,
    selection: Option<String>,
    context: Option<AppContext>,
    history_id: String,
) {
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
    let result = agent_loop(app, generation, &config, &tool_ctx, &mut messages).await;

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
    let _ = state.db.update_history(&row);
    history_updated(app);
    if !still_shown {
        let title = if row.status == "ok" { "Command finished" } else { "Command failed" };
        notify(app, title, &instruction);
    }
}

/// Streams completions, running tool calls between them, until the model answers.
async fn agent_loop(
    app: &AppHandle,
    generation: u64,
    config: &TranscriptionConfig,
    ctx: &tools::ToolContext,
    messages: &mut Vec<Value>,
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
    for step in 0..MAX_STEPS {
        // The last step gets no tools, so the model has to answer.
        let offered: &[Value] = if step + 1 == MAX_STEPS { &[] } else { &schemas };
        update(app, generation, |r| r.text.clear());
        let completion = llm
            .stream(messages, offered, |delta| {
                let c = commands(app);
                let mut run = c.run.lock().unwrap();
                if run.generation != generation {
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
        if completion.tool_calls.is_empty() {
            return Ok((completion.text.trim().to_string(), sources));
        }
        messages.push(llm::assistant_tool_message(&completion));
        for call in &completion.tool_calls {
            let args: Value = serde_json::from_str(&call.arguments).unwrap_or_else(|_| json!({}));
            let output = run_tool(app, generation, config, ctx, &call.name, &args, &mut sources).await;
            messages.push(json!({"role":"tool","tool_call_id": call.id, "content": output}));
        }
    }
    Err("The command took too many steps".into())
}

async fn run_tool(
    app: &AppHandle,
    generation: u64,
    config: &TranscriptionConfig,
    ctx: &tools::ToolContext,
    name: &str,
    args: &Value,
    sources: &mut Vec<Source>,
) -> String {
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
        tools::run(ctx, name, args).await.map(|r| {
            sources.extend(r.sources);
            r.content
        })
    } else if let Some((server, tool, policy)) = resolved {
        let decision = if policy == "ask" { ask_approval(app, generation, &server, &tool, args).await } else { policy.clone() };
        match decision.as_str() {
            "deny" => Ok("The user declined this tool call.".into()),
            _ => mcp.call(&server, &tool, args.clone()).await,
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

/// Shows the approval card and waits (Enter = once, Esc/close/60 s = deny). "always" persists.
async fn ask_approval(app: &AppHandle, generation: u64, server: &str, tool: &str, args: &Value) -> String {
    // Nobody can see a card for a closed window or a superseded run.
    if !is_open(app) || commands(app).run.lock().unwrap().generation != generation {
        return "deny".into();
    }
    let request_id = uuid::Uuid::new_v4().to_string();
    let (tx, rx) = oneshot::channel();
    commands(app).approvals.lock().unwrap().insert(request_id.clone(), tx);
    // All of it: padding benign fields must not push a recipient or path out of view.
    let summary = serde_json::to_string_pretty(args).unwrap_or_else(|_| args.to_string());
    update(app, generation, |r| {
        r.phase = "approval".into();
        r.approval = Some(Approval {
            request_id: request_id.clone(),
            server: server.into(),
            tool: tool.into(),
            summary,
            expires_in_s: APPROVAL_TIMEOUT.as_secs(),
        });
    });
    let decision = match tokio::time::timeout(APPROVAL_TIMEOUT, rx).await {
        Ok(Ok(d)) => d,
        _ => "deny".into(),
    };
    commands(app).approvals.lock().unwrap().remove(&request_id);
    update(app, generation, |r| {
        r.approval = None;
        r.phase = "tool".into();
    });
    if decision == "always" {
        let (server, tool) = (server.to_string(), tool.to_string());
        if let Err(e) = crate::update_config(app, |c| {
            c.mcp_servers.entry(server).or_default().tool_policies.insert(tool, "auto".into());
        }) {
            eprintln!("{e}");
        }
        return "auto".into();
    }
    decision
}

fn decide(app: &AppHandle, request_id: &str, decision: &str) {
    if let Some(tx) = commands(app).approvals.lock().unwrap().remove(request_id) {
        let _ = tx.send(decision.into());
    }
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
    let answer = plain_text(&current_answer(app).ok_or("No answer to paste yet")?);
    hide_window(app);
    let keep = app.state::<AppState>().config.lock().unwrap().copy_to_clipboard;
    let app = app.clone();
    std::thread::spawn(move || {
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
