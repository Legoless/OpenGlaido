use reqwest::multipart::{Form, Part};
use reqwest::Client;
use serde::{Deserialize, Serialize};
use std::time::Duration;

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct TranscriptionConfig {
    pub endpoint_url: String,       // e.g. "https://api.groq.com/openai/v1/audio/transcriptions" or custom URL
    pub api_key: String,
    pub model_name: String,         // e.g. "whisper-large-v3-turbo" or "whisper-1"
    pub language: Option<String>,
    pub temperature: Option<f32>,
    pub mode: String,               // "default", "email", "chat", "code", "raw"
    pub enable_llm_formatting: bool,
    pub llm_endpoint_url: Option<String>, // e.g. "https://api.groq.com/openai/v1/chat/completions" or Ollama
    pub llm_model_name: Option<String>,
    pub custom_formatting_prompt: Option<String>,
    pub sound_feedback: bool,       // play audio chimes on start/stop
    pub hotkey: String,             // default: "CommandOrControl+Shift+Space"
}

impl Default for TranscriptionConfig {
    fn default() -> Self {
        Self {
            endpoint_url: "https://api.groq.com/openai/v1/audio/transcriptions".to_string(),
            api_key: "".to_string(),
            model_name: "whisper-large-v3-turbo".to_string(),
            language: None,
            temperature: Some(0.0),
            mode: "default".to_string(),
            enable_llm_formatting: true,
            llm_endpoint_url: Some("https://api.groq.com/openai/v1/chat/completions".to_string()),
            llm_model_name: Some("llama-3.3-70b-versatile".to_string()),
            custom_formatting_prompt: Some("You are a voice dictation text cleanup assistant. Clean up this raw transcription: fix capitalization and punctuation, and remove conversational filler sounds (um, uh). Keep exact words and meaning. Return ONLY the cleaned text with no introductory or meta commentary.".to_string()),
            sound_feedback: true,
            hotkey: "CommandOrControl+Shift+Space".to_string(),
        }
    }
}

#[derive(Deserialize)]
struct WhisperResponse {
    text: String,
}

#[derive(Serialize, Deserialize)]
struct ChatMessage {
    role: String,
    content: String,
}

#[derive(Serialize)]
struct ChatCompletionRequest {
    model: String,
    messages: Vec<ChatMessage>,
    temperature: f32,
}

#[derive(Deserialize)]
struct ChatCompletionChoice {
    message: ChatMessage,
}

#[derive(Deserialize)]
struct ChatCompletionResponse {
    choices: Vec<ChatCompletionChoice>,
}

pub async fn transcribe_audio(
    wav_bytes: Vec<u8>,
    config: &TranscriptionConfig,
    initial_prompt: Option<String>,
) -> Result<String, String> {
    let client = Client::builder()
        .timeout(Duration::from_secs(45))
        .build()
        .map_err(|e| e.to_string())?;

    let part = Part::bytes(wav_bytes)
        .file_name("audio.wav")
        .mime_str("audio/wav")
        .map_err(|e| e.to_string())?;

    let mut form = Form::new()
        .text("model", config.model_name.clone())
        .part("file", part);

    if let Some(prompt) = initial_prompt {
        if !prompt.trim().is_empty() {
            form = form.text("prompt", prompt);
        }
    }

    if let Some(lang) = &config.language {
        if !lang.trim().is_empty() {
            form = form.text("language", lang.clone());
        }
    }

    if let Some(temp) = config.temperature {
        form = form.text("temperature", temp.to_string());
    }

    let mut request = client.post(&config.endpoint_url).multipart(form);
    if !config.api_key.trim().is_empty() {
        request = request.header("Authorization", format!("Bearer {}", config.api_key.trim()));
    }

    let response = request
        .send()
        .await
        .map_err(|e| format!("Network request failed: {}", e))?;

    if !response.status().is_success() {
        let status = response.status();
        let body = response
            .text()
            .await
            .unwrap_or_else(|_| "Unknown error".to_string());
        return Err(format!("STT API returned error {}: {}", status, body));
    }

    let whisper_res: WhisperResponse = response
        .json()
        .await
        .map_err(|e| format!("Failed to parse STT response JSON: {}", e))?;

    let raw_text = whisper_res.text.trim().to_string();

    // If raw mode or formatting is disabled, return raw transcript
    if config.mode == "raw" || !config.enable_llm_formatting || raw_text.is_empty() {
        return Ok(raw_text);
    }

    // Determine system prompt according to active mode
    let effective_prompt = match config.mode.as_str() {
        "email" => "You are an email dictation assistant. Format the transcription into clean, professional email sentences with proper punctuation, polite phrasing, and paragraph breaks where appropriate. Remove filler words (um, uh). Output ONLY the final email text.",
        "chat" => "You are a casual chat dictation assistant. Format this speech for quick messaging (Slack/Discord): keep it concise, natural, well-punctuated, without filler sounds. Do NOT add formal greetings. Output ONLY the message.",
        "code" => "You are a developer dictation assistant. Format this speech for technical/coding contexts: convert spoken symbols ('open paren', 'arrow function', 'camelCase') into proper code or technical phrasing. Output ONLY the resulting text.",
        _ => config.custom_formatting_prompt.as_deref().unwrap_or(
            "Clean up this raw transcription: fix capitalization and punctuation, and remove conversational filler sounds (um, uh). Return ONLY the cleaned text."
        ),
    };

    // Run AI text cleanup pass
    if let (Some(llm_url), Some(llm_model)) = (
        &config.llm_endpoint_url,
        &config.llm_model_name,
    ) {
        if !llm_url.trim().is_empty() && !llm_model.trim().is_empty() {
            match format_transcript_with_llm(&client, &raw_text, llm_url, llm_model, &config.api_key, effective_prompt).await {
                Ok(formatted) => return Ok(formatted),
                Err(err) => {
                    eprintln!("Formatting pass failed, falling back to raw text: {}", err);
                    return Ok(raw_text);
                }
            }
        }
    }

    Ok(raw_text)
}

async fn format_transcript_with_llm(
    client: &Client,
    raw_text: &str,
    llm_url: &str,
    llm_model: &str,
    api_key: &str,
    system_prompt: &str,
) -> Result<String, String> {
    let chat_req = ChatCompletionRequest {
        model: llm_model.to_string(),
        messages: vec![
            ChatMessage {
                role: "system".to_string(),
                content: system_prompt.to_string(),
            },
            ChatMessage {
                role: "user".to_string(),
                content: raw_text.to_string(),
            },
        ],
        temperature: 0.1,
    };

    let mut req = client.post(llm_url).json(&chat_req);
    if !api_key.trim().is_empty() {
        req = req.header("Authorization", format!("Bearer {}", api_key.trim()));
    }

    let resp = req.send().await.map_err(|e| e.to_string())?;
    if !resp.status().is_success() {
        return Err(format!("LLM status: {}", resp.status()));
    }

    let completion: ChatCompletionResponse = resp.json().await.map_err(|e| e.to_string())?;
    if let Some(choice) = completion.choices.into_iter().next() {
        let cleaned = choice.message.content.trim().to_string();
        if !cleaned.is_empty() {
            return Ok(cleaned);
        }
    }

    Ok(raw_text.to_string())
}
