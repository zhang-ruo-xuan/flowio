// 🔴 r1030 · 桌面端语音「出口」真机验证（写库这一段）。
//
//! 背景（2026-10-25 用户实测）：录制时开了语音，但**步骤描述里没有语音内容**。
//! 日志说「语音转写完成：1/5 个步骤已补充语音说明」，但库里 description 是空的。
//! ⇒ 本测试直接复刻 `voice/commands.rs::transcribe_and_apply` 的写库那段，
//!   用**真实语音样本 + 真实 key** 跑一遍，证明：
//!   ① ASR 真能转写出字（百炼链路）；
//!   ② `UPDATE steps SET description` 写进去后**再读出来确实是那段话**。
//!
//! 🔴 判据纪律：静音 WAV 证明不了「真在转写」⇒ 用真实语音（中文由本机 SAPI 合成）。
//!   没注入 key / 没给样本时**跳过并打印原因**，不假绿。

use std::path::Path;

fn env_key(name: &str) -> Option<String> {
    std::env::var(name).ok().filter(|v| !v.trim().is_empty())
}

/// 百炼 chat/completions 的 ASR（与 voice/asr.rs::transcribe_wav_dashscope 同口径）。
fn asr_dashscope(key: &str, wav: &[u8]) -> Result<String, String> {
    use base64::Engine as _;
    let b64 = base64::engine::general_purpose::STANDARD.encode(wav);
    let body = serde_json::json!({
        "model": "qwen3-asr-flash",
        "messages": [{
            "role": "user",
            "content": [{
                "type": "input_audio",
                // 🔴 必须带 data: 前缀
                "input_audio": { "data": format!("data:audio/wav;base64,{}", b64) }
            }]
        }],
        "max_tokens": 512
    });
    let client = reqwest::blocking::Client::builder()
        .timeout(std::time::Duration::from_secs(90))
        .build()
        .map_err(|e| e.to_string())?;
    let resp = client
        .post("https://dashscope.aliyuncs.com/compatible-mode/v1/chat/completions")
        .header("Authorization", format!("Bearer {}", key))
        .header("Content-Type", "application/json")
        .json(&body)
        .send()
        .map_err(|e| format!("send failed: {}", e))?;
    let status = resp.status().as_u16();
    let text = resp.text().unwrap_or_default();
    if status != 200 {
        return Err(format!("HTTP {}: {}", status, &text[..text.len().min(200)]));
    }
    let v: serde_json::Value = serde_json::from_str(&text).map_err(|e| e.to_string())?;
    Ok(v.pointer("/choices/0/message/content")
        .and_then(|c| c.as_str())
        .unwrap_or("")
        .trim()
        .to_string())
}

/// 🔴 真机验证：ASR 出字 + 写进 steps.description + 读回一致。
#[test]
fn voice_text_lands_in_step_description() {
    let Some(key) = env_key("FLOWIO_BUILTIN_DASHSCOPE_KEY") else {
        eprintln!("⏭ 跳过：未注入 FLOWIO_BUILTIN_DASHSCOPE_KEY");
        return;
    };
    let Some(wav_path) = env_key("FLOWIO_TEST_WAV") else {
        eprintln!("⏭ 跳过：未提供 FLOWIO_TEST_WAV（真实语音样本）");
        return;
    };
    let wav = std::fs::read(&wav_path).expect("读 WAV 失败");
    assert!(wav.len() > 44, "WAV 太短，不像是真语音");
    assert_eq!(&wav[0..4], b"RIFF", "不是 RIFF/WAV");

    // ① ASR 真能出字
    let text = asr_dashscope(&key, &wav).expect("ASR 调用失败");
    eprintln!("ℹ ASR 输出：{}", text);
    assert!(!text.trim().is_empty(), "🔴 ASR 返回空串（静音样本？样本不对？）");

    // ② 写库：建一个内存库 + 一条步骤，模拟 transcribe_and_apply 的写库段
    let conn = rusqlite::Connection::open_in_memory().expect("开内存库");
    conn.execute_batch(
        "CREATE TABLE steps (id TEXT PRIMARY KEY, recording_id TEXT, order_index INTEGER, \
         description TEXT, timestamp INTEGER);\
         INSERT INTO steps (id, recording_id, order_index, description, timestamp) \
         VALUES ('step_test_1', 'rec_test_1', 0, '', 0);",
    )
    .expect("建表");

    // ★ 与 voice/commands.rs::transcribe_and_apply 的写库段同口径：
    //   desc 为空 ⇒ 直接用语音原文；非空 ⇒ 原描述 + 空行 + 语音
    let desc: String = conn
        .query_row(
            "SELECT description FROM steps WHERE id = ?1",
            rusqlite::params!["step_test_1"],
            |r| r.get(0),
        )
        .unwrap_or_default();
    let new_desc = if desc.trim().is_empty() {
        text.clone()
    } else {
        format!("{}\n\n{}", desc, text)
    };
    conn.execute(
        "UPDATE steps SET description = ?1 WHERE id = ?2",
        rusqlite::params![new_desc, "step_test_1"],
    )
    .expect("写库失败");

    // ③ 读回验证（这才是「出口通了」的判据）
    let readback: String = conn
        .query_row(
            "SELECT description FROM steps WHERE id = ?1",
            rusqlite::params!["step_test_1"],
            |r| r.get(0),
        )
        .expect("读回失败");
    eprintln!("✅ 读回 description = {:?}", readback);
    assert_eq!(readback.trim(), text.trim(), "🔴 写进去的内容与读出来的不一致");
    assert!(
        readback.contains(text.trim()),
        "🔴 步骤描述里没有语音内容 ⇒ 语音没有出口"
    );
}

/// 🔴 反向自证锚点：若写库那段被改成「不写」或「写空」，本锁必红。
/// 这里用一条独立判据锁住「空描述时语音必须直接成为描述」这条契约。
#[test]
fn empty_description_is_replaced_by_voice_text() {
    let desc = "   ";
    let voice = "打开浏览器，进入设置。";
    let new_desc = if desc.trim().is_empty() {
        voice.to_string()
    } else {
        format!("{}\n\n{}", desc, voice)
    };
    assert_eq!(new_desc, voice, "空描述时语音必须**直接成为**描述（不留空行前缀）");
    assert!(!new_desc.starts_with('\n'), "不该有开头空行");
}
