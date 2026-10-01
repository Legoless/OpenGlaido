//! Built-in commands the model can call: web search, read a page, YouTube, deep research,
//! math & dates, files & apps, history search, docs search.

use crate::db::Source;
use crate::transcribe::TranscriptionConfig;
use crate::AppState;
use serde::Serialize;
use serde_json::{json, Value};
use std::collections::HashSet;
use std::net::IpAddr;
use std::sync::Mutex;
use std::time::Duration;
use tauri::{AppHandle, Manager};

#[derive(Debug, Clone, Serialize)]
pub struct BuiltinTool {
    pub id: String,
    pub group: String, // "web" | "system" | "glaido"
    pub name: String,
    pub description: String,
    pub examples: Vec<String>,
    pub enabled: bool,
    pub available: bool,
    pub unavailable_reason: Option<String>,
}

struct Spec {
    id: &'static str,
    group: &'static str,
    name: &'static str,
    description: &'static str,
    examples: &'static [&'static str],
    /// Shown to the model.
    purpose: &'static str,
    parameters: fn() -> Value,
}

const SPECS: [Spec; 8] = [
    Spec {
        id: "web_search", group: "web", name: "Web search",
        description: "Finds pages and reads the top few.",
        examples: &["Search the web for the latest news on electric cars", "What's the weather in Ljubljana this weekend?"],
        purpose: "Search the web and read the top results. Use for current events, facts you are unsure of, or anything that needs fresh information.",
        parameters: || json!({"type":"object","properties":{"query":{"type":"string"}},"required":["query"]}),
    },
    Spec {
        id: "read_page", group: "web", name: "Read a page",
        description: "Reads the one page you point it at, or the page open in your browser.",
        examples: &["Summarize the page I'm on", "What does this article say about pricing?"],
        purpose: "Read a web page as text. Omit url to read the page currently open in the user's browser (or a link they selected).",
        parameters: || json!({"type":"object","properties":{"url":{"type":"string","description":"Optional; defaults to the open page"}}}),
    },
    Spec {
        id: "youtube", group: "web", name: "Summarize a YouTube video",
        description: "Works from the video's transcript. The video needs captions.",
        examples: &["Summarize this video", "What are the main points of the YouTube video I have open?"],
        purpose: "Get the transcript of a YouTube video. Omit url to use the video open in the user's browser.",
        parameters: || json!({"type":"object","properties":{"url":{"type":"string"}}}),
    },
    Spec {
        id: "deep_research", group: "web", name: "Deep research",
        description: "Reads across many sources and takes minutes. The write-up lands in your history.",
        examples: &["Research the pros and cons of heat pumps for old houses", "Do a deep dive on Tauri vs Electron"],
        purpose: "Start a long background research run on a big question. Returns immediately; the report is saved to history and the user is notified.",
        parameters: || json!({"type":"object","properties":{"question":{"type":"string"}},"required":["question"]}),
    },
    Spec {
        id: "math_dates", group: "system", name: "Math and dates",
        description: "Calculator, time zones and date questions, worked out exactly on your machine.",
        examples: &["What is 17.5% of 849?", "What is the date 10 days from now?", "It is 9 am Pacific, what is that for me?"],
        purpose: "Exact local maths and dates. action=calculate with expression (use * and /, percentages as 0.175*849); action=now (optional timezone, IANA name); action=add_days with days; action=convert_time with time (HH:MM), from and to IANA time zones (to defaults to local).",
        parameters: || json!({"type":"object","properties":{
            "action":{"type":"string","enum":["calculate","now","add_days","convert_time"]},
            "expression":{"type":"string"},"timezone":{"type":"string"},"days":{"type":"number"},
            "time":{"type":"string"},"from":{"type":"string"},"to":{"type":"string"}},"required":["action"]}),
    },
    Spec {
        id: "files_apps", group: "system", name: "Files and apps",
        description: "Finds, reads and opens files, folders and apps.",
        examples: &["Find the contract I saved last month", "Open my Downloads folder", "Open that screenshot in Preview"],
        purpose: "Local files and apps. action=find with query (file names/contents); action=read with path (text files up to 200 KB); action=open with path (documents, media and folders, in their default app); action=open_app with app.",
        parameters: || json!({"type":"object","properties":{
            "action":{"type":"string","enum":["find","read","open","open_app"]},
            "query":{"type":"string"},"path":{"type":"string"},"app":{"type":"string"}},"required":["action"]}),
    },
    Spec {
        id: "history_search", group: "glaido", name: "Search your history",
        description: "Finds anything you have dictated on this computer.",
        examples: &["What did I dictate about the budget yesterday?", "Find the email I dictated to Sarah"],
        purpose: "Search the user's own past dictations and command answers (local only). When they want something back, answer with that text only so it can be pasted.",
        parameters: || json!({"type":"object","properties":{"query":{"type":"string"}},"required":["query"]}),
    },
    Spec {
        id: "docs_search", group: "glaido", name: "Search documentation",
        description: "Answers questions about OpenGlaido itself.",
        examples: &["How do I change my dictation hotkey?", "What does raw text do?"],
        purpose: "Read OpenGlaido's own documentation to answer questions about the app.",
        parameters: || json!({"type":"object","properties":{"question":{"type":"string"}}}),
    },
];

const DOCS: &str = concat!(include_str!("../../../README.md"), "\n\n", include_str!("../../../AGENTS.md"));

fn unavailable_reason(id: &str, config: &TranscriptionConfig) -> Option<String> {
    let needs_search = matches!(id, "web_search" | "deep_research");
    let configured = match config.search_provider.as_str() {
        "brave" | "tavily" => !config.search_api_key.trim().is_empty(),
        "searxng" => !config.searxng_url.trim().is_empty(),
        _ => false,
    };
    (needs_search && !configured).then(|| "Add a search provider".to_string())
}

pub fn builtin_tools(config: &TranscriptionConfig) -> Vec<BuiltinTool> {
    SPECS
        .iter()
        .map(|s| {
            let reason = unavailable_reason(s.id, config);
            BuiltinTool {
                id: s.id.into(),
                group: s.group.into(),
                name: s.name.into(),
                description: s.description.into(),
                examples: s.examples.iter().map(|e| e.to_string()).collect(),
                enabled: config.builtin_tools.get(s.id).copied().unwrap_or(true),
                available: reason.is_none(),
                unavailable_reason: reason,
            }
        })
        .collect()
}

/// OpenAI tool schemas for the enabled, available built-ins.
pub fn schemas(config: &TranscriptionConfig) -> Vec<Value> {
    builtin_tools(config)
        .iter()
        .filter(|t| t.enabled && t.available)
        .filter_map(|t| SPECS.iter().find(|s| s.id == t.id))
        .map(|s| json!({"type":"function","function":{"name": s.id, "description": s.purpose, "parameters": (s.parameters)()}}))
        .collect()
}

pub fn is_builtin(name: &str) -> bool {
    SPECS.iter().any(|s| s.id == name)
}

/// Consent is per call, including file discovery. Existing safe open actions reveal no data.
/// Tool output, a model-supplied argument, or an MCP policy cannot grant this consent.
pub fn requires_approval(name: &str, args: &Value) -> bool {
    name == "history_search" || (name == "files_apps" && !matches!(args["action"].as_str().map(str::trim), Some("open" | "open_app")))
}

fn check_dispatch(config: &TranscriptionConfig, name: &str, args: &Value, approved: bool) -> Result<(), String> {
    if !builtin_tools(config).iter().any(|t| t.id == name && t.enabled && t.available) {
        return Err(format!("{name} is turned off"));
    }
    if requires_approval(name, args) && !approved {
        return Err("The user declined this tool call.".into());
    }
    Ok(())
}

/// Short progress label for the command window.
pub fn label(name: &str, args: &Value) -> String {
    let arg = |k: &str| args.get(k).and_then(Value::as_str).unwrap_or("").to_string();
    match name {
        "web_search" => format!("Searching the web for “{}”", arg("query")),
        "read_page" => "Reading the page".into(),
        "youtube" => "Reading the video transcript".into(),
        "deep_research" => "Starting deep research".into(),
        "math_dates" => "Working it out".into(),
        "files_apps" => match arg("action").as_str() {
            "find" => format!("Looking for “{}”", arg("query")),
            "read" => "Reading the file".into(),
            _ => "Opening".into(),
        },
        "history_search" => format!("Searching your history for “{}”", arg("query")),
        "docs_search" => "Reading the documentation".into(),
        other => format!("Running {other}"),
    }
}

/// What a tool run needs to know about the command it serves.
pub struct ToolContext {
    pub app: AppHandle,
    pub config: TranscriptionConfig,
    /// The page open in the frontmost browser when the command started.
    pub page_url: Option<String>,
    pub selection: Option<String>,
    /// URLs the user put in front of the model (open page, selection, what they said): trusted,
    /// may be local. Search results join `public_urls`. Anything else is refused, so injected
    /// text can't make the model fetch attacker URLs (exfiltration) or internal hosts (SSRF).
    pub user_urls: HashSet<String>,
    pub public_urls: Mutex<HashSet<String>>,
}

impl ToolContext {
    pub fn new(app: AppHandle, config: TranscriptionConfig, page_url: Option<String>, selection: Option<String>, instruction: &str) -> Self {
        let mut user_urls: HashSet<String> = all_urls(instruction).into_iter().chain(selection.as_deref().map(all_urls).unwrap_or_default()).collect();
        user_urls.extend(page_url.clone());
        let user_urls = user_urls.into_iter().map(|u| normalize_url(&u)).collect();
        Self { app, config, page_url, selection, user_urls, public_urls: Mutex::default() }
    }

    /// The URL to fetch and whether it may point at a local/private host: only a user URL whose
    /// own host is local (an intranet page they have open). A public link, even a selected one,
    /// can't redirect or resolve into the local network.
    async fn check_url(&self, url: &str) -> Result<(String, bool), String> {
        let key = normalize_url(url);
        if !(key.starts_with("http://") || key.starts_with("https://")) {
            return Err("Only web pages (http/https) can be read".into());
        }
        if self.user_urls.contains(&key) {
            return Ok((url.to_string(), points_to_private(url).await));
        }
        if self.public_urls.lock().unwrap().contains(&key) {
            return Ok((url.to_string(), false));
        }
        Err("I can only read the page you have open, links you selected or said, and search results".into())
    }

    /// The allow-lists, so a follow-up turn can still read what the previous turn could.
    pub fn urls(&self) -> (HashSet<String>, HashSet<String>) {
        (self.user_urls.clone(), self.public_urls.lock().unwrap().clone())
    }

    pub fn inherit_urls(&mut self, (user, public): (HashSet<String>, HashSet<String>)) {
        self.user_urls.extend(user);
        self.public_urls.get_mut().unwrap().extend(public);
    }
}

fn all_urls(text: &str) -> Vec<String> {
    text.split_whitespace()
        .map(|w| w.trim_matches(|c: char| matches!(c, '<' | '>' | '(' | ')' | '"' | '\'' | ',' | '.')))
        .filter(|w| w.starts_with("http://") || w.starts_with("https://"))
        .map(str::to_string)
        .collect()
}

/// Comparable form: no fragment, no trailing slash.
fn normalize_url(url: &str) -> String {
    let url = url.trim();
    let url = url.split('#').next().unwrap_or(url);
    url.trim_end_matches('/').to_string()
}

/// localhost, *.local, loopback/private/link-local IPs (by name only; see `PublicDns`).
pub fn is_private_host(host: &str) -> bool {
    let host = host.trim_start_matches('[').trim_end_matches(']').trim_end_matches('.').to_ascii_lowercase();
    if host == "localhost" || [".localhost", ".local", ".internal", ".lan", ".home.arpa"].iter().any(|s| host.ends_with(s)) {
        return true;
    }
    host.parse().is_ok_and(is_private_ip)
}

fn is_private_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(ip) => {
            let [a, b, ..] = ip.octets();
            // 0/8 reaches this machine; 100.64/10 is carrier-grade NAT / Tailscale.
            ip.is_loopback() || ip.is_private() || ip.is_link_local() || ip.is_broadcast() || a == 0 || (a == 100 && (b & 0xc0) == 64)
        }
        IpAddr::V6(ip) => match ip.to_ipv4_mapped() {
            Some(v4) => is_private_ip(IpAddr::V4(v4)),
            None => ip.is_loopback() || ip.is_unspecified() || (ip.segments()[0] & 0xfe00) == 0xfc00 || (ip.segments()[0] & 0xffc0) == 0xfe80,
        },
    }
}

/// The URL's own host is local: by name, or because it resolves to a private address.
async fn points_to_private(url: &str) -> bool {
    let Some(host) = reqwest::Url::parse(url).ok().and_then(|u| u.host_str().map(str::to_string)) else { return false };
    is_private_host(&host)
        || tokio::net::lookup_host((host.as_str(), 0)).await.is_ok_and(|mut addrs| addrs.any(|a| is_private_ip(a.ip())))
}

/// DNS that refuses names resolving to local/private addresses (localtest.me, *.nip.io, rebinding).
struct PublicDns;

impl reqwest::dns::Resolve for PublicDns {
    fn resolve(&self, name: reqwest::dns::Name) -> reqwest::dns::Resolving {
        Box::pin(async move {
            let addrs: Vec<std::net::SocketAddr> = tokio::net::lookup_host((name.as_str(), 0)).await?.collect();
            if addrs.iter().any(|a| is_private_ip(a.ip())) {
                return Err(format!("{} is on the local network", name.as_str()).into());
            }
            Ok(Box::new(addrs.into_iter()) as reqwest::dns::Addrs)
        })
    }
}

pub struct ToolResult {
    pub content: String,
    pub sources: Vec<Source>,
}

impl ToolResult {
    fn text(content: impl Into<String>) -> Self {
        Self { content: content.into(), sources: Vec::new() }
    }
}

pub(super) async fn run(ctx: &ToolContext, name: &str, args: &Value, approved: bool) -> Result<ToolResult, String> {
    // The model only sees enabled tools, but it can still name a hidden one.
    check_dispatch(&ctx.config, name, args, approved)?;
    let arg = |k: &str| args.get(k).and_then(Value::as_str).map(str::trim).filter(|s| !s.is_empty()).map(str::to_string);
    match name {
        "web_search" => {
            let result = web_search(&ctx.config, &arg("query").ok_or("query is required")?).await?;
            ctx.public_urls.lock().unwrap().extend(result.sources.iter().map(|s| normalize_url(&s.url)));
            Ok(result)
        }
        "read_page" => {
            let url = arg("url").or_else(|| ctx.selection.as_deref().and_then(first_url)).or(ctx.page_url.clone());
            let url = url.ok_or("No page is open in the browser; ask the user for a link")?;
            let (url, allow_private) = ctx.check_url(&url).await?;
            let text = fetch_text(&url, allow_private).await?;
            Ok(ToolResult { content: text, sources: vec![Source { title: url.clone(), url }] })
        }
        "youtube" => {
            // No allow-list check: only the 11-character video id ever goes (to youtube.com).
            let url = arg("url").or_else(|| ctx.selection.as_deref().and_then(first_url)).or(ctx.page_url.clone());
            let id = url.as_deref().and_then(youtube_id).ok_or("No YouTube video found; open one or give me the link")?;
            let transcript = youtube_transcript(&id).await?;
            let url = format!("https://www.youtube.com/watch?v={id}");
            Ok(ToolResult { content: transcript, sources: vec![Source { title: "YouTube video".into(), url }] })
        }
        "deep_research" => {
            let question = arg("question").ok_or("question is required")?;
            spawn_deep_research(ctx.app.clone(), ctx.config.clone(), question);
            Ok(ToolResult::text("Deep research started in the background. Tell the user the write-up will appear in their history and they'll get a notification."))
        }
        "math_dates" => math_dates(args).map(ToolResult::text),
        "files_apps" => files_apps(args).map(ToolResult::text),
        "history_search" => {
            let state = ctx.app.state::<AppState>();
            let rows = state.db.search_history(&arg("query").ok_or("query is required")?, 8)?;
            if rows.is_empty() {
                return Ok(ToolResult::text("No matching dictations."));
            }
            let lines: Vec<String> = rows
                .iter()
                .map(|r| {
                    let text = r.answer.as_deref().filter(|a| !a.is_empty()).unwrap_or(&r.text);
                    format!("[{}] {}", &r.created_at[..16.min(r.created_at.len())], text)
                })
                .collect();
            Ok(ToolResult::text(lines.join("\n")))
        }
        "docs_search" => Ok(ToolResult::text(DOCS)),
        other => Err(format!("Unknown tool {other}")),
    }
}

// ---- web ----

fn http() -> Result<reqwest::Client, String> {
    http_with(true)
}

/// `allow_private` = false: redirects to local/private hosts are refused (SSRF).
fn http_with(allow_private: bool) -> Result<reqwest::Client, String> {
    let policy = reqwest::redirect::Policy::custom(move |attempt| {
        if attempt.previous().len() >= 5 {
            attempt.error("too many redirects")
        } else if !allow_private && attempt.url().host_str().is_none_or(is_private_host) {
            attempt.error("redirect to a local address")
        } else {
            attempt.follow()
        }
    });
    let builder = reqwest::Client::builder();
    let builder = if allow_private { builder } else { builder.dns_resolver(std::sync::Arc::new(PublicDns)) };
    builder
        .redirect(policy)
        .timeout(Duration::from_secs(20))
        .user_agent("Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/17.0 Safari/605.1.15 OpenGlaido")
        .build()
        .map_err(|e| e.to_string())
}

pub fn first_url(text: &str) -> Option<String> {
    text.split_whitespace()
        .map(|w| w.trim_matches(|c: char| matches!(c, '<' | '>' | '(' | ')' | '"' | '\'' | ',' | '.')))
        .find(|w| w.starts_with("http://") || w.starts_with("https://"))
        .map(str::to_string)
}

/// Readable text of an HTML page: drops script/style/nav chrome and tags, collapses whitespace.
pub fn html_to_text(html: &str) -> String {
    let mut out = String::with_capacity(html.len() / 4);
    let lower = html.to_ascii_lowercase();
    let mut i = 0;
    while i < html.len() {
        if html.as_bytes()[i] == b'<' {
            let skip = ["script", "style", "noscript", "svg", "nav", "footer", "head"]
                .iter()
                .find(|t| lower[i + 1..].starts_with(*t) && lower[i + 1 + t.len()..].starts_with(|c: char| c == '>' || c.is_whitespace()));
            if let Some(tag) = skip {
                let close = format!("</{tag}");
                i = lower[i..].find(&close).map_or(html.len(), |p| i + p + close.len());
            }
            let end = html[i..].find('>').map_or(html.len(), |p| i + p + 1);
            let tag = &lower[i..end];
            if ["<p", "<br", "<div", "<li", "<h1", "<h2", "<h3", "<tr", "</p", "</div", "</li"].iter().any(|t| tag.starts_with(t)) {
                out.push('\n');
            }
            i = end;
        } else {
            let next = html[i..].find('<').map_or(html.len(), |p| i + p);
            out.push_str(&html[i..next]);
            i = next;
        }
    }
    let out = out.replace("&nbsp;", " ").replace("&amp;", "&").replace("&lt;", "<").replace("&gt;", ">").replace("&quot;", "\"").replace("&#39;", "'");
    out.lines().map(|l| l.split_whitespace().collect::<Vec<_>>().join(" ")).filter(|l| !l.is_empty()).collect::<Vec<_>>().join("\n")
}

const PAGE_LIMIT: usize = 12_000;
/// Never buffer more than this from a page.
const BODY_LIMIT: usize = 2 * 1024 * 1024;

async fn fetch_text(url: &str, allow_private: bool) -> Result<String, String> {
    use futures_util::StreamExt;
    let host = reqwest::Url::parse(url).map_err(|e| format!("Bad link {url}: {e}"))?.host_str().unwrap_or("").to_string();
    if !allow_private && is_private_host(&host) {
        return Err(format!("{host} is a local address"));
    }
    let response = http_with(allow_private)?.get(url).send().await.map_err(|e| format!("Couldn't open {url}: {e}"))?;
    if !response.status().is_success() {
        return Err(format!("{url} returned {}", response.status()));
    }
    let content_type = response.headers().get("content-type").and_then(|v| v.to_str().ok()).unwrap_or("").to_ascii_lowercase();
    let is_html = content_type.is_empty() || content_type.contains("html");
    if !(is_html || content_type.starts_with("text/") || content_type.contains("json") || content_type.contains("xml")) {
        return Err(format!("{url} isn't a web page ({content_type})"));
    }
    let mut body = Vec::new();
    let mut stream = response.bytes_stream();
    while let Some(chunk) = stream.next().await {
        body.extend_from_slice(&chunk.map_err(|e| e.to_string())?);
        if body.len() >= BODY_LIMIT {
            break;
        }
    }
    let body = String::from_utf8_lossy(&body).into_owned();
    let text = if is_html { html_to_text(&body) } else { body };
    Ok(text.chars().take(PAGE_LIMIT).collect())
}

struct Hit {
    title: String,
    url: String,
    snippet: String,
}

async fn search(config: &TranscriptionConfig, query: &str, count: usize) -> Result<Vec<Hit>, String> {
    let client = http()?;
    let key = config.search_api_key.trim();
    let hits = |v: &Value, list: &str, snippet: &str| -> Vec<Hit> {
        v.pointer(list).and_then(Value::as_array).into_iter().flatten().take(count).map(|r| Hit {
            title: r.get("title").and_then(Value::as_str).unwrap_or("").to_string(),
            url: r.get("url").and_then(Value::as_str).unwrap_or("").to_string(),
            snippet: r.get(snippet).and_then(Value::as_str).unwrap_or("").to_string(),
        }).filter(|h| !h.url.is_empty()).collect()
    };
    let result = match config.search_provider.as_str() {
        "brave" => {
            let v: Value = client.get("https://api.search.brave.com/res/v1/web/search").query(&[("q", query), ("count", &count.to_string())])
                .header("X-Subscription-Token", key).header("Accept", "application/json")
                .send().await.map_err(|e| e.to_string())?.error_for_status().map_err(|e| e.to_string())?.json().await.map_err(|e| e.to_string())?;
            hits(&v, "/web/results", "description")
        }
        "tavily" => {
            let v: Value = client.post("https://api.tavily.com/search").json(&json!({"api_key": key, "query": query, "max_results": count}))
                .send().await.map_err(|e| e.to_string())?.error_for_status().map_err(|e| e.to_string())?.json().await.map_err(|e| e.to_string())?;
            hits(&v, "/results", "content")
        }
        "searxng" => {
            let base = config.searxng_url.trim().trim_end_matches('/');
            let v: Value = client.get(format!("{base}/search")).query(&[("q", query), ("format", "json")])
                .send().await.map_err(|e| e.to_string())?.error_for_status().map_err(|e| e.to_string())?.json().await.map_err(|e| e.to_string())?;
            hits(&v, "/results", "content")
        }
        _ => return Err("No search provider is set up (Commands › Web search)".into()),
    };
    Ok(result)
}

async fn web_search(config: &TranscriptionConfig, query: &str) -> Result<ToolResult, String> {
    let hits = search(config, query, 6).await.map_err(|e| format!("Web search failed: {e}"))?;
    let mut content = String::new();
    // Read the top three pages; snippets for the rest.
    for (i, hit) in hits.iter().enumerate() {
        let body = if i < 3 { fetch_text(&hit.url, false).await.ok().map(|t| t.chars().take(4000).collect()) } else { None };
        content.push_str(&format!("## [{}] {}\n{}\n{}\n\n", i + 1, hit.title, hit.url, body.unwrap_or_else(|| hit.snippet.clone())));
    }
    let sources = hits.into_iter().map(|h| Source { title: h.title, url: h.url }).collect();
    Ok(ToolResult { content, sources })
}

pub fn youtube_id(url: &str) -> Option<String> {
    let id = if let Some((_, rest)) = url.split_once("youtu.be/") {
        rest
    } else if url.contains("youtube.com/") {
        url.split_once("v=").map(|(_, r)| r).or_else(|| url.split_once("/shorts/").map(|(_, r)| r))?
    } else {
        return None;
    };
    let id: String = id.chars().take_while(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_').collect();
    (id.len() == 11).then_some(id)
}

async fn youtube_transcript(id: &str) -> Result<String, String> {
    let client = http()?;
    let page = client.get(format!("https://www.youtube.com/watch?v={id}")).header("Accept-Language", "en")
        .send().await.map_err(|e| e.to_string())?.text().await.map_err(|e| e.to_string())?;
    let tracks = page.split_once("\"captionTracks\":").map(|(_, r)| r).ok_or("This video has no captions")?;
    let base = tracks.split_once("\"baseUrl\":\"").and_then(|(_, r)| r.split_once('"')).map(|(u, _)| u.replace("\\u0026", "&")).ok_or("This video has no captions")?;
    let xml = client.get(base).send().await.map_err(|e| e.to_string())?.text().await.map_err(|e| e.to_string())?;
    let text = html_to_text(&xml.replace("</text>", "</text>\n"));
    if text.trim().is_empty() {
        return Err("Couldn't read the captions".into());
    }
    Ok(text.chars().take(40_000).collect())
}

fn spawn_deep_research(app: AppHandle, config: TranscriptionConfig, question: String) {
    tauri::async_runtime::spawn(async move {
        let state = app.state::<AppState>();
        let id = uuid::Uuid::new_v4().to_string();
        let mut row = crate::db::HistoryEntry {
            kind: "command".into(),
            status: "running".into(),
            text: question.clone(),
            raw_text: question.clone(),
            ..crate::db::HistoryEntry::new(id)
        };
        let _ = state.db.insert_history(&row);
        crate::history_updated(&app);
        let result = research(&app, &config, &question).await;
        match result {
            Ok((report, sources)) => {
                row.status = "ok".into();
                row.answer = Some(report);
                row.sources = sources;
            }
            Err(e) => {
                row.status = "failed".into();
                row.error = Some(e);
            }
        }
        let _ = state.db.update_history(&row);
        crate::history_updated(&app);
        super::notify(&app, "Deep research finished", &question);
    });
}

async fn research(app: &AppHandle, config: &TranscriptionConfig, question: &str) -> Result<(String, Vec<Source>), String> {
    let mut hits = search(config, question, 8).await?;
    let more = search(config, &format!("{question} analysis"), 6).await.unwrap_or_default();
    for h in more {
        if !hits.iter().any(|x| x.url == h.url) {
            hits.push(h);
        }
    }
    let mut notes = String::new();
    let mut sources = Vec::new();
    for hit in hits.iter().take(10) {
        if let Ok(text) = fetch_text(&hit.url, false).await {
            sources.push(Source { title: hit.title.clone(), url: hit.url.clone() });
            notes.push_str(&format!("## [{}] {}\n{}\n{}\n\n", sources.len(), hit.title, hit.url, text.chars().take(3500).collect::<String>()));
        }
    }
    if sources.is_empty() {
        return Err("Couldn't read any sources".into());
    }
    let llm = super::llm_from(app, config)?;
    let messages = vec![
        json!({"role":"system","content":"You write thorough, well-structured research reports in markdown. Cite sources inline as [n] using the numbered sources provided. End with a Sources list."}),
        json!({"role":"user","content": format!("Question: {question}\n\nSources:\n{notes}")}),
    ];
    let completion = llm.stream(&messages, &[], |_| {}).await?;
    Ok((completion.text, sources))
}

// ---- math & dates ----

/// Integer literals → floats so "7/2" is 3.5, and "x% of y" / "x%" → fractions.
pub fn prepare_expression(expr: &str) -> String {
    let expr = expr.replace('×', "*").replace('÷', "/").replace(',', "");
    let expr = expr.to_lowercase().replace(" of ", " * ");
    let mut out = String::new();
    let chars: Vec<char> = expr.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        if chars[i].is_ascii_digit() || (chars[i] == '.' && chars.get(i + 1).is_some_and(|c| c.is_ascii_digit())) {
            let start = i;
            while i < chars.len() && (chars[i].is_ascii_digit() || chars[i] == '.') {
                i += 1;
            }
            let mut num: String = chars[start..i].iter().collect();
            if !num.contains('.') {
                num.push_str(".0");
            }
            if chars.get(i) == Some(&'%') {
                i += 1;
                num = format!("({num}/100.0)");
            }
            out.push_str(&num);
        } else {
            out.push(chars[i]);
            i += 1;
        }
    }
    out
}

pub fn calculate(expr: &str) -> Result<String, String> {
    let value = evalexpr::eval(&prepare_expression(expr)).map_err(|e| format!("Can't calculate “{expr}”: {e}"))?;
    Ok(match value {
        evalexpr::Value::Float(f) if f.fract() == 0.0 && f.abs() < 1e15 => format!("{}", f as i64),
        evalexpr::Value::Float(f) => format!("{}", (f * 1e10).round() / 1e10),
        other => other.to_string(),
    })
}

fn tz(name: &str) -> Result<chrono_tz::Tz, String> {
    let aliases = [("pacific", "America/Los_Angeles"), ("pt", "America/Los_Angeles"), ("pst", "America/Los_Angeles"),
        ("eastern", "America/New_York"), ("et", "America/New_York"), ("est", "America/New_York"), ("central", "America/Chicago"),
        ("cet", "Europe/Berlin"), ("uk", "Europe/London"), ("gmt", "Etc/GMT"), ("utc", "UTC")];
    let lower = name.trim().to_lowercase();
    let name = aliases.iter().find(|(a, _)| *a == lower).map_or(name.trim(), |(_, n)| *n);
    name.parse().map_err(|_| format!("Unknown time zone “{name}” (use an IANA name like Europe/Paris)"))
}

fn math_dates(args: &Value) -> Result<String, String> {
    use chrono::{Local, NaiveTime, TimeZone, Utc};
    let arg = |k: &str| args.get(k).and_then(Value::as_str).map(str::trim).filter(|s| !s.is_empty());
    match arg("action").unwrap_or("calculate") {
        "calculate" => calculate(arg("expression").ok_or("expression is required")?),
        "now" => Ok(match arg("timezone") {
            Some(z) => Utc::now().with_timezone(&tz(z)?).format("%A %Y-%m-%d %H:%M %Z").to_string(),
            None => Local::now().format("%A %Y-%m-%d %H:%M (%:z, local)").to_string(),
        }),
        "add_days" => {
            let days = args.get("days").and_then(Value::as_f64).ok_or("days is required")?;
            let date = (days.abs() < 1e6)
                .then(|| chrono::Duration::try_days(days as i64))
                .flatten()
                .and_then(|d| Local::now().checked_add_signed(d))
                .ok_or("That date is out of range")?;
            Ok(date.format("%A %Y-%m-%d").to_string())
        }
        "convert_time" => {
            let time = NaiveTime::parse_from_str(arg("time").ok_or("time is required")?, "%H:%M").map_err(|_| "time must be HH:MM")?;
            let from = tz(arg("from").ok_or("from is required")?)?;
            let today = Utc::now().with_timezone(&from).date_naive();
            let at = from.from_local_datetime(&today.and_time(time)).single().ok_or("That time doesn't exist there today")?;
            Ok(match arg("to") {
                Some(to) => at.with_timezone(&tz(to)?).format("%H:%M %Z (%A)").to_string(),
                None => at.with_timezone(&Local).format("%H:%M local time (%A)").to_string(),
            })
        }
        other => Err(format!("Unknown action {other}")),
    }
}

// ---- files & apps ----

const READ_LIMIT: u64 = 200 * 1024;

fn files_apps(args: &Value) -> Result<String, String> {
    let arg = |k: &str| args.get(k).and_then(Value::as_str).map(str::trim).filter(|s| !s.is_empty());
    let home = std::env::var("HOME").or_else(|_| std::env::var("USERPROFILE")).unwrap_or_default();
    let expand = |p: &str| if let Some(rest) = p.strip_prefix("~/") { format!("{home}/{rest}") } else { p.to_string() };
    match arg("action").ok_or("action is required")? {
        "find" => find_files(&home, arg("query").ok_or("query is required")?),
        "read" => read_user_file(&home, &expand(arg("path").ok_or("path is required")?)),
        "open" => open_path(&expand(arg("path").ok_or("path is required")?)),
        "open_app" => open_app(arg("app").ok_or("app is required")?),
        other => Err(format!("Unknown action {other}")),
    }
}

/// Text files in the user's own folders only: never dotfiles/dot-folders (keys, tokens,
/// shell history) or ~/Library, and never devices or pipes.
fn read_user_file(home: &str, path: &str) -> Result<String, String> {
    let real = std::fs::canonicalize(path).map_err(|e| format!("Can't read {path}: {e}"))?;
    let home_real = std::fs::canonicalize(home).map_err(|e| e.to_string())?;
    let inside = real.strip_prefix(&home_real).map_err(|_| format!("{path} is outside your home folder"))?;
    let hidden = inside.components().any(|c| c.as_os_str().to_string_lossy().starts_with('.'));
    if hidden || inside.starts_with("Library") || inside.starts_with("AppData") {
        return Err(format!("{path} is in a private folder I don't read"));
    }
    let meta = std::fs::metadata(&real).map_err(|e| format!("Can't read {path}: {e}"))?;
    if !meta.is_file() {
        return Err(format!("{path} isn't a file"));
    }
    if meta.len() > READ_LIMIT {
        return Err(format!("{path} is larger than 200 KB"));
    }
    String::from_utf8(std::fs::read(&real).map_err(|e| e.to_string())?).map_err(|_| format!("{path} isn't a text file"))
}

#[cfg(target_os = "macos")]
fn find_files(home: &str, query: &str) -> Result<String, String> {
    let query = query.replace(['"', '\''], "");
    let by_name = format!("kMDItemDisplayName == \"*{query}*\"cd");
    let mut paths = Vec::new();
    for q in [by_name.as_str(), query.as_str()] {
        let out = std::process::Command::new("mdfind").args(["-onlyin", home, q]).output().map_err(|e| e.to_string())?;
        for line in String::from_utf8_lossy(&out.stdout).lines() {
            if paths.len() < 15 && !paths.iter().any(|p| p == line) && !line.contains("/Library/") && !line.contains("/.") {
                paths.push(line.to_string());
            }
        }
    }
    Ok(if paths.is_empty() { "Nothing found.".into() } else { paths.join("\n") })
}

#[cfg(not(target_os = "macos"))]
fn find_files(home: &str, query: &str) -> Result<String, String> {
    // ponytail: a bounded walk of the usual folders; Windows Search integration would be better.
    let query = query.to_lowercase();
    let mut found = Vec::new();
    let mut stack: Vec<(std::path::PathBuf, u8)> =
        ["Desktop", "Documents", "Downloads", "Pictures"].iter().map(|d| (std::path::Path::new(home).join(d), 0)).collect();
    while let Some((dir, depth)) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else { continue };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.file_name().is_some_and(|n| n.to_string_lossy().to_lowercase().contains(&query)) {
                found.push(path.display().to_string());
            }
            if path.is_dir() && depth < 3 {
                stack.push((path, depth + 1));
            }
            if found.len() >= 15 {
                return Ok(found.join("\n"));
            }
        }
    }
    Ok(if found.is_empty() { "Nothing found.".into() } else { found.join("\n") })
}

/// What a spoken command may open: documents and media (never anything that runs).
const OPENABLE: [&str; 44] = [
    "pdf", "txt", "md", "rtf", "csv", "json", "xml", "log", "html", "htm", "doc", "docx", "xls", "xlsx", "ppt", "pptx", "odt",
    "ods", "odp", "pages", "numbers", "key", "epub", "eml", "png", "jpg", "jpeg", "gif", "heic", "webp", "tiff", "bmp", "svg",
    "mp3", "m4a", "wav", "aac", "flac", "mp4", "mov", "m4v", "mkv", "webm", "avi",
];

/// Opens a document or folder in its default app. Allow-list, no app choice: injected text
/// must not be able to run code ("open x.command", `open -a Terminal notes.txt`, .app bundles).
fn open_path(path: &str) -> Result<String, String> {
    let target = validated_open_path(path)?;
    // Open exactly what was checked, not a link which LaunchServices/ShellExecute may follow.
    tauri_plugin_opener::open_path(target, None::<&str>).map_err(|e| format!("Couldn't open {path}: {e}"))?;
    Ok(format!("Opened {path}"))
}

fn validated_open_path(path: &str) -> Result<std::path::PathBuf, String> {
    // dunce preserves the resolved target while making Windows paths usable by ShellExecute.
    #[cfg(windows)]
    let target = dunce::canonicalize(path);
    #[cfg(not(windows))]
    let target = std::fs::canonicalize(path);
    let p = target.map_err(|_| format!("{path} doesn't exist"))?;
    let meta = std::fs::symlink_metadata(&p).map_err(|_| format!("{path} doesn't exist"))?;
    let ext = p.extension().map(|e| e.to_string_lossy().to_ascii_lowercase()).unwrap_or_default();
    // macOS bundles (.app, .prefPane, .workflow…) are folders that launch.
    let openable = if meta.is_dir() { !p.join("Contents").exists() && ext != "app" }
        else { meta.is_file() && OPENABLE.contains(&ext.as_str()) };
    if !openable {
        return Err(format!("I only open documents, media and folders, not {path}"));
    }
    #[cfg(target_os = "macos")]
    {
        use objc2_foundation::{NSNumber, NSURL, NSURLIsAliasFileKey};
        // Finder aliases are regular files, not POSIX symlinks. Fail closed if their type
        // cannot be determined; opening one would let macOS pick a different target.
        let is_alias = objc2::rc::autoreleasepool(|_| -> Result<bool, String> {
            let url = NSURL::from_file_path(&p).ok_or_else(|| format!("Can't check {path}"))?;
            let mut value = None;
            // Foundation documents NSURLIsAliasFileKey as an NSNumber boolean.
            unsafe { url.getResourceValue_forKey_error(&mut value, NSURLIsAliasFileKey) }
                .map_err(|_| format!("Can't check whether {path} is a Finder alias"))?;
            value.and_then(|v| v.downcast::<NSNumber>().ok()).map(|v| v.boolValue())
                .ok_or_else(|| format!("Can't check whether {path} is a Finder alias"))
        })?;
        if is_alias { return Err(format!("I don't open Finder aliases; choose the original document instead of {path}")); }
    }
    Ok(p)
}

fn open_app(app: &str) -> Result<String, String> {
    #[cfg(target_os = "macos")]
    {
        // `open -a` takes the name as one argument (no shell); it only launches installed apps.
        match std::process::Command::new("open").args(["-a", app]).status() {
            Ok(s) if s.success() => Ok(format!("Opened {app}")),
            _ => Err(format!("Couldn't find an app called {app}")),
        }
    }
    #[cfg(not(target_os = "macos"))]
    {
        // ponytail: opening apps by name needs a Start-menu lookup on Windows.
        Err(format!("Opening apps by name ({app}) isn't supported on Windows yet; open a file or folder instead"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn private_dispatch_requires_per_call_consent_even_if_arguments_claim_approval() {
        let mut cfg = TranscriptionConfig::default();
        for (name, args) in [
            ("files_apps", json!({"action":" read ", "path":"~/Documents/private.txt", "approved":true, "policy":"auto"})),
            ("files_apps", json!({"action":"find", "query":"contracts"})),
            ("history_search", json!({"query":"passwords", "approved":true})),
        ] {
            assert!(check_dispatch(&cfg, name, &args, false).is_err());
            assert!(check_dispatch(&cfg, name, &args, true).is_ok());
            // Approval of the previous call never changes the default for the next one.
            assert!(check_dispatch(&cfg, name, &args, false).is_err());
            cfg.builtin_tools.insert(name.into(), false);
            assert!(check_dispatch(&cfg, name, &args, true).is_err());
            cfg.builtin_tools.remove(name);
        }
        for (name, args) in [
            ("files_apps", json!({"action":"open_app", "app":"Calculator"})),
            ("files_apps", json!({"action":"open", "path":"~/Documents/note.txt"})),
            ("math_dates", json!({"action":"now"})),
            ("read_page", json!({})),
            ("docs_search", json!({})),
        ] {
            assert!(check_dispatch(&cfg, name, &args, false).is_ok());
        }
        assert!(check_dispatch(&cfg, "invented_tool", &json!({}), true).is_err());
    }

    #[test]
    fn math_is_exact() {
        assert_eq!(calculate("17.5% of 849").unwrap(), "148.575");
        assert_eq!(calculate("7/2").unwrap(), "3.5");
        assert_eq!(calculate("1,200 * 3").unwrap(), "3600");
        assert_eq!(calculate("(2+3)*4").unwrap(), "20");
        assert!(calculate("foo(").is_err());
    }

    #[test]
    fn time_conversion_and_zones() {
        assert!(tz("pacific").is_ok() && tz("Europe/Ljubljana").is_ok() && tz("Mars/Olympus").is_err());
        let out = math_dates(&json!({"action":"convert_time","time":"09:00","from":"UTC","to":"Asia/Tokyo"})).unwrap();
        assert!(out.starts_with("18:00"), "{out}");
        assert!(math_dates(&json!({"action":"add_days","days":10})).unwrap().len() > 10);
    }

    #[test]
    fn html_text_extraction() {
        let html = "<html><head><title>x</title><style>p{}</style></head><body><nav>menu</nav><h1>Title</h1><p>Hello&nbsp;<b>world</b> &amp; friends</p><script>var a=1<2;</script><div>Two</div></body></html>";
        assert_eq!(html_to_text(html), "Title\nHello world & friends\nTwo");
    }

    #[test]
    fn open_refuses_anything_that_runs() {
        let dir = std::env::temp_dir().join(format!("og-open-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(dir.join("Evil.app/Contents")).unwrap();
        std::fs::create_dir_all(dir.join("Tool.workflow/Contents")).unwrap();
        std::fs::create_dir_all(dir.join("Empty.app")).unwrap();
        std::fs::create_dir_all(dir.join("Documents")).unwrap();
        for f in ["run.command", "script", "notes.txt.sh"] {
            std::fs::write(dir.join(f), "").unwrap();
        }
        for f in ["note.txt", "photo.JPG", "report.pdf"] {
            std::fs::write(dir.join(f), "").unwrap();
        }
        for f in ["Evil.app", "Empty.app", "Tool.workflow", "run.command", "script", "notes.txt.sh", "missing.pdf"] {
            assert!(validated_open_path(dir.join(f).to_str().unwrap()).is_err(), "{f}");
        }
        for f in ["Documents", "note.txt", "photo.JPG", "report.pdf"] {
            assert_eq!(validated_open_path(dir.join(f).to_str().unwrap()).unwrap().canonicalize().unwrap(), std::fs::canonicalize(dir.join(f)).unwrap());
        }
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn open_validates_symlink_targets_and_rejects_special_files() {
        use std::os::unix::{fs::symlink, net::UnixListener};

        let dir = std::env::temp_dir().join(format!("og-{}", uuid::Uuid::new_v4().simple()));
        std::fs::create_dir_all(dir.join("Evil.app/Contents")).unwrap();
        std::fs::create_dir_all(dir.join("Tool.workflow/Contents")).unwrap();
        for f in ["run.command", "script.sh"] {
            std::fs::write(dir.join(f), "").unwrap();
        }
        std::fs::write(dir.join("note.txt"), "document").unwrap();
        for (link, target) in [
            ("command.txt", "run.command"),
            ("shell.txt", "script.sh"),
            ("chain.txt", "command.txt"),
            ("application.txt", "Evil.app"),
            ("workflow.txt", "Tool.workflow"),
            ("dangling.txt", "missing.txt"),
            ("linked-folder", "."),
            ("document.txt", "note.txt"),
        ] {
            symlink(target, dir.join(link)).unwrap();
        }
        let socket = UnixListener::bind(dir.join("s.txt")).unwrap();
        for f in ["command.txt", "shell.txt", "chain.txt", "application.txt", "workflow.txt", "dangling.txt", "linked-folder/command.txt", "s.txt"] {
            assert!(validated_open_path(dir.join(f).to_str().unwrap()).is_err(), "{f}");
        }
        assert_eq!(validated_open_path(dir.join("linked-folder").to_str().unwrap()).unwrap(), std::fs::canonicalize(&dir).unwrap());
        let original = validated_open_path(dir.join("document.txt").to_str().unwrap()).unwrap();
        assert_eq!(original, std::fs::canonicalize(dir.join("note.txt")).unwrap());
        // The opener receives the resolved target, so retargeting the original link cannot redirect it.
        std::fs::remove_file(dir.join("document.txt")).unwrap();
        symlink("run.command", dir.join("document.txt")).unwrap();
        assert_eq!(std::fs::read_to_string(original).unwrap(), "document");
        assert!(validated_open_path(dir.join("document.txt").to_str().unwrap()).is_err());
        drop(socket);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn open_rejects_native_finder_aliases_disguised_as_documents() {
        use objc2_foundation::{NSString, NSURL, NSURLBookmarkCreationOptions};

        let dir = std::env::temp_dir().join(format!("og-alias-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        for (target, alias) in [("run.command", "script-alias.txt"), ("note.txt", "document-alias.txt")] {
            let path = dir.join(target);
            std::fs::write(&path, "").unwrap();
            let target_url = NSURL::fileURLWithPath(&NSString::from_str(path.to_str().unwrap()));
            let data = target_url.bookmarkDataWithOptions_includingResourceValuesForKeys_relativeToURL_error(
                NSURLBookmarkCreationOptions::SuitableForBookmarkFile, None, None,
            ).unwrap();
            let alias_path = dir.join(alias);
            let alias_url = NSURL::fileURLWithPath(&NSString::from_str(alias_path.to_str().unwrap()));
            NSURL::writeBookmarkData_toURL_options_error(&data, &alias_url, 0).unwrap();
            assert!(alias_path.is_file());
            assert!(validated_open_path(alias_path.to_str().unwrap()).is_err(), "{alias}");
        }
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn private_hosts_are_recognised() {
        for h in [
            "localhost", "localhost.", "127.0.0.1", "10.0.0.5", "192.168.1.1", "172.16.3.4", "169.254.1.1", "[::1]", "fd00::1",
            "printer.local", "0.0.0.0", "[::ffff:127.0.0.1]", "[::ffff:192.168.1.1]", "100.100.1.1", "0.1.2.3",
        ] {
            assert!(is_private_host(h), "{h}");
        }
        for h in ["example.com", "8.8.8.8", "2606:4700::1111", "github.com", "100.128.0.1"] {
            assert!(!is_private_host(h), "{h}");
        }
    }

    #[test]
    fn user_files_only() {
        let home = std::env::temp_dir().join(format!("og-home-{}", std::process::id()));
        std::fs::create_dir_all(home.join(".ssh")).unwrap();
        std::fs::create_dir_all(home.join("Documents")).unwrap();
        std::fs::write(home.join(".ssh/id_ed25519"), "secret").unwrap();
        std::fs::write(home.join("Documents/note.txt"), "hello").unwrap();
        let h = home.to_str().unwrap();
        assert_eq!(read_user_file(h, &format!("{h}/Documents/note.txt")).unwrap(), "hello");
        assert!(read_user_file(h, &format!("{h}/.ssh/id_ed25519")).is_err());
        assert!(read_user_file(h, &format!("{h}/Documents/../.ssh/id_ed25519")).is_err());
        assert!(read_user_file(h, "/etc/hosts").is_err());
        assert!(read_user_file(h, &format!("{h}/Documents")).is_err());
        std::fs::remove_dir_all(&home).unwrap();
    }

    #[test]
    fn model_urls_need_the_user_or_search() {
        // Only the pure parts: ToolContext needs an AppHandle, so check the URL helpers.
        assert_eq!(normalize_url("https://a.com/x/#frag"), "https://a.com/x");
        assert_eq!(all_urls("read (https://a.com/x) and http://b.org."), ["https://a.com/x", "http://b.org"]);
        assert!(math_dates(&json!({"action":"add_days","days":1e18})).is_err());
    }

    #[test]
    fn urls_and_youtube_ids() {
        assert_eq!(first_url("see (https://example.com/a) now").as_deref(), Some("https://example.com/a"));
        assert_eq!(first_url("no links"), None);
        assert_eq!(youtube_id("https://www.youtube.com/watch?v=dQw4w9WgXcQ&t=1").as_deref(), Some("dQw4w9WgXcQ"));
        assert_eq!(youtube_id("https://youtu.be/dQw4w9WgXcQ").as_deref(), Some("dQw4w9WgXcQ"));
        assert_eq!(youtube_id("https://example.com/watch?v=x"), None);
    }

    #[test]
    fn availability_follows_search_setup() {
        let mut cfg = TranscriptionConfig::default();
        let web = |cfg: &TranscriptionConfig| builtin_tools(cfg).into_iter().find(|t| t.id == "web_search").unwrap();
        assert!(!web(&cfg).available);
        assert_eq!(schemas(&cfg).len(), 6);
        cfg.search_provider = "searxng".into();
        cfg.searxng_url = "http://localhost:8080".into();
        assert!(web(&cfg).available);
        cfg.builtin_tools.insert("files_apps".into(), false);
        let names: Vec<String> = schemas(&cfg).iter().map(|s| s["function"]["name"].as_str().unwrap().to_string()).collect();
        assert!(names.contains(&"web_search".to_string()) && !names.contains(&"files_apps".to_string()));
    }
}
