//! Custom tools: local MCP servers speaking JSON-RPC over stdio, configured in the standard
//! `{"mcpServers": {...}}` format at <app_data_dir>/mcp.json.

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncWrite, AsyncWriteExt, BufReader};
use tokio::sync::oneshot;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
#[serde(default)]
pub struct ServerSpec {
    pub command: String,
    pub args: Vec<String>,
    pub env: BTreeMap<String, String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct McpConfig {
    #[serde(rename = "mcpServers")]
    pub servers: BTreeMap<String, ServerSpec>,
}

#[derive(Debug, Clone, Serialize)]
pub struct McpTool {
    pub name: String,
    pub description: String,
    pub policy: String, // "auto" | "ask" | "deny"
    #[serde(skip)]
    pub schema: Value,
    #[serde(skip)]
    pub read_only: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct McpServer {
    pub name: String,
    pub enabled: bool,
    pub status: String, // "stopped" | "starting" | "running" | "failed"
    pub error: Option<String>,
    pub folder: Option<String>,
    pub description: Option<String>,
    pub tools: Vec<McpTool>,
}

// ---- JSON-RPC client ----

type Pending = Arc<Mutex<HashMap<u64, oneshot::Sender<Result<Value, String>>>>>;

pub struct Client {
    writer: tokio::sync::Mutex<Box<dyn AsyncWrite + Send + Unpin>>,
    pending: Pending,
    next_id: AtomicU64,
    /// Last lines the server wrote to stderr (for failure messages).
    pub stderr: Arc<Mutex<Vec<String>>>,
}

impl Client {
    pub fn new(reader: impl AsyncRead + Send + Unpin + 'static, writer: impl AsyncWrite + Send + Unpin + 'static) -> Arc<Self> {
        let pending: Pending = Arc::default();
        let reader_pending = pending.clone();
        tokio::spawn(async move {
            let mut reader = BufReader::new(reader);
            let mut buf = Vec::new();
            // Byte lines: one non-UTF-8 line (a stray log) must not end the client.
            while matches!(reader.read_until(b'\n', &mut buf).await, Ok(n) if n > 0) {
                let line = String::from_utf8_lossy(&buf).into_owned();
                buf.clear();
                let Ok(msg) = serde_json::from_str::<Value>(&line) else { continue };
                let Some(id) = msg.get("id").and_then(Value::as_u64) else { continue };
                let reply = match (msg.get("result"), msg.get("error")) {
                    (Some(r), _) => Ok(r.clone()),
                    (_, Some(e)) => Err(e.get("message").and_then(Value::as_str).unwrap_or("error").to_string()),
                    _ => continue,
                };
                if let Some(tx) = reader_pending.lock().unwrap().remove(&id) {
                    let _ = tx.send(reply);
                }
            }
            // Server gone: fail everything still waiting.
            for (_, tx) in reader_pending.lock().unwrap().drain() {
                let _ = tx.send(Err("The server stopped".into()));
            }
        });
        Arc::new(Self { writer: tokio::sync::Mutex::new(Box::new(writer)), pending, next_id: AtomicU64::new(1), stderr: Arc::default() })
    }

    async fn send(&self, msg: &Value) -> Result<(), String> {
        let mut line = serde_json::to_string(msg).map_err(|e| e.to_string())?;
        line.push('\n');
        let mut w = self.writer.lock().await;
        w.write_all(line.as_bytes()).await.map_err(|e| e.to_string())?;
        w.flush().await.map_err(|e| e.to_string())
    }

    pub async fn request(&self, method: &str, params: Value, timeout: Duration) -> Result<Value, String> {
        let id = self.next_id.fetch_add(1, Ordering::SeqCst);
        let (tx, rx) = oneshot::channel();
        self.pending.lock().unwrap().insert(id, tx);
        if let Err(e) = self.send(&json!({"jsonrpc":"2.0","id":id,"method":method,"params":params})).await {
            self.pending.lock().unwrap().remove(&id);
            return Err(e);
        }
        match tokio::time::timeout(timeout, rx).await {
            Ok(Ok(reply)) => reply,
            Ok(Err(_)) => Err("The server stopped".into()),
            Err(_) => {
                self.pending.lock().unwrap().remove(&id);
                Err(format!("{method} timed out"))
            }
        }
    }

    pub async fn initialize(&self) -> Result<(), String> {
        let params = json!({"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"OpenGlaido","version":env!("CARGO_PKG_VERSION")}});
        self.request("initialize", params, Duration::from_secs(20)).await?;
        self.send(&json!({"jsonrpc":"2.0","method":"notifications/initialized"})).await
    }

    pub async fn list_tools(&self) -> Result<Vec<McpTool>, String> {
        let mut tools = Vec::new();
        let mut cursor: Option<String> = None;
        loop {
            let params = cursor.as_ref().map_or(json!({}), |c| json!({"cursor": c}));
            let result = self.request("tools/list", params, Duration::from_secs(20)).await?;
            for t in result.get("tools").and_then(Value::as_array).into_iter().flatten() {
                let read_only = t.pointer("/annotations/readOnlyHint").and_then(Value::as_bool).unwrap_or(false);
                tools.push(McpTool {
                    name: t.get("name").and_then(Value::as_str).unwrap_or("").to_string(),
                    description: t.get("description").and_then(Value::as_str).unwrap_or("").to_string(),
                    policy: if read_only { "auto" } else { "ask" }.into(),
                    schema: t.get("inputSchema").cloned().unwrap_or_else(|| json!({"type":"object"})),
                    read_only,
                });
            }
            let next = result.get("nextCursor").and_then(Value::as_str).map(str::to_string);
            // A server repeating its cursor would page forever.
            if next.is_none() || next == cursor {
                return Ok(tools);
            }
            cursor = next;
        }
    }

    pub async fn call_tool(&self, name: &str, arguments: Value) -> Result<String, String> {
        let result = self.request("tools/call", json!({"name":name,"arguments":arguments}), Duration::from_secs(120)).await?;
        let text: Vec<String> = result
            .get("content")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|c| c.get("text").and_then(Value::as_str).map(str::to_string))
            .collect();
        let text = if text.is_empty() { result.to_string() } else { text.join("\n") };
        if result.get("isError").and_then(Value::as_bool) == Some(true) {
            Err(text)
        } else {
            Ok(text)
        }
    }
}

// ---- servers ----

struct Running {
    client: Arc<Client>,
    _child: tokio::process::Child, // kill_on_drop
}

#[derive(Default)]
struct Entry {
    status: String,
    error: Option<String>,
    tools: Vec<McpTool>,
    running: Option<Running>,
}

pub struct Manager {
    path: PathBuf,
    entries: Mutex<HashMap<String, Entry>>,
}

/// Where GUI apps don't look by default but runtimes usually live.
fn search_path() -> String {
    let home = std::env::var("HOME").or_else(|_| std::env::var("USERPROFILE")).unwrap_or_default();
    let extra = [format!("{home}/.local/bin"), format!("{home}/.cargo/bin"), format!("{home}/.bun/bin"),
        "/opt/homebrew/bin".into(), "/usr/local/bin".into()];
    let sep = if cfg!(windows) { ";" } else { ":" };
    let mut parts: Vec<String> = std::env::var("PATH").unwrap_or_default().split(sep).map(str::to_string).collect();
    parts.extend(extra);
    parts.join(sep)
}

pub fn find_executable(name: &str) -> Option<PathBuf> {
    let sep = if cfg!(windows) { ';' } else { ':' };
    let exts: &[&str] = if cfg!(windows) { &[".exe", ".cmd", ".bat", ""] } else { &[""] };
    search_path().split(sep).flat_map(|dir| exts.iter().map(move |e| Path::new(dir).join(format!("{name}{e}")))).find(|p| p.is_file())
}

/// Plain-language cause of a start failure (docs/beta-tools "When a server won't start").
pub fn failure_cause(spawn_error: Option<&std::io::Error>, command: &str, exit: Option<i32>, stderr: &str) -> String {
    if spawn_error.is_some_and(|e| e.kind() == std::io::ErrorKind::NotFound) {
        return format!("It could not find the runtime “{command}”. Install it, or point the config at the copy you have.");
    }
    let low = stderr.to_lowercase();
    if low.contains("modulenotfounderror") || low.contains("cannot find module") || low.contains("no module named") {
        return "The server needs a package that isn't installed. Run the install step in its folder.".into();
    }
    if low.contains("environment variable") || low.contains("keyerror") || low.contains("api_key") && low.contains("not set") {
        return "The server needs an environment variable that isn't set. Add the key to its env file.".into();
    }
    match (spawn_error, exit) {
        (Some(e), _) => format!("It could not run the server: {e}"),
        (None, Some(code)) => format!("It could not run the server (exit code {code})."),
        (None, None) => "It started and didn't answer as an MCP server (usually a server printing to stdout).".into(),
    }
}

/// "My Server" + "search_pages" → "mcp__my_server__search_pages" (the model's tool name).
pub fn tool_id(server: &str, tool: &str) -> String {
    let clean = |s: &str| s.chars().map(|c| if c.is_ascii_alphanumeric() || c == '-' { c.to_ascii_lowercase() } else { '_' }).collect::<String>();
    let id = format!("mcp__{}__{}", clean(server), clean(tool));
    id.chars().take(64).collect()
}

impl Manager {
    pub fn new(app_dir: &Path) -> Self {
        Self { path: app_dir.join("mcp.json"), entries: Mutex::default() }
    }

    pub fn config_path(&self) -> &Path {
        &self.path
    }

    pub fn read(&self) -> McpConfig {
        std::fs::read_to_string(&self.path).ok().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default()
    }

    pub fn write(&self, config: &McpConfig) -> Result<(), String> {
        let json = serde_json::to_string_pretty(config).map_err(|e| e.to_string())?;
        std::fs::write(&self.path, json).map_err(|e| format!("Couldn't write {}: {e}", self.path.display()))
    }

    pub fn list(&self, prefs: &BTreeMap<String, crate::transcribe::McpServerPrefs>) -> Vec<McpServer> {
        let entries = self.entries.lock().unwrap();
        self.read()
            .servers
            .into_iter()
            .map(|(name, spec)| {
                let entry = entries.get(&name);
                let pref = prefs.get(&name);
                let tools = entry.map(|e| e.tools.clone()).unwrap_or_default().into_iter().map(|mut t| {
                    if let Some(p) = pref.and_then(|p| p.tool_policies.get(&t.name)) {
                        t.policy = p.clone();
                    }
                    t
                }).collect();
                McpServer {
                    enabled: pref.is_some_and(|p| p.enabled),
                    status: entry.map_or("stopped".into(), |e| e.status.clone()),
                    error: entry.and_then(|e| e.error.clone()),
                    folder: spec.cwd.clone(),
                    description: spec.description.clone(),
                    tools,
                    name,
                }
            })
            .collect()
    }

    /// Starts (or restarts) one server and loads its tools. Errors land in its status.
    pub async fn start(&self, name: &str) {
        let Some(spec) = self.read().servers.get(name).cloned() else { return };
        self.stop(name);
        self.entries.lock().unwrap().insert(name.into(), Entry { status: "starting".into(), ..Default::default() });
        let result = spawn(&spec).await;
        let mut entries = self.entries.lock().unwrap();
        // Stopped or deleted while it was starting: drop the new process (kill_on_drop).
        if !entries.contains_key(name) || !self.read().servers.contains_key(name) {
            entries.remove(name);
            return;
        }
        let entry = entries.entry(name.into()).or_default();
        match result {
            Ok((running, tools)) => {
                *entry = Entry { status: "running".into(), error: None, tools, running: Some(running) };
            }
            Err(e) => {
                *entry = Entry { status: "failed".into(), error: Some(e), tools: Vec::new(), running: None };
            }
        }
    }

    pub fn stop(&self, name: &str) {
        self.entries.lock().unwrap().remove(name);
    }

    pub fn stop_all(&self) {
        self.entries.lock().unwrap().clear();
    }

    /// (model-facing id, server, tool) for every tool of every enabled, running server; ids are
    /// made unique (folding and truncation can collide).
    fn tool_table(&self, prefs: &BTreeMap<String, crate::transcribe::McpServerPrefs>) -> Vec<(String, McpServer, McpTool)> {
        let mut seen = std::collections::HashSet::new();
        let mut table = Vec::new();
        for server in self.list(prefs).into_iter().filter(|s| s.enabled && s.status == "running") {
            for tool in server.tools.clone() {
                let base = tool_id(&server.name, &tool.name);
                let mut id = base.clone();
                let mut n = 2;
                while !seen.insert(id.clone()) {
                    let suffix = format!("_{n}");
                    id = format!("{}{suffix}", &base[..base.len().min(64 - suffix.len())]);
                    n += 1;
                }
                table.push((id, server.clone(), tool));
            }
        }
        table
    }

    /// Tool schemas (model-facing names) of running servers, minus denied tools.
    pub fn schemas(&self, prefs: &BTreeMap<String, crate::transcribe::McpServerPrefs>) -> Vec<Value> {
        let table = self.tool_table(prefs);
        let entries = self.entries.lock().unwrap();
        table
            .into_iter()
            .filter(|(_, _, t)| t.policy != "deny")
            .map(|(id, server, t)| {
                let raw = entries.get(&server.name).map(|e| e.tools.clone()).unwrap_or_default();
                let schema = raw.iter().find(|r| r.name == t.name).map_or(json!({"type":"object"}), |r| r.schema.clone());
                json!({"type":"function","function":{"name": id, "description": format!("[{}] {}", server.name, t.description), "parameters": schema}})
            })
            .collect()
    }

    /// (server, tool, policy) for a model-facing tool name.
    pub fn resolve(&self, prefs: &BTreeMap<String, crate::transcribe::McpServerPrefs>, id: &str) -> Option<(String, String, String)> {
        self.tool_table(prefs).into_iter().find(|(i, _, _)| i == id).map(|(_, s, t)| (s.name, t.name, t.policy))
    }

    pub async fn call(&self, server: &str, tool: &str, args: Value) -> Result<String, String> {
        let client = self.entries.lock().unwrap().get(server).and_then(|e| e.running.as_ref().map(|r| r.client.clone()));
        client.ok_or_else(|| format!("{server} isn't running"))?.call_tool(tool, args).await
    }

    /// Adds the servers from a folder's mcp.json / .mcp.json (run from that folder).
    pub fn import_folder(&self, folder: &Path) -> Result<Vec<String>, String> {
        let file = ["mcp.json", ".mcp.json"].iter().map(|f| folder.join(f)).find(|p| p.is_file())
            .ok_or_else(|| format!("No mcp.json in {}", folder.display()))?;
        let imported: McpConfig = serde_json::from_str(&std::fs::read_to_string(&file).map_err(|e| e.to_string())?)
            .map_err(|e| format!("{} isn't valid MCP config: {e}", file.display()))?;
        if imported.servers.is_empty() {
            return Err(format!("{} lists no servers", file.display()));
        }
        let mut config = self.read();
        let mut names = Vec::new();
        for (name, mut spec) in imported.servers {
            // No cwd or a relative one ("./srv"): relative to the imported folder, not to our own.
            spec.cwd = Some(spec.cwd.as_deref().map_or(folder.to_path_buf(), |c| folder.join(c)).display().to_string());
            // Re-importing the same folder replaces its server; a clash with another folder gets a suffix.
            let mut unique = name.clone();
            let mut n = 2;
            while config.servers.get(&unique).is_some_and(|s| s.cwd != spec.cwd) {
                unique = format!("{name} {n}");
                n += 1;
            }
            config.servers.insert(unique.clone(), spec);
            names.push(unique);
        }
        self.write(&config)?;
        Ok(names)
    }

    pub fn delete(&self, name: &str) -> Result<(), String> {
        self.stop(name);
        let mut config = self.read();
        config.servers.remove(name);
        self.write(&config)
    }
}

async fn spawn(spec: &ServerSpec) -> Result<(Running, Vec<McpTool>), String> {
    if let Some(cwd) = spec.cwd.as_deref().filter(|c| !Path::new(c).is_dir()) {
        return Err(format!("The server's folder is gone ({cwd}). If you moved it, import it again from its new location."));
    }
    let mut cmd = tokio::process::Command::new(find_executable(&spec.command).unwrap_or_else(|| spec.command.clone().into()));
    // Our PATH first, so a PATH in the server's own env (nvm, volta…) wins.
    cmd.env("PATH", search_path())
        .args(&spec.args)
        .envs(&spec.env)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true);
    if let Some(cwd) = &spec.cwd {
        cmd.current_dir(cwd);
    }
    #[cfg(windows)]
    cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW: no console window per server
    let mut child = cmd.spawn().map_err(|e| failure_cause(Some(&e), &spec.command, None, ""))?;
    let (stdin, stdout, stderr) = (child.stdin.take().unwrap(), child.stdout.take().unwrap(), child.stderr.take().unwrap());
    let client = Client::new(stdout, stdin);
    let log = client.stderr.clone();
    tokio::spawn(async move {
        let mut reader = BufReader::new(stderr);
        let mut buf = Vec::new();
        while matches!(reader.read_until(b'\n', &mut buf).await, Ok(n) if n > 0) {
            let line = String::from_utf8_lossy(&buf).trim_end().to_string();
            buf.clear();
            let mut log = log.lock().unwrap();
            log.push(line);
            if log.len() > 40 {
                log.remove(0);
            }
        }
    });
    let tools = async {
        client.initialize().await?;
        client.list_tools().await
    }
    .await;
    match tools {
        Ok(tools) => Ok((Running { client, _child: child }, tools)),
        Err(_) => {
            tokio::time::sleep(Duration::from_millis(200)).await;
            let exit = child.try_wait().ok().flatten().and_then(|s| s.code());
            let stderr = client.stderr.lock().unwrap().join("\n");
            let cause = failure_cause(None, &spec.command, exit, &stderr);
            Err(if stderr.trim().is_empty() { cause } else { format!("{cause}\n\n{}", stderr.trim()) })
        }
    }
}

// ---- new server scaffold ----

pub fn install_command(language: &str) -> &'static str {
    match (language, cfg!(windows)) {
        ("typescript", false) => "curl -fsSL https://bun.sh/install | bash",
        ("typescript", true) => "powershell -c \"irm bun.sh/install.ps1 | iex\"",
        (_, false) => "curl -LsSf https://astral.sh/uv/install.sh | sh",
        (_, true) => "powershell -ExecutionPolicy ByPass -c \"irm https://astral.sh/uv/install.ps1 | iex\"",
    }
}

pub fn runtime_for(language: &str) -> &'static str {
    if language == "typescript" { "bun" } else { "uv" }
}

/// Writes an empty server (FastMCP/uv or MCP SDK/bun) and its path-free mcp.json.
pub fn scaffold(folder: &Path, language: &str, name: &str) -> Result<(), String> {
    std::fs::create_dir_all(folder).map_err(|e| e.to_string())?;
    let write = |file: &str, body: String| std::fs::write(folder.join(file), body).map_err(|e| e.to_string());
    let slug: String = name.chars().filter(|c| c.is_ascii()).map(|c| if c.is_ascii_alphanumeric() { c.to_ascii_lowercase() } else { '-' }).collect();
    let slug = match slug.trim_matches('-') { "" => "server".to_string(), s => s.to_string() };
    // A JSON string literal is also a valid Python and TypeScript string literal.
    let quoted = serde_json::to_string(name).map_err(|e| e.to_string())?;
    let (command, args) = if language == "typescript" { ("bun", ["run", "server.ts"]) } else { ("uv", ["run", "server.py"]) };
    let config = McpConfig {
        servers: [(name.to_string(), ServerSpec { command: command.into(), args: args.map(String::from).to_vec(), ..Default::default() })].into(),
    };
    write("mcp.json", serde_json::to_string_pretty(&config).map_err(|e| e.to_string())? + "\n")?;
    if language == "typescript" {
        write("package.json", format!("{{\n  \"name\": \"{slug}\",\n  \"private\": true,\n  \"type\": \"module\",\n  \"dependencies\": {{ \"@modelcontextprotocol/sdk\": \"^1\", \"zod\": \"^3\" }}\n}}\n"))?;
        write("server.ts", format!("import {{ McpServer }} from \"@modelcontextprotocol/sdk/server/mcp.js\";\nimport {{ StdioServerTransport }} from \"@modelcontextprotocol/sdk/server/stdio.js\";\n\nconst server = new McpServer({{ name: {quoted}, version: \"0.1.0\" }});\n\n// Add tools with server.tool(...). Never print to stdout: it carries the protocol.\n\nawait server.connect(new StdioServerTransport());\n"))?;
    } else {
        write("pyproject.toml", format!("[project]\nname = \"{slug}\"\nversion = \"0.1.0\"\nrequires-python = \">=3.10\"\ndependencies = [\"fastmcp\"]\n"))?;
        write("server.py", format!("from fastmcp import FastMCP\n\nmcp = FastMCP({quoted})\n\n# Add tools with @mcp.tool. Never print to stdout: it carries the protocol.\n\nif __name__ == \"__main__\":\n    mcp.run()\n"))?;
    }
    Ok(())
}

pub fn agent_prompt(folder: &Path, language: &str, name: &str) -> String {
    let (lang, run) = if language == "typescript" { ("TypeScript (bun, @modelcontextprotocol/sdk)", "bun run server.ts") } else { ("Python (uv, FastMCP)", "uv run server.py") };
    format!(
        "I'm building a custom tool for the OpenGlaido dictation app. It is a local MCP server named \"{name}\" in {}, written in {lang} and started with `{run}` over stdio.\n\
         Add the tools I describe below to it. Keep each tool small and specific, give it a clear description, mark read-only tools with readOnlyHint, never print to stdout, and keep mcp.json path-free. Put API keys in a local .env file the server loads.\n\n\
         What I want it to do: <describe the service and actions here>",
        folder.display()
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn client_talks_json_rpc_over_a_pipe() {
        let (client_side, server_side) = tokio::io::duplex(64 * 1024);
        let (client_read, client_write) = tokio::io::split(client_side);
        let (server_read, mut server_write) = tokio::io::split(server_side);
        // A tiny fake server: answers initialize, tools/list and tools/call.
        tokio::spawn(async move {
            let mut lines = BufReader::new(server_read).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                let msg: Value = serde_json::from_str(&line).unwrap();
                let Some(id) = msg.get("id").cloned() else { continue };
                let result = match msg["method"].as_str().unwrap() {
                    "initialize" => json!({"protocolVersion":"2025-06-18","capabilities":{},"serverInfo":{"name":"t"}}),
                    "tools/list" => json!({"tools":[
                        {"name":"get_weather","description":"Weather","inputSchema":{"type":"object"},"annotations":{"readOnlyHint":true}},
                        {"name":"add_task","description":"Add","inputSchema":{"type":"object"}}]}),
                    "tools/call" => json!({"content":[{"type":"text","text": format!("sunny in {}", msg["params"]["arguments"]["city"].as_str().unwrap())}]}),
                    _ => json!({}),
                };
                let reply = format!("{}\n", json!({"jsonrpc":"2.0","id":id,"result":result}));
                server_write.write_all(reply.as_bytes()).await.unwrap();
            }
        });
        let client = Client::new(client_read, client_write);
        client.initialize().await.unwrap();
        let tools = client.list_tools().await.unwrap();
        assert_eq!(tools.iter().map(|t| (t.name.as_str(), t.policy.as_str())).collect::<Vec<_>>(), [("get_weather", "auto"), ("add_task", "ask")]);
        assert_eq!(client.call_tool("get_weather", json!({"city":"Ljubljana"})).await.unwrap(), "sunny in Ljubljana");
    }

    #[test]
    fn config_parsing_import_and_ids() {
        let dir = std::env::temp_dir().join(format!("og-mcp-test-{}", std::process::id()));
        let folder = dir.join("weather");
        std::fs::create_dir_all(&folder).unwrap();
        std::fs::write(folder.join("mcp.json"), r#"{"mcpServers":{"Weather":{"command":"uv","args":["run","server.py"],"env":{"K":"v"}}}}"#).unwrap();
        let manager = Manager::new(&dir);
        assert_eq!(manager.import_folder(&folder).unwrap(), ["Weather"]);
        // Same folder again replaces instead of duplicating.
        assert_eq!(manager.import_folder(&folder).unwrap(), ["Weather"]);
        let config = manager.read();
        assert_eq!(config.servers["Weather"].cwd.as_deref(), Some(folder.to_str().unwrap()));
        assert_eq!(config.servers["Weather"].env["K"], "v");
        assert!(manager.import_folder(&dir.join("missing")).is_err());
        manager.delete("Weather").unwrap();
        assert!(manager.read().servers.is_empty());
        assert_eq!(tool_id("My Server!", "search_pages"), "mcp__my_server___search_pages");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn failure_causes_read_like_the_docs() {
        let not_found = std::io::Error::new(std::io::ErrorKind::NotFound, "x");
        assert!(failure_cause(Some(&not_found), "uv", None, "").contains("could not find the runtime “uv”"));
        assert!(failure_cause(None, "uv", Some(1), "ModuleNotFoundError: No module named 'fastmcp'").contains("package"));
        assert!(failure_cause(None, "uv", Some(1), "KeyError: 'NOTION_TOKEN'").contains("environment variable"));
        assert!(failure_cause(None, "uv", Some(3), "boom").contains("exit code 3"));
        assert!(failure_cause(None, "uv", None, "").contains("didn't answer as an MCP server"));
    }

    #[test]
    fn scaffold_writes_a_path_free_server() {
        let dir = std::env::temp_dir().join(format!("og-mcp-scaffold-{}", std::process::id()));
        scaffold(&dir, "python", "Notes \"Pro\" ž").unwrap();
        let mcp: McpConfig = serde_json::from_str(&std::fs::read_to_string(dir.join("mcp.json")).unwrap()).unwrap();
        assert_eq!(mcp.servers["Notes \"Pro\" ž"].command, "uv");
        assert!(std::fs::read_to_string(dir.join("server.py")).unwrap().contains(r#"FastMCP("Notes \"Pro\" ž")"#));
        assert!(std::fs::read_to_string(dir.join("pyproject.toml")).unwrap().contains("name = \"notes--pro\""));
        assert!(dir.join("server.py").is_file() && dir.join("pyproject.toml").is_file());
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
