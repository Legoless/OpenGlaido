//! Local models: the download catalog, the downloader and the whisper.cpp / llama.cpp runtimes.

mod catalog;
pub mod llama;
pub mod whisper;

pub use catalog::CATALOG;

use futures_util::StreamExt;
use reqwest::Client;
use serde::Serialize;
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, LazyLock, Mutex};
use std::time::{Duration, Instant};
use tauri::{AppHandle, Emitter, Manager};
use tokio::io::AsyncWriteExt;

#[derive(Debug, Clone, Serialize)]
pub struct CatalogModel {
    pub id: &'static str,
    /// "stt" (whisper.cpp ggml) or "llm" (GGUF for llama.cpp).
    pub kind: &'static str,
    pub name: &'static str,
    pub notes: &'static str,
    pub file: &'static str,
    pub url: &'static str,
    pub size_bytes: u64,
    pub sha256: &'static str,
    pub english_only: bool,
    /// 1–5, relative on Apple Silicon.
    pub speed: u8,
    pub accuracy: u8,
    pub recommended: bool,
    pub license: &'static str,
    /// LLM prompt format: "chatml", "gemma4" (hand-built turns), "mistral-v7-tekken". Empty for STT.
    #[serde(skip)]
    pub template: &'static str,
}

#[derive(Debug, Clone, Serialize)]
pub struct DownloadProgress {
    pub id: String,
    pub received: u64,
    pub total: u64,
    /// "downloading" | "verifying" | "done" | "failed" | "cancelled"
    pub state: String,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct LocalModel {
    #[serde(flatten)]
    pub model: CatalogModel,
    pub downloaded: bool,
    /// Bytes of an unfinished download kept for resuming (0 = none).
    pub partial_bytes: u64,
    pub download: Option<DownloadProgress>,
}

/// The latest progress per model id (running, or how the last attempt ended) and its cancel flag.
type Downloads = HashMap<String, (DownloadProgress, Arc<AtomicBool>)>;
static DOWNLOADS: LazyLock<Mutex<Downloads>> = LazyLock::new(Default::default);

const UNSUPPORTED: &str = "Local models are only available on macOS";

pub fn supported() -> bool {
    cfg!(target_os = "macos")
}

pub fn find(id: &str) -> Option<&'static CatalogModel> {
    CATALOG.iter().find(|m| m.id == id)
}

pub fn models_dir(app: &AppHandle) -> PathBuf {
    let dir = app.path().app_data_dir().unwrap_or_else(|_| PathBuf::from("./data")).join("models");
    if let Err(e) = std::fs::create_dir_all(&dir) {
        eprintln!("Couldn't create {}: {}", dir.display(), e);
    }
    dir
}

/// The model file when it is fully downloaded (exists with the catalog size).
pub fn path_if_downloaded(app: &AppHandle, id: &str) -> Option<PathBuf> {
    let model = find(id)?;
    let path = models_dir(app).join(model.file);
    (std::fs::metadata(&path).ok()?.len() == model.size_bytes).then_some(path)
}

/// Whether the settings still run catalog model `id` locally.
fn selected(app: &AppHandle, id: &str) -> bool {
    let state = app.state::<crate::AppState>();
    let c = state.config.lock().unwrap();
    (c.stt_source == "local" && c.local_stt_model == id) || (c.llm_source == "local" && c.local_llm_model == id)
}

#[tauri::command]
pub fn list_local_models(app: AppHandle) -> Vec<LocalModel> {
    let downloads = DOWNLOADS.lock().unwrap();
    let dir = models_dir(&app);
    CATALOG
        .iter()
        .map(|m| LocalModel {
            model: m.clone(),
            downloaded: path_if_downloaded(&app, m.id).is_some(),
            partial_bytes: std::fs::metadata(part_path(&dir.join(m.file))).map_or(0, |f| f.len()),
            download: downloads.get(m.id).map(|(p, _)| p.clone()),
        })
        .collect()
}

#[tauri::command]
pub fn local_models_supported() -> bool {
    supported()
}

fn in_flight(progress: &DownloadProgress) -> bool {
    matches!(progress.state.as_str(), "downloading" | "verifying")
}

#[tauri::command]
pub async fn download_model(app: AppHandle, id: String) -> Result<(), String> {
    let model = find(&id).ok_or("Unknown model")?;
    let cancel = Arc::new(AtomicBool::new(false));
    let progress = |received, state: &str, error| DownloadProgress {
        id: id.clone(),
        received,
        total: model.size_bytes,
        state: state.to_string(),
        error,
    };
    {
        let mut downloads = DOWNLOADS.lock().unwrap();
        if downloads.get(&id).is_some_and(|(p, _)| in_flight(p)) {
            return Err("Already downloading".into());
        }
        downloads.insert(id.clone(), (progress(0, "downloading", None), cancel.clone()));
    }
    let report = |p: DownloadProgress| {
        let mut downloads = DOWNLOADS.lock().unwrap();
        if p.state == "done" {
            // The list shows `downloaded` instead.
            downloads.remove(&p.id);
        } else if let Some((current, _)) = downloads.get_mut(&p.id) {
            *current = p.clone();
        }
        drop(downloads);
        let _ = app.emit("model-download", p);
    };

    let client = Client::builder()
        .connect_timeout(Duration::from_secs(15))
        .read_timeout(Duration::from_secs(30))
        .build()
        .map_err(|e| e.to_string())?;
    let dest = models_dir(&app).join(model.file);
    let mut received = 0;
    let result = download_to(&client, model.url, &dest, model.size_bytes, model.sha256, &cancel, |r, state| {
        received = r;
        report(progress(r, state, None));
    })
    .await;
    match result {
        Ok(()) => {
            report(progress(model.size_bytes, "done", None));
            // If it's the selected model, load it now so the first dictation doesn't wait for it.
            let config = app.state::<crate::AppState>().config.lock().unwrap().clone();
            crate::load_local_models(&app, &config, None);
        }
        Err(_) if cancel.load(Ordering::Relaxed) => report(progress(received, "cancelled", None)),
        Err(e) => {
            eprintln!("Download of {id} failed: {e}");
            report(progress(received, "failed", Some(e.clone())));
            return Err(e);
        }
    }
    Ok(())
}

/// Where an unfinished download of `dest` is kept.
fn part_path(dest: &Path) -> PathBuf {
    dest.with_file_name(format!("{}.part", dest.file_name().unwrap_or_default().to_string_lossy()))
}

/// Downloads `url` into `<dest>.part`, resuming a partial file, verifies the size and SHA-256 of the whole
/// file and renames it to `dest`. `on_progress(received, state)` runs at most 10×/s while downloading.
/// Cancelling keeps the .part file; a size or checksum mismatch deletes it.
async fn download_to(
    client: &Client,
    url: &str,
    dest: &Path,
    size: u64,
    sha256: &str,
    cancel: &AtomicBool,
    mut on_progress: impl FnMut(u64, &'static str),
) -> Result<(), String> {
    let part = part_path(dest);
    let mut received = std::fs::metadata(&part).map(|m| m.len()).unwrap_or(0);
    if received > size {
        received = 0;
    }
    let mismatch = |message: &str| {
        let _ = std::fs::remove_file(&part);
        message.to_string()
    };

    if received < size {
        let mut request = client.get(url);
        if received > 0 {
            request = request.header(reqwest::header::RANGE, format!("bytes={received}-"));
        }
        let response = request.send().await.map_err(|e| {
            eprintln!("Model download request failed: {e}");
            let host = reqwest::Url::parse(url).ok().and_then(|u| u.host_str().map(str::to_string));
            format!("Couldn't reach {}", host.as_deref().unwrap_or(url))
        })?;
        received = resume_offset(response.status().as_u16(), received)?;
        if response.content_length().is_some_and(|len| received + len != size) {
            return Err(mismatch("The server sent a different file than expected. Try again later."));
        }

        let file = tokio::fs::OpenOptions::new()
            .create(true)
            .write(true)
            .append(received > 0)
            .truncate(received == 0)
            .open(&part)
            .await
            .map_err(save_error)?;
        let mut file = tokio::io::BufWriter::with_capacity(1 << 20, file);
        let mut stream = response.bytes_stream();
        let mut last = Instant::now();
        on_progress(received, "downloading");
        loop {
            // Wakes up regularly, so Cancel works even while the connection is stalled.
            let next = tokio::time::timeout(Duration::from_millis(250), stream.next()).await;
            if cancel.load(Ordering::Relaxed) {
                file.flush().await.map_err(save_error)?;
                return Err("Cancelled".into());
            }
            let Ok(next) = next else { continue };
            let Some(chunk) = next else { break };
            let chunk = match chunk {
                Ok(chunk) => chunk,
                Err(e) => {
                    eprintln!("Model download interrupted: {e}");
                    file.flush().await.map_err(save_error)?;
                    return Err("The download was interrupted. Check your connection and try again.".into());
                }
            };
            received += chunk.len() as u64;
            if received > size {
                return Err(mismatch("The downloaded file is larger than expected. Try again."));
            }
            file.write_all(&chunk).await.map_err(save_error)?;
            if last.elapsed() >= Duration::from_millis(100) {
                last = Instant::now();
                on_progress(received, "downloading");
            }
        }
        file.flush().await.map_err(save_error)?;
        if received != size {
            return Err(mismatch("The download ended early. Try again."));
        }
    }

    on_progress(size, "verifying");
    let path = part.clone();
    let hash = tokio::task::spawn_blocking(move || -> std::io::Result<String> {
        let mut hasher = Sha256::new();
        std::io::copy(&mut std::fs::File::open(path)?, &mut hasher)?;
        Ok(hex::encode(hasher.finalize()))
    })
    .await
    .map_err(|e| e.to_string())?
    .map_err(save_error)?;
    if !hash.eq_ignore_ascii_case(sha256) {
        return Err(mismatch("The downloaded file is damaged (checksum mismatch). Try again."));
    }
    std::fs::rename(&part, dest).map_err(save_error)
}

/// Where the body of a (possibly ranged) response starts: 206 continues at `existing`, 200 restarts at 0.
fn resume_offset(status: u16, existing: u64) -> Result<u64, String> {
    match status {
        206 if existing > 0 => Ok(existing),
        200 => Ok(0),
        _ => Err(format!("The download failed: the server returned {status}")),
    }
}

fn save_error(e: std::io::Error) -> String {
    if e.kind() == ErrorKind::StorageFull {
        "Not enough disk space for this model".into()
    } else {
        format!("Couldn't save the model: {e}")
    }
}

#[tauri::command]
pub fn cancel_model_download(id: String) {
    if let Some((_, cancel)) = DOWNLOADS.lock().unwrap().get(&id) {
        cancel.store(true, Ordering::Relaxed);
    }
}

#[tauri::command]
pub fn delete_model(app: AppHandle, id: String) -> Result<(), String> {
    let model = find(&id).ok_or("Unknown model")?;
    if let Some((_, cancel)) = DOWNLOADS.lock().unwrap().remove(&id) {
        cancel.store(true, Ordering::Relaxed);
    }
    let path = models_dir(&app).join(model.file);
    match model.kind {
        "stt" => whisper::unload_path(&path),
        _ => llama::unload_path(&path),
    }
    for file in [part_path(&path), path] {
        match std::fs::remove_file(&file) {
            Err(e) if e.kind() != ErrorKind::NotFound => return Err(format!("Couldn't delete {}: {e}", file.display())),
            _ => {}
        }
    }
    Ok(())
}

/// Model ids from GET {base_url}/models. `kind` "stt" or "llm" filters the list.
#[tauri::command]
pub async fn list_provider_models(base_url: String, api_key: String, kind: String) -> Result<Vec<String>, String> {
    let base = base_url.trim().trim_end_matches('/');
    let mut url = format!("{base}/models");
    if kind == "stt" && base.contains("openrouter.ai") {
        url.push_str("?output_modalities=transcription");
    }
    let host = reqwest::Url::parse(base).ok().and_then(|u| u.host_str().map(str::to_string));
    let unreachable = format!("Couldn't reach {}", host.as_deref().unwrap_or(base));
    let client = Client::builder().timeout(Duration::from_secs(10)).build().map_err(|e| e.to_string())?;
    let mut request = client.get(&url);
    if !api_key.trim().is_empty() {
        request = request.bearer_auth(api_key.trim());
    }
    let response = request.send().await.map_err(|e| {
        eprintln!("Listing models at {url} failed: {e}");
        unreachable
    })?;
    let status = response.status().as_u16();
    let text = response.text().await.unwrap_or_default();
    match status {
        401 | 403 => return Err("The provider rejected the API key".into()),
        // Gemini answers a bad key with 400 "Please pass a valid API key".
        400 if text.to_lowercase().contains("api key") => return Err("The provider rejected the API key".into()),
        404 => return Err("This provider doesn't list its models; type the model name".into()),
        status if !(200..300).contains(&status) => return Err(format!("The provider returned an error ({status})")),
        _ => {}
    }
    let body: Value = serde_json::from_str(&text).map_err(|_| "The provider sent an unexpected model list".to_string())?;
    Ok(model_ids(&body, &kind))
}

#[derive(Clone, Copy)]
enum KeyCheck {
    Models,
    OpenRouter,
    ElevenLabs,
}

/// Only these known endpoints require authentication. A public/custom model list cannot prove a key works.
fn key_check_endpoint(base: &str) -> Option<(String, KeyCheck)> {
    let base = base.trim().trim_end_matches('/');
    let (path, check) = match base {
        "https://api.openai.com/v1" | "https://api.groq.com/openai/v1" | "https://api.mistral.ai/v1"
        | "https://generativelanguage.googleapis.com/v1beta/openai" => ("models", KeyCheck::Models),
        "https://openrouter.ai/api/v1" => ("key", KeyCheck::OpenRouter),
        "https://api.elevenlabs.io/v1" => ("user", KeyCheck::ElevenLabs),
        _ => return None,
    };
    Some((format!("{base}/{path}"), check))
}

/// Checks authentication only, without generating text/audio or checking model access or credit balance.
/// False means no key or no supported authenticated check; a failed check never blocks saving a key.
#[tauri::command]
pub async fn validate_provider_key(base_url: String, api_key: String) -> Result<bool, String> {
    let Some((url, check)) = key_check_endpoint(&base_url) else { return Ok(false) };
    check_provider_key(&url, &api_key, check).await
}

async fn check_provider_key(url: &str, api_key: &str, check: KeyCheck) -> Result<bool, String> {
    let key = api_key.trim();
    if key.is_empty() {
        return Ok(false);
    }
    let client = Client::builder()
        .timeout(Duration::from_secs(10))
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .map_err(|_| "Couldn't check the API key".to_string())?;
    let request = client.get(url);
    let request = match check {
        KeyCheck::ElevenLabs => request.header("xi-api-key", key),
        _ => request.bearer_auth(key),
    };
    // Never include request errors, URLs, or response bodies: they may contain credentials.
    let response = request.send().await.map_err(|_| "Couldn't reach the provider to check the API key".to_string())?;
    let status = response.status().as_u16();
    if !(200..300).contains(&status) {
        return Err(format!("Couldn't verify the API key (HTTP {status})"));
    }
    let unexpected = "The provider sent an unexpected API key check response";
    let body: Value = response.json().await.map_err(|_| unexpected.to_string())?;
    let verified = body.get("error").is_none()
        && match check {
            KeyCheck::Models => body.get("data").and_then(Value::as_array).is_some_and(|models| {
                models.iter().all(|model| model.get("id").and_then(Value::as_str).is_some_and(|id| !id.is_empty()))
            }),
            KeyCheck::OpenRouter => body.pointer("/data/label").is_some_and(Value::is_string),
            KeyCheck::ElevenLabs => body.get("user_id").and_then(Value::as_str).is_some_and(|id| !id.is_empty()),
        };
    if !verified {
        return Err(unexpected.into());
    }
    Ok(true)
}

/// Ids from `{data: [{id}]}` or a bare `[{id}]`, without Gemini's `models/` prefix, filtered by `kind`,
/// sorted and deduped.
fn model_ids(body: &Value, kind: &str) -> Vec<String> {
    const STT: &[&str] = &["whisper", "transcribe", "voxtral", "parakeet", "nova"];
    const NOT_CHAT: &[&str] = &[
        "whisper", "transcribe", "tts", "embed", "guard", "moderation", "dall-e", "image", "audio", "realtime", "voxtral",
    ];
    let items = body.get("data").unwrap_or(body).as_array().map(Vec::as_slice).unwrap_or_default();
    let mut ids: Vec<String> = items
        .iter()
        .filter_map(|m| m.get("id")?.as_str())
        .map(|id| id.strip_prefix("models/").unwrap_or(id).to_string())
        .filter(|id| {
            let lower = id.to_lowercase();
            let any = |words: &[&str]| words.iter().any(|w| lower.contains(w));
            match kind {
                "stt" => any(STT),
                "llm" => !any(NOT_CHAT),
                _ => true,
            }
        })
        .collect();
    ids.sort();
    ids.dedup();
    ids
}

#[tauri::command]
pub fn reveal_models_folder(app: AppHandle) -> Result<(), String> {
    let dir = models_dir(&app);
    tauri_plugin_opener::open_path(&dir, None::<&str>).map_err(|e| format!("Couldn't open {}: {e}", dir.display()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use tokio::io::AsyncReadExt;
    use tokio::net::TcpListener;

    #[test]
    fn key_checks_use_only_known_authenticated_endpoints() {
        for (base, suffix) in [
            ("https://api.openai.com/v1", "models"),
            ("https://api.groq.com/openai/v1", "models"),
            ("https://api.mistral.ai/v1", "models"),
            ("https://generativelanguage.googleapis.com/v1beta/openai", "models"),
            ("https://openrouter.ai/api/v1", "key"),
            ("https://api.elevenlabs.io/v1", "user"),
        ] {
            assert_eq!(key_check_endpoint(&format!(" {base}/ ")).unwrap().0, format!("{base}/{suffix}"));
        }
        for base in [
            "http://api.openai.com/v1", "https://api.openai.com.evil.test/v1", "https://evil.test/api.openai.com/v1",
            "https://api.openai.com:444/v1", "https://api.openai.com/v1?key=secret", "https://api.openai.com/v2",
            "http://localhost:1234/v1", "not a URL",
        ] {
            assert!(key_check_endpoint(base).is_none());
        }
    }

    async fn serve_key_check(status: u16, body: &str, location: Option<&str>) -> (String, tokio::task::JoinHandle<String>) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}/check", listener.local_addr().unwrap());
        let body = body.to_string();
        let redirect = location.map(|url| format!("Location: {url}\r\n")).unwrap_or_default();
        let served = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut request = Vec::new();
            let mut buf = [0u8; 1024];
            while !request.windows(4).any(|w| w == b"\r\n\r\n") {
                let n = socket.read(&mut buf).await.unwrap();
                assert!(n > 0);
                request.extend_from_slice(&buf[..n]);
            }
            let response = format!(
                "HTTP/1.1 {status} Result\r\nContent-Type: application/json\r\nContent-Length: {}\r\n{redirect}Connection: close\r\n\r\n{body}",
                body.len()
            );
            socket.write_all(response.as_bytes()).await.unwrap();
            String::from_utf8(request).unwrap()
        });
        (url, served)
    }

    #[tokio::test]
    async fn provider_key_checks_require_authenticated_response_shapes() {
        for (check, body, header) in [
            (KeyCheck::Models, r#"{"data":[{"id":"test-model"}]}"#, "authorization: Bearer fixture-key"),
            (KeyCheck::Models, r#"{"data":[]}"#, "authorization: Bearer fixture-key"),
            (KeyCheck::OpenRouter, r#"{"data":{"label":"fixture"}}"#, "authorization: Bearer fixture-key"),
            (KeyCheck::ElevenLabs, r#"{"user_id":"fixture-user"}"#, "xi-api-key: fixture-key"),
        ] {
            let (url, served) = serve_key_check(200, body, None).await;
            assert_eq!(check_provider_key(&url, " fixture-key ", check).await, Ok(true));
            let request = served.await.unwrap();
            assert!(request.starts_with("GET /check HTTP/1.1\r\n"));
            assert!(request.contains(header), "missing expected authentication header");
            assert_eq!(request.matches("fixture-key").count(), 1);
        }
        assert_eq!(validate_provider_key("https://api.openai.com/v1".into(), "  ".into()).await, Ok(false));
        assert_eq!(validate_provider_key("https://custom.test/v1".into(), "fixture-key".into()).await, Ok(false));
    }

    #[tokio::test]
    async fn provider_key_checks_reject_errors_and_malformed_responses_without_echoing_secrets() {
        for (status, body, check) in [
            (401, "fixture-key", KeyCheck::Models),
            (403, "fixture-key", KeyCheck::ElevenLabs),
            (400, "fixture-key", KeyCheck::Models),
            (500, "fixture-key", KeyCheck::OpenRouter),
            (200, "fixture-key", KeyCheck::Models),
            (200, "", KeyCheck::Models),
            (200, r#"{"error":"fixture-key"}"#, KeyCheck::Models),
            (200, r#"{"data":[{}]}"#, KeyCheck::Models),
            (200, r#"{"data":[]}"#, KeyCheck::OpenRouter),
            (200, r#"{"user_id":""}"#, KeyCheck::ElevenLabs),
        ] {
            let (url, served) = serve_key_check(status, body, None).await;
            let error = check_provider_key(&format!("{url}?secret=fixture-key"), "fixture-key", check).await.unwrap_err();
            assert!(!error.contains("fixture-key") && !error.contains(&url));
            served.await.unwrap();
        }
        let error = check_provider_key("http://127.0.0.1:1/check?secret=fixture-key", "fixture-key", KeyCheck::Models)
            .await.unwrap_err();
        assert!(!error.contains("fixture-key") && !error.contains("127.0.0.1"));
    }

    #[tokio::test]
    async fn provider_key_checks_never_forward_credentials_on_redirects() {
        for check in [KeyCheck::Models, KeyCheck::ElevenLabs] {
            let (target, target_served) = serve_key_check(200, r#"{"data":[],"user_id":"fixture-user"}"#, None).await;
            let (source, source_served) = serve_key_check(302, "", Some(&target)).await;
            assert!(check_provider_key(&source, "fixture-key", check).await.unwrap_err().contains("302"));
            assert!(source_served.await.unwrap().contains("fixture-key"));
            assert!(!target_served.is_finished());
            target_served.abort();
        }
    }

    #[test]
    fn model_ids_parse_and_filter() {
        let body = json!({ "data": [
            { "id": "whisper-large-v3-turbo" }, { "id": "openai/gpt-oss-20b" }, { "id": "playai-tts" },
            { "id": "meta-llama/llama-guard-4-12b" }, { "id": "gpt-4o-mini-transcribe" }, { "id": "openai/gpt-oss-20b" },
            { "id": "text-embedding-3-small" }, { "id": "voxtral-mini-latest" }, { "id": "gpt-realtime" }, { "object": "x" },
        ]});
        assert_eq!(model_ids(&body, "stt"), ["gpt-4o-mini-transcribe", "voxtral-mini-latest", "whisper-large-v3-turbo"]);
        assert_eq!(model_ids(&body, "llm"), ["openai/gpt-oss-20b"]);
        // Bare arrays (Together) and Gemini's "models/" prefix.
        let bare = json!([{ "id": "models/gemini-3.5-flash-lite" }, { "id": "models/text-embedding-004" }]);
        assert_eq!(model_ids(&bare, "llm"), ["gemini-3.5-flash-lite"]);
        assert!(model_ids(&json!({ "error": "nope" }), "llm").is_empty());
    }

    #[test]
    fn local_models_serialize_flat_for_the_ui() {
        // src/types.ts LocalModel: the catalog fields at the top level, without the prompt template.
        let model = LocalModel { model: CATALOG[0].clone(), downloaded: true, partial_bytes: 0, download: None };
        let json = serde_json::to_value(model).unwrap();
        assert_eq!((json["id"].as_str(), json["size_bytes"].as_u64()), (Some(CATALOG[0].id), Some(CATALOG[0].size_bytes)));
        assert!(json["downloaded"] == true && json["download"].is_null());
        assert!(json.get("model").is_none() && json.get("template").is_none());
    }

    #[test]
    fn resume_offset_follows_the_status() {
        assert_eq!(resume_offset(206, 1000), Ok(1000));
        assert_eq!(resume_offset(200, 1000), Ok(0));
        assert_eq!(resume_offset(200, 0), Ok(0));
        assert!(resume_offset(206, 0).is_err());
        assert!(resume_offset(404, 0).is_err());
        assert!(resume_offset(416, 10).is_err());
    }

    /// Serves `body` over HTTP on localhost; honours `Range: bytes=N-` when `ranges` is true.
    /// Returns the URL and the Range start of every request (None = no Range header).
    async fn serve(body: Vec<u8>, ranges: bool) -> (String, Arc<Mutex<Vec<Option<usize>>>>) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}/model.bin", listener.local_addr().unwrap());
        let seen = Arc::new(Mutex::new(Vec::new()));
        let log = seen.clone();
        tokio::spawn(async move {
            loop {
                let (mut socket, _) = listener.accept().await.unwrap();
                let mut request = Vec::new();
                let mut buf = [0u8; 1024];
                while !request.windows(4).any(|w| w == b"\r\n\r\n") {
                    let n = socket.read(&mut buf).await.unwrap();
                    request.extend_from_slice(&buf[..n]);
                }
                let request = String::from_utf8_lossy(&request).to_lowercase();
                let start = request
                    .lines()
                    .find_map(|l| l.strip_prefix("range: bytes="))
                    .map(|r| r.trim().trim_end_matches('-').parse::<usize>().unwrap());
                log.lock().unwrap().push(start);
                let (status, slice) = match start.filter(|_| ranges) {
                    Some(s) => ("206 Partial Content", &body[s..]),
                    None => ("200 OK", &body[..]),
                };
                let head = format!("HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", slice.len());
                socket.write_all(head.as_bytes()).await.unwrap();
                socket.write_all(slice).await.unwrap();
            }
        });
        (url, seen)
    }

    #[tokio::test]
    async fn download_resumes_verifies_and_rejects_bad_hashes() {
        let body: Vec<u8> = (0..5000u32).map(|i| (i * 7 % 251) as u8).collect();
        let sha = hex::encode(Sha256::digest(&body));
        let dir = std::env::temp_dir().join(format!("og-download-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let dest = dir.join("model.bin");
        let part = dir.join("model.bin.part");
        let client = Client::new();
        let cancel = AtomicBool::new(false);
        let size = body.len() as u64;

        // Resume: the server continues after the 1200 bytes already on disk.
        let (url, seen) = serve(body.clone(), true).await;
        std::fs::write(&part, &body[..1200]).unwrap();
        let mut states = Vec::new();
        download_to(&client, &url, &dest, size, &sha, &cancel, |r, s| states.push((r, s))).await.unwrap();
        assert_eq!(std::fs::read(&dest).unwrap(), body);
        assert!(!part.exists());
        assert_eq!(*seen.lock().unwrap(), [Some(1200)]);
        assert_eq!(states.first(), Some(&(1200, "downloading")));
        assert_eq!(states.last(), Some(&(size, "verifying")));

        // A server that ignores Range (200) restarts the file instead of appending to stale bytes.
        std::fs::remove_file(&dest).unwrap();
        let (url, seen) = serve(body.clone(), false).await;
        std::fs::write(&part, vec![0xAA; 3000]).unwrap();
        download_to(&client, &url, &dest, size, &sha, &cancel, |_, _| {}).await.unwrap();
        assert_eq!(std::fs::read(&dest).unwrap(), body);
        assert_eq!(*seen.lock().unwrap(), [Some(3000)]);

        // A checksum mismatch (bad resumed bytes) fails and deletes the .part file.
        std::fs::remove_file(&dest).unwrap();
        let (url, _) = serve(body.clone(), true).await;
        std::fs::write(&part, vec![0xAA; 1200]).unwrap();
        let err = download_to(&client, &url, &dest, size, &sha, &cancel, |_, _| {}).await.unwrap_err();
        assert!(err.contains("checksum"), "{err}");
        assert!(!part.exists() && !dest.exists());

        // Cancelling keeps the .part file for a later resume.
        let (url, _) = serve(body.clone(), true).await;
        cancel.store(true, Ordering::Relaxed);
        assert!(download_to(&client, &url, &dest, size, &sha, &cancel, |_, _| {}).await.is_err());
        assert!(part.exists() && !dest.exists());
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
