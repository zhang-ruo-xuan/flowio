use serde::Serialize;
use std::time::Duration;

use crate::ai::types::AiConfig;

/// A chat message sent to the LLM API.
///
/// `content` can be a plain string or an array of content parts
/// (for vision models, including `image_url` with base64 data URIs).
#[derive(Debug, Clone, Serialize)]
pub struct Message {
    pub role: String,
    pub content: serde_json::Value,
}

/// Internal request payload matching OpenAI-compatible chat completions API.
#[derive(Serialize)]
struct ChatRequest<'a> {
    model: &'a str,
    messages: &'a [Message],
    max_tokens: u32,
    temperature: f32,
}

/// Internal response shape — only extract what we need.
#[derive(serde::Deserialize)]
struct ChatResponse {
    choices: Vec<Choice>,
}

#[derive(serde::Deserialize)]
struct Choice {
    message: ChoiceMessage,
}

#[derive(serde::Deserialize)]
struct ChoiceMessage {
    content: String,
}

/// Call the LLM API synchronously using `reqwest::blocking`.
///
/// Sends a POST request to `cfg.provider.base_url` with a Bearer token
/// (`cfg.api_key`). Timeout is 60 seconds.
///
/// # Errors
/// Returns a `String` error that includes HTTP status code and response
/// body when the server returns a non-2xx status.
pub fn call_llm(cfg: &AiConfig, messages: Vec<Message>) -> Result<String, String> {
    let client = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(60))
        .build()
        .map_err(|e| format!("Failed to build HTTP client: {}", e))?;

    let request_body = ChatRequest {
        model: &cfg.provider.model,
        messages: &messages,
        max_tokens: 1024,
        temperature: 0.3,
    };

    let response = client
        .post(&cfg.provider.base_url)
        .header("Authorization", format!("Bearer {}", cfg.api_key))
        .header("Content-Type", "application/json")
        .json(&request_body)
        .send()
        .map_err(|e| format!("HTTP request failed: {}", e))?;

    let status = response.status();

    if !status.is_success() {
        let status_code = status.as_u16();
        let body = response
            .text()
            .unwrap_or_else(|_| "<unable to read body>".to_string());
        return Err(format!(
            "LLM API returned HTTP {}: {}",
            status_code, body
        ));
    }

    let chat_response: ChatResponse = response
        .json()
        .map_err(|e| format!("Failed to parse LLM response JSON: {}", e))?;

    chat_response
        .choices
        .into_iter()
        .next()
        .map(|c| c.message.content)
        .ok_or_else(|| "LLM response contained no choices".to_string())
}
