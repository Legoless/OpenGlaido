//! OpenAI-compatible chat completions with streaming and tool calls (Groq, OpenAI, Ollama, …),
//! or the local llama.cpp model (text only).

use futures_util::StreamExt;
use serde_json::{json, Value};
use std::time::Duration;
use tauri::AppHandle;

/// One tool call the model asked for (arguments are a JSON string, as the API sends them).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct ToolCall {
    pub id: String,
    pub name: String,
    pub arguments: String,
}

/// Result of one streamed completion: the text and/or the tool calls.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Completion {
    pub text: String,
    pub tool_calls: Vec<ToolCall>,
}

/// Accumulates streamed chunks ("data: {...}" SSE lines) into a `Completion`.
#[derive(Default)]
pub struct Assembler {
    /// Raw bytes: a UTF-8 character may be split across network chunks.
    buffer: Vec<u8>,
    pub completion: Completion,
    /// [DONE] or a finish_reason arrived: the answer is complete.
    pub done: bool,
    /// At least one SSE data line was seen (else the body may be a plain JSON completion).
    pub saw_data: bool,
    /// An error object the provider sent instead of (or in the middle of) the stream.
    pub error: Option<String>,
    /// Every byte, for the non-streaming fallback.
    raw: Vec<u8>,
}

fn error_message(v: &Value) -> Option<String> {
    let e = v.get("error").filter(|e| !e.is_null())?;
    Some(e.get("message").and_then(Value::as_str).map(str::to_string).unwrap_or_else(|| e.to_string()))
}

impl Assembler {
    /// Feeds raw bytes from the response; returns the text delta that arrived (may be empty).
    pub fn push(&mut self, bytes: &[u8]) -> String {
        self.buffer.extend_from_slice(bytes);
        if self.raw.len() < 1 << 20 {
            self.raw.extend_from_slice(bytes);
        }
        let mut delta_text = String::new();
        while let Some(pos) = self.buffer.iter().position(|b| *b == b'\n') {
            let line: Vec<u8> = self.buffer.drain(..=pos).collect();
            let line = String::from_utf8_lossy(&line);
            let line = line.trim();
            let Some(data) = line.strip_prefix("data:") else { continue };
            self.saw_data = true;
            let data = data.trim();
            if data == "[DONE]" {
                self.done = true;
                continue;
            }
            let Ok(chunk) = serde_json::from_str::<Value>(data) else { continue };
            if let Some(e) = error_message(&chunk) {
                self.error = Some(e);
                continue;
            }
            if chunk.pointer("/choices/0/finish_reason").is_some_and(|f| !f.is_null()) {
                self.done = true;
            }
            let Some(delta) = chunk.pointer("/choices/0/delta") else { continue };
            if let Some(text) = delta.get("content").and_then(Value::as_str) {
                delta_text.push_str(text);
                self.completion.text.push_str(text);
            }
            for call in delta.get("tool_calls").and_then(Value::as_array).into_iter().flatten() {
                let calls = &self.completion.tool_calls;
                let new_id = call.get("id").and_then(Value::as_str).filter(|id| !id.is_empty());
                // Some OpenAI-compatible servers omit `index`: a new id starts a new call.
                let index = match call.get("index").and_then(Value::as_u64) {
                    Some(i) => i as usize,
                    None => match (calls.last(), new_id) {
                        (Some(last), Some(id)) if !last.id.is_empty() && last.id != id => calls.len(),
                        (None, _) => 0,
                        _ => calls.len() - 1,
                    },
                };
                if self.completion.tool_calls.len() <= index {
                    self.completion.tool_calls.resize(index + 1, ToolCall::default());
                }
                let slot = &mut self.completion.tool_calls[index];
                if let Some(id) = call.get("id").and_then(Value::as_str) {
                    slot.id = id.to_string();
                }
                if let Some(name) = call.pointer("/function/name").and_then(Value::as_str) {
                    slot.name.push_str(name);
                }
                if let Some(args) = call.pointer("/function/arguments").and_then(Value::as_str) {
                    slot.arguments.push_str(args);
                }
            }
        }
        delta_text
    }

    /// The stream ended: a plain (non-streaming) JSON body, an error, or a cut-off stream.
    pub fn finish(mut self) -> Result<Completion, String> {
        self.push(b"\n");
        if let Some(e) = self.error {
            return Err(format!("The language model returned an error: {e}"));
        }
        if !self.saw_data {
            let body: Value = serde_json::from_slice(&self.raw).map_err(|_| "The language model sent an unreadable answer".to_string())?;
            if let Some(e) = error_message(&body) {
                return Err(format!("The language model returned an error: {e}"));
            }
            let message = body.pointer("/choices/0/message").ok_or("The language model sent an empty answer")?;
            let text = message.get("content").and_then(Value::as_str).unwrap_or("").to_string();
            let tool_calls = message
                .get("tool_calls")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .map(|c| ToolCall {
                    id: c.get("id").and_then(Value::as_str).unwrap_or("").to_string(),
                    name: c.pointer("/function/name").and_then(Value::as_str).unwrap_or("").to_string(),
                    arguments: c.pointer("/function/arguments").and_then(Value::as_str).unwrap_or("{}").to_string(),
                })
                .collect();
            return Ok(Completion { text, tool_calls });
        }
        if !self.done {
            return Err("The answer stream broke off; try again".into());
        }
        Ok(self.completion)
    }
}

pub enum Llm {
    Remote { url: String, model: String, api_key: String, temperature: f64 },
    /// A downloaded catalog model: no tool calls.
    Local { app: AppHandle, model_id: String, temperature: f64, max_tokens: u32 },
}

impl Llm {
    pub fn is_local(&self) -> bool {
        matches!(self, Llm::Local { .. })
    }

    /// Sampling temperature and (local only; cloud models stop on their own) the answer's token limit.
    pub fn tuned(mut self, temp: f64, max: u32) -> Self {
        match &mut self {
            Llm::Remote { temperature, .. } => *temperature = temp,
            Llm::Local { temperature, max_tokens, .. } => (*temperature, *max_tokens) = (temp, max),
        }
        self
    }

    /// Streams one completion; `on_text` gets every text delta as it arrives.
    pub async fn stream(
        &self,
        messages: &[Value],
        tools: &[Value],
        mut on_text: impl FnMut(&str),
    ) -> Result<Completion, String> {
        let (url, api_key, body) = match self {
            Llm::Remote { url, model, api_key, temperature } => (url, api_key, request_body(model, messages, tools, *temperature)),
            Llm::Local { app, model_id, temperature, max_tokens } => {
                // The runtime wants a Send + 'static callback: pieces come back over a channel.
                let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<String>();
                let chat = crate::models::llama::chat(app, model_id, local_messages(messages), *temperature as f32, *max_tokens, move |piece| {
                    let _ = tx.send(piece.to_string());
                });
                tokio::pin!(chat);
                let text = loop {
                    tokio::select! {
                        result = &mut chat => break result?,
                        Some(piece) = rx.recv() => on_text(&piece),
                    }
                };
                while let Ok(piece) = rx.try_recv() {
                    on_text(&piece);
                }
                return Ok(Completion { text, tool_calls: Vec::new() });
            }
        };
        let client = reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(15))
            .timeout(Duration::from_secs(180))
            .build()
            .map_err(|e| e.to_string())?;
        let mut request = client.post(url).json(&body);
        if !api_key.trim().is_empty() {
            request = request.bearer_auth(api_key.trim());
        }
        let response = request.send().await.map_err(|e| {
            eprintln!("LLM request failed: {e}");
            "Couldn't reach the language model endpoint (Settings › Model)".to_string()
        })?;
        if !response.status().is_success() {
            let status = response.status();
            let text = response.text().await.unwrap_or_default();
            eprintln!("LLM returned {status}: {text}");
            return Err(match status.as_u16() {
                401 | 403 => "The language model rejected the API key (Settings › Model)".to_string(),
                404 => "Language model endpoint or model not found (Settings › Model)".to_string(),
                _ => format!("The language model returned {status}"),
            });
        }
        let mut assembler = Assembler::default();
        let mut stream = response.bytes_stream();
        while let Some(chunk) = stream.next().await {
            let chunk = chunk.map_err(|e| format!("The answer stream broke off: {e}"))?;
            let delta = assembler.push(&chunk);
            if !delta.is_empty() {
                on_text(&delta);
            }
        }
        assembler.finish()
    }
}

fn request_body(model: &str, messages: &[Value], tools: &[Value], temperature: f64) -> Value {
    let mut body = json!({ "model": model, "messages": messages, "stream": true, "temperature": temperature });
    if !tools.is_empty() {
        body["tools"] = Value::Array(tools.to_vec());
    }
    // gpt-oss reasons at medium effort by default: slow for dictation cleanup and short answers.
    if model.contains("gpt-oss") {
        body["reasoning_effort"] = json!("low");
    }
    // OpenAI's GPT-6 Luna/Sol reject temperature and tools unless reasoning is off.
    let bare = model.strip_prefix("openai/").unwrap_or(model);
    if bare.starts_with("gpt-6-luna") || bare.starts_with("gpt-6-sol") {
        body["reasoning_effort"] = json!("none");
    }
    body
}

/// OpenAI-style messages as (role, text) for the local model: tool calls and tool results are dropped.
fn local_messages(messages: &[Value]) -> Vec<(String, String)> {
    messages
        .iter()
        .filter_map(|m| {
            let role = m.get("role").and_then(Value::as_str)?;
            let content = m.get("content").and_then(Value::as_str)?;
            ["system", "user", "assistant"].contains(&role).then(|| (role.to_string(), content.to_string()))
        })
        .collect()
}

/// The assistant message that records the tool calls (sent back with the tool results).
pub fn assistant_tool_message(completion: &Completion) -> Value {
    json!({
        "role": "assistant",
        "content": if completion.text.is_empty() { Value::Null } else { Value::String(completion.text.clone()) },
        "tool_calls": completion.tool_calls.iter().map(|c| json!({
            "id": c.id, "type": "function", "function": { "name": c.name, "arguments": c.arguments }
        })).collect::<Vec<_>>(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn assembles_text_across_chunk_boundaries() {
        let mut a = Assembler::default();
        let d1 = a.push(b"data: {\"choices\":[{\"delta\":{\"content\":\"Hel\"}}]}\n\ndata: {\"choi");
        let d2 = a.push(b"ces\":[{\"delta\":{\"content\":\"lo\"}}]}\n\ndata: [DONE]\n\n");
        assert_eq!((d1.as_str(), d2.as_str()), ("Hel", "lo"));
        assert_eq!(a.completion.text, "Hello");
        assert!(a.done && a.completion.tool_calls.is_empty());
    }

    #[test]
    fn assembles_parallel_tool_calls() {
        let mut a = Assembler::default();
        for line in [
            r#"{"choices":[{"delta":{"tool_calls":[{"index":0,"id":"c1","function":{"name":"web_","arguments":""}}]}}]}"#,
            r#"{"choices":[{"delta":{"tool_calls":[{"index":0,"function":{"name":"search","arguments":"{\"que"}}]}}]}"#,
            r#"{"choices":[{"delta":{"tool_calls":[{"index":1,"id":"c2","function":{"name":"math_dates","arguments":"{}"}}]}}]}"#,
            r#"{"choices":[{"delta":{"tool_calls":[{"index":0,"function":{"arguments":"ry\":\"x\"}"}}]}}]}"#,
        ] {
            a.push(format!("data: {line}\n").as_bytes());
        }
        assert_eq!(a.completion.tool_calls, vec![
            ToolCall { id: "c1".into(), name: "web_search".into(), arguments: r#"{"query":"x"}"#.into() },
            ToolCall { id: "c2".into(), name: "math_dates".into(), arguments: "{}".into() },
        ]);
        let msg = assistant_tool_message(&a.completion);
        assert_eq!(msg["tool_calls"][0]["function"]["name"], "web_search");
        assert!(msg["content"].is_null());
    }

    #[test]
    fn multibyte_split_across_chunks() {
        let line = "data: {\"choices\":[{\"delta\":{\"content\":\"čž 日本\"},\"finish_reason\":\"stop\"}]}\n".as_bytes();
        let mut a = Assembler::default();
        for chunk in line.chunks(3) {
            a.push(chunk);
        }
        assert_eq!(a.finish().unwrap().text, "čž 日本");
    }

    #[test]
    fn errors_and_non_streaming_bodies() {
        let mut a = Assembler::default();
        a.push(b"data: {\"choices\":[{\"delta\":{\"content\":\"Par\"}}]}\ndata: {\"error\":{\"message\":\"rate limit\"}}\n");
        assert!(a.finish().unwrap_err().contains("rate limit"));
        let mut a = Assembler::default();
        a.push(b"data: {\"choices\":[{\"delta\":{\"content\":\"cut\"}}]}\n");
        assert!(a.finish().unwrap_err().contains("broke off"));
        let mut a = Assembler::default();
        a.push(br#"{"choices":[{"message":{"content":"plain json"}}],"error":null}"#);
        assert_eq!(a.finish().unwrap().text, "plain json");
    }

    #[test]
    fn tool_calls_without_index() {
        let mut a = Assembler::default();
        for line in [
            r#"{"choices":[{"delta":{"tool_calls":[{"id":"a","function":{"name":"web_search","arguments":"{}"}}]}}]}"#,
            r#"{"choices":[{"delta":{"tool_calls":[{"id":"b","function":{"name":"math_dates","arguments":"{}"}}]},"finish_reason":"tool_calls"}]}"#,
        ] {
            a.push(format!("data: {line}\n").as_bytes());
        }
        let calls = a.finish().unwrap().tool_calls;
        assert_eq!(calls.iter().map(|c| c.name.as_str()).collect::<Vec<_>>(), ["web_search", "math_dates"]);
    }

    #[test]
    fn local_messages_keep_text_only() {
        let completion = Completion {
            text: String::new(),
            tool_calls: vec![ToolCall { id: "c1".into(), name: "web_search".into(), arguments: "{}".into() }],
        };
        let messages = [
            json!({"role": "system", "content": "Be brief."}),
            json!({"role": "user", "content": "Weather?"}),
            assistant_tool_message(&completion),
            json!({"role": "tool", "tool_call_id": "c1", "content": "Sunny"}),
            json!({"role": "assistant", "content": "It's sunny."}),
        ];
        let pair = |r: &str, c: &str| (r.to_string(), c.to_string());
        assert_eq!(local_messages(&messages), [pair("system", "Be brief."), pair("user", "Weather?"), pair("assistant", "It's sunny.")]);
    }

    #[test]
    fn request_bodies() {
        let messages = [json!({"role": "user", "content": "Hi"})];
        let body = request_body("openai/gpt-oss-20b", &messages, &[], 0.1);
        assert_eq!((body["reasoning_effort"].as_str(), body["temperature"].as_f64()), (Some("low"), Some(0.1)));
        assert!(body.get("tools").is_none() && body["stream"] == true);
        let body = request_body("gpt-5.4-mini", &messages, &[json!({"type": "function"})], 0.3);
        assert!(body.get("reasoning_effort").is_none() && body["tools"].as_array().unwrap().len() == 1);
        // GPT-6 Luna/Sol only take temperature and tools with reasoning off.
        for model in ["gpt-6-luna", "openai/gpt-6-sol-2026-08"] {
            assert_eq!(request_body(model, &messages, &[], 0.3)["reasoning_effort"], "none", "{model}");
        }
    }

    #[test]
    fn ignores_comments_and_garbage() {
        let mut a = Assembler::default();
        assert_eq!(a.push(b": keep-alive\nevent: x\ndata: not json\n"), "");
        assert_eq!(a.completion, Completion::default());
    }
}
