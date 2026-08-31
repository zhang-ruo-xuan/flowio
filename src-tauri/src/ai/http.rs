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
        .post(normalize_chat_url(&cfg.provider.base_url))
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
        return Err(classify_llm_error(status_code, &body, &cfg.provider.model));
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

/// Classify a non-2xx LLM API response into a user-friendly error,
/// distinguishing model-level errors from API-key errors.
fn classify_llm_error(status_code: u16, text: &str, model: &str) -> String {
    let lower = text.to_lowercase();

    const MODEL_ERR_KEYWORDS: [&str; 19] = [
        "model not found", "model does not exist", "model not exist", "modelerror",
        "not supported", "no such model", "access to model denied", "accessdenied",
        "unpurchased", "not purchased", "not eligible", "model access denied",
        "model permission", "not allowed to access model", "model unavailable",
        "does not exist", "not_found_error", "模型不存在", "do not have access",
    ];
    const KEY_ERR_KEYWORDS: [&str; 9] = [
        "invalid api key", "invalid api-key", "incorrect api key", "incorrect api-key",
        "api key not valid", "invalid key", "autherror", "authentication", "unauthorized",
    ];
    const ACCOUNT_ERR_KEYWORDS: [&str; 8] = [
        "access denied", "forbidden", "arrears", "欠费", "quota",
        "account", "被禁用", "insufficient",
    ];

    if MODEL_ERR_KEYWORDS.iter().any(|k| lower.contains(k)) {
        format!("模型错误：{}——{}", model, extract_error_text(text))
    } else if KEY_ERR_KEYWORDS.iter().any(|k| lower.contains(k)) {
        "API Key 无效，请检查后重试".to_string()
    } else if ACCOUNT_ERR_KEYWORDS.iter().any(|k| lower.contains(k)) {
        format!("账户或权限异常（HTTP {}）：{}", status_code, extract_error_text(text))
    } else if status_code == 404 {
        "接口地址或路径错误（HTTP 404），请检查 base_url 是否填写正确（应为基础地址，不含 /chat/completions）".to_string()
    } else {
        format!("LLM API 返回错误（HTTP {}）：{}", status_code, extract_error_text(text))
    }
}

/// Extract a friendly error message from a provider error body.
/// Prefers `error.message`, then `error`, then `message` JSON fields.
fn extract_error_text(text: &str) -> String {
    if let Ok(v) = serde_json::from_str::<serde_json::Value>(text) {
        if let Some(msg) = v.pointer("/error/message").and_then(|m| m.as_str()) {
            return msg.to_string();
        }
        if let Some(err) = v.get("error").and_then(|e| e.as_str()) {
            return err.to_string();
        }
        if let Some(msg) = v.get("message").and_then(|m| m.as_str()) {
            return msg.to_string();
        }
    }
    text.chars().take(200).collect()
}

/// Normalize a provider base URL into a full `/chat/completions` endpoint.
///
/// - Already ends with `/chat/completions` → reused as-is.
/// - Otherwise → trailing slashes trimmed, then `/chat/completions` appended.
///
/// Custom providers store the *base* address (e.g.
/// `https://dashscope.aliyuncs.com/compatible-mode/v1`); call_llm must
/// target the actual chat endpoint.
pub fn normalize_chat_url(base_url: &str) -> String {
    let trimmed = base_url.trim_end_matches('/');
    if trimmed.ends_with("/chat/completions") {
        trimmed.to_string()
    } else {
        format!("{}/chat/completions", trimmed)
    }
}

#[cfg(test)]
mod http_tests {
    use super::*;

    fn cfg_with_url(base_url: &str) -> AiConfig {
        AiConfig {
            provider: crate::ai::types::AiProvider {
                id: "custom".into(),
                name: "自定义".into(),
                base_url: base_url.into(),
                model: "qwen3.6-plus".into(),
            },
            api_key: "sk-test".into(),
        }
    }

    // ── normalize_chat_url ─────────────────────────────────────────

    #[test]
    fn normalize_base_address_appends_chat_completions() {
        assert_eq!(
            normalize_chat_url("https://dashscope.aliyuncs.com/compatible-mode/v1"),
            "https://dashscope.aliyuncs.com/compatible-mode/v1/chat/completions"
        );
    }

    #[test]
    fn normalize_trims_trailing_slashes() {
        assert_eq!(
            normalize_chat_url("https://open.bigmodel.cn/api/paas/v4///"),
            "https://open.bigmodel.cn/api/paas/v4/chat/completions"
        );
    }

    #[test]
    fn normalize_reuses_full_endpoint() {
        assert_eq!(
            normalize_chat_url("https://open.bigmodel.cn/api/paas/v4/chat/completions"),
            "https://open.bigmodel.cn/api/paas/v4/chat/completions"
        );
        assert_eq!(
            normalize_chat_url("https://api.deepseek.com/v1/chat/completions/"),
            "https://api.deepseek.com/v1/chat/completions"
        );
    }

    #[test]
    fn normalize_root_path_falls_back_to_relative() {
        assert_eq!(normalize_chat_url(""), "/chat/completions");
        assert_eq!(normalize_chat_url("/"), "/chat/completions");
    }

    #[test]
    fn call_llm_uses_normalized_url() {
        // call_llm 不能直接对基础地址发 POST；normalize 后的 URL 才是目标端点。
        // 这里验证 normalize 的端到端结果（实际网络请求由集成层验证）。
        let cfg = cfg_with_url("https://api.example.com/v1");
        let url = normalize_chat_url(&cfg.provider.base_url);
        assert_eq!(url, "https://api.example.com/v1/chat/completions");
    }

    // ── classify_llm_error ─────────────────────────────────────────

    #[test]
    fn classify_model_error_wins_over_key_and_account() {
        // 千问实测：有效 Key + 未购买模型 → 403 AccessDenied
        let err = classify_llm_error(
            403,
            r#"{"error":{"code":"AccessDenied","message":"Model qwen3.6-plus has not been purchased"}}"#,
            "qwen3.6-plus",
        );
        assert!(err.starts_with("模型错误：qwen3.6-plus"), "err: {}", err);
        assert!(err.contains("not been purchased"), "err: {}", err);
    }

    #[test]
    fn classify_model_not_found_404() {
        let err = classify_llm_error(404, r#"{"error":{"message":"model does not exist"}}"#, "foo");
        assert!(err.starts_with("模型错误：foo"), "err: {}", err);
    }

    #[test]
    fn classify_chinese_model_not_found() {
        let err = classify_llm_error(400, r#"{"error":{"message":"模型不存在或已下线"}}"#, "foo");
        assert!(err.starts_with("模型错误：foo"), "err: {}", err);
    }

    #[test]
    fn classify_invalid_key_401() {
        let err = classify_llm_error(
            401,
            r#"{"error":{"message":"Incorrect API key provided"}}"#,
            "qwen3.6-plus",
        );
        assert_eq!(err, "API Key 无效，请检查后重试");
    }

    #[test]
    fn classify_account_arrears_400() {
        // 千问实测：内置 Key 失效 → 400 Access denied（账户欠费/被禁用）
        let err = classify_llm_error(400, r#"{"error":{"message":"Access denied"}}"#, "qwen3.6-plus");
        assert!(err.starts_with("账户或权限异常（HTTP 400）"), "err: {}", err);
    }

    #[test]
    fn classify_forbidden_403_without_model_keywords() {
        let err = classify_llm_error(403, r#"{"message":"Forbidden"}"#, "m");
        assert!(err.starts_with("账户或权限异常（HTTP 403）"), "err: {}", err);
    }

    #[test]
    fn classify_404_without_keywords_is_url_error() {
        let err = classify_llm_error(404, "Not Found", "m");
        assert!(err.contains("接口地址或路径错误（HTTP 404）"), "err: {}", err);
        assert!(err.contains("base_url"), "err: {}", err);
    }

    #[test]
    fn classify_unknown_status_falls_back() {
        let err = classify_llm_error(500, "internal error", "m");
        assert!(err.starts_with("LLM API 返回错误（HTTP 500）"), "err: {}", err);
        assert!(err.contains("internal error"), "err: {}", err);
    }

    #[test]
    fn classify_empty_body_404_uses_status_code() {
        let err = classify_llm_error(404, "", "m");
        assert!(err.contains("HTTP 404"), "err: {}", err);
    }

    // ── extract_error_text ─────────────────────────────────────────

    #[test]
    fn extract_prefers_error_message_field() {
        let text = r#"{"error":{"message":"Model not found","code":"NotFoundError"}}"#;
        assert_eq!(extract_error_text(text), "Model not found");
    }

    #[test]
    fn extract_falls_back_to_error_string() {
        let text = r#"{"error":"bad gateway"}"#;
        assert_eq!(extract_error_text(text), "bad gateway");
    }

    #[test]
    fn extract_falls_back_to_top_level_message() {
        let text = r#"{"message":"quota exceeded"}"#;
        assert_eq!(extract_error_text(text), "quota exceeded");
    }

    #[test]
    fn extract_non_json_truncates_to_200_chars() {
        let long = "x".repeat(500);
        let out = extract_error_text(&long);
        assert_eq!(out.len(), 200);
        let plain = extract_error_text("plain text error");
        assert_eq!(plain, "plain text error");
    }
}
