//! 教程翻译（PRD F2，T1–T9）。
//!
//! 一次性把「教程标题 + 全部步骤标题/描述」打包发给 LLM，要求输出严格
//! JSON；结果作为**译文副本**存进 `recordings.translations`（按语言键），
//! 原文永不修改（T2）。截图/标注/UI 一律不翻（T1）。
//!
//! 存储格式：
//! ```json
//! { "en": { "title": "...", "steps": { "0": { "title": "...", "description": "..." } } } }
//! ```
//! `steps` 以 order_index（字符串键）映射，缺失的步骤回退原文（T6 安全网）。

use serde::{Deserialize, Serialize};
use serde_json::Value;
use tauri::{AppHandle, Emitter, State};

use super::http::{call_llm_with_tokens, Message};
use crate::DbState;

/// 首期支持的目标语言（lang 代码 → 展示名）。原文即中文时仍允许翻译，
/// 由用户自行决定。
pub const TARGET_LANGS: &[(&str, &str)] = &[
    ("zh", "中文"),
    ("en", "English"),
    ("ja", "日本語"),
    ("ko", "한국어"),
    ("es", "Español"),
];

/// `translate://done` 事件载荷。
///
/// 🔴 必须 camelCase：前端 `StepEditor.tsx` 用 `ev.payload.recordingId` 读。
/// 蛇形的 `recording_id` 会让前端匹配失败 → `setTranslating(false)` 永远不执行，
/// UI 永远卡在「翻译中...」（后端其实早已完成——2026-09-04 实测踩坑）。
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct TranslateDoneEvent {
    recording_id: String,
    lang: String,
    ok: bool,
    summary: String,
}

/// 单个步骤的译文（LLM 输出行）。
#[derive(Debug, Deserialize)]
struct TranslatedStep {
    i: i64,
    #[serde(default)]
    t: String,
    #[serde(default)]
    d: String,
}

/// 构造翻译请求的 user 消息内容。
///
/// 纯函数（便于单测）：输入语言名 + (order_index, title, description) 列表，
/// 输出完整 prompt 文本。
pub fn build_translate_prompt(
    lang_label: &str,
    steps: &[(i64, String, String)],
    recording_title: &str,
) -> String {
    let items: Vec<Value> = steps
        .iter()
        .map(|(i, t, d)| {
            serde_json::json!({ "i": i, "t": t, "d": d })
        })
        .collect();
    format!(
        "请把下面这份操作教程从原文翻译成{}。\n\
         要求：\n\
         1. 只翻译文本含义，保持操作指令清晰、口语自然，符合{}用户的使用习惯；\n\
         2. 步骤描述里的语音转写内容（与普通说明混在一起）同样需要翻译；\n\
         3. 不要增删步骤，不要合并或改写内容；\n\
         4. 严格输出 JSON 对象，不要输出任何解释、markdown 代码块或其他文字。\n\
         输出格式：{{\"title\":\"教程标题译文\",\"steps\":[{{\"i\":步骤序号,\"t\":\"步骤标题译文\",\"d\":\"步骤描述译文\"}},...]}}，i 必须与输入一致，steps 数量必须与输入一致。\n\
         \n\
         待翻译步骤列表（教程标题：{}）：\n{}",
        lang_label,
        lang_label,
        recording_title,
        serde_json::to_string(&items).unwrap_or_else(|_| "[]".to_string())
    )
}

/// LLM 翻译输出整体（对象格式）。
#[derive(Debug, Deserialize)]
struct TranslatedGuide {
    #[serde(default)]
    title: String,
    #[serde(default)]
    steps: Vec<TranslatedStep>,
}

/// 从 LLM 回复中提取译文 JSON（容忍 ```json 围栏 / 前后杂文）。
pub fn parse_translate_response(text: &str) -> Result<TranslatedGuide, String> {
    let trimmed = text.trim();
    let mut candidates: Vec<&str> = Vec::new();
    if trimmed.starts_with('{') {
        candidates.push(trimmed);
    }
    if let (Some(s), Some(e)) = (trimmed.find('{'), trimmed.rfind('}')) {
        if e > s {
            candidates.push(&trimmed[s..=e]);
        }
    }
    for cand in candidates {
        if let Ok(v) = serde_json::from_str::<TranslatedGuide>(cand) {
            return Ok(v);
        }
    }
    Err(format!("译文 JSON 解析失败（原文前 120 字：{}）", {
        let t: String = text.chars().take(120).collect();
        t
    }))
}

/// 读取指定录制现有译文 JSON（无 → 空 json!({})）。
///
/// `pub(crate)`：脱敏命令需要读它来作废「原文被改动过」的步骤译文（隐私一致性）。
pub(crate) fn load_translations(conn: &rusqlite::Connection, recording_id: &str) -> Result<Value, String> {
    match crate::db::get_recording_translations_raw(conn, recording_id)? {
        Some(s) => serde_json::from_str(&s)
            .map_err(|e| format!("现有译文 JSON 损坏：{}（将新建覆盖）", e)),
        None => Ok(serde_json::json!({})),
    }
}

/// 翻译整本教程（后台线程执行），完成后 emit `translate://done`。
///
/// T3（重复翻译覆盖确认）由前端在调用前完成；后端无条件覆盖同语言副本。
#[tauri::command]
pub fn translate_recording(
    db: State<'_, DbState>,
    app: AppHandle,
    recording_id: String,
    lang: String,
) -> Result<(), String> {
    // 语言代码白名单校验
    if !TARGET_LANGS.iter().any(|(code, _)| *code == lang) {
        return Err(format!("不支持的目标语言：{}", lang));
    }

    // ── 主线程读数据：标题 + 步骤 + 现有译文 + AI 配置 ──────────
    let (recording_title, steps, existing) = {
        let conn = db.0.lock().map_err(|e| e.to_string())?;
        let title: String = conn
            .query_row(
                "SELECT title FROM recordings WHERE id = ?1",
                rusqlite::params![recording_id],
                |row| row.get(0),
            )
            .map_err(|e| format!("recording not found: {}", e))?;
        let mut stmt = conn
            .prepare(
                "SELECT order_index, title, description FROM steps \
                 WHERE recording_id = ?1 ORDER BY order_index",
            )
            .map_err(|e| e.to_string())?;
        let steps: Vec<(i64, String, String)> = stmt
            .query_map(rusqlite::params![recording_id], |row| {
                Ok((row.get(0)?, row.get(1)?, row.get(2)?))
            })
            .map_err(|e| e.to_string())?
            .filter_map(|r| r.ok())
            .collect();
        let existing = load_translations(&conn, &recording_id)?;

        // 首次翻译前快照原文（key = "__orig"）：此后编辑器文字会被译文覆盖，
        // 原文只剩这份快照，供「原文」选项一键恢复。仅在译文副本完全为空时
        // 快照（即当前步骤文字尚未被任何译文覆盖）；已有副本的存量教程不补拍。
        let mut existing = existing;
        if existing.as_object().map(|o| o.is_empty()).unwrap_or(true) {
            let mut steps_map = serde_json::Map::new();
            for (idx, t, d) in &steps {
                steps_map.insert(
                    idx.to_string(),
                    serde_json::json!({ "title": t, "description": d }),
                );
            }
            existing["__orig"] = serde_json::json!({
                "title": title,
                "steps": Value::Object(steps_map),
            });
            let snap_json = serde_json::to_string(&existing)
                .map_err(|e| format!("原文快照序列化失败：{}", e))?;
            crate::db::set_recording_translations_raw(&conn, &recording_id, &snap_json)?;
        }

        (title, steps, existing)
    };

    if steps.is_empty() {
        return Err("本次录制没有步骤，没有可翻译的内容".to_string());
    }

    let config = {
        let conn = db.0.lock().map_err(|e| e.to_string())?;
        super::commands::resolve_ai_config(&conn)
    };

    let lang_label = TARGET_LANGS
        .iter()
        .find(|(code, _)| *code == lang)
        .map(|(_, label)| label.to_string())
        .unwrap_or_else(|| lang.clone());

    let app_handle = app.clone();
    let rid = recording_id.clone();
    // recording_title 会被 move 进线程，先留一份给线程外的全局完成提醒
    let title_for_notify = recording_title.clone();
    std::thread::spawn(move || {
        let result = run_translation(rid.clone(), lang.clone(), lang_label, recording_title, steps, existing, config);
        match &result {
            Ok(summary) => crate::ai::commands::ai_dbg_log(&format!(
                "[TRANSLATE] done for {} ({}): {}",
                rid, lang, summary
            )),
            Err(msg) => crate::ai::commands::ai_dbg_log(&format!(
                "[TRANSLATE] failed for {} ({}): {}",
                rid, lang, msg
            )),
        }
        let (ok, summary) = match result {
            Ok(s) => (true, s),
            Err(m) => (false, m),
        };
        let _ = app_handle.emit(
            "translate://done",
            TranslateDoneEvent {
                recording_id: rid.clone(),
                lang: lang.clone(),
                ok,
                summary: summary.clone(),
            },
        );
        // 全局完成提醒（任意页面弹窗 + Windows 系统通知，带教程名）
        crate::job_notify::emit_job_done(
            &app_handle,
            &rid,
            &title_for_notify,
            crate::job_notify::JobOp::Translate,
            ok,
            &summary,
            None,
        );
    });

    Ok(())
}

/// 翻译主体（后台线程内执行；独立 DB 连接写译文副本）。
#[allow(clippy::too_many_arguments)]
fn run_translation(
    recording_id: String,
    lang: String,
    lang_label: String,
    recording_title: String,
    steps: Vec<(i64, String, String)>,
    mut existing: Value,
    config: super::types::AiConfig,
) -> Result<String, String> {
    let prompt = build_translate_prompt(&lang_label, &steps, &recording_title);
    let response = call_llm_with_tokens(
        &config,
        vec![
            Message {
                role: "system".into(),
                content: "你是专业的软件操作教程翻译引擎。只输出严格的 JSON 数组，不要任何多余文字。".into(),
            },
            Message {
                role: "user".into(),
                content: serde_json::Value::String(prompt),
            },
        ],
        8192,
    )?;

    let TranslatedGuide { title: translated_title, steps: translated } = parse_translate_response(&response)?;
    if translated.is_empty() {
        return Err("模型返回了空译文，请重试".to_string());
    }

    // 组装 steps 映射（order_index 字符串键）
    let mut steps_map = serde_json::Map::new();
    for t in translated {
        steps_map.insert(
            t.i.to_string(),
            serde_json::json!({ "title": t.t, "description": t.d }),
        );
    }
    if let Some(obj) = existing.as_object_mut() {
        obj.insert(
            lang.clone(),
            serde_json::json!({
                "title": if translated_title.trim().is_empty() { recording_title } else { translated_title },
                "steps": Value::Object(steps_map),
            }),
        );
    }

    let json = serde_json::to_string(&existing).map_err(|e| format!("译文序列化失败：{}", e))?;
    let conn = crate::db::init_db_file();
    crate::db::set_recording_translations_raw(&conn, &recording_id, &json)?;

    Ok(format!("已生成 {} 译文副本（{} 个步骤）", lang_label, steps.len()))
}

/// 读取某条录制的全部译文副本（前端编辑器预览/导出语言切换用）。
#[tauri::command]
pub fn get_recording_translations(
    db: State<'_, DbState>,
    recording_id: String,
) -> Result<Option<Value>, String> {
    let conn = db.0.lock().map_err(|e| e.to_string())?;
    match crate::db::get_recording_translations_raw(&conn, &recording_id)? {
        Some(s) => serde_json::from_str(&s)
            .map(Some)
            .map_err(|e| format!("译文 JSON 损坏：{}", e)),
        None => Ok(None),
    }
}

/// ★ T8 译文可编辑：保存用户在编辑器中修改后的译文副本（整体覆盖）。
/// translations 必须是 JSON 对象（{ lang: { title, steps } }），结构校验从宽——
/// 与 AI 生成共用同一存储，缺失字段由预览/导出端回退原文兜底。
#[tauri::command]
pub fn save_recording_translations(
    db: State<'_, DbState>,
    recording_id: String,
    translations: Value,
) -> Result<(), String> {
    if !translations.is_object() {
        return Err("译文必须是 JSON 对象".into());
    }
    let json = serde_json::to_string(&translations).map_err(|e| format!("译文序列化失败：{}", e))?;
    let conn = db.0.lock().map_err(|e| e.to_string())?;
    crate::db::set_recording_translations_raw(&conn, &recording_id, &json)
}

#[cfg(test)]
mod translate_tests {
    use super::*;

    /// 事件载荷必须 camelCase：前端按 `recordingId` 匹配，蛇形会让
    /// 「翻译中...」永远不复位（后端实际早已完成）。
    #[test]
    fn translate_done_event_is_camel_case() {
        let ev = TranslateDoneEvent {
            recording_id: "rec-1".into(),
            lang: "en".into(),
            ok: true,
            summary: "ok".into(),
        };
        let v = serde_json::to_value(&ev).unwrap();
        assert!(v.get("recordingId").is_some(), "必须输出 recordingId，实际：{}", v);
        assert!(v.get("recording_id").is_none());
    }

    #[test]
    fn prompt_contains_lang_and_steps_and_voice_rule() {
        let p = build_translate_prompt(
            "English",
            &[(0, "打开设置".into(), "点击右上角齿轮\n\n【语音】这里是为了进入设置".into())],
            "测试教程",
        );
        assert!(p.contains("English"));
        assert!(p.contains("测试教程"));
        assert!(p.contains("【语音】"));
        assert!(p.contains("\"i\":0"));
        assert!(p.contains("严格输出 JSON 对象"));
    }

    #[test]
    fn parse_plain_json_object() {
        let out = parse_translate_response(
            r#"{"title":"My Guide","steps":[{"i":0,"t":"Open settings","d":"Click the gear"}]}"#,
        )
        .expect("should parse");
        assert_eq!(out.title, "My Guide");
        assert_eq!(out.steps.len(), 1);
        assert_eq!(out.steps[0].i, 0);
        assert_eq!(out.steps[0].t, "Open settings");
        assert_eq!(out.steps[0].d, "Click the gear");
    }

    #[test]
    fn parse_with_code_fence_and_prose() {
        let text = "好的，以下是译文：\n```json\n{\"title\":\"保存\",\"steps\":[{\"i\":2,\"t\":\"保存\",\"d\":\"Ctrl+S\"}]}\n```\n完毕";
        let out = parse_translate_response(text).expect("should parse fenced");
        assert_eq!(out.title, "保存");
        assert_eq!(out.steps[0].i, 2);
    }

    #[test]
    fn parse_missing_fields_default_empty() {
        let out = parse_translate_response(r#"{"steps":[{"i":5}]}"#).expect("t/d optional");
        assert_eq!(out.title, "");
        assert_eq!(out.steps[0].t, "");
        assert_eq!(out.steps[0].d, "");
    }

    #[test]
    fn parse_garbage_is_err() {
        assert!(parse_translate_response("完全不是 JSON").is_err());
        assert!(parse_translate_response("").is_err());
    }
}
