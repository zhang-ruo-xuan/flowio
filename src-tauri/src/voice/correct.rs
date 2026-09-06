//! 语音转写后的 AI 纠错（方案③，2026-09-05 东北口音改进）。
//!
//! 背景：GLM-ASR 对东北口音会产生近音错字（如「绘图模式」→「会模式」）。
//! 热词 + 跨段上下文（见 `asr.rs`）能解决一部分；本模块再把**转写结果 +
//! 该步骤截图**交给视觉 LLM 做一次校对——这是竞品（scribehow / ElevenLabs
//! Scribe）做不到的：它们只看音频，我们能看到说话时屏幕上到底是什么。
//!
//! 🔴 防编造纪律（沿用 AI 精简四规则的教训）：prompt 严格限定「只修同音/
//! 近音错字」，禁止添加原文没有的内容；返回值再做长度守卫（±50%），
//! 任何失败一律保留原文——纠错宁可不改，也不能编。

use rusqlite::params;

use crate::ai::http::{call_llm, Message};
use crate::ai::types::AiConfig;

/// 一次转写最多纠错的步骤数（每步一次带图 LLM 调用）。
/// 2026-09-06：12 → 30 —— 超长教程原本超过 12 步就整段不清洗（错字直接留给用户）。
pub const CORRECTION_MAX_STEPS: usize = 30;

/// settings 表的开关键（设置 → 录制设置「语音 AI 纠错」）。
const SETTING_KEY: &str = "voice_ai_correct";

/// 纠错开关是否打开（默认开；显式写 "0" 才关闭）。
pub fn correction_enabled(conn: &rusqlite::Connection) -> bool {
    conn.query_row(
        "SELECT value FROM settings WHERE key = ?1",
        params!(SETTING_KEY),
        |r| r.get::<_, String>(0),
    )
    .map(|v| v != "0")
    .unwrap_or(true)
}

/// 纠错是否应该用内置免费通道而非用户当前配置（纯函数）。
///
/// 纠错要发**带截图的视觉请求**：用户把默认服务商设成千问/DeepSeek 等
/// 第三方（没有智谱视觉模型）时，拿他的配置发图必然失败 → 纠错对该用户
/// 静默失效。与 ASR Key 的回落策略（`resolve_asr_key`）一致：非智谱系
/// 回落内置免费通道（glm-4.6v-flash，视觉可用）；智谱系沿用用户配置
/// （默认 glm-4.6v-flash 本身就是视觉模型）。
pub fn should_use_builtin_for_correction(cfg: &AiConfig) -> bool {
    !crate::voice::asr::asr_supported(&cfg.provider.id, &cfg.provider.base_url)
}

/// 一条待纠错目标：步骤 id + 该步骤原有描述前缀（语音追加前的内容）+ 语音转写原文。
pub struct CorrectionTarget {
    pub step_id: String,
    pub desc_prefix: String,
    pub voice_text: String,
}

/// 构建纠错提示词（纯函数）。
/// ★ 2026-09-06 与插件端 voice-correct.ts 同口径升级：告知模型 ASR 常见错型
/// （「复制」误成「不复」「就是」等）、允许用截图界面文字校准操作词、
/// 短句（≤12 字）明显不符时允许整句校准；防编造纪律不变。
pub fn build_correction_prompt(voice_text: &str) -> String {
    format!(
        "你在校对一段语音识别结果。用户边操作电脑边说话，说的话被语音识别转写成了下面的文字。\
识别可能产生同音字/近音字错误，且操作词的**首字**最易错。\
实测常见错型：「复制」误成「不复」「不至」「就是」；「拖拽」误成「拖住」；\
「粘贴」误成「铁贴」；「下一个」误成「下个」。\n\n\
【原始转写】\n{}\n\n\
后面会附上这一步操作时的屏幕截图。截图上有按钮、链接、菜单等**界面文字**，\
是校对操作词最可靠的依据。\n\n\
校对规则（必须严格遵守）：\n\
1. 优先用截图上的界面文字（按钮/链接/菜单名）校准转写里的词，只修正同音字/近音字/漏字级别的差异；\n\
2. 原文 12 字以内且与截图界面明显对不上时，允许按截图上的界面文字整句校准；\n\
3. 除上述两类修正外禁止改写句式、调整语序，禁止添加原文和截图都支持不了的任何内容（位置、步骤数一律禁止编造）；\n\
4. 截图无法确认、且不确定是不是错字的地方，一律保持原样；\n\
5. 只输出修正后的文字本身，不要任何解释、引号或前缀。",
        voice_text
    )
}

/// 校验模型返回的纠错文本（纯函数）：剥引号/空白后，非空、长度在原文 ±50%
/// 以内才算通过（防止模型删改内容或输出解释）。通过返回清洗后的文本。
pub fn validate_correction(original: &str, corrected: &str) -> Option<String> {
    let mut t = corrected.trim();
    // 模型偶尔会包一层引号（成对的单/双/中文引号）
    const PAIRS: [(&str, &str); 5] = [("\"", "\""), ("'", "'"), ("“", "”"), ("‘", "’"), ("「", "」")];
    for (open, close) in PAIRS {
        if t.starts_with(open) && t.ends_with(close) && t.chars().count() > 1 {
            t = t[open.len()..t.len() - close.len()].trim();
        }
    }
    if t.is_empty() || t == original.trim() {
        return None;
    }
    let (ol, cl) = (original.chars().count(), t.chars().count());
    if cl * 2 < ol || cl > ol * 3 / 2 {
        return None;
    }
    Some(t.to_string())
}

/// 组装纠错消息（纯函数）：system 立规矩，user 文本 + 可选截图。
pub fn build_correction_messages(prompt: &str, image_b64: &str) -> Vec<Message> {
    let user_content = if image_b64.is_empty() {
        serde_json::Value::String(prompt.to_string())
    } else {
        serde_json::Value::Array(vec![
            serde_json::json!({ "type": "text", "text": prompt }),
            serde_json::json!({
                "type": "image_url",
                "image_url": { "url": format!("data:image/jpeg;base64,{}", image_b64) }
            }),
        ])
    };
    vec![
        Message {
            role: "system".into(),
            content: serde_json::Value::String(
                "你是语音识别校对助手。你只修正同音/近音错字，绝不添加、改写或删减内容。".into(),
            ),
        },
        Message {
            role: "user".into(),
            content: user_content,
        },
    ]
}

/// 读一步的截图（优先带点击标注的 annotated 图）并按 action_type 裁剪，
/// 返回 JPEG base64；无图返回空串。x/y 等用 Option 兼容 NULL 行。
fn step_image(
    conn: &rusqlite::Connection,
    step_id: &str,
) -> String {
    let row = conn
        .query_row(
            "SELECT screenshot_path, annotated_path, x, y, crop_w, crop_h, action_type \
             FROM steps WHERE id = ?1",
            params!(step_id),
            |r| {
                Ok((
                    r.get::<_, Option<String>>(0)?.unwrap_or_default(),
                    r.get::<_, Option<String>>(1)?.unwrap_or_default(),
                    r.get::<_, Option<i32>>(2)?,
                    r.get::<_, Option<i32>>(3)?,
                    r.get::<_, Option<i32>>(4)?,
                    r.get::<_, Option<i32>>(5)?,
                    r.get::<_, Option<String>>(6)?.unwrap_or_default(),
                ))
            },
        )
        .ok();
    let Some((screenshot_path, annotated_path, x, y, cw, ch, action_type)) = row else {
        return String::new();
    };
    let path = if annotated_path.is_empty() { screenshot_path.as_str() } else { annotated_path.as_str() };
    let raw = crate::ai::commands::read_file_base64(path);
    if raw.is_empty() {
        return String::new();
    }
    crate::ai::pipeline::crop_and_encode_typed(&raw, x, y, cw, ch, &action_type)
}

/// 对本轮转写出文本的步骤做 AI 纠错，直接写库。
/// 返回 (修正条数, 失败原因列表)——失败一律保留原文，不影响转写结果。
pub fn correct_voice_steps(
    conn: &rusqlite::Connection,
    config: &AiConfig,
    targets: &[CorrectionTarget],
) -> (usize, Vec<String>) {
    let mut fixed = 0usize;
    let mut errors: Vec<String> = Vec::new();
    for t in targets.iter().take(CORRECTION_MAX_STEPS) {
        let image = step_image(conn, &t.step_id);
        let messages = build_correction_messages(&build_correction_prompt(&t.voice_text), &image);
        match call_llm(config, messages) {
            Ok(raw) => match validate_correction(&t.voice_text, &raw) {
                Some(corrected) => {
                    // ★ 2026-09-06: 与写入侧一致，纠错后也不再加「【语音】」前缀
                    let new_desc = if t.desc_prefix.trim().is_empty() {
                        corrected.clone()
                    } else {
                        format!("{}\n\n{}", t.desc_prefix, corrected)
                    };
                    match conn.execute(
                        "UPDATE steps SET description = ?1 WHERE id = ?2",
                        params!(new_desc, t.step_id),
                    ) {
                        Ok(_) => {
                            fixed += 1;
                            crate::ai::commands::ai_dbg_log(&format!(
                                "[VOICE][AI-CORRECT] step {} fixed: {:?} -> {:?}",
                                t.step_id,
                                truncate_log(&t.voice_text),
                                truncate_log(&corrected)
                            ));
                        }
                        Err(e) => errors.push(format!("写库失败：{}", e)),
                    }
                }
                None => {
                    crate::ai::commands::ai_dbg_log(&format!(
                        "[VOICE][AI-CORRECT] step {} no change (identical / guard rejected: {:?})",
                        t.step_id,
                        truncate_log(&raw)
                    ));
                }
            },
            Err(e) => {
                errors.push(e);
            }
        }
    }
    (fixed, errors)
}

fn truncate_log(s: &str) -> String {
    s.chars().take(60).collect()
}

#[cfg(test)]
mod correct_tests {
    use super::*;

    #[test]
    fn correction_prompt_bans_fabrication() {
        let p = build_correction_prompt("右下角这里也是会模式");
        assert!(p.contains("禁止添加"));
        assert!(p.contains("只修正"));
        assert!(p.contains("右下角这里也是会模式"));
    }

    #[test]
    fn validate_correction_accepts_homophone_fix() {
        assert_eq!(
            validate_correction("右下角这里也是会模式", "右下角这里也是绘图模式").as_deref(),
            Some("右下角这里也是绘图模式")
        );
        // 一致 → None（没必要写库）
        assert!(validate_correction("原样", "  原样  ").is_none());
        // 空回复 → None
        assert!(validate_correction("原文", "  ").is_none());
        // 剥引号后确实有改动 → Some
        assert_eq!(
            validate_correction("打开表各", "\"打开表格\"").as_deref(),
            Some("打开表格")
        );
        // 剥引号后与原文一致 → None（无变化）
        assert!(validate_correction("打开表格", "\"打开表格\"").is_none());
    }

    /// 🔴 防编造守卫：回复比原文短太多（删内容）或长太多（加内容）都拒绝。
    #[test]
    fn validate_correction_rejects_length_outliers() {
        let long: String = "字".repeat(30);
        assert!(validate_correction("短句", &long).is_none());
        assert!(validate_correction(&long, "短").is_none());
        // 边界内允许：原文 10 字，修正 6~15 字
        let orig = "一二三四五六七八九十";
        assert!(validate_correction(orig, "一二三四五六").is_some());
        assert!(validate_correction(orig, "一二三四五六七八九十一二三四五").is_some());
    }

    #[test]
    fn correction_messages_shape_with_and_without_image() {
        let msgs = build_correction_messages("P", "");
        assert_eq!(msgs.len(), 2);
        assert!(msgs[1].content.is_string(), "无图时 user content 应是纯文本");

        let msgs = build_correction_messages("P", "IMG");
        assert_eq!(msgs.len(), 2);
        let arr = msgs[1].content.as_array().unwrap();
        assert_eq!(arr[0]["type"], "text");
        assert_eq!(arr[1]["type"], "image_url");
        assert_eq!(arr[1]["image_url"]["url"], "data:image/jpeg;base64,IMG");
    }

    /// 第三方服务商（无视觉模型）纠错必须回落内置免费通道；智谱系用用户配置。
    #[test]
    fn correction_channel_fallback_for_third_party() {
        use crate::ai::types::{AiConfig, AiProvider};
        let third = AiConfig {
            provider: AiProvider {
                id: "custom_1".into(),
                name: "qwen".into(),
                base_url: "https://dashscope.aliyuncs.com/compatible-mode/v1".into(),
                model: "qwen-max".into(),
            },
            api_key: "sk-x".into(),
        };
        assert!(should_use_builtin_for_correction(&third));
        let mine = AiConfig {
            provider: AiProvider {
                id: "zhipu".into(),
                name: "我的智谱".into(),
                base_url: "https://open.bigmodel.cn/api/paas/v4".into(),
                model: "glm-4.6v-flash".into(),
            },
            api_key: "sk-z".into(),
        };
        assert!(!should_use_builtin_for_correction(&mine));
    }
}
