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
    /// 🔴 r1030：智谱私有字段 —— 关掉思考型模型的 thinking。
    ///
    ///   背景：桌面默认模型 `glm-4.6v-flash` 是**思考型**模型，纯文本提问时
    ///   会把 max_tokens 整份额度花在 reasoning_content 上，标准 content 回空串
    ///   ⇒ 桌面端「AI 摘要 / 自动描述」拿到空内容（表现为"点了没反应"）。
    ///   网站侧早在 2026-10-08 就实测过同一现象并用这个字段修掉：
    ///   单次从 328 token / 11.5s 降到 58 token / 2.3s。
    ///
    ///   🔴 只对智谱端点发这个字段：它是智谱私有参数，千问 / DeepSeek 等
    ///   标准 OpenAI 兼容端点收到未知参数可能直接 400（网站侧同款纪律）。
    #[serde(skip_serializing_if = "Option::is_none")]
    thinking: Option<ThinkingDisabled>,
}

/// 智谱 `thinking: {type:"disabled"}` 的序列化形态。
#[derive(Serialize)]
struct ThinkingDisabled {
    #[serde(rename = "type")]
    kind: &'static str,
}

impl ChatRequest<'_> {
    /// 按服务商决定要不要关掉思考。
    ///
    /// 判定依据是 **base_url 主机名**（不是模型名）：同一个模型名在不同端点下
    /// 字段支持度不同，按 host 判最稳。
    fn thinking_for(base_url: &str) -> Option<ThinkingDisabled> {
        let host = base_url
            .split("//")
            .nth(1)
            .unwrap_or("")
            .split('/')
            .next()
            .unwrap_or("");
        if host.eq_ignore_ascii_case("open.bigmodel.cn") || host.eq_ignore_ascii_case("api.z.ai") {
            Some(ThinkingDisabled { kind: "disabled" })
        } else {
            None
        }
    }

    /// 构造实际发出的请求体。
    ///
    /// 🔴 **单独抽成函数**（而不是内联在调用点）：这样测试能锁住「调用点真的
    ///   带了 thinking」这件事 —— 只测 `thinking_for` 的锁是假绿（把调用点
    ///   的 `thinking:` 改成 None 它照样绿，r1030 变异自证实测过）。
    fn build<'a>(
        model: &'a str,
        messages: &'a [Message],
        max_tokens: u32,
        temperature: f32,
        base_url: &str,
    ) -> ChatRequest<'a> {
        ChatRequest {
            model,
            messages,
            max_tokens,
            temperature,
            thinking: ChatRequest::thinking_for(base_url),
        }
    }
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
    /// 标准输出。**思考型模型**（如 glm-4.6v-flash）纯文本提问时可能把
    /// max_tokens 预算全花在 thinking 上、content 留空 ⇒ 下面有回落逻辑。
    #[serde(default)]
    content: String,
    /// 思考型模型的思维链字段（智谱 GLM 系列 / DeepSeek reasoner）。
    /// 仅当 content 为空时作为兜底内容使用。
    #[serde(default)]
    reasoning_content: Option<String>,
}

/// Call the LLM API synchronously using `reqwest::blocking`.
///
/// Sends a POST request to `cfg.provider.base_url` with a Bearer token
/// (`cfg.api_key`). Single-attempt timeout is 120 seconds.
///
/// # 429 限流重试
/// 免费通道（glm-4.6v-flash）高峰期高频返回 HTTP 429「1305 该模型当前访问量
/// 过大」（实测连续 17 步生成时 15 步被限流）。此处按 3s/6s/12s/24s 指数退避
/// 重试最多 4 次（总等待约 45s，足够跨过一次限流峰值）；网络瞬断（send Err）同样重试。
/// 其他非 2xx 立即失败。
///
/// # Errors
/// Returns a `String` error that includes HTTP status code and response
/// body when the server returns a non-2xx status.
pub fn call_llm(cfg: &AiConfig, messages: Vec<Message>) -> Result<String, String> {
    call_llm_with_tokens(cfg, messages, 1024)
}

/// [`call_llm`] 的可调 `max_tokens` 版本。
///
/// 默认 1024 对「逐步生成标题/描述」够用；翻译整本教程（全部步骤打包
/// 一次请求输出完整 JSON）必须放宽到数千 token，否则 JSON 会被截断。
pub fn call_llm_with_tokens(
    cfg: &AiConfig,
    messages: Vec<Message>,
    max_tokens: u32,
) -> Result<String, String> {
    let client = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(120))
        .build()
        .map_err(|e| format!("Failed to build HTTP client: {}", e))?;

    let request_body = ChatRequest::build(
        &cfg.provider.model,
        &messages,
        max_tokens,
        0.3,
        &cfg.provider.base_url,
    );

    const MAX_RETRIES: usize = 4;
    // 🔴 内置免费通道 Key 池轮换（2026-09-07）：智谱限流按账号计，单账号退避
    //    重试只会继续撞 429（实测整轮扫描 5/5 步全 429）。仅当当前 Key 属于
    //    内置池时轮换；用户自配 Key 绝不覆盖。
    let key_pool: Vec<String> =
        if cfg.provider.id == "flowio-free"
            && crate::ai::commands::builtin_free_key_pool().iter().any(|k| *k == cfg.api_key)
        {
            crate::ai::commands::builtin_free_key_pool()
        } else {
            vec![cfg.api_key.clone()]
        };
    let mut key_idx = 0;
    let mut last_err = String::new();
    for attempt in 0..=MAX_RETRIES {
        if attempt > 0 {
            // 3s / 6s / 12s / 24s 指数退避（封顶 24s），跨过限流峰值
            let delay = (3u64 << (attempt - 1)).min(24);
            std::thread::sleep(Duration::from_secs(delay));
        }
        let active_key = &key_pool[key_idx];
        let response = match client
            .post(normalize_chat_url(&cfg.provider.base_url))
            .header("Authorization", format!("Bearer {}", active_key))
            .header("Content-Type", "application/json")
            .json(&request_body)
            .send()
        {
            Ok(r) => r,
            Err(e) => {
                last_err = format!("HTTP request failed: {}", e);
                continue;
            }
        };

        let status = response.status();

        if status.as_u16() == 429 {
            let body = response
                .text()
                .unwrap_or_else(|_| "<unable to read body>".to_string());
            if key_pool.len() > 1 {
                key_idx = (key_idx + 1) % key_pool.len();
                crate::ai::commands::ai_dbg_log(&format!(
                    "[AI] LLM 429，切换内置 Key 池（尾 {}）后重试",
                    &key_pool[key_idx][key_pool[key_idx].len().saturating_sub(6)..]
                ));
            }
            last_err = classify_llm_error(429, &body, &cfg.provider.model);
            continue;
        }

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

        return chat_response
            .choices
            .into_iter()
            .next()
            .map(|c| pick_response_text(&c.message))
            .ok_or_else(|| "LLM response contained no choices".to_string());
    }

    Err(format!("{}（已重试 {} 次仍失败）", last_err, MAX_RETRIES))
}

/// 从一条响应消息里挑出「给用户看」的正文。
///
/// 🔴 r1030：思考型模型（glm-4.6v-flash 等）纯文本提问时，max_tokens 预算被
///   thinking 吃光 ⇒ 标准 `content` 回空串，而有用内容在 `reasoning_content`。
///   旧逻辑只取 content ⇒ 桌面端「AI 摘要 / 自动描述」拿到空内容
///   （实测：带图时 content 有值、纯文本时为空 ⇒ 表现为"点了没反应"）。
///
///   规则：content 非空 ⇒ 用 content（正常路径优先）；content 为空 ⇒ 回落
///   reasoning_content（剥掉思维链标记）；两者都空 ⇒ 保持空串。
///
/// 🔴 **单独抽成函数**（而不是内联在调用点）：这样测试能直接锁住真实取值
///   路径，删掉回落逻辑时锁必红（r1030 变异自证踩过这个坑：只测纯函数
///   `strip_reasoning_markers` 的锁是假绿）。
fn pick_response_text(message: &ChoiceMessage) -> String {
    let primary = message.content.trim();
    if !primary.is_empty() {
        return message.content.clone();
    }
    let reasoning = message.reasoning_content.clone().unwrap_or_default();
    let fallback = strip_reasoning_markers(&reasoning);
    if fallback.trim().is_empty() {
        message.content.clone()
    } else {
        fallback
    }
}

/// 去掉思考型模型思维链里的标记行（如 `<think>…</think>`、`<reasoning>…</reasoning>`），
/// 只留下真正给用户看的正文。
fn strip_reasoning_markers(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    let mut rest = raw;
    // 反复剥掉成对的标记块（有的模型会输出多段）
    loop {
        let mut found = false;
        for (open, close) in [("<think>", "</think>"), ("<reasoning>", "</reasoning>")] {
            if let Some(start) = rest.find(open) {
                if let Some(end) = rest[start..].find(close) {
                    let after = &rest[start + open.len()..start + end];
                    out.push_str(after);
                    rest = &rest[start + end + close.len()..];
                    found = true;
                    break;
                }
            }
        }
        if !found {
            break;
        }
    }
    out.push_str(rest);
    out.trim().to_string()
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
    } else if status_code == 429 {
        // 免费通道高峰期限流是主因；提示用户错峰或改用自有 Key
        format!(
            "{}（HTTP 429）。免费通道高峰期访问量过大被限流，已自动重试仍失败。建议：① 稍后几分钟再试；② 在「设置 → AI」里配置自己的 AI Key 走私有额度，更稳定。",
            extract_error_text(text)
        )
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

    // ── r1030：思考型模型（glm-4.6v-flash）兼容 ────────────────────────

    /// 🔴 智谱端点必须带 `thinking:{type:"disabled"}`。
    ///
    /// 原因（实测 2026-10-25）：`glm-4.6v-flash` 是思考型模型，纯文本提问时
    /// 把 max_tokens 整份额度花在 reasoning_content 上、标准 content 回空串
    /// ⇒ 桌面端「AI 摘要 / 自动描述」拿到空内容（表现为"点了没反应"）。
    /// 网站侧 2026-10-08 已用同一字段修掉（328 token/11.5s → 58 token/2.3s）。
    #[test]
    fn thinking_disabled_only_for_zhipu_hosts() {
        assert!(ChatRequest::thinking_for("https://open.bigmodel.cn/api/paas/v4/chat/completions").is_some());
        assert!(ChatRequest::thinking_for("https://api.z.ai/api/paas/v4/chat/completions").is_some());
        // 🔴 智谱私有字段，标准 OpenAI 兼容端点收到可能 400 ⇒ 不能发
        assert!(ChatRequest::thinking_for("https://dashscope.aliyuncs.com/compatible-mode/v1/chat/completions").is_none());
        assert!(ChatRequest::thinking_for("https://api.deepseek.com/v1/chat/completions").is_none());
        assert!(ChatRequest::thinking_for("https://api.openai.com/v1/chat/completions").is_none());
    }

    /// `thinking` 字段序列化形态必须是 `{"type":"disabled"}`。
    #[test]
    fn thinking_serializes_as_type_disabled() {
        let v = serde_json::to_value(ChatRequest::thinking_for("https://open.bigmodel.cn").unwrap()).unwrap();
        assert_eq!(v["type"], "disabled");
    }

    /// 🔴 智谱请求体里**必须真的带上** `thinking:{"type":"disabled"}`。
    ///
    /// ⚠ 走 `ChatRequest::build`（**真实构造路径**，与调用点同一份）——
    ///   否则把调用点的 thinking 去掉它照样绿（r1030 变异自证实测的假绿）。
    #[test]
    fn zhipu_request_body_carries_thinking_disabled() {
        let body = ChatRequest::build(
            "glm-4.6v-flash",
            &[],
            1024,
            0.3,
            "https://open.bigmodel.cn/api/paas/v4/chat/completions",
        );
        let v = serde_json::to_value(&body).unwrap();
        assert_eq!(v["thinking"]["type"], "disabled");
    }

    /// 🔴 第三方请求体里**不许**出现 `thinking` 字段（智谱私有参数会 400）。
    #[test]
    fn third_party_request_body_has_no_thinking_field() {
        for host in [
            "https://api.openai.com/v1/chat/completions",
            "https://api.deepseek.com/v1/chat/completions",
            "https://dashscope.aliyuncs.com/compatible-mode/v1/chat/completions",
        ] {
            let body = ChatRequest::build("m", &[], 1024, 0.3, host);
            let v = serde_json::to_value(&body).unwrap();
            assert!(
                v.get("thinking").is_none(),
                "{} 的请求体不该带 thinking 字段",
                host
            );
        }
    }

    /// 🔴 思考型模型纯文本返回空 content 时，回落 reasoning_content。
    ///
    /// ⚠ 直接调**真实取值函数** `pick_response_text`（与调用点同一份）——
    ///   早期版本只测 `strip_reasoning_markers` 纯函数 ⇒ 删掉调用点的回落
    ///   逻辑它照样绿（r1030 变异自证实测到的假绿，已修）。
    #[test]
    fn falls_back_to_reasoning_content_when_content_empty() {
        let json = r#"{"choices":[{"message":{"content":"","reasoning_content":"用户要关闭个性化广告。先找设置入口。"}}]}"#;
        let resp: ChatResponse = serde_json::from_str(json).unwrap();
        let m = resp.choices.into_iter().next().unwrap().message;
        assert!(m.content.trim().is_empty());
        let picked = pick_response_text(&m);
        assert_eq!(picked, "用户要关闭个性化广告。先找设置入口。");
        // 🔴 反向自证锚点：删掉调用点的回落 ⇒ picked 变空串 ⇒ 本锁必红
        assert!(!picked.trim().is_empty());
    }

    /// content 有值时**不该**被 reasoning 覆盖（正常路径优先）。
    #[test]
    fn does_not_override_non_empty_content_with_reasoning() {
        let json = r#"{"choices":[{"message":{"content":"图片里是红色。","reasoning_content":"内部思考"}}]}"#;
        let resp: ChatResponse = serde_json::from_str(json).unwrap();
        let m = resp.choices.into_iter().next().unwrap().message;
        assert_eq!(pick_response_text(&m), "图片里是红色。");
    }

    /// content 与 reasoning 都空 ⇒ 保持空串（不塞垃圾）。
    #[test]
    fn both_content_and_reasoning_empty_stays_empty() {
        let json = r#"{"choices":[{"message":{"content":"","reasoning_content":""}}]}"#;
        let resp: ChatResponse = serde_json::from_str(json).unwrap();
        let m = resp.choices.into_iter().next().unwrap().message;
        assert_eq!(pick_response_text(&m).trim(), "");
    }

    /// 思维链标记块要被剥掉（`<think>…</think>`）。
    ///
    /// 注意语义：标记**块**被移除，但块**内部**的文字保留（思维过程本身
    /// 也是有效信息，不能整段丢）。所以结果是"内部文字 + 标记外的正文"。
    #[test]
    fn strips_think_tags_from_reasoning() {
        let raw = "<think>先分析图片颜色</think>图里是蓝色。";
        let out = strip_reasoning_markers(raw);
        assert!(!out.contains("<think>"));
        assert!(!out.contains("</think>"));
        assert!(out.contains("图里是蓝色。"));
    }

    /// content 与 reasoning 都空 ⇒ 不返回垃圾（保持空）。
    #[test]
    fn empty_reasoning_stays_empty() {
        assert_eq!(strip_reasoning_markers("   "), "");
        assert_eq!(strip_reasoning_markers(""), "");
    }
}
