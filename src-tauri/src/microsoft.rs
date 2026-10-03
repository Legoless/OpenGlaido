//! Azure MAI transcription adapters. Resource URLs and headers are distinct from OpenAI.
use crate::transcribe::TranscriptionConfig;
use reqwest::{multipart::{Form, Part}, Url};
use serde_json::{json, Value};
use std::time::Duration;

pub const BATCH_MODEL: &str = "mai-transcribe-2";
pub const LIVE_MODEL: &str = "mai-transcribe-2-streaming";
pub const DEFAULT_DEPLOYMENT: &str = "MAI-Transcribe-2-Streaming";
const ENDPOINT_ERROR: &str = "Transcription failed: check the Azure resource URL in Settings › Model";
const KEY_ERROR: &str = "Transcription failed: check the API key in Settings › Model";
const RESPONSE_ERROR: &str = "Transcription failed: unexpected response from the speech-to-text endpoint";

pub fn is_model(model: &str) -> bool { matches!(model, BATCH_MODEL | LIVE_MODEL) }

/// Accept resource roots only; never forward Azure keys to an arbitrary custom endpoint.
fn resource_url(endpoint: &str, live: bool) -> Result<Url, String> {
    let mut url = Url::parse(endpoint.trim()).map_err(|_| ENDPOINT_ERROR.to_string())?;
    let host = url.host_str().unwrap_or_default().to_string();
    let resource = host.strip_suffix(".services.ai.azure.com")
        .or_else(|| host.strip_suffix(".cognitiveservices.azure.com")).filter(|name| !name.is_empty());
    let azure = resource.is_some();
    let loopback = cfg!(test) && matches!(host.as_str(), "localhost" | "127.0.0.1" | "[::1]");
    if !(azure && url.scheme() == "https" || loopback && matches!(url.scheme(), "http" | "https"))
        || !url.username().is_empty() || url.password().is_some() || url.query().is_some()
        || url.fragment().is_some() || !matches!(url.path(), "" | "/") {
        return Err(ENDPOINT_ERROR.into());
    }
    if let Some(resource) = resource {
        // A Foundry resource exposes both documented FQDNs under the same custom subdomain.
        // Select the service host only for requests; keep the saved URL/key scope unchanged.
        let suffix = if live { "services.ai.azure.com" } else { "cognitiveservices.azure.com" };
        url.set_host(Some(&format!("{resource}.{suffix}"))).map_err(|_| ENDPOINT_ERROR)?;
    }
    Ok(url)
}

fn batch_url(endpoint: &str) -> Result<Url, String> {
    let mut url = resource_url(endpoint, false)?;
    url.set_path("/speechtotext/transcriptions:transcribe");
    url.set_query(Some("api-version=2025-10-15"));
    Ok(url)
}

pub fn validate_endpoint(config: &TranscriptionConfig) -> Result<(), String> {
    if !is_model(&config.model_name) { return Err("Transcription failed: choose a Microsoft transcription model".into()); }
    resource_url(&config.endpoint_url, config.model_name == LIVE_MODEL)?;
    if config.stt_deployment.chars().any(char::is_control) {
        return Err("Transcription failed: check the Azure deployment name in Settings › Model".into());
    }
    Ok(())
}

pub fn live_url(config: &TranscriptionConfig) -> Result<String, String> {
    let mut url = resource_url(&config.endpoint_url, true)?;
    let scheme = if url.scheme() == "https" { "wss" } else { "ws" };
    url.set_scheme(scheme).map_err(|_| ENDPOINT_ERROR)?;
    url.set_path("/mai/v1/realtime");
    url.set_query(Some("intent=transcription"));
    Ok(url.to_string())
}

fn single_language(config: &TranscriptionConfig) -> Option<&str> {
    let languages: Vec<_> = config.languages.iter().map(|lang| lang.trim()).filter(|lang| !lang.is_empty()).collect();
    match languages.as_slice() { [language] => Some(*language), _ => None }
}

pub fn live_session(config: &TranscriptionConfig, rate: u32) -> Value {
    let deployment = config.stt_deployment.trim();
    // The dedicated MAI Realtime schema does not expose dictionary keywords or prompts.
    // Local replacement rules still run after its final transcript is received.
    json!({ "type": "session.update", "session": { "type": "transcription", "audio": { "input": {
        "format": { "type": "audio/pcm", "rate": rate },
        "transcription": { "model": if deployment.is_empty() { DEFAULT_DEPLOYMENT } else { deployment }, "language": single_language(config) },
        "turn_detection": null, "noise_reduction": null
    }}}})
}

fn batch_definition(config: &TranscriptionConfig, vocabulary: &[String]) -> Value {
    let mut definition = json!({ "enhancedMode": { "enabled": true, "model": "MAI-Transcribe-2", "modelOptions": { "transcribeStyle": "verbatim" } } });
    if let Some(language) = single_language(config) { definition["locales"] = json!([language]); }
    let mut seen = std::collections::BTreeSet::new();
    let phrases: Vec<_> = vocabulary.iter().map(|phrase| phrase.trim())
        .filter(|phrase| !phrase.is_empty() && !phrase.chars().any(char::is_control) && seen.insert(*phrase)).collect();
    if !phrases.is_empty() { definition["phraseList"] = json!({ "phrases": phrases }); }
    definition
}

fn transcript(response: &Value) -> Result<String, String> {
    let combined = response["combinedPhrases"].as_array().ok_or(RESPONSE_ERROR)?;
    let phrases: Result<Vec<_>, _> = combined.iter().map(|phrase| phrase["text"].as_str().map(str::trim).ok_or(RESPONSE_ERROR)).collect();
    Ok(phrases?.into_iter().filter(|text| !text.is_empty()).collect::<Vec<_>>().join(" "))
}

pub async fn transcribe(wav: Vec<u8>, config: &TranscriptionConfig, vocabulary: &[String]) -> Result<String, String> {
    if config.model_name != BATCH_MODEL { return Err("Transcription failed: choose a Microsoft transcription model".into()); }
    let url = batch_url(&config.endpoint_url)?;
    let mut key = reqwest::header::HeaderValue::from_str(config.api_key.trim()).map_err(|_| KEY_ERROR)?;
    if config.api_key.trim().is_empty() { return Err(KEY_ERROR.into()); }
    key.set_sensitive(true);
    // Azure subscription headers survive default redirect cleanup. Reject redirects entirely.
    let client = reqwest::Client::builder().redirect(reqwest::redirect::Policy::none())
        .timeout(Duration::from_secs(240)).build().map_err(|_| "Transcription failed: couldn't reach the speech-to-text endpoint")?;
    let form = Form::new()
        .part("audio", Part::bytes(wav).file_name("audio.wav").mime_str("audio/wav").map_err(|_| RESPONSE_ERROR)?)
        .text("definition", batch_definition(config, vocabulary).to_string());
    let response = client.post(url).header("Ocp-Apim-Subscription-Key", key).multipart(form).send().await
        .map_err(|error| if error.is_timeout() { "Transcription failed: Microsoft transcription timed out" } else { "Transcription failed: couldn't reach the speech-to-text endpoint" })?;
    if !response.status().is_success() {
        return Err(match response.status().as_u16() {
            401 | 403 => KEY_ERROR.to_string(),
            404 => ENDPOINT_ERROR.to_string(),
            code => format!("Transcription failed: the server returned HTTP {code}"),
        });
    }
    transcript(&response.json::<Value>().await.map_err(|_| RESPONSE_ERROR)?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::{io::{AsyncReadExt, AsyncWriteExt}, net::TcpListener};

    fn test_config(endpoint: String) -> TranscriptionConfig {
        TranscriptionConfig { stt_provider: "microsoft".into(), model_name: BATCH_MODEL.into(), endpoint_url: endpoint, api_key: "azure-test-secret".into(), languages: vec![" sl ".into()], ..Default::default() }
    }

    #[test]
    fn azure_urls_deployments_and_languages_are_explicit() {
        let mut config = test_config("https://test.services.ai.azure.com".into());
        assert!(validate_endpoint(&config).is_ok());
        assert_eq!(live_url(&config).unwrap(), "wss://test.services.ai.azure.com/mai/v1/realtime?intent=transcription");
        let input = live_session(&config, 24000)["session"]["audio"]["input"].clone();
        assert_eq!(input["transcription"], json!({ "model": DEFAULT_DEPLOYMENT, "language": "sl" }));
        assert_eq!(input["format"], json!({ "type": "audio/pcm", "rate": 24000 }));
        assert!(input["turn_detection"].is_null() && input["noise_reduction"].is_null());
        config.stt_deployment = "my-custom-deployment".into();
        config.languages.push("en".into());
        assert_eq!(live_session(&config, 24000)["session"]["audio"]["input"]["transcription"], json!({ "model": "my-custom-deployment", "language": null }));
        let definition = batch_definition(&config, &[" OpenGlaido ".into(), "OpenGlaido".into(), "".into(), "bad\nterm".into(), "東京".into()]);
        assert_eq!(definition["phraseList"]["phrases"], json!(["OpenGlaido", "東京"]));
        assert!(definition.get("locales").is_none());
        assert!(!live_session(&config, 24000).to_string().contains("keywords"));
        assert!(resource_url("https://test.cognitiveservices.azure.com", false).is_ok());
        assert!(resource_url("https://test.cognitiveservices.azure.com", true).is_ok());
        for url in ["http://test.services.ai.azure.com", "https://evil.com", "https://test.services.ai.azure.com.evil.com", "https://test.services.ai.azure.com/v1", "https://user:secret@test.services.ai.azure.com", "https://test.services.ai.azure.com?api-key=secret", "https://test.services.ai.azure.com#secret"] {
            assert!(resource_url(url, false).is_err(), "{url}");
        }
        assert_eq!(transcript(&json!({ "combinedPhrases": [{"text":" Hello "}, {"text":"Živjo."}] })).unwrap(), "Hello Živjo.");
        assert_eq!(transcript(&json!({ "combinedPhrases": [] })).unwrap(), "");
        assert!(transcript(&json!({ "text":"wrong protocol" })).is_err());
        assert!(transcript(&json!({ "combinedPhrases": [{"text": 1}] })).is_err());
    }

    #[test]
    fn either_foundry_root_selects_the_correct_service_without_changing_key_scope() {
        for endpoint in ["https://same-resource.services.ai.azure.com", "https://same-resource.cognitiveservices.azure.com/"] {
            let mut config = test_config(endpoint.into());
            let saved_endpoint = config.endpoint_url.clone();
            assert!(validate_endpoint(&config).is_ok());
            assert_eq!(batch_url(&config.endpoint_url).unwrap().as_str(), "https://same-resource.cognitiveservices.azure.com/speechtotext/transcriptions:transcribe?api-version=2025-10-15");
            config.model_name = LIVE_MODEL.into();
            assert!(validate_endpoint(&config).is_ok());
            assert_eq!(live_url(&config).unwrap(), "wss://same-resource.services.ai.azure.com/mai/v1/realtime?intent=transcription");
            assert_eq!(config.endpoint_url, saved_endpoint);
            assert_eq!(config.api_key, "azure-test-secret");
        }
        assert_eq!(batch_url("http://127.0.0.1:3210").unwrap().as_str(), "http://127.0.0.1:3210/speechtotext/transcriptions:transcribe?api-version=2025-10-15");
        let config = test_config("http://127.0.0.1:3210".into());
        assert_eq!(live_url(&config).unwrap(), "ws://127.0.0.1:3210/mai/v1/realtime?intent=transcription");
    }

    async fn request(listener: TcpListener, status: &str, body: &str) -> Vec<u8> {
        let (mut socket, _) = listener.accept().await.unwrap();
        let mut bytes = Vec::new();
        let mut buffer = [0; 4096];
        loop {
            let count = socket.read(&mut buffer).await.unwrap();
            assert!(count > 0);
            bytes.extend_from_slice(&buffer[..count]);
            if let Some(end) = bytes.windows(4).position(|window| window == b"\r\n\r\n") {
                let headers = String::from_utf8_lossy(&bytes[..end]).to_ascii_lowercase();
                let length: usize = headers.lines().find_map(|line| line.strip_prefix("content-length: ")).unwrap().trim().parse().unwrap();
                if bytes.len() >= end + 4 + length { break; }
            }
        }
        socket.write_all(format!("HTTP/1.1 {status}\r\nContent-Length: {}\r\nContent-Type: application/json\r\nConnection: close\r\n\r\n{body}", body.len()).as_bytes()).await.unwrap();
        bytes
    }

    #[tokio::test]
    async fn batch_contract_sends_audio_definition_hints_and_subscription_header() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let config = test_config(format!("http://{}", listener.local_addr().unwrap()));
        let server = tokio::spawn(async move { request(listener, "200 OK", r#"{"combinedPhrases":[{"text":"Živjo OpenGlaido"}]}"#).await });
        assert_eq!(transcribe(b"WAV-BYTES".to_vec(), &config, &["OpenGlaido".into()]).await.unwrap(), "Živjo OpenGlaido");
        let bytes = server.await.unwrap();
        let text = String::from_utf8(bytes).unwrap();
        assert!(text.starts_with("POST /speechtotext/transcriptions:transcribe?api-version=2025-10-15 "));
        assert!(text.to_ascii_lowercase().contains("ocp-apim-subscription-key: azure-test-secret"));
        assert!(!text.to_ascii_lowercase().contains("authorization:"));
        assert!(text.contains("name=\"audio\"; filename=\"audio.wav\"") && text.contains("WAV-BYTES"));
        assert!(text.contains("name=\"definition\""));
        assert!(text.contains(&batch_definition(&config, &["OpenGlaido".into()]).to_string()));
    }

    #[tokio::test]
    async fn batch_failures_hide_key_and_audio_and_never_follow_redirects() {
        let destination = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let config = test_config(format!("http://{}", listener.local_addr().unwrap()));
        let location = format!("http://{}", destination.local_addr().unwrap());
        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut buffer = [0; 4096];
            assert!(socket.read(&mut buffer).await.unwrap() > 0);
            socket.write_all(format!("HTTP/1.1 307 Temporary Redirect\r\nLocation: {location}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").as_bytes()).await.unwrap();
        });
        assert!(transcribe(b"PRIVATE AUDIO".to_vec(), &config, &[]).await.unwrap_err().contains("HTTP 307"));
        server.await.unwrap();
        assert!(tokio::time::timeout(Duration::from_millis(100), destination.accept()).await.is_err());
        for (status, body) in [("401 Unauthorized", "azure-test-secret PRIVATE AUDIO"), ("200 OK", r#"{"unexpected":"azure-test-secret"}"#)] {
            let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
            let config = test_config(format!("http://{}", listener.local_addr().unwrap()));
            let server = tokio::spawn(async move { request(listener, status, body).await });
            let error = transcribe(b"PRIVATE AUDIO".to_vec(), &config, &[]).await.unwrap_err();
            assert!(!error.contains("azure-test-secret") && !error.contains("PRIVATE AUDIO"));
            server.await.unwrap();
        }
    }
}
