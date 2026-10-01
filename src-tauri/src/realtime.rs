//! One push-to-talk recording per OpenAI or ElevenLabs live transcription session.
use crate::audio::{AudioChunk, StreamInput};
use crate::transcribe::TranscriptionConfig;
use base64::{engine::general_purpose::STANDARD, Engine};
use futures_util::{SinkExt, StreamExt};
use serde_json::{json, Value};
use std::io::Cursor;
use std::sync::{atomic::{AtomicBool, Ordering}, Arc};
use std::time::Duration;
use tokio::net::TcpStream;
use tokio::sync::{mpsc, oneshot};
use tokio::time::timeout;
use tokio_tungstenite::{connect_async, tungstenite::{client::IntoClientRequest, Message}, MaybeTlsStream, WebSocketStream};

const RATE: u32 = 24_000;
const PACKET_BYTES: usize = 960 * 2; // 40 ms of PCM16.
const IO_TIMEOUT: Duration = Duration::from_secs(10);
const FINAL_TIMEOUT: Duration = Duration::from_secs(45);
type Socket = WebSocketStream<MaybeTlsStream<TcpStream>>;

pub fn is_live_model(model: &str) -> bool {
    model == "scribe_v2_realtime" || model == "gpt-live-transcribe" || model.strip_prefix("gpt-live-transcribe-").is_some_and(|suffix| {
        suffix.len() == 10 && suffix.bytes().enumerate().all(|(i, c)| {
            if i == 4 || i == 7 { c == b'-' } else { c.is_ascii_digit() }
        })
    })
}

pub struct LiveTranscription {
    finish_tx: Option<oneshot::Sender<()>>,
    task: tauri::async_runtime::JoinHandle<Result<String, String>>,
}

impl LiveTranscription {
    /// Starts connecting in the background; the bounded queue preserves speech during setup.
    pub fn start(config: &TranscriptionConfig, vocabulary: Vec<String>) -> Result<(Self, StreamInput), String> {
        let elevenlabs = config.model_name == "scribe_v2_realtime";
        let url = if elevenlabs { elevenlabs_url(config, &vocabulary)? } else { realtime_url(&config.endpoint_url)? };
        if config.api_key.trim().is_empty() {
            return Err("Transcription failed: check the API key in Settings › Model".into());
        }
        let mut request = url.into_client_request().map_err(|_| "Transcription failed: check the endpoint URL in Settings › Model")?;
        let (header, value) = if elevenlabs { ("xi-api-key", config.api_key.trim().to_string()) } else { ("Authorization", format!("Bearer {}", config.api_key.trim())) };
        let mut value = tokio_tungstenite::tungstenite::http::HeaderValue::from_str(&value)
            .map_err(|_| "Transcription failed: check the API key in Settings › Model")?;
        value.set_sensitive(true);
        request.headers_mut().insert(header, value);
        let session = (!elevenlabs).then(|| session_update(config, (!vocabulary.is_empty()).then(|| vocabulary.join(", "))));
        let api_key = config.api_key.trim().to_string();
        // ponytail: bounded callback queue; batch in the audio worker if very small buffers need more setup time.
        let (tx, rx) = mpsc::channel(2048);
        let overflowed = Arc::new(AtomicBool::new(false));
        let overflow_check = overflowed.clone();
        let (finish_tx, finish_rx) = oneshot::channel();
        let task = tauri::async_runtime::spawn(async move {
            let (mut socket, _) = timeout(IO_TIMEOUT, connect_async(request)).await
                .map_err(|_| "Transcription failed: live transcription timed out".to_string())?
                .map_err(|error| match error {
                    tokio_tungstenite::tungstenite::Error::Http(response) => match response.status().as_u16() {
                        401 | 403 => "Transcription failed: check the API key in Settings › Model".into(),
                        code => format!("Transcription failed: the server returned HTTP {code}"),
                    },
                    _ => "Transcription failed: couldn't reach the speech-to-text endpoint".into(),
                })?;
            if let Some(session) = session { send(&mut socket, session).await?; }
            timeout(IO_TIMEOUT, async {
                loop {
                    let event = receive(&mut socket, &api_key).await?;
                    if (!elevenlabs && event["type"] == "session.updated") || (elevenlabs && event["message_type"] == "session_started") { return Ok::<_, String>(()); }
                }
            }).await.map_err(|_| "Transcription failed: live transcription timed out".to_string())??;
            if elevenlabs { stream_elevenlabs(&mut socket, rx, finish_rx, &overflow_check, &api_key).await }
            else { stream(&mut socket, rx, finish_rx, &overflow_check, &api_key).await }
        });
        Ok((Self { finish_tx: Some(finish_tx), task }, StreamInput { tx, overflowed }))
    }

    /// Call after physically stopping the microphone; callbacks can otherwise retain senders.
    pub fn finish_input(&mut self) {
        if let Some(tx) = self.finish_tx.take() { let _ = tx.send(()); }
    }

    pub async fn finish(mut self) -> Result<String, String> {
        self.finish_input();
        timeout(FINAL_TIMEOUT, &mut self.task).await
            .map_err(|_| "Transcription failed: live transcription timed out".to_string())?
            .map_err(|_| "Transcription failed: unexpected response from the speech-to-text endpoint".to_string())?
    }
}

impl Drop for LiveTranscription {
    fn drop(&mut self) { self.task.abort(); }
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

fn session_update(config: &TranscriptionConfig, prompt: Option<String>) -> Value {
    let mut transcription = json!({ "model": config.model_name, "delay": "low" });
    if let Some(prompt) = prompt.filter(|s| !s.trim().is_empty()) { transcription["prompt"] = json!(prompt); }
    let languages: Vec<_> = config.languages.iter().map(|l| l.trim()).filter(|l| !l.is_empty()).collect();
    if !languages.is_empty() { transcription["languages"] = json!(languages); }
    json!({ "type": "session.update", "session": { "type": "transcription", "audio": { "input": {
        "format": { "type": "audio/pcm", "rate": RATE }, "transcription": transcription, "turn_detection": null
    }}}})
}

async fn send(socket: &mut Socket, event: Value) -> Result<(), String> {
    timeout(IO_TIMEOUT, socket.send(Message::Text(event.to_string().into()))).await
        .map_err(|_| "Transcription failed: live transcription timed out".to_string())?
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

async fn stream(socket: &mut Socket, mut rx: mpsc::Receiver<AudioChunk>, mut finish: oneshot::Receiver<()>, overflowed: &AtomicBool, api_key: &str) -> Result<String, String> {
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
    // The API requires at least 100 ms to commit. Very short accidental presses produce no text.
    if total_bytes + pending.len() < (RATE as usize / 10) * 2 { return Ok(String::new()); }
    if !pending.is_empty() {
        send(socket, json!({ "type": "input_audio_buffer.append", "audio": STANDARD.encode(&pending) })).await?;
    }
    send(socket, json!({ "type": "input_audio_buffer.commit" })).await?;
    timeout(FINAL_TIMEOUT, final_transcript(socket, api_key)).await
        .map_err(|_| "Transcription failed: live transcription timed out".to_string())?
}

async fn final_transcript(socket: &mut Socket, api_key: &str) -> Result<String, String> {
    let mut committed: Option<String> = None;
    let mut completed: Option<(String, String)> = None;
    loop {
        let event = receive(socket, api_key).await?;
        match event["type"].as_str() {
            Some("input_audio_buffer.committed") => {
                committed = event["item_id"].as_str().map(str::to_string);
            },
            Some("conversation.item.input_audio_transcription.completed") => {
                if let (Some(id), Some(text)) = (event["item_id"].as_str(), event["transcript"].as_str()) {
                    if committed.as_deref().is_none_or(|expected| expected == id) {
                        completed = Some((id.to_string(), text.trim().to_string()));
                    }
                }
            },
            _ => {},
        }
        if let (Some(expected), Some((id, text))) = (&committed, &completed) {
            if expected == id { return Ok(text.clone()); }
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
    timeout(FINAL_TIMEOUT, async {
        while outstanding > 0 {
            let event = receive(socket, api_key).await?;
            collect_elevenlabs(&event, &mut segments, &mut outstanding)?;
        }
        Ok::<_, String>(segments.join(" "))
    }).await.map_err(|_| "Transcription failed: live transcription timed out".to_string())?
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
            assert_eq!(input["transcription"], json!({ "model": "gpt-live-transcribe", "prompt": "OpenGlaido, Tauri", "languages": ["en", "sl"], "delay": "low" }));
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
    async fn missing_session_ack_times_out_and_closes_socket() {
        let (listener, config) = bind_server().await;
        let server = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let mut socket = accept_async(stream).await.unwrap();
            assert_eq!(read(&mut socket).await["type"], "session.update");
            let closed = timeout(IO_TIMEOUT + Duration::from_secs(3), socket.next()).await.unwrap();
            assert!(!matches!(closed, Some(Ok(Message::Text(_)))));
        });
        let (live, _input) = LiveTranscription::start(&config, vec![]).unwrap();
        assert!(live.finish().await.unwrap_err().contains("timed out"));
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
