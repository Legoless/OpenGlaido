//! OpenAI-compatible chat completions with streaming and tool calls (Groq, OpenAI, Ollama, …).

use futures_util::StreamExt;
use serde_json::{json, Value};
use std::time::Duration;

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

pub struct Llm {
    pub url: String,
    pub model: String,
    pub api_key: String,
}

impl Llm {
    /// Streams one completion; `on_text` gets every text delta as it arrives.
    pub async fn stream(
        &self,
        messages: &[Value],
        tools: &[Value],
        mut on_text: impl FnMut(&str),
    ) -> Result<Completion, String> {
        let client = reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(15))
            .timeout(Duration::from_secs(180))
            .build()
            .map_err(|e| e.to_string())?;
        let mut body = json!({ "model": self.model, "messages": messages, "stream": true, "temperature": 0.3 });
        if !tools.is_empty() {
            body["tools"] = Value::Array(tools.to_vec());
        }
        let mut request = client.post(&self.url).json(&body);
        if !self.api_key.trim().is_empty() {
            request = request.bearer_auth(self.api_key.trim());
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
    fn ignores_comments_and_garbage() {
        let mut a = Assembler::default();
        assert_eq!(a.push(b": keep-alive\nevent: x\ndata: not json\n"), "");
        assert_eq!(a.completion, Completion::default());
    }
}
