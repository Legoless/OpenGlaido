//! One push-to-talk recording per OpenAI, ElevenLabs, or Azure MAI live session.
use crate::audio::{AudioChunk, StreamInput};
use crate::transcribe::TranscriptionConfig;
use base64::{engine::general_purpose::STANDARD, Engine};
use futures_util::{future::Either, stream::FuturesOrdered, SinkExt, StreamExt};
use serde_json::{json, Value};
use std::io::Cursor;
use std::sync::{atomic::{AtomicBool, Ordering}, Arc};
use std::time::Duration;
use tokio::net::TcpStream;
use tokio::sync::{mpsc, oneshot};
use tokio::time::{timeout, timeout_at, Instant};
use tokio_tungstenite::{connect_async, tungstenite::{client::IntoClientRequest, Message}, MaybeTlsStream, WebSocketStream};

const RATE: u32 = 24_000;
const PACKET_BYTES: usize = 960 * 2; // 40 ms of PCM16.
const IO_TIMEOUT: Duration = Duration::from_secs(10);
const SETUP_ATTEMPTS: u32 = 3;
const FINAL_TIMEOUT: Duration = Duration::from_secs(45);
const CONNECT_TIMEOUT_ERROR: &str = "Transcription failed: timed out connecting to the live speech-to-text endpoint";
const SESSION_TIMEOUT_ERROR: &str = "Transcription failed: timed out waiting for the live session to start";
const SEND_TIMEOUT_ERROR: &str = "Transcription failed: timed out sending to the live speech-to-text endpoint";
const FINAL_TIMEOUT_ERROR: &str = "Transcription failed: timed out waiting for the final live transcript";
const PROCESSING_TIMEOUT_ERROR: &str = "Transcription failed: timed out waiting for live processing";
type Socket = WebSocketStream<MaybeTlsStream<TcpStream>>;

pub fn is_live_model(model: &str) -> bool {
    model == crate::microsoft::LIVE_MODEL || model == "scribe_v2_realtime" || model == "gpt-live-transcribe" || model.strip_prefix("gpt-live-transcribe-").is_some_and(|suffix| {
        suffix.len() == 10 && suffix.bytes().enumerate().all(|(i, c)| {
            if i == 4 || i == 7 { c == b'-' } else { c.is_ascii_digit() }
        })
    })
}

pub struct LiveTranscription {
    finish_tx: Option<oneshot::Sender<()>>,
    final_timeout: Duration,
    task: tauri::async_runtime::JoinHandle<Result<String, String>>,
    local_cancel: Option<crate::processing::Cancellation>,
}

impl LiveTranscription {
    /// Starts connecting in the background; the bounded queue preserves speech during setup.
    pub fn start(config: &TranscriptionConfig, vocabulary: Vec<String>) -> Result<(Self, StreamInput), String> {
        Self::start_with_setup_timeout(config, vocabulary, IO_TIMEOUT)
    }

    fn start_with_setup_timeout(config: &TranscriptionConfig, vocabulary: Vec<String>, setup_timeout: Duration) -> Result<(Self, StreamInput), String> {
        let elevenlabs = config.model_name == "scribe_v2_realtime";
        let microsoft = config.model_name == crate::microsoft::LIVE_MODEL;
        let final_timeout = if microsoft { Duration::from_secs(240) } else { FINAL_TIMEOUT };
        let request = connection_request(config, &vocabulary)?;
        let session = (!elevenlabs).then(|| if microsoft { crate::microsoft::live_session(config, RATE) } else { session_update(config, &vocabulary) });
        let api_key = config.api_key.trim().to_string();
        // ponytail: bounded callback queue; batch in the audio worker if very small buffers need more setup time.
        let (tx, rx) = mpsc::channel(2048);
        let overflowed = Arc::new(AtomicBool::new(false));
        let overflow_check = overflowed.clone();
        let (finish_tx, finish_rx) = oneshot::channel();
        let task = tauri::async_runtime::spawn(async move {
            let mut socket = connect_session(request, session, &api_key, elevenlabs, microsoft, setup_timeout).await?;
            if elevenlabs { stream_elevenlabs(&mut socket, rx, finish_rx, &overflow_check, &api_key).await }
            else { stream(&mut socket, rx, finish_rx, &overflow_check, &api_key, microsoft).await }
        });
        Ok((Self { finish_tx: Some(finish_tx), final_timeout, task, local_cancel: None }, StreamInput { tx, overflowed }))
    }

    pub(crate) fn from_local(task: tauri::async_runtime::JoinHandle<Result<String, String>>, finish_tx: oneshot::Sender<()>, cancellation: crate::processing::Cancellation) -> Self {
        Self { finish_tx: Some(finish_tx), final_timeout: Duration::from_secs(600), task, local_cancel: Some(cancellation) }
    }

    /// Call after physically stopping the microphone; callbacks can otherwise retain senders.
    pub fn finish_input(&mut self) {
        if let Some(tx) = self.finish_tx.take() { let _ = tx.send(()); }
    }

    pub async fn finish(mut self) -> Result<String, String> {
        self.finish_input();
        timeout(self.final_timeout, &mut self.task).await
            .map_err(|_| PROCESSING_TIMEOUT_ERROR.to_string())?
            .map_err(|_| "Transcription failed: unexpected response from the speech-to-text endpoint".to_string())?
    }
}

impl Drop for LiveTranscription {
    fn drop(&mut self) {
        if let Some(cancellation) = &self.local_cancel { cancellation.cancel(); }
        self.task.abort();
    }
}

/// Performs the same local validation as starting a session, without opening a socket.
pub fn validate_live_config(config: &TranscriptionConfig) -> Result<(), String> {
    connection_request(config, &[]).map(|_| ())
}

async fn connect_session(request: tokio_tungstenite::tungstenite::http::Request<()>, session: Option<Value>, api_key: &str, elevenlabs: bool, microsoft: bool, attempt_timeout: Duration) -> Result<Socket, String> {
    // Each cloud provider gets three complete pre-audio setup attempts, at most
    // 10 seconds each / 30 seconds total. Keep the audio queue outside this loop.
    let setup_deadline = Instant::now() + attempt_timeout * SETUP_ATTEMPTS;
    let mut last_error = CONNECT_TIMEOUT_ERROR.to_string();
    for _ in 0..SETUP_ATTEMPTS {
        let deadline = (Instant::now() + attempt_timeout).min(setup_deadline);
        match connect_session_once(&request, session.as_ref(), api_key, elevenlabs, microsoft, deadline).await {
            Ok(socket) => return Ok(socket),
            Err(error) => {
                // Only retry setup timeouts, before any audio is sent. Preserve
                // explicit authentication, HTTP and provider errors immediately.
                if !matches!(error.as_str(), CONNECT_TIMEOUT_ERROR | SESSION_TIMEOUT_ERROR | SEND_TIMEOUT_ERROR) { return Err(error); }
                last_error = error;
            }
        }
    }
    Err(last_error)
}

async fn connect_session_once(request: &tokio_tungstenite::tungstenite::http::Request<()>, session: Option<&Value>, api_key: &str, elevenlabs: bool, microsoft: bool, deadline: Instant) -> Result<Socket, String> {
    let (mut socket, _) = timeout_at(deadline, connect_async(request.clone())).await
        .map_err(|_| CONNECT_TIMEOUT_ERROR.to_string())?
        .map_err(|error| -> String {
            match error {
                tokio_tungstenite::tungstenite::Error::Http(response) => match response.status().as_u16() {
                    401 | 403 => "Transcription failed: check the API key in Settings › Model".into(),
                    code => format!("Transcription failed: the server returned HTTP {code}"),
                },
                _ => "Transcription failed: couldn't reach the speech-to-text endpoint".into(),
            }
        })?;
    if microsoft {
        timeout_at(deadline, async {
            while receive(&mut socket, api_key).await?["type"] != "session.created" {}
            Ok::<_, String>(())
        }).await.map_err(|_| SESSION_TIMEOUT_ERROR.to_string())??;
    }
    if let Some(session) = session {
        timeout_at(deadline, send(&mut socket, session.clone())).await
            .map_err(|_| SEND_TIMEOUT_ERROR.to_string())??;
    }
    timeout_at(deadline, async {
        loop {
            let event = receive(&mut socket, api_key).await?;
            if (!elevenlabs && event["type"] == "session.updated") || (elevenlabs && event["message_type"] == "session_started") { return Ok::<_, String>(()); }
        }
    }).await.map_err(|_| SESSION_TIMEOUT_ERROR.to_string())??;
    Ok(socket)
}

fn connection_request(config: &TranscriptionConfig, vocabulary: &[String]) -> Result<tokio_tungstenite::tungstenite::http::Request<()>, String> {
    if !is_live_model(&config.model_name) {
        return Err("Backup transcription providers must use a real-time model".into());
    }
    let elevenlabs = config.model_name == "scribe_v2_realtime";
    let microsoft = config.model_name == crate::microsoft::LIVE_MODEL;
    if microsoft { crate::microsoft::validate_endpoint(config)?; }
    let url = if microsoft { crate::microsoft::live_url(config)? } else if elevenlabs { elevenlabs_url(config, vocabulary)? } else { realtime_url(&config.endpoint_url)? };
    if config.api_key.trim().is_empty() {
        return Err("Transcription failed: check the API key in Settings › Model".into());
    }
    let mut request = url.into_client_request().map_err(|_| "Transcription failed: check the endpoint URL in Settings › Model")?;
    let (header, value) = if microsoft { ("api-key", config.api_key.trim().to_string()) } else if elevenlabs { ("xi-api-key", config.api_key.trim().to_string()) } else { ("Authorization", format!("Bearer {}", config.api_key.trim())) };
    let mut value = tokio_tungstenite::tungstenite::http::HeaderValue::from_str(&value)
        .map_err(|_| "Transcription failed: check the API key in Settings › Model")?;
    value.set_sensitive(true);
    request.headers_mut().insert(header, value);
    Ok(request)
}

/// All configured live providers receive the recording from the start. Results are
/// preferred in settings order, while each provider's finalization runs concurrently.
pub struct LiveTranscriptionGroup {
    attempts: Vec<(String, Result<LiveTranscription, String>)>,
}

impl LiveTranscriptionGroup {
    pub fn start_with_primary(
        config: &TranscriptionConfig,
        primary: Result<(LiveTranscription, StreamInput), String>,
        vocabulary: Vec<String>,
    ) -> Result<(Self, Vec<StreamInput>), String> {
        let mut attempts = Vec::new();
        let mut inputs = Vec::new();
        for (model, attempt) in std::iter::once((model_label(config), primary)).chain(config.live_backup_configs().into_iter().map(|backup| {
            (model_label(&backup), LiveTranscription::start(&backup, vocabulary.clone()))
        })) {
            match attempt {
                Ok((live, input)) => {
                    attempts.push((model, Ok(live)));
                    inputs.push(input);
                }
                Err(error) => attempts.push((model, Err(error))),
            }
        }
        if inputs.is_empty() {
            let multiple = attempts.len() > 1;
            return Err(combined_errors(attempts.into_iter().filter_map(|(model, attempt)| attempt.err().map(|error| label_error(error, &model, multiple))).collect()));
        }
        Ok((Self { attempts }, inputs))
    }

    pub fn finish_input(&mut self) {
        for (_, attempt) in &mut self.attempts {
            if let Ok(live) = attempt { live.finish_input(); }
        }
    }

    pub async fn finish(mut self) -> Result<String, String> {
        self.finish_input();
        let mut pending = FuturesOrdered::new();
        let multiple = self.attempts.len() > 1;
        for (model, attempt) in self.attempts {
            pending.push_back(async move {
                let result = match attempt { Ok(live) => live.finish().await, Err(error) => Err(error) };
                result.map_err(|error| label_error(error, &model, multiple))
            });
        }
        preferred_result(pending).await
    }
}

async fn preferred_result<F: std::future::Future<Output = Result<String, String>>>(mut pending: FuturesOrdered<F>) -> Result<String, String> {
    let mut errors = Vec::new();
    while let Some(result) = pending.next().await {
        match result {
            // An empty successful transcript retains the usual quiet no-speech behavior.
            Ok(text) => return Ok(text),
            Err(error) => errors.push(error),
        }
    }
    Err(combined_errors(errors))
}

fn model_label(config: &TranscriptionConfig) -> String {
    if config.stt_source == "local" { config.local_stt_model.clone() } else { config.model_name.clone() }
}

fn label_error(error: String, model: &str, multiple: bool) -> String {
    if multiple { format!("Transcription failed: {model}: {}", error.strip_prefix("Transcription failed: ").unwrap_or(&error)) } else { error }
}

fn combined_errors(errors: Vec<String>) -> String {
    if errors.is_empty() { "Transcription failed: check the endpoint URL in Settings › Model".into() }
    else { errors.join("; ") }
}

fn websocket_url(endpoint: &str) -> Result<reqwest::Url, String> {
    let invalid = || "Transcription failed: check the endpoint URL in Settings › Model".to_string();
    let mut url = reqwest::Url::parse(endpoint.trim()).map_err(|_| invalid())?;
    if !url.username().is_empty() || url.password().is_some() || url.fragment().is_some() { return Err(invalid()); }
    let scheme = match url.scheme() {
        "https" | "wss" => "wss",
        "http" | "ws" if matches!(url.host_str(), Some("localhost" | "127.0.0.1" | "[::1]")) => "ws",
        _ => return Err(invalid()),
    };
    url.set_scheme(scheme).map_err(|_| invalid())?;
    Ok(url)
}

fn realtime_url(endpoint: &str) -> Result<String, String> {
    let mut url = websocket_url(endpoint)?;
    let path = url.path().trim_end_matches('/');
    let path = if let Some(base) = path.strip_suffix("/audio/transcriptions") {
        format!("{base}/realtime")
    } else if path.ends_with("/realtime") {
        path.to_string()
    } else { return Err("Transcription failed: check the endpoint URL in Settings › Model".into()); };
    url.set_path(&path);
    url.set_query(Some("intent=transcription"));
    Ok(url.to_string())
}

/// Both Scribe APIs reject oversized/invalid keyterms. Keep valid dictionary phrases intact.
pub fn elevenlabs_keyterms(vocabulary: &[String], live: bool) -> Vec<String> {
    let mut terms = Vec::new();
    let mut seen = std::collections::BTreeSet::new();
    for phrase in vocabulary {
        if phrase.chars().any(|c| c.is_control() || "<>{}[]\\".contains(c)) { continue; }
        let term = phrase.trim();
        if term.is_empty() || term.chars().count() > if live { 20 } else { 49 }
            || term.split_whitespace().count() > 5 || !seen.insert(term.to_string()) { continue; }
        terms.push(term.to_string());
        // ponytail: cap batch hints at 100 to avoid its extra 20s minimum billing; expose more only with explicit cost consent.
        if terms.len() == if live { 50 } else { 100 } { break; }
    }
    terms
}

fn elevenlabs_url(config: &TranscriptionConfig, vocabulary: &[String]) -> Result<String, String> {
    let mut url = websocket_url(&config.endpoint_url)?;
    let path = url.path().trim_end_matches('/');
    let path = if path.ends_with("/speech-to-text") { format!("{path}/realtime") }
        else if path.ends_with("/speech-to-text/realtime") { path.to_string() }
        else { return Err("Transcription failed: check the endpoint URL in Settings › Model".into()); };
    url.set_path(&path);
    url.set_query(None);
    {
        let mut query = url.query_pairs_mut();
        query.append_pair("model_id", "scribe_v2_realtime").append_pair("audio_format", "pcm_24000").append_pair("commit_strategy", "manual");
        let languages: Vec<_> = config.languages.iter().map(|s| s.trim()).filter(|s| !s.is_empty()).collect();
        if let Some((first, rest)) = languages.split_first() {
            query.append_pair("language_code", first);
            for language in rest { query.append_pair("secondary_languages", language); }
        }
        for term in elevenlabs_keyterms(vocabulary, true) { query.append_pair("keyterms", &term); }
    }
    Ok(url.to_string())
}

fn session_update(config: &TranscriptionConfig, vocabulary: &[String]) -> Value {
    let mut transcription = json!({ "model": config.model_name, "delay": "low" });
    // Literal vocabulary belongs in keywords. Invalid entries reject the whole session, so
    // omit those hints without changing the dictionary or its local replacement rules.
    let mut seen = std::collections::BTreeSet::new();
    let keywords: Vec<_> = vocabulary.iter().map(|term| term.trim())
        .filter(|term| !term.is_empty() && !term.contains(['<', '>', '\r', '\n']) && seen.insert(*term))
        .collect();
    if !keywords.is_empty() { transcription["keywords"] = json!(keywords); }
    let languages: Vec<_> = config.languages.iter().map(|l| l.trim()).filter(|l| !l.is_empty()).collect();
    if !languages.is_empty() { transcription["languages"] = json!(languages); }
    json!({ "type": "session.update", "session": { "type": "transcription", "audio": { "input": {
        "format": { "type": "audio/pcm", "rate": RATE }, "transcription": transcription, "turn_detection": null
    }}}})
}

async fn send(socket: &mut Socket, event: Value) -> Result<(), String> {
    timeout(IO_TIMEOUT, socket.send(Message::Text(event.to_string().into()))).await
        .map_err(|_| SEND_TIMEOUT_ERROR.to_string())?
        .map_err(|_| "Transcription failed: live transcription connection was interrupted".to_string())
}

fn parse_event(message: Option<Result<Message, tokio_tungstenite::tungstenite::Error>>, api_key: &str) -> Result<Option<Value>, String> {
    match message {
        Some(Ok(Message::Text(text))) => {
            let event: Value = serde_json::from_str(&text).map_err(|_| "Transcription failed: unexpected response from the speech-to-text endpoint")?;
            if event["type"] == "error" || event["type"] == "conversation.item.input_audio_transcription.failed" || event["error"].is_string() {
                let message = event["error"]["message"].as_str().or_else(|| event["error"].as_str()).unwrap_or("the speech model returned an error");
                let message = message.replace(api_key, "[redacted]").chars().filter(|c| !c.is_control()).take(300).collect::<String>();
                return Err(format!("Transcription failed: {message}"));
            }
            Ok(Some(event))
        },
        Some(Ok(Message::Ping(_) | Message::Pong(_))) => Ok(None),
        _ => Err("Transcription failed: live transcription connection was interrupted".into()),
    }
}

async fn receive(socket: &mut Socket, api_key: &str) -> Result<Value, String> {
    loop {
        if let Some(event) = parse_event(socket.next().await, api_key)? { return Ok(event); }
    }
}

async fn stream(socket: &mut Socket, mut rx: mpsc::Receiver<AudioChunk>, mut finish: oneshot::Receiver<()>, overflowed: &AtomicBool, api_key: &str, microsoft: bool) -> Result<String, String> {
    let mut resampler = PcmResampler::default();
    let mut pending = Vec::new();
    let mut total_bytes = 0;
    let mut finishing = false;
    loop {
        if overflowed.load(Ordering::Acquire) { return Err("Transcription failed: the connection was too slow for live audio; retry from History".into()); }
        tokio::select! {
            biased;
            message = socket.next() => { parse_event(message, api_key)?; },
            _ = &mut finish, if !finishing => {
                finishing = true;
                rx.close(); // Drain captured audio even when CPAL still owns a sender.
            },
            chunk = rx.recv() => match chunk {
                Some(chunk) => {
                    resampler.push(&chunk.samples, chunk.rate, &mut pending)?;
                    if microsoft && total_bytes + pending.len() > RATE as usize * 3600 * 2 {
                        return Err("Transcription failed: Microsoft live sessions are limited to one hour; retry from History".into());
                    }
                    while pending.len() >= PACKET_BYTES {
                        send(socket, json!({ "type": "input_audio_buffer.append", "audio": STANDARD.encode(&pending[..PACKET_BYTES]) })).await?;
                        pending.drain(..PACKET_BYTES);
                        total_bytes += PACKET_BYTES;
                    }
                },
                None => break,
            },
        }
    }
    resampler.finish(&mut pending);
    if overflowed.load(Ordering::Acquire) { return Err("Transcription failed: the connection was too slow for live audio; retry from History".into()); }
    // OpenAI requires at least 100 ms; MAI accepts any nonempty PCM16 buffer.
    let minimum_bytes = if microsoft { 2 } else { (RATE as usize / 10) * 2 };
    if total_bytes + pending.len() < minimum_bytes { return Ok(String::new()); }
    if !pending.is_empty() {
        send(socket, json!({ "type": "input_audio_buffer.append", "audio": STANDARD.encode(&pending) })).await?;
    }
    send(socket, json!({ "type": "input_audio_buffer.commit" })).await?;
    wait_for_final(if microsoft { Duration::from_secs(240) } else { FINAL_TIMEOUT }, final_transcript(socket, api_key, microsoft)).await
}

async fn wait_for_final<F: std::future::Future<Output = Result<String, String>>>(duration: Duration, transcript: F) -> Result<String, String> {
    timeout(duration, transcript).await.map_err(|_| FINAL_TIMEOUT_ERROR.to_string())?
}

async fn final_transcript(socket: &mut Socket, api_key: &str, microsoft: bool) -> Result<String, String> {
    let mut committed: Option<String> = None;
    let mut committed_seen = false;
    let mut completed: Option<(Option<String>, String)> = None;
    loop {
        let event = receive(socket, api_key).await?;
        match event["type"].as_str() {
            Some("input_audio_buffer.committed") => {
                committed = event["item_id"].as_str().map(str::to_string);
                // MAI's documented final events can omit item_id. There is exactly one
                // manual commit per recording; OpenAI still requires matching IDs.
                committed_seen = microsoft || committed.is_some();
            },
            Some("conversation.item.input_audio_transcription.completed") => {
                let id = event["item_id"].as_str().map(str::to_string);
                if let Some(text) = event["transcript"].as_str() {
                    if (microsoft || id.is_some()) && committed.as_ref().is_none_or(|expected| id.as_ref() == Some(expected) || microsoft && id.is_none()) {
                        completed = Some((id, text.trim().to_string()));
                    }
                } else { return Err("Transcription failed: unexpected response from the speech-to-text endpoint".into()); }
            },
            _ => {},
        }
        if committed_seen {
            if let Some((id, text)) = &completed {
                if committed.is_none() || committed.as_ref() == id.as_ref() || microsoft && id.is_none() { return Ok(text.clone()); }
            }
        }
    }
}

fn elevenlabs_audio(bytes: &[u8], commit: bool) -> Value {
    json!({ "message_type": "input_audio_chunk", "audio_base_64": STANDARD.encode(bytes), "sample_rate": RATE, "commit": commit })
}

fn collect_elevenlabs(event: &Value, segments: &mut Vec<String>, outstanding: &mut usize) -> Result<(), String> {
    if event["message_type"] == "committed_transcript" {
        let text = event["text"].as_str().ok_or("Transcription failed: unexpected response from the speech-to-text endpoint")?.trim();
        if !text.is_empty() { segments.push(text.to_string()); }
        *outstanding = outstanding.saturating_sub(1);
    }
    Ok(())
}

async fn stream_elevenlabs(socket: &mut Socket, mut rx: mpsc::Receiver<AudioChunk>, mut finish: oneshot::Receiver<()>, overflowed: &AtomicBool, api_key: &str) -> Result<String, String> {
    const PACKET: usize = RATE as usize / 10 * 2; // ElevenLabs recommends 100 ms–1 s chunks.
    const SEGMENT: usize = RATE as usize * 20 * 2;
    let mut resampler = PcmResampler::default();
    let mut pending = Vec::new();
    let mut segment_bytes = 0;
    let mut outstanding = 0;
    let mut segments = Vec::new();
    let mut finishing = false;
    loop {
        if overflowed.load(Ordering::Acquire) { return Err("Transcription failed: the connection was too slow for live audio; retry from History".into()); }
        tokio::select! {
            biased;
            message = socket.next() => {
                if let Some(event) = parse_event(message, api_key)? { collect_elevenlabs(&event, &mut segments, &mut outstanding)?; }
            },
            _ = &mut finish, if !finishing => { finishing = true; rx.close(); },
            chunk = rx.recv() => match chunk {
                Some(chunk) => {
                    resampler.push(&chunk.samples, chunk.rate, &mut pending)?;
                    while pending.len() >= PACKET {
                        send(socket, elevenlabs_audio(&pending[..PACKET], false)).await?;
                        pending.drain(..PACKET);
                        segment_bytes += PACKET;
                        // Scribe auto-commits at ~36s without commit IDs. Commit earlier so delayed
                        // segment completions can be counted, including those arriving after release.
                        if segment_bytes >= SEGMENT {
                            send(socket, elevenlabs_audio(&[], true)).await?;
                            outstanding += 1;
                            segment_bytes = 0;
                        }
                    }
                },
                None => break,
            }
        }
    }
    resampler.finish(&mut pending);
    if overflowed.load(Ordering::Acquire) { return Err("Transcription failed: the connection was too slow for live audio; retry from History".into()); }
    if segment_bytes + pending.len() > 0 {
        // The documented processor starts after 2s. Pad short final turns with silence so a
        // brief dictation can still produce a committed result; this doesn't delay the HUD.
        let padding = (RATE as usize * 2 * 2).saturating_sub(segment_bytes + pending.len());
        pending.resize(pending.len() + padding, 0);
        for packet in pending.chunks(PACKET) { send(socket, elevenlabs_audio(packet, false)).await?; }
        send(socket, elevenlabs_audio(&[], true)).await?;
        outstanding += 1;
    }
    wait_for_final(FINAL_TIMEOUT, async {
        while outstanding > 0 {
            let event = receive(socket, api_key).await?;
            collect_elevenlabs(&event, &mut segments, &mut outstanding)?;
        }
        Ok::<_, String>(segments.join(" "))
    }).await
}

/// Linear interpolation, retaining the fractional position and last sample across callbacks.
/// ponytail: speech-quality resampling; use a band-limited resampler if aliasing becomes audible.
#[derive(Default)]
struct PcmResampler {
    rate: u32,
    input_count: u64,
    output_count: u64,
    last: f32,
}

impl PcmResampler {
    fn push(&mut self, samples: &[f32], rate: u32, out: &mut Vec<u8>) -> Result<(), String> {
        if rate == 0 || (self.rate != 0 && self.rate != rate) { return Err("Transcription failed: invalid audio recording".into()); }
        self.rate = rate;
        for &sample in samples {
            if !sample.is_finite() { return Err("Transcription failed: invalid audio recording".into()); }
            let sample = sample.clamp(-1.0, 1.0);
            let index = self.input_count;
            while self.output_count * rate as u64 <= index * RATE as u64 {
                let position = self.output_count * rate as u64;
                let fraction = if index == 0 { 1.0 } else { (position - (index - 1) * RATE as u64) as f32 / RATE as f32 };
                let value = self.last + (sample - self.last) * fraction;
                out.extend_from_slice(&((value.clamp(-1.0, 1.0) * 32767.0) as i16).to_le_bytes());
                self.output_count += 1;
            }
            self.last = sample;
            self.input_count += 1;
        }
        Ok(())
    }

    fn finish(&mut self, out: &mut Vec<u8>) {
        if self.rate == 0 { return; }
        let target = self.input_count * RATE as u64 / self.rate as u64;
        while self.output_count < target {
            out.extend_from_slice(&((self.last.clamp(-1.0, 1.0) * 32767.0) as i16).to_le_bytes());
            self.output_count += 1;
        }
    }
}

/// Retry/history uses the same live protocol when its selected model is live-only.
pub async fn transcribe_wav(config: &TranscriptionConfig, wav_bytes: Vec<u8>, vocabulary: Vec<String>) -> Result<String, String> {
    if !config.has_live_backups() {
        return transcribe_wav_single(config, wav_bytes, vocabulary).await;
    }
    let primary = transcribe_wav_single(config, wav_bytes.clone(), vocabulary.clone());
    with_live_backups(config, primary, wav_bytes, vocabulary).await
}

/// Retry providers replay independently, including Scribe's required real-time pacing.
/// Cancelling this future or selecting a result drops every unfinished live session.
pub async fn with_live_backups<F: std::future::Future<Output = Result<String, String>>>(
    config: &TranscriptionConfig,
    primary: F,
    wav_bytes: Vec<u8>,
    vocabulary: Vec<String>,
) -> Result<String, String> {
    let mut pending = FuturesOrdered::new();
    let backups = config.live_backup_configs();
    let multiple = !backups.is_empty();
    let model = model_label(config);
    pending.push_back(Either::Left(async move { primary.await.map_err(|error| label_error(error, &model, multiple)) }));
    for backup in backups {
        let wav_bytes = wav_bytes.clone();
        let vocabulary = vocabulary.clone();
        pending.push_back(Either::Right(async move {
            transcribe_wav_single(&backup, wav_bytes, vocabulary).await.map_err(|error| label_error(error, &model_label(&backup), multiple))
        }));
    }
    preferred_result(pending).await
}

async fn transcribe_wav_single(config: &TranscriptionConfig, wav_bytes: Vec<u8>, vocabulary: Vec<String>) -> Result<String, String> {
    let mut reader = hound::WavReader::new(Cursor::new(wav_bytes)).map_err(|_| "Transcription failed: invalid audio recording")?;
    let spec = reader.spec();
    if spec.channels == 0 || spec.sample_rate == 0 || !(1..=32).contains(&spec.bits_per_sample) {
        return Err("Transcription failed: invalid audio recording".into());
    }
    let samples: Result<Vec<f32>, _> = match spec.sample_format {
        hound::SampleFormat::Float => reader.samples::<f32>().collect(),
        hound::SampleFormat::Int => {
            let scale = (1u64 << (spec.bits_per_sample - 1)) as f32;
            reader.samples::<i32>().map(|s| s.map(|s| s as f32 / scale)).collect()
        },
    };
    let samples = samples.map_err(|_| "Transcription failed: invalid audio recording")?;
    if samples.len() % spec.channels as usize != 0 || samples.iter().any(|s| !s.is_finite()) {
        return Err("Transcription failed: invalid audio recording".into());
    }
    let (mut live, input) = LiveTranscription::start(config, vocabulary)?;
    for chunk in samples.chunks((spec.sample_rate as usize / 25).max(1) * spec.channels as usize) {
        let samples = chunk.chunks_exact(spec.channels as usize).map(|frame| frame.iter().sum::<f32>() / spec.channels as f32).collect();
        if input.tx.send(AudioChunk { samples, rate: spec.sample_rate }).await.is_err() { break; }
        // Scribe's documented file-streaming flow paces input to avoid its processing queue limit.
        if config.model_name == "scribe_v2_realtime" {
            tokio::time::sleep(Duration::from_secs_f64(chunk.len() as f64 / (spec.sample_rate as f64 * spec.channels as f64))).await;
        }
    }
    live.finish_input();
    live.finish().await
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;
    use tokio_tungstenite::accept_async;

    fn config(port: u16) -> TranscriptionConfig {
        TranscriptionConfig {
            endpoint_url: format!("http://127.0.0.1:{port}/v1/audio/transcriptions"),
            api_key: "test-secret".into(), model_name: "gpt-live-transcribe".into(),
            languages: vec!["en".into(), " sl ".into()], ..Default::default()
        }
    }

    async fn bind_server() -> (TcpListener, TranscriptionConfig) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let config = config(listener.local_addr().unwrap().port());
        (listener, config)
    }

    async fn read(socket: &mut WebSocketStream<TcpStream>) -> Value {
        let message = timeout(Duration::from_secs(3), socket.next()).await.unwrap().unwrap().unwrap();
        serde_json::from_str(message.to_text().unwrap()).unwrap()
    }

    async fn write(socket: &mut WebSocketStream<TcpStream>, event: Value) {
        socket.send(Message::Text(event.to_string().into())).await.unwrap();
    }

    async fn ready(listener: TcpListener) -> WebSocketStream<TcpStream> {
        let (stream, _) = listener.accept().await.unwrap();
        let mut socket = accept_async(stream).await.unwrap();
        write(&mut socket, json!({ "type": "session.created" })).await;
        assert_eq!(read(&mut socket).await["type"], "session.update");
        write(&mut socket, json!({ "type": "session.updated" })).await;
        socket
    }

    async fn read_opening_request(stream: &mut TcpStream) {
        let mut request = Vec::new();
        timeout(Duration::from_secs(3), async {
            while !request.ends_with(b"\r\n\r\n") {
                let mut byte = [0];
                stream.read_exact(&mut byte).await.unwrap();
                request.push(byte[0]);
                assert!(request.len() < 16_384);
            }
        }).await.unwrap();
    }

    fn set_backups(config: &mut TranscriptionConfig, first: &TranscriptionConfig, second: &TranscriptionConfig) {
        config.stt_backup_1_enabled = true;
        config.stt_backup_1_provider = "openai".into();
        config.stt_backup_1_endpoint_url = first.endpoint_url.clone();
        config.stt_backup_1_model_name = first.model_name.clone();
        config.stt_backup_1_api_key = first.api_key.clone();
        config.stt_backup_2_enabled = true;
        config.stt_backup_2_provider = "openai".into();
        config.stt_backup_2_endpoint_url = second.endpoint_url.clone();
        config.stt_backup_2_model_name = second.model_name.clone();
        config.stt_backup_2_api_key = second.api_key.clone();
    }

    async fn serve_result(listener: TcpListener, result: Result<&str, &str>) {
        let mut socket = ready(listener).await;
        let mut bytes = 0;
        loop {
            let event = read(&mut socket).await;
            if event["type"] == "input_audio_buffer.commit" { break; }
            assert_eq!(event["type"], "input_audio_buffer.append");
            bytes += STANDARD.decode(event["audio"].as_str().unwrap()).unwrap().len();
        }
        assert_eq!(bytes, 9600);
        match result {
            Ok(text) => {
                write(&mut socket, json!({ "type": "input_audio_buffer.committed", "item_id": "result" })).await;
                write(&mut socket, json!({ "type": "conversation.item.input_audio_transcription.completed", "item_id": "result", "transcript": text })).await;
            }
            Err(message) => write(&mut socket, json!({ "type": "error", "error": { "message": message } })).await,
        }
    }

    fn test_group(attempts: Vec<Result<LiveTranscription, String>>) -> LiveTranscriptionGroup {
        LiveTranscriptionGroup { attempts: attempts.into_iter().enumerate().map(|(index, attempt)| (format!("provider {index}"), attempt)).collect() }
    }

    fn recording_wav() -> Vec<u8> {
        let mut wav = Cursor::new(Vec::new());
        let mut writer = hound::WavWriter::new(&mut wav, hound::WavSpec {
            channels: 1, sample_rate: RATE, bits_per_sample: 16, sample_format: hound::SampleFormat::Int,
        }).unwrap();
        for _ in 0..4800 { writer.write_sample(10000_i16).unwrap(); }
        writer.finalize().unwrap();
        wav.into_inner()
    }

    #[tokio::test]
    async fn live_group_streams_to_five_providers_and_falls_back_in_priority_order() {
        for preferred in 0..5 {
            let (primary_listener, mut config) = bind_server().await;
            let (first_listener, first) = bind_server().await;
            let (second_listener, second) = bind_server().await;
            let (third_listener, third) = bind_server().await;
            let (fourth_listener, fourth) = bind_server().await;
            set_backups(&mut config, &first, &second);
            config.stt_backup_3_enabled = true;
            config.stt_backup_3_provider = "openai".into();
            config.stt_backup_3_endpoint_url = third.endpoint_url;
            config.stt_backup_3_model_name = third.model_name;
            config.stt_backup_3_api_key = third.api_key;
            config.stt_backup_4_enabled = true;
            config.stt_backup_4_provider = "openai".into();
            config.stt_backup_4_endpoint_url = fourth.endpoint_url;
            config.stt_backup_4_model_name = fourth.model_name;
            config.stt_backup_4_api_key = fourth.api_key;
            let names = ["primary", "first backup", "second backup", "third backup", "fourth backup"];
            let servers: Vec<_> = [primary_listener, first_listener, second_listener, third_listener, fourth_listener].into_iter().enumerate().map(|(index, listener)| {
                tokio::spawn(serve_result(listener, if index < preferred { Err("unavailable") } else { Ok(names[index]) }))
            }).collect();
            let primary = LiveTranscription::start(&config, vec![]);
            let (mut group, inputs) = LiveTranscriptionGroup::start_with_primary(&config, primary, vec![]).unwrap();
            assert_eq!(inputs.len(), 5);
            for input in &inputs {
                input.tx.send(AudioChunk { samples: vec![0.2; 4800], rate: RATE }).await.unwrap();
            }
            group.finish_input();
            for server in servers { server.await.unwrap(); }
            assert_eq!(group.finish().await.unwrap(), names[preferred]);
            assert!(inputs.iter().all(|input| input.tx.is_closed()));
        }
    }

    #[tokio::test]
    async fn live_group_recovers_from_invalid_primary_and_retry_replays_backups() {
        for retry in [false, true] {
            let (first_listener, first) = bind_server().await;
            let (second_listener, second) = bind_server().await;
            let mut config = first.clone();
            config.api_key.clear();
            set_backups(&mut config, &first, &second);
            let first_server = tokio::spawn(serve_result(first_listener, Err("unavailable")));
            let second_server = tokio::spawn(serve_result(second_listener, Ok("recovered")));
            let text = if retry {
                let mut wav = Cursor::new(Vec::new());
                let mut writer = hound::WavWriter::new(&mut wav, hound::WavSpec { channels: 1, sample_rate: RATE, bits_per_sample: 16, sample_format: hound::SampleFormat::Int }).unwrap();
                for _ in 0..4800 { writer.write_sample(10000_i16).unwrap(); }
                writer.finalize().unwrap();
                transcribe_wav(&config, wav.into_inner(), vec![]).await.unwrap()
            } else {
                let primary = LiveTranscription::start(&config, vec![]);
                let (group, inputs) = LiveTranscriptionGroup::start_with_primary(&config, primary, vec![]).unwrap();
                assert_eq!(inputs.len(), 2);
                for input in inputs {
                    input.tx.send(AudioChunk { samples: vec![0.2; 4800], rate: RATE }).await.unwrap();
                }
                group.finish().await.unwrap()
            };
            assert_eq!(text, "recovered");
            first_server.await.unwrap();
            second_server.await.unwrap();
        }
    }

    #[tokio::test]
    async fn retry_uses_the_fifth_provider_when_only_that_backup_is_enabled() {
        let (listener, backup) = bind_server().await;
        let mut config = backup.clone();
        config.api_key.clear();
        config.stt_backup_4_enabled = true;
        config.stt_backup_4_provider = "openai".into();
        config.stt_backup_4_endpoint_url = backup.endpoint_url;
        config.stt_backup_4_model_name = backup.model_name;
        config.stt_backup_4_api_key = backup.api_key;
        let server = tokio::spawn(serve_result(listener, Ok("fifth provider recovered")));
        let mut wav = Cursor::new(Vec::new());
        let mut writer = hound::WavWriter::new(&mut wav, hound::WavSpec {
            channels: 1, sample_rate: RATE, bits_per_sample: 16, sample_format: hound::SampleFormat::Int,
        }).unwrap();
        for _ in 0..4800 { writer.write_sample(10000_i16).unwrap(); }
        writer.finalize().unwrap();
        assert_eq!(transcribe_wav(&config, wav.into_inner(), vec![]).await.unwrap(), "fifth provider recovered");
        server.await.unwrap();
    }

    fn pending_local() -> (LiveTranscription, crate::processing::Cancellation) {
        let (finish_tx, _finish_rx) = oneshot::channel();
        let cancellation = crate::processing::Cancellation::default();
        let task = tauri::async_runtime::spawn(std::future::pending::<Result<String, String>>());
        (LiveTranscription::from_local(task, finish_tx, cancellation.clone()), cancellation)
    }

    #[tokio::test]
    async fn live_group_finalization_deadlines_run_concurrently() {
        let (release, primary_gate) = oneshot::channel();
        let (finish_tx, _finish_rx) = oneshot::channel();
        let task = tauri::async_runtime::spawn(async move { primary_gate.await.unwrap(); Err("primary failed".into()) });
        let primary = LiveTranscription::from_local(task, finish_tx, crate::processing::Cancellation::default());
        let (mut backup, cancelled) = pending_local();
        backup.final_timeout = Duration::from_millis(1);
        let (finish_tx, _finish_rx) = oneshot::channel();
        let task = tauri::async_runtime::spawn(async { Ok("third result".into()) });
        let third = LiveTranscription::from_local(task, finish_tx, crate::processing::Cancellation::default());
        let group = test_group(vec![Ok(primary), Ok(backup), Ok(third)]);
        let result = tokio::spawn(group.finish());
        // The backup must time out while the primary is still waiting, rather than
        // beginning its timeout only after the primary's failure.
        timeout(Duration::from_secs(3), cancelled.cancelled()).await.unwrap();
        release.send(()).unwrap();
        assert_eq!(result.await.unwrap().unwrap(), "third result");
    }

    #[tokio::test]
    async fn live_group_preserves_empty_success_and_aborts_unused_or_cancelled_sessions() {
        let (backup, backup_cancelled) = pending_local();
        let (third, third_cancelled) = pending_local();
        let (fourth, fourth_cancelled) = pending_local();
        let (fifth, fifth_cancelled) = pending_local();
        let (finish_tx, _finish_rx) = oneshot::channel();
        let primary_cancelled = crate::processing::Cancellation::default();
        let task = tauri::async_runtime::spawn(async { Ok(String::new()) });
        let primary = LiveTranscription::from_local(task, finish_tx, primary_cancelled.clone());
        let group = test_group(vec![Ok(primary), Ok(backup), Ok(third), Ok(fourth), Ok(fifth)]);
        assert_eq!(group.finish().await.unwrap(), "");
        assert!(primary_cancelled.is_cancelled() && backup_cancelled.is_cancelled() && third_cancelled.is_cancelled());
        assert!(fourth_cancelled.is_cancelled() && fifth_cancelled.is_cancelled());

        let (primary, primary_cancelled) = pending_local();
        let (backup, backup_cancelled) = pending_local();
        let (third, third_cancelled) = pending_local();
        let (fourth, fourth_cancelled) = pending_local();
        let (fifth, fifth_cancelled) = pending_local();
        let group = test_group(vec![Ok(primary), Ok(backup), Ok(third), Ok(fourth), Ok(fifth)]);
        let cancellation = crate::processing::Cancellation::default();
        cancellation.cancel();
        assert_eq!(cancellation.run(group.finish()).await.unwrap_err(), crate::processing::CANCELLED);
        assert!(primary_cancelled.is_cancelled() && backup_cancelled.is_cancelled() && third_cancelled.is_cancelled());
        assert!(fourth_cancelled.is_cancelled() && fifth_cancelled.is_cancelled());
    }

    #[tokio::test]
    async fn live_group_reports_failure_only_when_all_providers_fail() {
        let group = test_group(vec![Err("primary failure".into()), Err("backup failure".into()), Err("third failure".into())]);
        assert_eq!(group.finish().await.unwrap_err(), "Transcription failed: provider 0: primary failure; Transcription failed: provider 1: backup failure; Transcription failed: provider 2: third failure");
        let mut config = config(1);
        config.api_key.clear();
        let primary = LiveTranscription::start(&config, vec![]);
        assert_eq!(LiveTranscriptionGroup::start_with_primary(&config, primary, vec![]).err().unwrap(), "Transcription failed: check the API key in Settings › Model");
    }

    #[tokio::test]
    async fn recording_and_retry_retain_setup_and_runtime_failures_for_every_model() {
        for retry in [false, true] {
            let (listener, mut first) = bind_server().await;
            first.model_name = "gpt-live-transcribe-2026-09-01".into();
            let mut second = config(1);
            second.model_name = "invalid-live-model".into();
            let mut config = config(1);
            config.api_key.clear();
            set_backups(&mut config, &first, &second);
            let server = tokio::spawn(serve_result(listener, Err("backup unavailable")));
            let error = if retry {
                transcribe_wav(&config, recording_wav(), vec![]).await.unwrap_err()
            } else {
                let primary = LiveTranscription::start(&config, vec![]);
                let (group, inputs) = LiveTranscriptionGroup::start_with_primary(&config, primary, vec![]).unwrap();
                assert_eq!(inputs.len(), 1);
                inputs[0].tx.send(AudioChunk { samples: vec![0.2; 4800], rate: RATE }).await.unwrap();
                group.finish().await.unwrap_err()
            };
            assert_eq!(error, "Transcription failed: gpt-live-transcribe: check the API key in Settings › Model; Transcription failed: gpt-live-transcribe-2026-09-01: backup unavailable; Transcription failed: invalid-live-model: Backup transcription providers must use a real-time model");
            server.await.unwrap();
        }
    }

    #[tokio::test]
    async fn immediate_failures_and_local_retry_use_the_configured_model_labels() {
        let mut config = config(1);
        config.stt_source = "local".into();
        config.local_stt_model = "vibevoice-asr-streaming-7b".into();
        let mut first = config.clone();
        first.model_name = "scribe_v2_realtime".into();
        first.api_key.clear();
        let mut second = first.clone();
        second.model_name = "gpt-live-transcribe".into();
        second.endpoint_url = "invalid".into();
        set_backups(&mut config, &first, &second);
        let expected = "Transcription failed: vibevoice-asr-streaming-7b: local model failed; Transcription failed: scribe_v2_realtime: check the endpoint URL in Settings › Model; Transcription failed: gpt-live-transcribe: check the endpoint URL in Settings › Model";
        let error = LiveTranscriptionGroup::start_with_primary(&config, Err("local model failed".into()), vec![]).err().unwrap();
        assert_eq!(error, expected);
        let error = with_live_backups(&config, async { Err("local model failed".into()) }, recording_wav(), vec![]).await.unwrap_err();
        assert_eq!(error, expected);
    }

    #[tokio::test]
    async fn final_transcript_and_outer_processing_deadlines_have_distinct_errors_and_cancel() {
        let error = wait_for_final(Duration::from_millis(1), std::future::pending::<Result<String, String>>()).await.unwrap_err();
        assert_eq!(error, FINAL_TIMEOUT_ERROR);
        let (mut live, cancelled) = pending_local();
        live.final_timeout = Duration::from_millis(1);
        assert_eq!(live.finish().await.unwrap_err(), PROCESSING_TIMEOUT_ERROR);
        assert!(cancelled.is_cancelled());
        assert_ne!(FINAL_TIMEOUT_ERROR, SESSION_TIMEOUT_ERROR);
        assert_ne!(FINAL_TIMEOUT_ERROR, PROCESSING_TIMEOUT_ERROR);
    }

    #[test]
    fn model_and_endpoint_detection() {
        assert!(is_live_model("gpt-live-transcribe"));
        assert!(is_live_model("scribe_v2_realtime"));
        assert!(is_live_model("gpt-live-transcribe-2026-09-01"));
        assert!(!is_live_model("gpt-transcribe"));
        assert!(!is_live_model("gpt-live-transcribe-custom"));
        assert_eq!(realtime_url("https://api.openai.com/v1/audio/transcriptions").unwrap(), "wss://api.openai.com/v1/realtime?intent=transcription");
        assert!(realtime_url("http://example.com/v1/audio/transcriptions").is_err());
        assert!(realtime_url("https://user:secret@example.com/v1/audio/transcriptions").is_err());
        assert!(realtime_url("https://api.openai.com/v1/chat/completions").is_err());
    }

    #[test]
    fn resampling_is_continuous_across_callback_boundaries() {
        for rate in [16_000, 24_000, 44_100, 48_000] {
            let samples: Vec<_> = (0..rate).map(|i| (i as f32 * 0.04).sin() * 0.8).collect();
            let mut full = PcmResampler::default();
            let mut expected = Vec::new();
            full.push(&samples, rate, &mut expected).unwrap();
            full.finish(&mut expected);
            assert_eq!(expected.len(), RATE as usize * 2);
            for chunk_size in [1, 127, 512, 1024] {
                let mut streamed = PcmResampler::default();
                let mut actual = Vec::new();
                for chunk in samples.chunks(chunk_size) { streamed.push(chunk, rate, &mut actual).unwrap(); }
                streamed.finish(&mut actual);
                assert_eq!(actual, expected, "sample rate {rate}, chunk size {chunk_size}");
            }
        }
        let mut resampler = PcmResampler::default();
        let mut pcm = Vec::new();
        resampler.push(&[-2.0, 2.0], RATE, &mut pcm).unwrap();
        assert_eq!(pcm, [(-32767_i16).to_le_bytes(), 32767_i16.to_le_bytes()].concat());
        assert!(resampler.push(&[0.0], 48_000, &mut pcm).is_err());
        assert!(PcmResampler::default().push(&[f32::NAN], RATE, &mut pcm).is_err());
    }

    #[tokio::test]
    async fn streams_before_release_and_waits_for_matching_committed_transcript() {
        let (listener, config) = bind_server().await;
        let (received_tx, received_rx) = oneshot::channel();
        let server = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            #[allow(clippy::result_large_err)] // The WebSocket handshake callback fixes this error type.
            let mut socket = tokio_tungstenite::accept_hdr_async(stream, |request: &tokio_tungstenite::tungstenite::handshake::server::Request, response| {
                assert_eq!(request.uri(), "/v1/realtime?intent=transcription");
                assert_eq!(request.headers()["authorization"], "Bearer test-secret");
                assert!(!request.headers().contains_key("openai-beta"));
                Ok(response)
            }).await.unwrap();
            write(&mut socket, json!({ "type": "session.created" })).await;
            let session = read(&mut socket).await;
            let input = &session["session"]["audio"]["input"];
            assert_eq!(session["session"]["type"], "transcription");
            assert_eq!(input["format"], json!({ "type": "audio/pcm", "rate": RATE }));
            assert_eq!(input["transcription"], json!({ "model": "gpt-live-transcribe", "keywords": ["OpenGlaido", "Tauri"], "languages": ["en", "sl"], "delay": "low" }));
            assert!(input["turn_detection"].is_null());
            write(&mut socket, json!({ "type": "session.updated" })).await;
            let first = read(&mut socket).await;
            assert_eq!(first["type"], "input_audio_buffer.append");
            let mut bytes = STANDARD.decode(first["audio"].as_str().unwrap()).unwrap().len();
            received_tx.send(()).unwrap();
            loop {
                let event = read(&mut socket).await;
                if event["type"] == "input_audio_buffer.commit" { break; }
                assert_eq!(event["type"], "input_audio_buffer.append");
                bytes += STANDARD.decode(event["audio"].as_str().unwrap()).unwrap().len();
            }
            assert_eq!(bytes, 9600); // Both 100 ms chunks reached the server, including queued tail.
            write(&mut socket, json!({ "type": "input_audio_buffer.committed", "item_id": "current" })).await;
            write(&mut socket, json!({ "type": "conversation.item.input_audio_transcription.completed", "item_id": "old", "transcript": "wrong" })).await;
            write(&mut socket, json!({ "type": "conversation.item.input_audio_transcription.delta", "item_id": "current", "delta": "partial" })).await;
            write(&mut socket, json!({ "type": "conversation.item.input_audio_transcription.completed", "item_id": "current", "transcript": "  final transcript  " })).await;
        });
        let (mut live, input) = LiveTranscription::start(&config, vec!["OpenGlaido".into(), "Tauri".into()]).unwrap();
        input.tx.send(AudioChunk { samples: vec![0.2; 4410], rate: 44_100 }).await.unwrap();
        timeout(Duration::from_secs(3), received_rx).await.unwrap().unwrap(); // Still recording.
        input.tx.send(AudioChunk { samples: vec![0.3; 4410], rate: 44_100 }).await.unwrap();
        live.finish_input();
        assert_eq!(live.finish().await.unwrap(), "final transcript");
        assert!(input.tx.is_closed()); // Keeping callback sender alive cannot block finishing.
        server.await.unwrap();
    }

    #[tokio::test]
    async fn server_errors_are_reported_without_exposing_credentials() {
        let (listener, config) = bind_server().await;
        let server = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let mut socket = accept_async(stream).await.unwrap();
            assert_eq!(read(&mut socket).await["type"], "session.update");
            write(&mut socket, json!({ "type": "error", "error": { "message": "model unavailable for test-secret" } })).await;
        });
        let (live, input) = LiveTranscription::start(&config, vec![]).unwrap();
        input.tx.send(AudioChunk { samples: vec![0.3; 4800], rate: RATE }).await.unwrap();
        let error = live.finish().await.unwrap_err();
        assert!(error.contains("model unavailable"));
        assert!(!error.contains("test-secret"));
        server.await.unwrap();
    }

    #[tokio::test]
    async fn unexpected_close_and_overflow_never_return_a_partial_transcript() {
        let (listener, config) = bind_server().await;
        let server = tokio::spawn(async move {
            let mut socket = ready(listener).await;
            socket.close(None).await.unwrap();
        });
        let (live, input) = LiveTranscription::start(&config, vec![]).unwrap();
        input.tx.send(AudioChunk { samples: vec![0.3; 4800], rate: RATE }).await.unwrap();
        assert!(live.finish().await.is_err());
        server.await.unwrap();

        let (listener, config) = bind_server().await;
        let server = tokio::spawn(async move { let _socket = ready(listener).await; });
        let (live, input) = LiveTranscription::start(&config, vec![]).unwrap();
        input.overflowed.store(true, Ordering::Release);
        assert!(live.finish().await.unwrap_err().contains("too slow"));
        server.await.unwrap();
    }

    #[tokio::test]
    async fn dropping_recording_aborts_socket_and_empty_audio_does_not_commit() {
        for cancel in [true, false] {
            let (listener, config) = bind_server().await;
            let (ready_tx, ready_rx) = oneshot::channel();
            let server = tokio::spawn(async move {
                let mut socket = ready(listener).await;
                ready_tx.send(()).unwrap();
                let result = timeout(Duration::from_secs(3), socket.next()).await.unwrap();
                assert!(!matches!(result, Some(Ok(Message::Text(_))))); // No empty commit; connection released.
            });
            let (live, _input) = LiveTranscription::start(&config, vec![]).unwrap();
            ready_rx.await.unwrap();
            if cancel { drop(live); } else { assert_eq!(live.finish().await.unwrap(), ""); }
            server.await.unwrap();
        }
    }

    #[tokio::test]
    async fn opening_handshake_timeouts_recover_on_third_attempt_and_deliver_queued_audio_once() {
        let (listener, config) = bind_server().await;
        let server = tokio::spawn(async move {
            for _ in 0..2 {
                let (mut stalled, _) = listener.accept().await.unwrap();
                read_opening_request(&mut stalled).await;
                let mut byte = [0];
                assert_eq!(timeout(Duration::from_secs(3), stalled.read(&mut byte)).await.unwrap().unwrap(), 0);
            }
            // Failed connections get only the handshake. The third receives
            // one session update, exactly one recording's PCM, and one final commit.
            serve_result(listener, Ok("reconnected")).await;
        });
        let (live, input) = LiveTranscription::start_with_setup_timeout(&config, vec![], Duration::from_millis(250)).unwrap();
        input.tx.send(AudioChunk { samples: vec![0.2; 4800], rate: RATE }).await.unwrap();
        assert_eq!(live.finish().await.unwrap(), "reconnected");
        assert!(input.tx.is_closed());
        server.await.unwrap();
    }

    #[tokio::test]
    async fn three_opening_handshake_timeouts_report_connection_phase_and_stop_retrying() {
        let (listener, config) = bind_server().await;
        let server = tokio::spawn(async move {
            for _ in 0..3 {
                let (mut stalled, _) = listener.accept().await.unwrap();
                read_opening_request(&mut stalled).await;
                let mut byte = [0];
                assert_eq!(timeout(Duration::from_secs(3), stalled.read(&mut byte)).await.unwrap().unwrap(), 0);
            }
            assert!(timeout(Duration::from_millis(150), listener.accept()).await.is_err());
        });
        let (live, input) = LiveTranscription::start_with_setup_timeout(&config, vec![], Duration::from_millis(250)).unwrap();
        assert_eq!(live.finish().await.unwrap_err(), CONNECT_TIMEOUT_ERROR);
        assert!(input.tx.is_closed());
        server.await.unwrap();
    }

    #[tokio::test]
    async fn opening_http_failures_and_cancellation_never_reconnect() {
        for status in [401, 403, 500] {
            let (listener, config) = bind_server().await;
            let server = tokio::spawn(async move {
                let (mut stream, _) = listener.accept().await.unwrap();
                read_opening_request(&mut stream).await;
                stream.write_all(format!("HTTP/1.1 {status} Error\r\nContent-Length: 0\r\n\r\n").as_bytes()).await.unwrap();
                assert!(timeout(Duration::from_millis(150), listener.accept()).await.is_err());
            });
            let (live, _input) = LiveTranscription::start_with_setup_timeout(&config, vec![], Duration::from_millis(250)).unwrap();
            let error = live.finish().await.unwrap_err();
            assert_eq!(error, if status == 500 { "Transcription failed: the server returned HTTP 500" } else { "Transcription failed: check the API key in Settings › Model" });
            server.await.unwrap();
        }
        let (listener, config) = bind_server().await;
        let (started_tx, started_rx) = oneshot::channel();
        let server = tokio::spawn(async move {
            let (mut stalled, _) = listener.accept().await.unwrap();
            read_opening_request(&mut stalled).await;
            started_tx.send(()).unwrap();
            let mut byte = [0];
            assert_eq!(timeout(Duration::from_secs(3), stalled.read(&mut byte)).await.unwrap().unwrap(), 0);
            assert!(timeout(Duration::from_millis(300), listener.accept()).await.is_err());
        });
        let (live, input) = LiveTranscription::start_with_setup_timeout(&config, vec![], Duration::from_millis(250)).unwrap();
        started_rx.await.unwrap();
        drop(live);
        server.await.unwrap();
        assert!(input.tx.is_closed());
    }

    #[tokio::test]
    async fn missing_session_ack_times_out_and_closes_socket() {
        let (listener, config) = bind_server().await;
        let server = tokio::spawn(async move {
            for _ in 0..3 {
                let (stream, _) = listener.accept().await.unwrap();
                let mut socket = accept_async(stream).await.unwrap();
                assert_eq!(read(&mut socket).await["type"], "session.update");
                let closed = timeout(Duration::from_secs(3), socket.next()).await.unwrap();
                assert!(!matches!(closed, Some(Ok(Message::Text(_)))));
            }
            assert!(timeout(Duration::from_millis(150), listener.accept()).await.is_err());
        });
        let (live, _input) = LiveTranscription::start_with_setup_timeout(&config, vec![], Duration::from_millis(250)).unwrap();
        assert_eq!(live.finish().await.unwrap_err(), SESSION_TIMEOUT_ERROR);
        server.await.unwrap();
    }

    #[tokio::test]
    async fn each_cloud_provider_retries_session_setup_before_sending_audio() {
        for provider in ["openai", "elevenlabs", "microsoft"] {
            let (listener, config) = bind_server().await;
            let config = match provider {
                "elevenlabs" => scribe_config(config),
                "microsoft" => mai_config(config),
                _ => config,
            };
            let elevenlabs = provider == "elevenlabs";
            let microsoft = provider == "microsoft";
            let server = tokio::spawn(async move {
                for attempt in 0..3 {
                    let (stream, _) = listener.accept().await.unwrap();
                    let mut socket = accept_async(stream).await.unwrap();
                    // MAI's first attempt stalls before session.created; subsequent
                    // attempts exercise the shared session.updated timeout.
                    if !(elevenlabs || microsoft && attempt == 0) {
                        write(&mut socket, json!({ "type": "session.created" })).await;
                        assert_eq!(read(&mut socket).await["type"], "session.update");
                    }
                    if attempt < 2 {
                        let closed = timeout(Duration::from_secs(3), socket.next()).await.unwrap();
                        assert!(!matches!(closed, Some(Ok(Message::Text(_))))); // No audio before readiness.
                        continue;
                    }
                    let ready = if elevenlabs { json!({ "message_type": "session_started" }) } else { json!({ "type": "session.updated" }) };
                    write(&mut socket, ready).await;
                    let mut bytes = 0;
                    loop {
                        let chunk = read(&mut socket).await;
                        let committed = if elevenlabs { chunk["commit"] == true } else { chunk["type"] == "input_audio_buffer.commit" };
                        if committed { break; }
                        bytes += STANDARD.decode(chunk[if elevenlabs { "audio_base_64" } else { "audio" }].as_str().unwrap()).unwrap().len();
                    }
                    assert_eq!(bytes, if elevenlabs { RATE as usize * 2 * 2 } else { 9600 });
                    if elevenlabs {
                        write(&mut socket, json!({ "message_type": "committed_transcript", "text": "recovered" })).await;
                    } else {
                        write(&mut socket, json!({ "type": "input_audio_buffer.committed", "item_id": "result" })).await;
                        write(&mut socket, json!({ "type": "conversation.item.input_audio_transcription.completed", "item_id": "result", "transcript": "recovered" })).await;
                    }
                }
            });
            let (live, input) = LiveTranscription::start_with_setup_timeout(&config, vec![], Duration::from_millis(250)).unwrap();
            input.tx.send(AudioChunk { samples: vec![0.2; 4800], rate: RATE }).await.unwrap();
            assert_eq!(live.finish().await.unwrap(), "recovered");
            assert!(input.tx.is_closed());
            server.await.unwrap();
        }
    }

    #[tokio::test]
    async fn connection_and_session_ack_share_the_same_setup_deadline() {
        assert_eq!(IO_TIMEOUT * SETUP_ATTEMPTS, Duration::from_secs(30));
        let (listener, config) = bind_server().await;
        let server = tokio::spawn(async move {
            for _ in 0..3 {
                let (stream, _) = listener.accept().await.unwrap();
                tokio::time::sleep(Duration::from_millis(200)).await; // Uses half of the attempt's budget.
                let mut socket = accept_async(stream).await.unwrap();
                assert_eq!(read(&mut socket).await["type"], "session.update");
                let closed = timeout(Duration::from_secs(3), socket.next()).await.unwrap();
                assert!(!matches!(closed, Some(Ok(Message::Text(_)))));
            }
            assert!(timeout(Duration::from_millis(150), listener.accept()).await.is_err());
        });
        let attempt_timeout = Duration::from_millis(400);
        let started = Instant::now();
        let (live, _input) = LiveTranscription::start_with_setup_timeout(&config, vec![], attempt_timeout).unwrap();
        assert_eq!(live.finish().await.unwrap_err(), SESSION_TIMEOUT_ERROR);
        let elapsed = started.elapsed();
        assert!(elapsed >= attempt_timeout * SETUP_ATTEMPTS);
        // Allow scheduling overhead, but not another full ACK budget per attempt.
        assert!(elapsed < attempt_timeout * (SETUP_ATTEMPTS + 1), "setup took {elapsed:?}");
        server.await.unwrap();
    }

    #[tokio::test]
    async fn retry_decodes_stereo_wav_and_uses_the_same_protocol() {
        let (listener, config) = bind_server().await;
        let server = tokio::spawn(async move {
            let mut socket = ready(listener).await;
            let mut bytes = Vec::new();
            loop {
                let event = read(&mut socket).await;
                if event["type"] == "input_audio_buffer.commit" { break; }
                bytes.extend(STANDARD.decode(event["audio"].as_str().unwrap()).unwrap());
            }
            assert_eq!(bytes.len(), 9600);
            assert!(bytes.iter().all(|b| *b == 0)); // Opposite stereo channels are mixed to mono.
            write(&mut socket, json!({ "type": "input_audio_buffer.committed", "item_id": "retry" })).await;
            write(&mut socket, json!({ "type": "conversation.item.input_audio_transcription.completed", "item_id": "retry", "transcript": "retried" })).await;
        });
        let mut wav = Cursor::new(Vec::new());
        let mut writer = hound::WavWriter::new(&mut wav, hound::WavSpec { channels: 2, sample_rate: 16_000, bits_per_sample: 16, sample_format: hound::SampleFormat::Int }).unwrap();
        for _ in 0..3200 { writer.write_sample(10000_i16).unwrap(); writer.write_sample(-10000_i16).unwrap(); }
        writer.finalize().unwrap();
        assert_eq!(transcribe_wav(&config, wav.into_inner(), vec![]).await.unwrap(), "retried");
        server.await.unwrap();
        assert!(transcribe_wav(&config, vec![1, 2, 3], vec![]).await.unwrap_err().contains("invalid audio"));
    }

    fn scribe_config(mut config: TranscriptionConfig) -> TranscriptionConfig {
        config.endpoint_url = config.endpoint_url.replace("/audio/transcriptions", "/speech-to-text");
        config.model_name = "scribe_v2_realtime".into();
        config.stt_provider = "elevenlabs".into();
        config
    }

    #[tokio::test]
    async fn elevenlabs_streams_before_release_and_collects_all_delayed_commits() {
        let (listener, config) = bind_server().await;
        let config = scribe_config(config);
        let (first_commit_tx, first_commit_rx) = oneshot::channel();
        let server = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            #[allow(clippy::result_large_err)] // The library handshake callback fixes this error type.
            let mut socket = tokio_tungstenite::accept_hdr_async(stream, |request: &tokio_tungstenite::tungstenite::handshake::server::Request, response| {
                assert_eq!(request.headers()["xi-api-key"], "test-secret");
                assert!(!request.headers().contains_key("authorization"));
                assert_eq!(request.uri(), "/v1/speech-to-text/realtime?model_id=scribe_v2_realtime&audio_format=pcm_24000&commit_strategy=manual&language_code=en&secondary_languages=sl&keyterms=OpenGlaido&keyterms=Tauri");
                Ok(response)
            }).await.unwrap();
            write(&mut socket, json!({ "message_type": "session_started" })).await;
            let mut total_bytes = 0;
            let mut commits = 0;
            let mut signal = Some(first_commit_tx);
            loop {
                let chunk = read(&mut socket).await;
                assert_eq!(chunk["message_type"], "input_audio_chunk");
                assert_eq!(chunk["sample_rate"], RATE);
                assert!(chunk.get("previous_text").is_none());
                let audio = STANDARD.decode(chunk["audio_base_64"].as_str().unwrap()).unwrap();
                if chunk["commit"] == true {
                    assert!(audio.is_empty());
                    commits += 1;
                    if commits == 1 {
                        assert_eq!(total_bytes, RATE as usize * 20 * 2);
                        signal.take().unwrap().send(()).unwrap(); // User still holds the hotkey.
                    } else { break; }
                } else { total_bytes += audio.len(); }
            }
            assert_eq!(total_bytes, RATE as usize * 22 * 2); // Final 200 ms padded to a 2s segment.
            write(&mut socket, json!({ "message_type": "committed_transcript", "text": " First segment. " })).await;
            write(&mut socket, json!({ "message_type": "partial_transcript", "text": "unfinished" })).await;
            write(&mut socket, json!({ "message_type": "committed_transcript", "text": "Last segment." })).await;
        });
        let (mut live, input) = LiveTranscription::start(&config, vec!["OpenGlaido".into(), "Tauri".into(), "OpenGlaido".into()]).unwrap();
        input.tx.send(AudioChunk { samples: vec![0.2; RATE as usize * 20], rate: RATE }).await.unwrap();
        timeout(Duration::from_secs(3), first_commit_rx).await.unwrap().unwrap();
        input.tx.send(AudioChunk { samples: vec![0.3; RATE as usize / 5], rate: RATE }).await.unwrap();
        live.finish_input();
        assert_eq!(live.finish().await.unwrap(), "First segment. Last segment.");
        server.await.unwrap();
    }

    #[tokio::test]
    async fn elevenlabs_short_recording_pads_and_reports_provider_errors() {
        let (listener, config) = bind_server().await;
        let config = scribe_config(config);
        let server = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let mut socket = accept_async(stream).await.unwrap();
            write(&mut socket, json!({ "message_type": "session_started" })).await;
            let mut bytes = 0;
            loop {
                let chunk = read(&mut socket).await;
                bytes += STANDARD.decode(chunk["audio_base_64"].as_str().unwrap()).unwrap().len();
                if chunk["commit"] == true { break; }
            }
            assert_eq!(bytes, RATE as usize * 2 * 2);
            write(&mut socket, json!({ "message_type": "quota_exceeded", "error": "Quota exceeded for test-secret" })).await;
        });
        let (live, input) = LiveTranscription::start(&config, vec![]).unwrap();
        input.tx.send(AudioChunk { samples: vec![0.2; RATE as usize / 5], rate: RATE }).await.unwrap();
        let error = live.finish().await.unwrap_err();
        assert!(error.contains("Quota exceeded"));
        assert!(!error.contains("test-secret"));
        server.await.unwrap();
    }

    #[tokio::test]
    async fn elevenlabs_close_fails_and_cancellation_releases_socket() {
        for cancel in [false, true] {
            let (listener, config) = bind_server().await;
            let config = scribe_config(config);
            let (ready_tx, ready_rx) = oneshot::channel();
            let server = tokio::spawn(async move {
                let (stream, _) = listener.accept().await.unwrap();
                let mut socket = accept_async(stream).await.unwrap();
                write(&mut socket, json!({ "message_type": "session_started" })).await;
                ready_tx.send(()).unwrap();
                if cancel {
                    let result = timeout(Duration::from_secs(3), socket.next()).await.unwrap();
                    assert!(!matches!(result, Some(Ok(Message::Text(_)))));
                } else { socket.close(None).await.unwrap(); }
            });
            let (live, input) = LiveTranscription::start(&config, vec![]).unwrap();
            ready_rx.await.unwrap();
            if cancel { drop(live); }
            else {
                let _ = input.tx.send(AudioChunk { samples: vec![0.2; 4800], rate: RATE }).await;
                assert!(live.finish().await.is_err());
            }
            server.await.unwrap();
        }
    }

    #[test]
    fn openai_dictionary_keywords_preserve_phrases_and_reject_invalid_hints() {
        let vocabulary = [" OpenGlaido ", "Tauri", "OpenGlaido", "Acme, Inc.", "東京", "C++", "[trace]", "<bad>", "bad>", "two\nlines", "two\rlines", " "]
            .into_iter().map(String::from).collect::<Vec<_>>();
        let cfg = config(123);
        let session = session_update(&cfg, &vocabulary);
        let transcription = &session["session"]["audio"]["input"]["transcription"];
        assert_eq!(transcription["keywords"], json!(["OpenGlaido", "Tauri", "Acme, Inc.", "東京", "C++", "[trace]"]));
        assert!(transcription.get("prompt").is_none());
        assert_eq!(transcription["model"], cfg.model_name);
        assert_eq!(transcription["languages"], json!(["en", "sl"]));
        for words in [vec![], vec![" ".into(), "<invalid>".into(), "two\nlines".into()]] {
            let session = session_update(&cfg, &words);
            let transcription = &session["session"]["audio"]["input"]["transcription"];
            assert!(transcription.get("keywords").is_none());
            assert!(transcription.get("prompt").is_none());
        }
        assert_eq!(vocabulary[0], " OpenGlaido "); // Hints never mutate the user's saved entries.
    }

    fn mai_config(mut config: TranscriptionConfig) -> TranscriptionConfig {
        config.endpoint_url = config.endpoint_url.replace("/v1/audio/transcriptions", "");
        config.model_name = crate::microsoft::LIVE_MODEL.into();
        config.stt_provider = "microsoft".into();
        config.stt_deployment = "my-azure-deployment".into();
        config
    }

    #[tokio::test]
    async fn microsoft_streams_before_release_drains_tail_and_accepts_idless_final() {
        let (listener, config) = bind_server().await;
        let config = mai_config(config);
        let (received_tx, received_rx) = oneshot::channel();
        let server = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            #[allow(clippy::result_large_err)] // The library handshake callback fixes this error type.
            let mut socket = tokio_tungstenite::accept_hdr_async(stream, |request: &tokio_tungstenite::tungstenite::handshake::server::Request, response| {
                assert_eq!(request.uri(), "/mai/v1/realtime?intent=transcription");
                assert_eq!(request.headers()["api-key"], "test-secret");
                assert!(!request.headers().contains_key("authorization"));
                Ok(response)
            }).await.unwrap();
            write(&mut socket, json!({ "type": "session.created" })).await;
            let session = read(&mut socket).await;
            assert_eq!(session["session"]["audio"]["input"], json!({
                "format": { "type": "audio/pcm", "rate": RATE },
                "transcription": { "model": "my-azure-deployment", "language": null },
                "turn_detection": null, "noise_reduction": null
            }));
            // No unsupported keyword or OpenAI delay fields reach MAI's dedicated schema.
            write(&mut socket, json!({ "type": "session.updated" })).await;
            let first = read(&mut socket).await;
            assert_eq!(first["type"], "input_audio_buffer.append");
            let mut bytes = STANDARD.decode(first["audio"].as_str().unwrap()).unwrap().len();
            received_tx.send(()).unwrap();
            write(&mut socket, json!({ "type": "conversation.item.input_audio_transcription.delta", "delta": "Wrong intermediate" })).await;
            write(&mut socket, json!({ "type": "conversation.item.input_audio_transcription.intermediate", "intermediate": "unfinished suffix" })).await;
            loop {
                let event = read(&mut socket).await;
                if event["type"] == "input_audio_buffer.commit" { break; }
                assert_eq!(event["type"], "input_audio_buffer.append");
                bytes += STANDARD.decode(event["audio"].as_str().unwrap()).unwrap().len();
            }
            assert_eq!(bytes, 9600);
            // Azure documents these without item IDs, unlike OpenAI's commit protocol.
            write(&mut socket, json!({ "type": "input_audio_buffer.committed" })).await;
            write(&mut socket, json!({ "type": "conversation.item.input_audio_transcription.completed", "transcript": "  Živjo OpenGlaido.  " })).await;
        });
        let (mut live, input) = LiveTranscription::start(&config, vec!["OpenGlaido".into()]).unwrap();
        input.tx.send(AudioChunk { samples: vec![0.2; 4410], rate: 44_100 }).await.unwrap();
        timeout(Duration::from_secs(3), received_rx).await.unwrap().unwrap();
        input.tx.send(AudioChunk { samples: vec![0.3; 4410], rate: 44_100 }).await.unwrap();
        live.finish_input();
        assert_eq!(live.finish().await.unwrap(), "Živjo OpenGlaido.");
        assert!(input.tx.is_closed());
        server.await.unwrap();
    }

    #[tokio::test]
    async fn microsoft_retry_and_completed_before_commit_use_the_same_live_contract() {
        let (listener, config) = bind_server().await;
        let config = mai_config(config);
        let server = tokio::spawn(async move {
            let mut socket = ready(listener).await;
            let mut bytes = 0;
            loop {
                let event = read(&mut socket).await;
                if event["type"] == "input_audio_buffer.commit" { break; }
                bytes += STANDARD.decode(event["audio"].as_str().unwrap()).unwrap().len();
            }
            assert_eq!(bytes, 1920); // MAI can finalize even a 40 ms saved clip.
            write(&mut socket, json!({ "type": "conversation.item.input_audio_transcription.completed", "item_id": "retry", "transcript": "Retried Azure audio." })).await;
            write(&mut socket, json!({ "type": "input_audio_buffer.committed", "item_id": "retry" })).await;
        });
        let mut wav = Cursor::new(Vec::new());
        let mut writer = hound::WavWriter::new(&mut wav, hound::WavSpec { channels: 1, sample_rate: 16_000, bits_per_sample: 16, sample_format: hound::SampleFormat::Int }).unwrap();
        for _ in 0..640 { writer.write_sample(10000_i16).unwrap(); }
        writer.finalize().unwrap();
        assert_eq!(transcribe_wav(&config, wav.into_inner(), vec![]).await.unwrap(), "Retried Azure audio.");
        server.await.unwrap();
    }

    #[tokio::test]
    async fn microsoft_finalization_accepts_optional_ids_but_rejects_mismatching_ids() {
        for (commit_id, final_id) in [(None, None), (Some("current"), None), (None, Some("current")), (Some("current"), Some("current"))] {
            let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
            let address = listener.local_addr().unwrap();
            let server = tokio::spawn(async move {
                let (stream, _) = listener.accept().await.unwrap();
                let mut socket = accept_async(stream).await.unwrap();
                let mut commit = json!({ "type": "input_audio_buffer.committed" });
                if let Some(id) = commit_id { commit["item_id"] = json!(id); }
                write(&mut socket, commit).await;
                if commit_id.is_some() {
                    write(&mut socket, json!({ "type": "conversation.item.input_audio_transcription.completed", "item_id": "old", "transcript": "must not return" })).await;
                }
                let mut final_event = json!({ "type": "conversation.item.input_audio_transcription.completed", "transcript": "complete" });
                if let Some(id) = final_id { final_event["item_id"] = json!(id); }
                write(&mut socket, final_event).await;
            });
            let (mut socket, _) = connect_async(format!("ws://{address}")).await.unwrap();
            assert_eq!(timeout(Duration::from_secs(3), final_transcript(&mut socket, "test-secret", true)).await.unwrap().unwrap(), "complete");
            server.await.unwrap();
        }
    }

    #[tokio::test]
    async fn microsoft_cancel_errors_and_disconnects_never_return_partial_text() {
        for case in ["cancel", "error", "disconnect", "overflow"] {
            let (listener, config) = bind_server().await;
            let config = mai_config(config);
            let (ready_tx, ready_rx) = oneshot::channel();
            let server = tokio::spawn(async move {
                let mut socket = ready(listener).await;
                ready_tx.send(()).unwrap();
                match case {
                    "cancel" => {
                        let result = timeout(Duration::from_secs(3), socket.next()).await.unwrap();
                        assert!(!matches!(result, Some(Ok(Message::Text(_)))));
                    },
                    "error" => {
                        write(&mut socket, json!({ "type": "conversation.item.input_audio_transcription.intermediate", "intermediate": "partial must not paste" })).await;
                        write(&mut socket, json!({ "type": "error", "error": { "message": "model unavailable for test-secret" } })).await;
                    },
                    _ => { socket.close(None).await.unwrap(); },
                }
            });
            let (live, input) = LiveTranscription::start(&config, vec![]).unwrap();
            ready_rx.await.unwrap();
            if case == "cancel" { drop(live); }
            else {
                // Feed real audio: with zero bytes the stream shortcuts to an empty
                // "no speech" result before the server error/close can be observed.
                let _ = input.tx.send(AudioChunk { samples: vec![0.2; 4800], rate: RATE }).await;
                if case == "overflow" { input.overflowed.store(true, Ordering::Release); }
                let error = live.finish().await.unwrap_err();
                assert!(!error.contains("test-secret"));
            }
            server.await.unwrap();
        }
    }

    #[test]
    fn elevenlabs_dictionary_terms_obey_each_api_limit() {
        let terms: Vec<String> = [" OpenGlaido", "Tauri", "OpenGlaido", "東京", "<invalid>", "one two three four five six", "bad\nterm", "", "Acme, Inc."].into_iter().map(String::from).collect();
        assert_eq!(elevenlabs_keyterms(&terms, true), ["OpenGlaido", "Tauri", "東京", "Acme, Inc."]);
        assert!(elevenlabs_keyterms(&[], false).is_empty());
        let twenty = "界".repeat(20);
        assert_eq!(elevenlabs_keyterms(std::slice::from_ref(&twenty), true), std::slice::from_ref(&twenty));
        assert!(elevenlabs_keyterms(&[twenty + "界"], true).is_empty());
        let forty_nine = "a".repeat(49);
        assert_eq!(elevenlabs_keyterms(std::slice::from_ref(&forty_nine), false), std::slice::from_ref(&forty_nine));
        assert!(elevenlabs_keyterms(&[forty_nine + "a"], false).is_empty());
        let many = (0..1100).map(|i| format!("term{i}")).collect::<Vec<_>>();
        assert_eq!(elevenlabs_keyterms(&many, true).len(), 50);
        assert_eq!(elevenlabs_keyterms(&many, false).len(), 100);
        let mut config = scribe_config(config(123));
        config.languages.clear();
        assert!(!elevenlabs_url(&config, &[" ".into()]).unwrap().contains("keyterms"));
        assert!(elevenlabs_url(&config, &["a&b".into()]).unwrap().ends_with("keyterms=a%26b"));
    }
}
