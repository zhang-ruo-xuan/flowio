//! 智谱 GLM-ASR 语音转写客户端。
//!
//! 接口：`POST https://open.bigmodel.cn/api/paas/v4/audio/transcriptions`
//! （multipart：`model` + `file`，wav/mp3，单段 ≤60s / ≤25MB）。
//! 429 / 网络瞬断按 3s/6s/12s 指数退避重试 3 次（与 ai::http::call_llm 一致）；
//! 但「余额/资源包」类 429 立即失败（见 `is_unrecoverable_quota_error`）。

use std::time::Duration;

/// ASR 端点（固定智谱；自定义服务商暂不支持 ASR）
pub const ASR_URL: &str = "https://open.bigmodel.cn/api/paas/v4/audio/transcriptions";

/// ASR 模型名。
///
/// 🔴 **必须是 `glm-asr-2512`，不能改成 `glm-asr`**（2026-09-03 实测踩坑，
/// 曾导致语音功能整体失效数个版本）：
/// | 模型名 | 内置免费 Key 实测结果 |
/// |---|---|
/// | `glm-asr-2512` | HTTP 200，中文转写逐字准确 ✅ |
/// | `glm-asr` | HTTP 429 / code 1113「余额不足或无可用资源包」❌ |
///
/// `glm-asr` 是**付费**模型，免费 Key 无其资源包；而失败时旧逻辑会把 429
/// 一律当限流退避重试 3 次（3s+6s+12s=21s），用户表现为「录了半天、
/// 最后提示未检测到语音内容」。改模型名前请重新跑一遍真实语音回归。
pub const ASR_MODEL: &str = "glm-asr-2512";

/// 当前 AI 配置是否属于「智谱系」通道。
///
/// 语音转写的端点是**硬编码智谱**的（`ASR_URL`），因此只有智谱的 Key 可用：
/// 内置免费通道（flowio-free）、用户自配的智谱（zhipu，base_url 指向
/// open.bigmodel.cn）。千问/DeepSeek 等第三方兼容端点没有
/// `/audio/transcriptions`，其 Key 打到智谱必然 401。
///
/// ⚠️ 返回 false **不代表语音要失效** —— 调用方应回落内置免费通道 Key
/// （见 `voice::commands::resolve_asr_key`），而不是拒绝服务。
pub fn asr_supported(provider_id: &str, base_url: &str) -> bool {
    matches!(provider_id, "zhipu" | "flowio-free")
        || base_url.contains("open.bigmodel.cn")
}

#[derive(serde::Deserialize)]
struct AsrResponse {
    text: String,
}

/// 转写一段 WAV 音频（≤55s），返回识别文本。
///
/// # Errors
/// HTTP 非 2xx 或重试耗尽时返回带原因的 String；响应体缺 `text` 字段时
/// 返回解析错误。
pub fn transcribe_wav(api_key: &str, wav_bytes: Vec<u8>) -> Result<String, String> {
    let client = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(90))
        .build()
        .map_err(|e| format!("Failed to build HTTP client: {}", e))?;

    const MAX_RETRIES: usize = 3;
    let mut last_err = String::new();
    for attempt in 0..=MAX_RETRIES {
        if attempt > 0 {
            std::thread::sleep(Duration::from_secs(3 << (attempt - 1)));
        }

        let part = reqwest::blocking::multipart::Part::bytes(wav_bytes.clone())
            .file_name("audio.wav")
            .mime_str("audio/wav")
            .map_err(|e| format!("multipart part error: {}", e))?;
        let form = reqwest::blocking::multipart::Form::new()
            .text("model", ASR_MODEL)
            .part("file", part);

        let response = match client
            .post(ASR_URL)
            .header("Authorization", format!("Bearer {}", api_key))
            .multipart(form)
            .send()
        {
            Ok(r) => r,
            Err(e) => {
                last_err = format!("HTTP request failed: {}", e);
                continue;
            }
        };

        let status = response.status();
        let body = response
            .text()
            .unwrap_or_else(|_| "<unable to read body>".to_string());

        if status.as_u16() == 429 {
            // ⚠️ 429 有两种，行为必须相反（2026-09-03 实测踩坑）：
            //   · code 1305「该模型当前访问量过大」= 限流，**重试有意义**
            //   · code 1113「余额不足或无可用资源包」= 账户/资源包问题，
            //     重试 3 次只会白等 21 秒（3s+6s+12s）后照样失败，
            //     必须立即返回可行动的错误。
            let text = extract_error_text(&body);
            if is_unrecoverable_quota_error(&body) {
                return Err(format!(
                    "语音转写配额不可用（HTTP 429）：{}。\
                     当前 Key 没有可用的语音识别资源包，请在设置 → AI 服务商中配置智谱 AI 的 API Key，\
                     或为智谱账户充值语音资源包后重试",
                    text
                ));
            }
            last_err = format!("HTTP 429 {}", text);
            continue;
        }
        if !status.is_success() {
            return Err(format!(
                "GLM-ASR 返回错误（HTTP {}）：{}",
                status.as_u16(),
                extract_error_text(&body)
            ));
        }

        let parsed: AsrResponse = serde_json::from_str(&body)
            .map_err(|e| format!("ASR 响应解析失败：{}（body: {}）", e, truncate(&body, 200)))?;
        return Ok(parsed.text.trim().to_string());
    }

    Err(format!("{}（已重试 {} 次仍失败）", last_err, MAX_RETRIES))
}

/// 429 是否属于「重试也没用」的配额/账户类错误。
///
/// 智谱实测错误码：`1113` = 余额不足或无可用资源包（不可恢复）；
/// `1305` = 模型访问量过大（限流，退避重试有效）。
/// 无法解析 code 时退化为按关键词匹配，宁可少重试一次也不要白等 21 秒。
fn is_unrecoverable_quota_error(body: &str) -> bool {
    if let Ok(v) = serde_json::from_str::<serde_json::Value>(body) {
        if let Some(code) = v.pointer("/error/code") {
            let code = code.as_str().unwrap_or_default().to_string()
                + &code.as_i64().map(|n| n.to_string()).unwrap_or_default();
            if code.contains("1113") {
                return true;
            }
        }
    }
    let msg = extract_error_text(body);
    [
        "余额不足",
        "无可用资源包",
        "资源包",
        "arrears",
        "insufficient balance",
        "quota exceeded",
        "欠费",
    ]
    .iter()
    .any(|k| msg.contains(k))
}

/// 与 ai::http::extract_error_text 同策略的错误信息提取。
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
    truncate(text, 200)
}

fn truncate(text: &str, max: usize) -> String {
    text.chars().take(max).collect()
}

#[cfg(test)]
mod asr_tests {
    use super::*;

    /// 🔴 回归防线：模型名曾写成 `glm-asr`（付费模型，免费 Key 无资源包），
    /// 导致内置通道用户语音功能整体失效、且每段白等 21s 重试。
    #[test]
    fn asr_model_is_the_free_tier_one() {
        assert_eq!(ASR_MODEL, "glm-asr-2512");
        assert_ne!(ASR_MODEL, "glm-asr", "glm-asr 需付费资源包，免费 Key 会返回 429/1113");
    }

    /// 1113（余额/资源包）必须立即失败；1305（限流）才值得退避重试。
    #[test]
    fn quota_429_is_not_retried_but_rate_limit_is() {
        assert!(is_unrecoverable_quota_error(
            r#"{"error":{"code":"1113","message":"余额不足或无可用资源包,请充值。"}}"#
        ));
        assert!(is_unrecoverable_quota_error(r#"{"error":{"code":1113}}"#));
        assert!(!is_unrecoverable_quota_error(
            r#"{"error":{"code":"1305","message":"该模型当前访问量过大，请您稍后再试"}}"#
        ));
        assert!(is_unrecoverable_quota_error("余额不足，请充值"));
    }

    #[test]
    fn asr_supported_for_zhipu_channels_only() {
        assert!(asr_supported("flowio-free", "https://open.bigmodel.cn/api/paas/v4/chat/completions"));
        assert!(asr_supported("zhipu", "https://open.bigmodel.cn/api/paas/v4"));
        assert!(asr_supported(
            "custom-x",
            "https://open.bigmodel.cn/api/paas/v4/chat/completions"
        ));
        assert!(!asr_supported(
            "qwen",
            "https://dashscope.aliyuncs.com/compatible-mode/v1"
        ));
        assert!(!asr_supported("deepseek", "https://api.deepseek.com/v1"));
    }

    #[test]
    fn extract_error_text_prefers_nested_message() {
        assert_eq!(
            extract_error_text(r#"{"error":{"code":"1305","message":"该模型当前访问量过大"}}"#),
            "该模型当前访问量过大"
        );
        assert_eq!(extract_error_text("plain"), "plain");
    }

    #[test]
    fn truncate_is_char_safe() {
        assert_eq!(truncate("中文abc", 2), "中文");
        assert_eq!(truncate("ab", 10), "ab");
    }
}
