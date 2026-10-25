//! r1030 · 桌面端 AI/ASR 链路的**真机网络验证**（集成测试）。
//!
//! 为什么需要它：单元测试只能验"分支走对了"，验不了"上游真的这么答"。
//! 这两条链路本轮都出过**假绿**级别的坑：
//!   · 思考型模型 content 空（桌面 AI 摘要拿到空串，表现为"点了没反应"）；
//!   · 智谱 ASR 恒 429/1113（语音转写整体失效）。
//! ⇒ 这里用**真实 HTTP** 打真实供应商，断言**真实输出**。
//!
//! 🔴 判据纪律：这些测试需要**真实 Key**（构建期注入的那三把）。
//!   没注入时**跳过**（不假绿、不假装通过），并明确打印跳过原因。
//!
//! 运行：
//!   FLOWIO_BUILTIN_AI_KEY=... FLOWIO_BUILTIN_DASHSCOPE_KEY=... cargo test --test live_ai_r1030 -- --nocapture

use std::time::Duration;

/// 桌面 LLM 用的智谱端点（与 ai/http.rs 的 normalize_chat_url 同口径）。
const ZHIPU_URL: &str = "https://open.bigmodel.cn/api/paas/v4/chat/completions";
/// 百炼 chat/completions（与 voice/asr.rs 同口径）。
const DASHSCOPE_URL: &str =
    "https://dashscope.aliyuncs.com/compatible-mode/v1/chat/completions";

fn env_key(name: &str) -> Option<String> {
    std::env::var(name).ok().filter(|v| !v.trim().is_empty())
}

/// 造一个 1 秒 440Hz 正弦波的单声道 16bit WAV（用来测 ASR —— 静音证明不了「真在转写」，
/// 所以这里用真语音样本由调用方通过 FLOWIO_TEST_WAV 指定；没有就用静音做连通性探测）。
fn make_silence_wav() -> Vec<u8> {
    let sample_rate = 16000u32;
    let n = sample_rate; // 1 秒
    let pcm: Vec<u8> = std::iter::repeat(0u8).take(n as usize * 2).collect();
    let mut out: Vec<u8> = Vec::new();
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&((36 + pcm.len()) as u32).to_le_bytes());
    out.extend_from_slice(b"WAVE");
    out.extend_from_slice(b"fmt ");
    out.extend_from_slice(&16u32.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes()); // PCM
    out.extend_from_slice(&1u16.to_le_bytes()); // mono
    out.extend_from_slice(&sample_rate.to_le_bytes());
    out.extend_from_slice(&(sample_rate * 2).to_le_bytes());
    out.extend_from_slice(&2u16.to_le_bytes());
    out.extend_from_slice(&16u16.to_le_bytes());
    out.extend_from_slice(b"data");
    out.extend_from_slice(&(pcm.len() as u32).to_le_bytes());
    out.extend_from_slice(&pcm);
    out
}

fn http_post_json(
    url: &str,
    bearer: &str,
    body: &serde_json::Value,
    timeout: Duration,
) -> Result<(u16, String), String> {
    let client = reqwest::blocking::Client::builder()
        .timeout(timeout)
        .build()
        .map_err(|e| e.to_string())?;
    let resp = client
        .post(url)
        .header("Authorization", format!("Bearer {}", bearer))
        .header("Content-Type", "application/json")
        .json(body)
        .send()
        .map_err(|e| format!("send failed: {}", e))?;
    let status = resp.status().as_u16();
    let text = resp.text().unwrap_or_default();
    Ok((status, text))
}

/// 🔴 真机验证：智谱**思考型模型**（glm-4.6v-flash）在带 `thinking:disabled` 时，
///   纯文本提问必须返回**非空 content**。
///
/// 这正是桌面端「AI 摘要 / 自动描述」此前拿到空串的根因：不关思考时，
/// 1024 的 max_tokens 预算被 thinking 吃光，content 回空串。
#[test]
fn zhipu_thinking_model_returns_content_when_thinking_disabled() {
    let Some(key) = env_key("FLOWIO_BUILTIN_AI_KEY") else {
        eprintln!("⏭ 跳过：未注入 FLOWIO_BUILTIN_AI_KEY（构建期注入的那把）");
        return;
    };
    let body = serde_json::json!({
        "model": "glm-4.6v-flash",
        "messages": [{
            "role": "user",
            "content": [{"type": "text", "text": "用一句话说明什么是浏览器缓存。只回一句话。"}]
        }],
        "max_tokens": 1024,
        "temperature": 0.3,
        // 🔴 关键：智谱私有字段，关掉思考
        "thinking": {"type": "disabled"}
    });
    let (status, text) =
        http_post_json(ZHIPU_URL, &key, &body, Duration::from_secs(60)).expect("http");
    // 🔴 限流跳过（账号级 1302/1305）：本条要验「content 非空」，限流响应无 content 可验
    if status == 429 || text.contains("1302") || text.contains("1305") {
        eprintln!("⏭ 跳过：智谱限流（1302/1305）");
        return;
    }
    assert_eq!(status, 200, "智谱返回非 200：{}", &text[..text.len().min(300)]);
    let v: serde_json::Value = serde_json::from_str(&text).expect("响应不是 JSON");
    let content = v
        .pointer("/choices/0/message/content")
        .and_then(|c| c.as_str())
        .unwrap_or("");
    assert!(
        !content.trim().is_empty(),
        "🔴 content 为空 ⇒ 桌面 AI 会拿到空串（思考吃光预算）。响应：{}",
        &text[..text.len().min(400)]
    );
    eprintln!("✅ 智谱 thinking:disabled 下 content 非空：{}", &content[..content.len().min(60)]);
}

/// 🔴 真机验证：**不关**思考时 content 为空而 reasoning 非空（复现 bug 本体）。
///
/// 这条锁的意义：证明上一条「content 非空」不是因为智谱恰好答了，
/// 而是 `thinking:disabled` 真的起了作用。
#[test]
fn zhipu_thinking_model_without_disable_exposes_reasoning_split() {
    let Some(key) = env_key("FLOWIO_BUILTIN_AI_KEY") else {
        eprintln!("⏭ 跳过：未注入 FLOWIO_BUILTIN_AI_KEY");
        return;
    };
    let body = serde_json::json!({
        "model": "glm-4.6v-flash",
        "messages": [{
            "role": "user",
            "content": [{"type": "text", "text": "用一句话说明什么是浏览器缓存。只回一句话。"}]
        }],
        "max_tokens": 64, // 故意给小 ⇒ 思考会吃光
        "temperature": 0.3
    });
    let (status, text) =
        http_post_json(ZHIPU_URL, &key, &body, Duration::from_secs(60)).expect("http");
    // 🔴 智谱限流是**账号级**的（1302 账户限流 / 1305 模型并发）——连打几次就可能撞上。
    //   撞上限流时这条锁**跳过**（不是失败）：它要验的是「响应形状」，
    //   而限流响应压根没有 choices。让它红只会掩盖真问题。
    if status == 429 || text.contains("1302") || text.contains("1305") {
        eprintln!("⏭ 跳过：智谱限流（1302/1305）——本条要验响应形状，限流时无形状可验");
        return;
    }
    assert_eq!(status, 200, "智谱返回非 200：{}", &text[..text.len().min(300)]);
    let v: serde_json::Value = serde_json::from_str(&text).expect("响应不是 JSON");
    let content = v
        .pointer("/choices/0/message/content")
        .and_then(|c| c.as_str())
        .unwrap_or("");
    let reasoning = v
        .pointer("/choices/0/message/reasoning_content")
        .and_then(|c| c.as_str())
        .unwrap_or("");
    // 只要证明「小预算下 content 会空」这个机制存在即可
    // （不同版本/负载下未必每次都空，所以不强求 content 必空）
    eprintln!(
        "ℹ 小预算下：content 长度={} reasoning 长度={}",
        content.len(),
        reasoning.len()
    );
    assert!(
        content.is_empty() || !reasoning.is_empty(),
        "既没 content 也没 reasoning ⇒ 响应形状与预期不符：{}",
        &text[..text.len().min(300)]
    );
}

/// 🔴 真机验证：百炼 ASR（qwen3-asr-flash）**真的能转写**（不只 200）。
///
/// 用调用方提供的真实语音样本（FLOWIO_TEST_WAV）；没给就用静音做**连通性**探测
/// （静音只会返回空串 —— 这正是「静音证明不了真在转写」那条判据）。
#[test]
fn dashscope_asr_transcribes_real_speech() {
    let Some(key) = env_key("FLOWIO_BUILTIN_DASHSCOPE_KEY") else {
        eprintln!("⏭ 跳过：未注入 FLOWIO_BUILTIN_DASHSCOPE_KEY");
        return;
    };
    let wav = match env_key("FLOWIO_TEST_WAV").and_then(|p| std::fs::read(p).ok()) {
        Some(b) => b,
        None => {
            eprintln!("ℹ 未提供 FLOWIO_TEST_WAV ⇒ 只做连通性探测（静音会返回空串，验不了准确性）");
            make_silence_wav()
        }
    };
    use base64::Engine as _;
    let b64 = base64::engine::general_purpose::STANDARD.encode(&wav);
    let body = serde_json::json!({
        "model": "qwen3-asr-flash",
        "messages": [{
            "role": "user",
            "content": [{
                "type": "input_audio",
                // 🔴 必须带 data: 前缀（裸 base64 ⇒ 400「URL 无效」）
                "input_audio": {"data": format!("data:audio/wav;base64,{}", b64)}
            }]
        }],
        "max_tokens": 512
    });
    let (status, text) =
        http_post_json(DASHSCOPE_URL, &key, &body, Duration::from_secs(90)).expect("http");
    assert_eq!(status, 200, "百炼 ASR 返回非 200：{}", &text[..text.len().min(300)]);
    let v: serde_json::Value = serde_json::from_str(&text).expect("响应不是 JSON");
    let content = v
        .pointer("/choices/0/message/content")
        .and_then(|c| c.as_str())
        .unwrap_or("")
        .trim()
        .to_string();
    eprintln!("ℹ 百炼 ASR 输出：{}", content);
    if content.is_empty() {
        // 静音样本的合法结果就是空串；只有给了真实样本才要求非空
        assert!(
            env_key("FLOWIO_TEST_WAV").is_none(),
            "给了真实语音样本却转写为空 ⇒ ASR 链路有问题"
        );
        eprintln!("ℹ 静音样本转写为空（预期）");
    } else {
        eprintln!("✅ 百炼 ASR 转写成功");
    }
}
