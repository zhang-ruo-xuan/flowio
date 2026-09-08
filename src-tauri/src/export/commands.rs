use base64::Engine;
use ab_glyph::Font;
use docx_rs::*;
use rusqlite::params;
use serde::{Deserialize, Serialize};
use std::sync::Mutex;
use std::time::{Duration, Instant};
use tauri::{Emitter, State};

use crate::redaction::{redact_screenshot_copy, redact_text_with_config, RedactMode};
use crate::DbState;

// ── Font cache for long-screenshot text rendering ────────────
fn load_font() -> Option<ab_glyph::FontArc> {
    let candidates = [
        "C:\\Windows\\Fonts\\msyh.ttc",
        "C:\\Windows\\Fonts\\msyh.ttf",
        "C:\\Windows\\Fonts\\simhei.ttf",
        "C:\\Windows\\Fonts\\arial.ttf",
    ];
    for path in &candidates {
        if let Ok(data) = std::fs::read(path) {
            if data.len() > 4 {
                // FontArc 需要静态生命周期数据，leak 一次（进程级单例，可接受）
                let static_data: &'static [u8] = Box::leak(data.into_boxed_slice());
                if let Ok(font) = ab_glyph::FontArc::try_from_slice(static_data) {
                    return Some(font);
                }
            }
        }
    }
    None
}

/// Global font singleton (lazy, once per process)
fn get_font() -> Option<ab_glyph::FontArc> {
    use std::sync::OnceLock;
    static FONT: OnceLock<Option<ab_glyph::FontArc>> = OnceLock::new();
    FONT.get_or_init(load_font).clone()
}

// ── Export result types ───────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ExportRecording {
    id: String,
    title: String,
    app_name: String,
    total_steps: i32,
    created_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ExportStep {
    id: String,
    order_index: i32,
    step_number: i32,
    action_type: String,
    title: String,
    description: String,
    tip: String,
    screenshot_path: String,
    // 裁剪坐标（前端裁图模式需要）
    x: Option<i32>,
    y: Option<i32>,
    crop_w: Option<i32>,
    crop_h: Option<i32>,
    // 变量标记（SKILL.md 自动化导出用）
    #[serde(default)]
    is_variable: bool,
    #[serde(default)]
    param_name: String,
}

// ── Tauri Command ─────────────────────────────────────────────

/// Export a recording to the specified format.
///
/// Supported formats: `"html"`, `"markdown"`, `"pdf"`, `"docx"` (or `"word"`), `"txt"`, `"long"`, `"skill"`.
/// `"skill"` = Agent Skills 1.0（SKILL.md）：有变量标记步骤时自动出变量版，否则固定版；
/// 截图捆绑到同目录 screenshots/ 并在步骤下引用。
/// If `output_path` is provided, write to that exact path; otherwise auto-generate
/// a path under the `output/` directory.
/// Returns the absolute path to the generated file.
#[tauri::command]
pub async fn export_recording(
    db: State<'_, DbState>,
    app: tauri::AppHandle,
    id: String,
    format: String,
    output_path: Option<String>,
    crop_step_ids: Option<Vec<String>>,
    lang: Option<String>,
) -> Result<String, String> {
    // 读取教程标题，供完成提醒展示（失败不致命，降级为空串）
    let title = {
        let conn = db.0.lock().map_err(|e| e.to_string())?;
        conn.query_row(
            "SELECT title FROM recordings WHERE id = ?1",
            rusqlite::params![id],
            |row| row.get::<_, String>(0),
        )
        .unwrap_or_default()
    };

    // async 命令运行在异步运行时线程，不阻塞 UI 主线程。内部 catch_unwind 防 panic 崩掉 IPC。
    // 进度回调用 app 的克隆，保留原 app 句柄供完成后发全局提醒。
    let app_for_progress = app.clone();
    let result: Result<String, String> = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let progress = move |done: usize, total: usize| {
            let _ = app_for_progress.emit(
                "export://progress",
                serde_json::json!({ "done": done, "total": total }),
            );
        };
        export_recording_inner(
            &db,
            &id,
            &format,
            output_path,
            crop_step_ids.as_deref(),
            Some(&progress),
            lang.as_deref(),
        )
    }))
    .map_err(|_| "export_recording panicked".to_string())?;

    // 全局完成提醒（任意页面弹窗 + Windows 系统通知，带教程名）
    match &result {
        Ok(path) => crate::job_notify::emit_job_done(
            &app,
            &id,
            &title,
            crate::job_notify::JobOp::Export,
            true,
            &format!("已导出为 {}", path),
        ),
        Err(e) => crate::job_notify::emit_job_done(
            &app,
            &id,
            &title,
            crate::job_notify::JobOp::Export,
            false,
            e,
        ),
    }
    result
}

/// 实际导出逻辑（在独立线程内执行，避免长任务占用 IPC / 阻塞 UI）。
/// 导出前强制自动脱敏：文本替换 + 截图生成脱敏副本，原图与数据库数据保持不动。
///
/// `lang`：非 None 时把标题/步骤文本替换为对应语言的译文副本
/// （PRD T6；缺失的步骤回退原文，截图永不翻译）。
#[allow(clippy::too_many_arguments)]
pub(crate) fn export_recording_inner(
    db: &State<DbState>,
    id: &str,
    format: &str,
    output_path: Option<String>,
    crop_step_ids: Option<&[String]>,
    progress: Option<&(dyn Fn(usize, usize) + Send + Sync)>,
    lang: Option<&str>,
) -> Result<String, String> {
    let (mut recording, mut steps) = load_export_data(db, id)?;
    if let Some(lang) = lang {
        apply_translation_override(db, id, lang, &mut recording, &mut steps)?;
    }
    // 构建 crop ID 集合用于 O(1) 查找
    let crop_set: std::collections::HashSet<&str> = crop_step_ids
        .map(|ids| ids.iter().map(|s| s.as_str()).collect())
        .unwrap_or_default();
    // Phase C: 导出前自动脱敏（开关键 redact_before_export 默认关闭）——
    // 仅当用户在「设置 → 脱敏」中开启「导出前自动脱敏」时才执行；
    // 文本替换 + 截图生成脱敏副本，原图与数据库数据保持不动。
    let steps = prepare_export_steps(db, steps, progress)?;

    // Determine the output path: use user-provided path if given, otherwise auto-generate
    let path = if let Some(ref user_path) = output_path {
        // Ensure parent directories exist
        let p = std::path::PathBuf::from(user_path);
        if let Some(parent) = p.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| format!("Failed to create output directory: {}", e))?;
        }
        p
    } else {
        let output_dir = std::path::PathBuf::from("output");
        std::fs::create_dir_all(&output_dir)
            .map_err(|e| format!("Failed to create output directory: {}", e))?;
        let timestamp = chrono::Utc::now().format("%Y%m%d_%H%M%S").to_string();
        generate_export_path(&output_dir, &recording.title, &format, &timestamp)
    };

    match format.to_lowercase().as_str() {
        "html" => {
            let content = export_html(&recording, &steps, &crop_set);
            std::fs::write(&path, content)
                .map_err(|e| format!("Failed to write HTML: {}", e))?;
            Ok(path.to_string_lossy().to_string())
        }
        "markdown" => {
            // Screenshots placed alongside the MD file (no subdirectory)
            let parent = path.parent().unwrap_or(std::path::Path::new("."));
            let content = export_markdown(&recording, &steps, &parent, &crop_set);
            std::fs::write(&path, content)
                .map_err(|e| format!("Failed to write Markdown: {}", e))?;
            Ok(path.to_string_lossy().to_string())
        }
        "pdf" => {
            export_pdf(&recording, &steps, &path, &crop_set)?;
            Ok(path.to_string_lossy().to_string())
        }
        "docx" | "word" => {
            export_docx(&recording, &steps, &path, &crop_set)?;
            Ok(path.to_string_lossy().to_string())
        }
        "txt" => {
            let content = export_txt(&recording, &steps);
            std::fs::write(&path, content)
                .map_err(|e| format!("Failed to write TXT: {}", e))?;
            Ok(path.to_string_lossy().to_string())
        }
        "skill" => {
            // Agent Skills 1.0（SKILL.md）：有任何变量标记步骤 → 变量版（##Inputs + {{param}}），
            // 否则固定版（sample 写死）。固定/变量都是正经 skill（总纲 §4.1）。
            // 截图捆绑到 SKILL.md 同目录 screenshots/ 并在步骤下引用——机看版核心差异。
            // 机器可执行数据另查一张表（见 SkillMachine 文档：不并进 ExportStep）。
            // 查失败退化成空表 → 只少 steps.json 与定位行，SKILL.md 照常出。
            let mut machine = match lock_db(&db.0) {
                Ok(conn) => load_skill_machine(&conn, &id),
                Err(_) => SkillMachine::new(),
            };
            export_skill_bundle(&path, &recording, &steps, &crop_set, &mut machine)
        }
        "long" => {
            export_long_image(&recording, &steps, &path, &crop_set)?;
            Ok(path.to_string_lossy().to_string())
        }
        other => Err(format!("Unsupported format: {}. Use html, markdown, txt, docx, pdf, or long.", other)),
    }
}

/// 带超时获取 DB 锁：录制/其它线程异常持锁时 5 秒后放弃并报错，
/// 避免导出无限等待锁导致"导出中"卡死。
fn lock_db<'a>(
    db: &'a std::sync::Mutex<rusqlite::Connection>,
) -> Result<std::sync::MutexGuard<'a, rusqlite::Connection>, String> {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        match db.try_lock() {
            Ok(guard) => return Ok(guard),
            Err(_) => {
                if Instant::now() >= deadline {
                    return Err(
                        "Database is busy, export aborted. Please try again.".to_string()
                    );
                }
                std::thread::sleep(Duration::from_millis(100));
            }
        }
    }
}

// ── Data loading ──────────────────────────────────────────────

fn load_export_data(
    db: &State<DbState>,
    id: &str,
) -> Result<(ExportRecording, Vec<ExportStep>), String> {
    let conn = db.0.lock().map_err(|e| e.to_string())?;

    let recording = conn
        .query_row(
            "SELECT id, title, app_name, total_steps, created_at \
             FROM recordings WHERE id = ?1",
            params![id],
            |row| {
                Ok(ExportRecording {
                    id: row.get(0)?,
                    title: row.get(1)?,
                    app_name: row.get(2)?,
                    total_steps: row.get(3)?,
                    created_at: row.get(4)?,
                })
            },
        )
        .map_err(|e| format!("Recording not found: {}", e))?;

    let mut stmt = conn
        .prepare(
            "SELECT id, order_index, step_number, action_type, title, description, \
             tip, screenshot_path, x, y, crop_w, crop_h, is_variable, param_name \
             FROM steps WHERE recording_id = ?1 ORDER BY order_index",
        )
        .map_err(|e| e.to_string())?;

    let rows = stmt
        .query_map(params![id], |row| {
            Ok(ExportStep {
                id: row.get(0)?,
                order_index: row.get(1)?,
                step_number: row.get(2)?,
                action_type: row.get(3)?,
                title: row.get(4)?,
                description: row.get(5)?,
                tip: row.get(6)?,
                screenshot_path: row.get(7)?,
                x: row.get(8)?,
                y: row.get(9)?,
                crop_w: row.get(10)?,
                crop_h: row.get(11)?,
                is_variable: row.get::<_, i64>(12)? != 0,
                param_name: row.get(13)?,
            })
        })
        .map_err(|e| e.to_string())?;

    let mut steps = Vec::new();
    for row in rows {
        steps.push(row.map_err(|e| e.to_string())?);
    }

    Ok((recording, steps))
}

/// 把译文副本覆盖到导出数据上（PRD T6）。
///
/// 只替换 recording.title 与每个步骤的 title/description；截图、坐标、
/// action_type 等一律不动。译文副本按 order_index（字符串键）映射，
/// 某步骤缺译文时回退原文。译文不存在时返回错误（前端不应出现该状态）。
fn apply_translation_override(
    db: &State<DbState>,
    id: &str,
    lang: &str,
    recording: &mut ExportRecording,
    steps: &mut Vec<ExportStep>,
) -> Result<(), String> {
    let conn = db.0.lock().map_err(|e| e.to_string())?;
    let raw = crate::db::get_recording_translations_raw(&conn, id)?
        .ok_or_else(|| format!("该教程没有译文副本（lang={}) ", lang))?;
    let parsed: serde_json::Value =
        serde_json::from_str(&raw).map_err(|e| format!("译文 JSON 损坏：{}", e))?;
    let Some(lang_obj) = parsed.get(lang) else {
        return Err(format!("没有 {} 语言的译文副本", lang));
    };

    if let Some(t) = lang_obj.get("title").and_then(|v| v.as_str()) {
        if !t.trim().is_empty() {
            recording.title = t.to_string();
        }
    }
    let steps_obj = lang_obj.get("steps").and_then(|v| v.as_object());
    for step in steps.iter_mut() {
        let key = step.order_index.to_string();
        let Some(entry) = steps_obj.and_then(|o| o.get(&key)) else {
            continue; // 缺译文 → 回退原文
        };
        if let Some(t) = entry.get("title").and_then(|v| v.as_str()) {
            if !t.trim().is_empty() {
                step.title = t.to_string();
            }
        }
        if let Some(d) = entry.get("description").and_then(|v| v.as_str()) {
            step.description = d.to_string();
        }
    }
    Ok(())
}

fn sanitize_filename(name: &str) -> String {
    let cleaned: String = name
        .chars()
        .map(|c| match c {
            '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|' => '_',
            _ => c,
        })
        .collect();

    // B7: Windows 保留名（CON/PRN/AUX/NUL/COM1-9/LPT1-9，含带扩展名形式 CON.txt）。
    // 直接命中会被 Windows 拒绝写入，追加 `_` 规避。
    let stem_len = cleaned
        .find('.')
        .unwrap_or(cleaned.len());
    let stem = cleaned[..stem_len].to_lowercase();
    let is_reserved = matches!(
        stem.as_str(),
        "con" | "prn" | "aux" | "nul"
            | "com1" | "com2" | "com3" | "com4" | "com5"
            | "com6" | "com7" | "com8" | "com9"
            | "lpt1" | "lpt2" | "lpt3" | "lpt4" | "lpt5"
            | "lpt6" | "lpt7" | "lpt8" | "lpt9"
    );
    if is_reserved {
        format!("{}_", cleaned)
    } else {
        cleaned
    }
}

/// 生成自动导出路径：`<输出目录>/<清洗标题>_<时间戳>.<扩展名>`。
/// 格式不识别时回退 `.txt`（与 export_recording_inner 的 match 一致）。
fn generate_export_path(
    output_dir: &std::path::Path,
    title: &str,
    format: &str,
    timestamp: &str,
) -> std::path::PathBuf {
    let sanitized_title = sanitize_filename(title);
    let ext = match format.to_lowercase().as_str() {
        "html" => "html",
        "markdown" => "md",
        "txt" => "txt",
        "pdf" => "pdf",
        "docx" | "word" => "docx",
        "long" => "png",
        "skill" => "md",
        _ => "txt",
    };
    output_dir.join(format!("{}_{}.{}", sanitized_title, timestamp, ext))
}

// ── Phase C: 导出前强制自动脱敏 ───────────────────────────────

/// 读取 redaction_config 中的导出前脱敏开关与覆盖方式。
/// 开关 `redact_before_export` 默认关闭（首次安装即关闭，与设置页/默认配置一致）；
/// `mode` 默认高斯模糊。
pub(crate) fn read_export_redact_policy(
    db: &State<DbState>,
) -> (bool, RedactMode) {
    let conn = match db.0.try_lock() {
        Ok(c) => c,
        // 取不到锁：保守地不脱敏（默认关闭），避免导出时强行改造截图
        Err(_) => return (false, RedactMode::Blur),
    };
    let mut stmt = match conn.prepare("SELECT value FROM settings WHERE key = ?1") {
        Ok(s) => s,
        Err(_) => return (false, RedactMode::Blur),
    };
    let json: Option<String> = stmt
        .query_row(params!["redaction_config"], |row| row.get(0))
        .ok();
    drop(stmt);
    drop(conn);
    let Some(s) = json else {
        // 首次安装：settings 表中尚无 redaction_config 行 → 默认关闭
        return (false, RedactMode::Blur);
    };
    let Ok(v) = serde_json::from_str::<serde_json::Value>(&s) else {
        return (false, RedactMode::Blur);
    };
    let enabled = v
        .get("redact_before_export")
        .and_then(|x| x.as_bool())
        .unwrap_or(false);
    let mode = RedactMode::from_key(
        v.get("mode").and_then(|x| x.as_str()),
    );
    (enabled, mode)
}

/// 导出前统一脱敏：文本（title/description/tip）直接替换为 `[已隐藏]`；
/// 截图通过 OCR 扫描生成脱敏副本（原图不动）。开关关闭时原样返回。
///
/// 性能：截图 OCR 并行处理。每张截图 OCR 需 1~15s（放大 + 引擎识别），
/// 串行处理会随步骤数线性累积成"导出中"久等；这里按 4 路并发扫描，
/// 且限制并发数避免放大图 BMP 内存飙高。进度通过 `export://progress` 上报。
pub(crate) fn prepare_export_steps(
    db: &State<DbState>,
    mut steps: Vec<ExportStep>,
    progress: Option<&(dyn Fn(usize, usize) + Send + Sync)>,
) -> Result<Vec<ExportStep>, String> {
    let (enabled, mode) = read_export_redact_policy(db);
    if !enabled {
        return Ok(steps);
    }
    let config = {
        let conn = lock_db(&db.0)?;
        crate::settings::commands::read_redaction_config(&conn)
    };

    // 文本脱敏廉价，串行处理
    for s in &mut steps {
        s.title = redact_text_with_config(&s.title, &config);
        s.description = redact_text_with_config(&s.description, &config);
        s.tip = redact_text_with_config(&s.tip, &config);
    }

    // 截图 OCR 脱敏并行：仅对存在截图的步骤建任务
    const MAX_CONCURRENCY: usize = 4;
    let jobs: Vec<(usize, std::path::PathBuf)> = steps
        .iter()
        .enumerate()
        .filter(|(_, s)| {
            !s.screenshot_path.is_empty() && std::path::Path::new(&s.screenshot_path).exists()
        })
        .map(|(i, s)| (i, std::path::PathBuf::from(&s.screenshot_path)))
        .collect();

    if !jobs.is_empty() {
        let results: Mutex<Vec<(usize, Result<Option<std::path::PathBuf>, String>)>> =
            Mutex::new(Vec::with_capacity(jobs.len()));
        let done = std::sync::atomic::AtomicUsize::new(0);
        let total = jobs.len();
        std::thread::scope(|scope| {
            let results = &results;
            let config = &config;
            let done = &done;
            for chunk in jobs.chunks(MAX_CONCURRENCY) {
                let owned = chunk.to_vec();
                scope.spawn(move || {
                    for (idx, path) in owned {
                        let r = redact_screenshot_copy(&path, config, mode);
                        results.lock().unwrap().push((idx, r));
                        if let Some(p) = progress {
                            p(done.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1, total);
                        }
                    }
                });
            }
        });
        for (idx, r) in results.into_inner().unwrap() {
            match r {
                Ok(Some(copy)) => steps[idx].screenshot_path = copy.to_string_lossy().to_string(),
                Ok(None) => {}
                Err(e) => {
                    // OCR 失败不阻断导出：保留原图并记录日志
                    log::warn!("export redact skip {}: {}", steps[idx].screenshot_path, e);
                }
            }
        }
    }
    Ok(steps)
}

// ── Export image cropping helper ──────────────────────────────

/// 导出裁剪：**与前端 cropAdaptiveBase64 逻辑完全一致**，保证导出图 = 编辑器里看到的图。
///
/// 坐标语义（与录制端 centered_crop_box 一致）：x/y = 裁剪框**中心**，crop_w/h = 框尺寸。
/// - 整窗类型（scroll/nav/screenshot）：不裁剪，原样返回；
/// - 有坐标（中心+尺寸）：外扩 CROP_PAD(8px)，type 步骤再左扩 1/3 宽带出 label，
///   贴边时整体平移进图内（不缩小裁剪框）；
/// - 无坐标：与前端一致返回全图（前端此场景同样显示全图）。
const EXPORT_CROP_PAD: i32 = 8;

/// 导出裁剪框的纯几何（不碰图片字节），便于单测。
///
/// ⚠️ 这套「居中锚点 → 外扩 PAD → type 左扩 1/3 宽」的算法在仓库里有 **4 份消费端实现**，
/// 任何一份改动都必须同步其余三份，否则编辑器里看到的图和导出/AI 的图会不一致：
///   1. `src/lib/cropAnchor.ts`          —— 前端 cropBoxFromAnchor / rectToCropAnchor（PAD=8）
///   2. `src/pages/StepEditor.tsx`       —— 前端 cropAdaptiveBase64（编辑器实际显示，PAD=8）
///   3. `src-tauri/src/export/commands.rs` —— 本函数（导出 docx/html/pdf，PAD=8）
///   4. `src-tauri/src/ai/pipeline.rs`   —— AI 管线裁图（PAD=24 / MIN=280 / 缩到屏幕 90%，
///                                          常量**刻意不同**，喂给 AI 的图需要更大上下文）
///
/// 另有 **1 份生产端实现**，语义与上面 4 份相反（它是**生成**落库值，上面 4 份是
/// **消费**落库值），常量取值也不同，**不在上面这份同步清单里**：
///   5. `src-tauri/src/recorder/commands.rs` —— 录制时算落库值（PAD=24 / MIN 420x280）。
///      这里的 PAD 会被 bake 进落库值，消费端还会再外扩一次，所以元素四周的
///      **实际留白 = 24（录制端）+ 消费端各自的 PAD**，不是任何单处的常量。
///
/// 一致性由 `npm run check:crop`（前端，第 6 段跨端黄金值）与
/// `both_consumers_must_contain_the_visible_element_rect`（cargo，导出 vs AI）共同兜底。
pub(crate) fn export_crop_box(
    img_w: i32,
    img_h: i32,
    x: Option<i32>,
    y: Option<i32>,
    crop_w: Option<i32>,
    crop_h: Option<i32>,
    action_type: &str,
) -> Option<(i32, i32, i32, i32)> {
    // 整窗类型不裁剪（与前端 Stage C 一致）
    if matches!(action_type, "scroll" | "nav" | "screenshot") {
        return None;
    }
    if img_w <= 0 || img_h <= 0 {
        return None;
    }

    // 分支选择（与前端 cropAdaptiveBase64 逐条对齐）：
    // ① 有元素矩形（中心+尺寸）→ 按矩形外扩裁剪，**不校验坐标正负**——前端同样不校验，
    //    元素中心落在屏幕外时两侧行为必须一致，否则编辑器显示裁图而导出变全图。
    // ② 只有坐标且非负 → 点击点为中心裁 40%（前端此分支才要求 x>=0 && y>=0）。
    // 前端 40% 兜底不加 CROP_PAD，此处用 add_pad 区分。
    let (cx, cy, rw0, rh0, add_pad) = match (x, y, crop_w, crop_h) {
        (Some(a), Some(b), Some(c), Some(d)) if c > 0 && d > 0 => (a, b, c, d, true),
        (Some(a), Some(b), _, _) if a >= 0 && b >= 0 => {
            let fw = ((img_w as f64) * 0.4).round() as i32;
            let fh = ((img_h as f64) * 0.4).round() as i32;
            (a, b, fw, fh, false)
        }
        _ => return None,
    };

    // ① 外扩 CROP_PAD
    let mut rw = if add_pad { rw0 + EXPORT_CROP_PAD * 2 } else { rw0 };
    let mut rh = if add_pad { rh0 + EXPORT_CROP_PAD * 2 } else { rh0 };

    // ② type 步骤左侧多留原始宽的 1/3，把左侧 label（如"用户名"）带进裁图。
    //    ⚠️ 左移量是 extra/2 而不是 extra：框宽 rw 已含 extra，居中项 cx - rw/2
    //    天然在左右各分到 extra/2。写成 - extra 会过冲，使右侧留白变成
    //    PAD - extra/2 —— 元素宽 > 48px 即为负值，右边缘被切（300px 输入框切 42px）。
    //    extra 用 round 而非整数截断，与前端 Math.round(cropW / 3) 对齐。
    // ③ drag 步骤高度方向多留上下文：拖拽选区多为扁长（表格行 ~25px），
    //    仅 +16px（EXPORT_CROP_PAD*2）看不出在拖哪里。额外至少 100px 并按选区高
    //    0.6× 增长（与前端 cropAdaptiveBase64 一致）。
    let mut top = cy - rh / 2;
    let left = if add_pad && action_type == "type" {
        let extra = ((rw0 as f64) / 3.0).round();
        rw += extra as i32;
        (cx as f64 - (rw as f64) / 2.0 - extra / 2.0).round() as i32
    } else if add_pad && action_type == "drag" {
        let extra_h = ((rh0 as f64) * 0.6).round().max(100.0);
        rh += extra_h as i32;
        top = (cy as f64 - (rh as f64) / 2.0).round() as i32;
        cx - rw / 2
    } else {
        cx - rw / 2
    };

    // 贴边平移进图内（不缩小裁剪框，避免内容被切掉一半）
    let rw = rw.min(img_w).max(1);
    let rh = rh.min(img_h).max(1);
    let left = left.clamp(0, img_w - rw);
    let top = top.clamp(0, img_h - rh);
    Some((left, top, rw, rh))
}

/// 空白裁剪守卫阈值 —— 与前端 `src/lib/cropBlank.ts::BLANK_CROP_DELTA` 同步（黄金值 24，
/// 锁在 `scripts/check-crop.mjs` 第 7 段）。JPEG 平坦区噪声 ±6 左右，24 有充分余量。
pub(crate) const BLANK_CROP_DELTA: i32 = 24;

/// 空白裁剪守卫：裁出的区域几乎纯色（三通道极差全部 ≤ BLANK_CROP_DELTA）时
/// 毫无信息量，调用方应回退**整图**。
///
/// 背景：WPS「正在打开文档」加载中录制时 UIA 元素树不可用，录制端把
/// 「Program Manager」桌面宿主窗口的伪元素矩形记成裁图锚点，裁出纯灰空白，
/// 而用户实际看到的加载动画在画面中心（2026-09-05 用户实测）。锚点本身指向
/// 空白区，退 40% 点击点兜底照样空白，唯一有意义的回退是整图。
///
/// ⚠️ 同步清单：前端 `src/lib/cropBlank.ts`（编辑器/发布/封面）与
/// `ai/pipeline.rs::crop_and_encode_typed`（AI 裁图）。算法与阈值必须一致。
pub(crate) fn is_blank_crop(img: &image::DynamicImage) -> bool {
    let rgb = img.to_rgb8();
    let (w, h) = (rgb.width() as usize, rgb.height() as usize);
    if w == 0 || h == 0 {
        return false;
    }
    // 步长采样（与前端同公式）：目标约 2 万样本。漏采的小特征在整图里仍在 ——
    // 回退整图永远不丢信息，故宁松勿严。
    let stride = (((w * h) as f64 / 20000.0).sqrt().floor() as usize).max(1);
    let mut min = [255u8; 3];
    let mut max = [0u8; 3];
    for y in (0..h).step_by(stride) {
        for x in (0..w).step_by(stride) {
            let p = rgb.get_pixel(x as u32, y as u32).0;
            for c in 0..3 {
                if p[c] < min[c] {
                    min[c] = p[c];
                }
                if p[c] > max[c] {
                    max[c] = p[c];
                }
            }
        }
    }
    max
        .iter()
        .zip(min.iter())
        .all(|(mx, mn)| (*mx as i32 - *mn as i32) <= BLANK_CROP_DELTA)
}

/// 返回 `(最终写出去的字节, 这些字节对应的真实像素尺寸)`。
///
/// ⚠️ 调用方必须拿**第二个返回值**去算显示尺寸，不要用原图尺寸：
/// DOCX 导出曾经传原图尺寸给 `Pic::new(cropped_bytes).size(w, h)`，
/// 于是裁剪后的图被按原图比例拉伸变形（如 1920×1080 原图裁出 800×600，
/// 显示成 16:9 的框 → 横向拉伸 33%），且小裁剪区域会被放大到 600px 宽变模糊。
/// 见测试 `docx_image_emu_matches_written_bytes`。
pub(crate) fn maybe_crop_screenshot_bytes(
    raw_bytes: Vec<u8>,
    x: Option<i32>,
    y: Option<i32>,
    crop_w: Option<i32>,
    crop_h: Option<i32>,
    action_type: &str,
) -> (Vec<u8>, Option<(usize, usize)>) {
    let mut img = match image::load_from_memory(&raw_bytes) {
        Ok(i) => i,
        Err(_) => return (raw_bytes, None),
    };
    let img_w = img.width() as i32;
    let img_h = img.height() as i32;
    let full_dims = Some((img.width() as usize, img.height() as usize));
    let (left, top, rw, rh) = match export_crop_box(img_w, img_h, x, y, crop_w, crop_h, action_type)
    {
        Some(v) => v,
        None => return (raw_bytes, full_dims),
    };

    let cropped = img.crop(left as u32, top as u32, rw as u32, rh as u32);
    // 空白裁剪守卫：区域几乎纯色时回退整图（与前端 lib/cropBlank.ts 同步）
    if is_blank_crop(&cropped) {
        return (raw_bytes, full_dims);
    }
    let mut buf: Vec<u8> = Vec::new();
    // 输出 JPEG 质量 85（与前端 canvas.toDataURL("image/jpeg", 0.85) 一致）
    let enc = image::codecs::jpeg::JpegEncoder::new_with_quality(&mut buf, 85);
    if cropped.write_with_encoder(enc).is_err() {
        return (raw_bytes, full_dims);
    }
    (buf, Some((rw as usize, rh as usize)))
}

// ── HTML export ───────────────────────────────────────────────

/// Generate the inner HTML body block (step cards).
/// Images are embedded as base64 data URIs for standalone output.
fn gen_html_body(
    recording: &ExportRecording,
    steps: &[ExportStep],
    crop_set: &std::collections::HashSet<&str>,
) -> String {
    let step_cards: String = steps
        .iter()
        .map(|s| {
            let screenshot_html = if s.screenshot_path.is_empty() {
                build_screenshot_placeholder_html().to_string()
            } else {
                let raw = std::fs::read(&s.screenshot_path).ok().unwrap_or_default();
                let is_crop = crop_set.contains(s.id.as_str());
                let data = if is_crop && !raw.is_empty() {
                    maybe_crop_screenshot_bytes(raw, s.x, s.y, s.crop_w, s.crop_h, &s.action_type).0
                } else {
                    raw
                };
                let encoded = base64::engine::general_purpose::STANDARD.encode(&data);
                let mime = mime_from_path(&s.screenshot_path);
                build_screenshot_img_html(mime, &encoded, s.step_number)
            };

            // Step header: "Step N: [type] title"
            let header = build_step_header_html(s.step_number, &s.action_type, &s.title);

            let desc_block = build_step_desc_html(&s.description);

            let tip_block = build_step_tip_html(&s.tip);

            format!(
                r#"    <div class="step-card">
      <div class="step-header">{header}</div>
{desc_block}      {screenshot_html}
{tip_block}    </div>"#,
                header = header,
                desc_block = desc_block,
                tip_block = tip_block,
                screenshot_html = screenshot_html,
            )
        })
        .collect::<Vec<_>>()
        .join("\n");

    build_html_document(&recording.title, recording.total_steps, &recording.created_at, &step_cards)
}

/// Generate standalone HTML with base64-embedded screenshots.
fn export_html(recording: &ExportRecording, steps: &[ExportStep], crop_set: &std::collections::HashSet<&str>) -> String {
    gen_html_body(recording, steps, crop_set)
}
fn htmlescape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}

fn mime_from_path(path: &str) -> &'static str {
    let lower = path.to_lowercase();
    if lower.ends_with(".png") {
        "image/png"
    } else if lower.ends_with(".jpg") || lower.ends_with(".jpeg") {
        "image/jpeg"
    } else if lower.ends_with(".gif") {
        "image/gif"
    } else if lower.ends_with(".webp") {
        "image/webp"
    } else if lower.ends_with(".bmp") {
        "image/bmp"
    } else {
        "image/png"
    }
}

// ── Markdown export ───────────────────────────────────────────

/// Generate a clean Markdown document (text only, no images):
///   - Title + metadata row
///   - Numbered step cards with header, description, tip
///   - No screenshot files are created or referenced
fn export_markdown(recording: &ExportRecording, steps: &[ExportStep], _ss_dir: &std::path::Path, _crop_set: &std::collections::HashSet<&str>) -> String {
    let step_list: String = steps
        .iter()
        .map(|s| {
            // ── Step header ───────────────
            let header = build_md_step_header(s.step_number, &s.action_type, &s.title);

            // ── Description ───────────────
            let desc_block = build_md_desc_block(&s.description);

            // ── Tip ───────────────────────
            let tip_block = build_md_tip_block(&s.tip);

            format!(
                "{header}\n\n{desc_block}{tip_block}",
                header = header,
                desc_block = desc_block,
                tip_block = tip_block,
            )
        })
        .collect::<Vec<_>>()
        .join("\n---\n\n");

    build_md_document(
        &recording.title,
        &recording.app_name,
        recording.total_steps,
        &recording.created_at,
        &step_list,
    )
}

// ── Agent Skills 1.0 (SKILL.md) export ────────────────────────

/// 把标题压成 Agent Skills 合法的 name（小写字母/数字/连字符，≤48 字符）。
/// 中文标题压不出 ASCII 词干时回退 `flowio-skill`。
fn skill_slug_name(title: &str) -> String {
    let mut slug = String::new();
    for c in title.chars() {
        if c.is_ascii_alphanumeric() {
            slug.push(c.to_ascii_lowercase());
        } else if (c == ' ' || c == '-' || c == '_') && !slug.ends_with('-') {
            slug.push('-');
        }
    }
    let slug = slug.trim_matches('-').to_string();
    if slug.is_empty() {
        "flowio-skill".to_string()
    } else {
        slug.chars().take(48).collect()
    }
}

/// 导出 skill 的**唯一**注册名（Agent Skills 1.0 要求 name 在注册表内唯一）。
///
/// 纯 `skill_slug_name` 对中文标题一律回退 `flowio-skill`，多个中文教程会互相覆盖——
/// MCP / 注册表无法区分。这里在 slug 后拼一段**由 `recording.id` 派生的稳定短后缀**
/// （仅 ASCII 字母数字，最长 8 位），保证同名不同教程也能共存，且仍满足
/// `^[a-z0-9][a-z0-9-]{0,63}$`（slug 已合规，后缀只追加合法字符）。
fn skill_unique_name(recording: &ExportRecording) -> String {
    let base = skill_slug_name(&recording.title);
    let clean: String = recording
        .id
        .chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .collect();
    // 取 id 末尾 8 位作为稳定后缀（短 id 则取全部）。
    let suffix: String = clean
        .chars()
        .rev()
        .take(8)
        .collect::<String>()
        .chars()
        .rev()
        .collect();
    let suffix = if suffix.is_empty() {
        "skill".to_string()
    } else {
        suffix
    };
    if base == "flowio-skill" {
        format!("flowio-skill-{}", suffix)
    } else {
        format!("{}-{}", base, suffix)
    }
}

/// YAML 单行字符串转义：换行压成空格、双引号转义。
fn yaml_oneline(s: &str) -> String {
    s.replace(['\r', '\n'], " ").replace('"', "\\\"")
}

// ── SKILL 机器可执行数据 ───────────────────────────────────────

/// 步号 → 该步的机器可执行定位数据。
///
/// **刻意不并进 `ExportStep`**：那个结构体在本文件里有 17 处测试字面量，加字段
/// 会全量 E0063。与 executor 侧 `load_step_intents` / `load_step_before_shots`
/// 同一套做法——附带数据一律另查一张表。
type SkillMachine = std::collections::HashMap<i32, SkillStepData>;

/// 一步操作的机器可执行描述。序列化后写进 `steps.json`，供 Agent 编程消费：
/// 人读的 SKILL.md 讲「做什么」，这份讲「点哪里、按什么、拿什么校验」。
#[derive(Debug, Clone, Default, serde::Serialize)]
struct SkillStepData {
    /// 步号（与 SKILL.md 的编号、screenshots/step-NN 一一对应）。
    n: i32,
    /// 动作类型：click / dblclick / rightclick / type / paste / drag / scroll / key / hotkey。
    action: String,
    /// 人读意图（步骤标题）。
    intent: String,
    /// 该意图**没说清点了什么**（形如「点击在「X」中」只报窗口）。
    /// 置 true 时消费方应以截图为准判断目标，别信这行文字。
    intent_vague: bool,
    /// 目标窗口标题（录制时的前台窗口）。
    window: String,
    /// 录制时的落点（屏幕逻辑坐标）。drag 用真实起点而非裁图框中心。
    #[serde(skip_serializing_if = "Option::is_none")]
    point: Option<[i32; 2]>,
    /// 拖拽终点。
    #[serde(skip_serializing_if = "Option::is_none")]
    drag_to: Option<[i32; 2]>,
    /// 滚轮增量（正=向上）。
    #[serde(skip_serializing_if = "Option::is_none")]
    scroll_delta: Option<i32>,
    /// 结构化组合键，如 `Ctrl+Shift+S`。
    #[serde(skip_serializing_if = "Option::is_none")]
    hotkey: Option<String>,
    /// 待输入文本（type / paste 步）。
    #[serde(skip_serializing_if = "Option::is_none")]
    text: Option<String>,
    /// 控件锚点（类名 + 窗口内归一化比例），比绝对坐标抗窗口移动。
    #[serde(skip_serializing_if = "Option::is_none")]
    anchor: Option<serde_json::Value>,
    /// 该步截图在包内的相对路径（bundle 落盘时回填）。
    #[serde(skip_serializing_if = "Option::is_none")]
    screenshot: Option<String>,
}

/// 键盘类动作：标题里的按键本身就是完整语义（「Enter 确认」），
/// 不存在「没说清点了什么」的问题。
fn is_keyboard_action(action_type: &str) -> bool {
    matches!(
        action_type,
        "key" | "keypress" | "hotkey" | "shortcut" | "combo"
    )
}

/// 落点基准：drag 的 `x/y` 存的是**裁图框中心**，真实起点在 `start_x/start_y`。
/// 与 executor 的 `delta_reference` 同一个坑，这里独立实现以免跨模块耦合。
fn skill_point(
    action_type: &str,
    x: Option<i32>,
    y: Option<i32>,
    start_x: Option<i32>,
    start_y: Option<i32>,
) -> Option<[i32; 2]> {
    if action_type == "drag" {
        if let (Some(sx), Some(sy)) = (start_x, start_y) {
            return Some([sx, sy]);
        }
    }
    match (x, y) {
        (Some(a), Some(b)) => Some([a, b]),
        _ => None,
    }
}

/// 裸动词表：标题只剩这些词时，等于什么都没说。
const BARE_VERBS: [&str; 9] = [
    "点击",
    "双击",
    "右键",
    "右键点击",
    "输入",
    "粘贴",
    "拖拽",
    "滚动",
    "单击",
];

/// 标题是否**没说清操作目标**。
///
/// 录制端自动生成的标题形如 `点击在「函数参数」中` / `在「插入函数」中输入`——
/// 只报得出所在窗口，报不出点的是哪个控件。AI 拿这种文字只能瞎猜，必须显式
/// 标出来让它改看截图。实测真实库 19 步里有 14 步属于此类。
fn title_lacks_target(action_type: &str, title: &str) -> bool {
    if is_keyboard_action(action_type) {
        return false;
    }
    let t = title.trim();
    if t.is_empty() {
        return true;
    }
    if BARE_VERBS.contains(&t) {
        return true;
    }
    // 剥掉「…」里的窗口名与「在…中」骨架，若只剩一个裸动词 → 模板串
    if let (Some(a), Some(b)) = (t.find('「'), t.rfind('」')) {
        if a < b {
            let outside = format!("{}{}", &t[..a], &t[b + '」'.len_utf8()..]);
            let skeleton = outside.replace('在', "").replace('中', "");
            if BARE_VERBS.contains(&skeleton.trim()) {
                return true;
            }
        }
    }
    false
}

/// SKILL 包的数据完备度。用于在 SKILL.md 里如实告知消费方哪些信息不可靠——
/// 与其让 AI 拿着模板标题瞎猜，不如明说「这几步请看图」。
#[derive(Debug, Clone, Default, PartialEq)]
struct SkillQuality {
    total: usize,
    /// 意图含糊（只报窗口 / 裸动词 / 空）的步数。
    vague: usize,
    /// 有落点坐标的步数。
    with_point: usize,
    /// 有控件锚点的步数。
    with_anchor: usize,
    /// 有截图的步数。
    with_shot: usize,
}

fn skill_quality(machine: &SkillMachine) -> SkillQuality {
    let mut q = SkillQuality {
        total: machine.len(),
        ..Default::default()
    };
    for d in machine.values() {
        if d.intent_vague {
            q.vague += 1;
        }
        if d.point.is_some() {
            q.with_point += 1;
        }
        if d.anchor.is_some() {
            q.with_anchor += 1;
        }
        if d.screenshot.is_some() {
            q.with_shot += 1;
        }
    }
    q
}

/// 生成「## 执行须知」正文。**含糊步骤为 0 时不产出告知段**（好数据不该被
/// 免责声明打扰）；有含糊步骤时列出具体步号，让消费方精确知道哪几步别信文字。
fn skill_notice(machine: &SkillMachine, q: &SkillQuality) -> String {
    let mut out = String::new();
    out.push_str("## 执行须知\n\n");
    out.push_str(
        "本技能由「录步 Flowio」录制真实桌面操作导出。每步给出目标窗口、录制时的\
         落点坐标与当时的界面截图（红圈标出点击位置）。\n\n",
    );
    out.push_str(
        "- 坐标是**录制那台机器当时的屏幕逻辑坐标**。分辨率、缩放或窗口位置一变就会失效，\
         此时以截图为准重新定位，不要直接照抄坐标。\n",
    );
    out.push_str("- 完整机器可读数据见同目录 `steps.json`（含控件锚点、组合键、输入文本）。\n");
    if q.vague > 0 {
        let mut nums: Vec<i32> = machine
            .values()
            .filter(|d| d.intent_vague)
            .map(|d| d.n)
            .collect();
        nums.sort_unstable();
        let list: Vec<String> = nums.iter().map(|n| n.to_string()).collect();
        out.push_str(&format!(
            "- ⚠️ 第 {} 步（共 {} 步中的 {} 步）标题由录制端自动生成，形如「点击在「窗口名」中」，\
             **只说明了所在窗口、没说明点的是哪个控件**。这些步骤请优先依据截图判断意图，\
             不要按标题字面理解。\n",
            list.join("、"),
            q.total,
            q.vague
        ));
    }
    out.push('\n');
    out
}

/// 读取机器可执行数据。任一列缺失只影响该字段，不影响整体导出。
fn load_skill_machine(conn: &rusqlite::Connection, recording_id: &str) -> SkillMachine {
    let mut map = SkillMachine::new();
    let Ok(mut stmt) = conn.prepare(
        "SELECT step_number, COALESCE(action_type,''), COALESCE(title,''), \
         COALESCE(window_title,''), x, y, start_x, start_y, end_x, end_y, \
         scroll_delta, hotkey_combo, uia_anchor, COALESCE(description,'') \
         FROM steps WHERE recording_id = ?1 ORDER BY order_index",
    ) else {
        return map;
    };
    let rows = stmt.query_map(params![recording_id], |r| {
        Ok((
            r.get::<_, i32>(0)?,
            r.get::<_, String>(1)?,
            r.get::<_, String>(2)?,
            r.get::<_, String>(3)?,
            r.get::<_, Option<i32>>(4)?,
            r.get::<_, Option<i32>>(5)?,
            r.get::<_, Option<i32>>(6)?,
            r.get::<_, Option<i32>>(7)?,
            r.get::<_, Option<i32>>(8)?,
            r.get::<_, Option<i32>>(9)?,
            r.get::<_, Option<i32>>(10)?,
            r.get::<_, Option<String>>(11)?,
            r.get::<_, Option<String>>(12)?,
            r.get::<_, String>(13)?,
        ))
    });
    let Ok(rows) = rows else { return map };
    for row in rows.filter_map(|r| r.ok()) {
        let (n, action, title, window, x, y, sx, sy, ex, ey, scroll, hotkey, anchor, desc) = row;
        let is_text = matches!(action.as_str(), "type" | "paste");
        map.insert(
            n,
            SkillStepData {
                n,
                intent_vague: title_lacks_target(&action, &title),
                intent: title.trim().to_string(),
                window: window.trim().to_string(),
                point: skill_point(&action, x, y, sx, sy),
                drag_to: match (ex, ey) {
                    (Some(a), Some(b)) => Some([a, b]),
                    _ => None,
                },
                scroll_delta: scroll,
                hotkey: hotkey.filter(|s| !s.trim().is_empty()),
                text: if is_text && !desc.trim().is_empty() {
                    Some(desc.trim().to_string())
                } else {
                    None
                },
                anchor: anchor
                    .filter(|s| !s.trim().is_empty())
                    .and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok()),
                screenshot: None,
                action,
            },
        );
    }
    map
}

/// 导出 Agent Skills 1.0（SKILL.md）。
///
/// - **变量版**：任一步骤被标记为变量（`is_variable` 且 `param_name` 非空）时输出——
///   frontmatter `description` 声明所需参数，`## Inputs` 列出参数与示例值，
///   步骤体把 sample（type 步骤的 description 即输入值）替换为 `{{param_name}}` 占位符。
/// - **固定版**：无变量标记时输出——sample 原样写死，纯人读/机看皆可的固定流程。
///   固定 skill 与变量 skill 是同等产物（总纲 §4.1），都走本导出器。
///
/// `images`：(步骤号, 相对路径)。非空时在对应步骤下插入截图引用——
/// 截图由 `export_skill_bundle` 预先复制到 SKILL.md 同目录（Agent Skills 规范
/// 支持 bundled resources），多模态执行层看图定位，这是机看版区别于 MD 导出的核心。
fn export_skill_md(
    recording: &ExportRecording,
    steps: &[ExportStep],
    images: &[(i32, String)],
    machine: &SkillMachine,
) -> String {
    // 收集变量步骤：(step, param_name, sample)
    let variables: Vec<(&ExportStep, String, String)> = steps
        .iter()
        .filter(|s| s.is_variable && !s.param_name.trim().is_empty())
        .map(|s| {
            (
                s,
                s.param_name.trim().to_string(),
                s.description.trim().to_string(),
            )
        })
        .collect();
    let variable_mode = !variables.is_empty();

    // ── frontmatter ──
    let name = skill_unique_name(recording);
    let mut desc = format!(
        "在 {} 中「{}」的分步操作流程，共 {} 步。",
        if recording.app_name.is_empty() { "目标应用" } else { &recording.app_name },
        recording.title,
        steps.len()
    );
    if variable_mode {
        let names: Vec<String> = variables.iter().map(|(_, p, _)| format!("`{}`", p)).collect();
        desc.push_str(&format!(" 运行前需提供参数：{}。", names.join("、")));
    }
    let desc = yaml_oneline(&desc);

    // ── 步骤体 ──
    let var_by_step: std::collections::HashMap<&str, &str> =
        variables.iter().map(|(s, p, _)| (s.id.as_str(), p.as_str())).collect();
    let img_by_step: std::collections::HashMap<i32, &str> =
        images.iter().map(|(n, p)| (*n, p.as_str())).collect();
    let mut body = String::new();
    for s in steps {
        let param = var_by_step.get(s.id.as_str()).copied();
        let title_line = if s.title.trim().is_empty() {
            format!("步骤 {}", s.step_number)
        } else {
            s.title.trim().to_string()
        };
        body.push_str(&format!("{}. **{}**", s.step_number, title_line));
        if s.action_type == "type" || s.action_type == "paste" {
            // 输入步骤：description 就是输入值（UIA 读到的输入框内容或按键累积）
            match param {
                Some(p) => {
                    let sample = s.description.trim();
                    body.push_str("：输入 `{{");
                    body.push_str(p);
                    body.push_str("}}`");
                    if sample.is_empty() {
                        body.push_str("（实际运行时由调用方提供）");
                    } else {
                        body.push_str(&format!(
                            "（示例值：`{}`，实际运行时由调用方提供）",
                            sample
                        ));
                    }
                }
                None => {
                    if !s.description.trim().is_empty() {
                        body.push_str(&format!("：输入 `{}`", s.description.trim()));
                    }
                }
            }
        } else if !s.description.trim().is_empty() {
            body.push_str(&format!("：{}", s.description.trim()));
        }
        body.push('\n');
        // 机器可执行线索：目标窗口 + 落点 / 组合键。标题说不清目标时这几行就是
        // 消费方唯一可靠的定位依据，必须写进人读正文而不只藏在 steps.json。
        if let Some(d) = machine.get(&s.step_number) {
            if !d.window.is_empty() {
                body.push_str(&format!("   - 目标窗口：`{}`\n", d.window));
            }
            if let Some(hk) = &d.hotkey {
                body.push_str(&format!("   - 组合键：`{}`\n", hk));
            }
            if let Some(p) = d.point {
                match d.drag_to {
                    Some(e) => body.push_str(&format!(
                        "   - 录制落点：`({}, {})` → `({}, {})`（拖拽）\n",
                        p[0], p[1], e[0], e[1]
                    )),
                    None => body.push_str(&format!("   - 录制落点：`({}, {})`\n", p[0], p[1])),
                }
            }
            if d.intent_vague {
                body.push_str("   - ⚠️ 本步标题未说明点的是哪个控件，请以截图为准定位。\n");
            }
        }
        if !s.tip.trim().is_empty() {
            body.push_str(&format!("   > 提示：{}\n", s.tip.trim()));
        }
        if let Some(rel) = img_by_step.get(&s.step_number) {
            body.push_str(&format!("   ![步骤 {} 截图]({})\n", s.step_number, rel));
        }
    }

    // ── 组装 ──
    let mut out = String::new();
    out.push_str("---\n");
    out.push_str(&format!("name: {}\n", name));
    out.push_str(&format!("description: \"{}\"\n", desc));
    out.push_str("---\n\n");
    out.push_str(&format!("# {}\n\n", recording.title));
    if !machine.is_empty() {
        let q = skill_quality(machine);
        out.push_str(&skill_notice(machine, &q));
    }
    if variable_mode {
        out.push_str("## Inputs\n\n");
        out.push_str("运行本技能前，由调用方（Agent / 用户 / 外部系统）提供以下参数：\n\n");
        for (s, p, sample) in &variables {
            if sample.is_empty() {
                out.push_str(&format!("- `{}` — 步骤 {} 的输入值\n", p, s.step_number));
            } else {
                out.push_str(&format!(
                    "- `{}` — 示例值：`{}`（来自步骤 {}）\n",
                    p, sample, s.step_number
                ));
            }
        }
        out.push('\n');
    }
    out.push_str("## Steps\n\n");
    out.push_str(&body);
    out
}

/// SKILL 导出落盘：把每步截图（按需裁剪）复制到 SKILL.md 同目录 `screenshots/`，
/// 再生成带截图引用的 SKILL.md。截图缺失的步骤跳过、整体不因个别图片失败而失败。
/// 这是「机看」版与 MD 导出的核心差异：执行层拿到的是 可读指令 + 可视定位 的捆绑包。
fn export_skill_bundle(
    output_path: &std::path::Path,
    recording: &ExportRecording,
    steps: &[ExportStep],
    crop_set: &std::collections::HashSet<&str>,
    machine: &mut SkillMachine,
) -> Result<String, String> {
    let parent = output_path
        .parent()
        .unwrap_or(std::path::Path::new("."));
    let ss_dir = parent.join("screenshots");
    let mut images: Vec<(i32, String)> = Vec::new();
    for s in steps {
        if s.screenshot_path.is_empty() {
            continue;
        }
        let raw = match std::fs::read(&s.screenshot_path) {
            Ok(b) if !b.is_empty() => b,
            _ => continue,
        };
        let bytes = if crop_set.contains(s.id.as_str()) {
            maybe_crop_screenshot_bytes(raw, s.x, s.y, s.crop_w, s.crop_h, &s.action_type).0
        } else {
            raw
        };
        std::fs::create_dir_all(&ss_dir)
            .map_err(|e| format!("Failed to create screenshots directory: {}", e))?;
        // 扩展名跟随源文件（录制截图实为 jpg），缺省回退 png
        let ext = std::path::Path::new(&s.screenshot_path)
            .extension()
            .and_then(|e| e.to_str())
            .map(|e| e.to_ascii_lowercase())
            .filter(|e| matches!(e.as_str(), "png" | "jpg" | "jpeg" | "webp"))
            .unwrap_or_else(|| "png".to_string());
        let fname = format!("step-{:02}.{}", s.step_number, ext);
        if std::fs::write(ss_dir.join(&fname), &bytes).is_ok() {
            let rel = format!("screenshots/{}", fname);
            if let Some(d) = machine.get_mut(&s.step_number) {
                d.screenshot = Some(rel.clone());
            }
            images.push((s.step_number, rel));
        }
    }
    let content = export_skill_md(recording, steps, &images, machine);
    std::fs::write(output_path, content)
        .map_err(|e| format!("Failed to write SKILL.md: {}", e))?;
    // steps.json：给「有点击工具的 Agent」用的结构化版本。写失败只是少一份
    // 附属数据，SKILL.md 已落盘，绝不因此让整个导出报错。
    if !machine.is_empty() {
        let mut ordered: Vec<&SkillStepData> = machine.values().collect();
        ordered.sort_by_key(|d| d.n);
        let q = skill_quality(machine);
        let payload = serde_json::json!({
            "skill": skill_unique_name(recording),
            "title": recording.title,
            "app": recording.app_name,
            "recorded_at": recording.created_at,
            "generated_by": "录步 Flowio",
            "coordinate_space": "录制机屏幕逻辑像素，左上角为原点；分辨率/缩放变化后需以截图重新定位",
            "quality": {
                "total": q.total,
                "intent_vague": q.vague,
                "with_point": q.with_point,
                "with_anchor": q.with_anchor,
                "with_screenshot": q.with_shot,
            },
            "steps": ordered,
        });
        if let Ok(text) = serde_json::to_string_pretty(&payload) {
            let _ = std::fs::write(parent.join("steps.json"), text);
        }
    }
    // skill.json：给「技能注册表 / MCP host」用的自描述清单——让外部 Agent 或
    // MCP 消费方无需解析 SKILL.md 即可登记并执行这个 skill（route ②「双输出：
    // 脚本 + SKILL.md/MCP 技能包」的收口）。写失败只是少一份注册元数据，
    // SKILL.md 与 steps.json 已落盘，绝不因此让整个导出报错（与 steps.json 同策略）。
    {
        // 动作直方图：消费方据以判断自身是否具备执行能力（如是否需要键盘注入）。
        let mut actions: std::collections::HashMap<String, u32> = std::collections::HashMap::new();
        for d in machine.values() {
            *actions.entry(d.action.clone()).or_insert(0) += 1;
        }
        let action_counts: std::collections::BTreeMap<String, u32> = actions.into_iter().collect();
        // 意图含糊的步号（对照 SKILL.md「执行须知」的免责段落）。
        let mut vague_steps: Vec<i32> = machine
            .values()
            .filter(|d| d.intent_vague)
            .map(|d| d.n)
            .collect();
        vague_steps.sort_unstable();
        let q = skill_quality(machine);
        // 变量步骤（若有）→ 消费方需要填的入参清单。
        let inputs: Vec<serde_json::Value> = steps
            .iter()
            .filter(|s| s.is_variable && !s.param_name.trim().is_empty())
            .map(|s| {
                let sample = if matches!(s.action_type.as_str(), "type" | "paste") {
                    s.description.trim()
                } else {
                    ""
                };
                serde_json::json!({
                    "name": s.param_name.trim(),
                    "sample": sample,
                })
            })
            .collect();
        let mut manifest = serde_json::json!({
            "schema": "flowio.skill.manifest/v1",
            "name": skill_unique_name(recording),
            "title": recording.title,
            "description": format!("由「录步 Flowio」录制并导出的桌面操作教程：{}。", recording.title),
            "version": "1.0.0",
            "generated_by": "录步 Flowio",
            "app": recording.app_name,
            "recorded_at": recording.created_at,
            "files": {
                "skill": "SKILL.md",
                "screenshots": "screenshots/"
            },
            "execution": {
                "entry": "SKILL.md",
                "coordinate_space": "录制机屏幕逻辑像素，左上角为原点；分辨率/缩放/窗口位置变化后坐标会失效，须以 screenshots/ 下截图重新定位",
                "runtime": "桌面 GUI 自动化（鼠标点击 / 键盘输入注入），需在目标应用已打开的桌面会话中执行",
                "fallback": "坐标或意图不可靠的步骤，以对应 step-NN 截图为准重新定位"
            },
            "quality": {
                "total": q.total,
                "intent_vague": q.vague,
                "with_point": q.with_point,
                "with_anchor": q.with_anchor,
                "with_screenshot": q.with_shot
            },
            "steps_summary": {
                "total": recording.total_steps,
                "actions": action_counts,
                "vague_steps": vague_steps
            }
        });
        // steps.json 仅在 machine 非空时落盘，清单据此决定是否引用。
        if !machine.is_empty() {
            manifest["files"]["steps"] = serde_json::json!("steps.json");
        }
        if !inputs.is_empty() {
            manifest["inputs"] = serde_json::json!(inputs);
        }
        if let Ok(text) = serde_json::to_string_pretty(&manifest) {
            let _ = std::fs::write(parent.join("skill.json"), text);
        }
    }
    Ok(output_path.to_string_lossy().to_string())
}

// ── TXT export ────────────────────────────────────────────────

/// 纯文本导出：标题 + 元信息 + 编号步骤（标题 / 描述 / 提示）。
/// 文本导出不含截图，故忽略 crop_set。
fn export_txt(recording: &ExportRecording, steps: &[ExportStep]) -> String {
    let mut out = String::new();
    out.push_str(&recording.title);
    out.push('\n');
    out.push_str(&format!(
        "应用：{}　步骤数：{}　创建于：{}",
        recording.app_name, recording.total_steps, recording.created_at
    ));
    out.push_str("\n\n");
    for s in steps {
        out.push_str(&format!("步骤 {}：{}\n", s.step_number, s.title));
        if !s.description.is_empty() {
            out.push_str(&s.description);
            out.push('\n');
        }
        if !s.tip.is_empty() {
            out.push_str(&format!("提示：{}\n", s.tip));
        }
        out.push('\n');
    }
    out
}

// ── Long image (stitched) export ──────────────────────────────

/// 把所有步骤的截图（裁图模式下应用裁剪）竖向拼接成一张长图 PNG。
///
/// 布局（从上到下）：
///   1. 教程标题头（深色背景 + 白色大字 + 元信息）
///   2. 每张步骤截图前带步骤头部（编号圆圈 + 标题文字）
///   3. 步骤间 24px 白色间隔
///
/// 统一宽度取各图最大宽度（上限 1440px），各图按比例缩放。
/// 无截图的步骤跳过；整篇都无截图时报错。
fn export_long_image(
    recording: &ExportRecording,
    steps: &[ExportStep],
    output_path: &std::path::Path,
    crop_set: &std::collections::HashSet<&str>,
) -> Result<(), String> {
    const GAP: u32 = 24;
    const MAX_WIDTH: u32 = 1440;
    // ── 教程标题头 ──
    const TITLE_H: u32 = 100; // 标题区高度
    const TITLE_BG: image::Rgba<u8> = image::Rgba([33, 41, 54, 255]); // slate-900
    const TITLE_TEXT: image::Rgba<u8> = image::Rgba([255, 255, 255, 255]);
    const SUBTEXT_COLOR: image::Rgba<u8> = image::Rgba([148, 163, 184, 255]); // slate-400
    // ── 步骤头部 ──
    const HEADER_H: u32 = 96; // 步骤标题头高度（需容纳 30px 编号 + 32px 标题）
    const HEADER_BG: image::Rgba<u8> = image::Rgba([248, 250, 252, 255]); // #f8fafc
    const BADGE_COLOR: image::Rgba<u8> = image::Rgba([99, 102, 241, 255]); // indigo-500
    const TEXT_COLOR: image::Rgba<u8> = image::Rgba([30, 41, 59, 255]); // slate-800
    const STEP_NUM_COLOR: image::Rgba<u8> = image::Rgba([255, 255, 255, 255]);

    let font = get_font();

    // 1) 准备每个步骤的最终位图（按需裁剪）+ 收集标题信息
    let mut prepared: Vec<image::DynamicImage> = Vec::new();
    let mut headers: Vec<(i32, String, String)> = Vec::new(); // (step_number, action_type, title)
    for s in steps {
        if s.screenshot_path.is_empty() {
            continue;
        }
        let raw = match std::fs::read(&s.screenshot_path) {
            Ok(b) if !b.is_empty() => b,
            _ => continue,
        };
        let bytes = if crop_set.contains(s.id.as_str()) {
            maybe_crop_screenshot_bytes(raw, s.x, s.y, s.crop_w, s.crop_h, &s.action_type).0
        } else {
            raw
        };
        match image::load_from_memory(&bytes) {
            Ok(img) => {
                headers.push((s.step_number, s.action_type.clone(), s.title.clone()));
                prepared.push(img);
            }
            Err(_) => continue,
        }
    }
    if prepared.is_empty() {
        return Err("该教程没有任何截图，无法导出长图".to_string());
    }

    // 2) 统一宽度
    let target_w: u32 = prepared
        .iter()
        .map(|i| i.width())
        .max()
        .unwrap_or(1)
        .min(MAX_WIDTH)
        .max(1);

    let scaled: Vec<image::DynamicImage> = prepared
        .iter()
        .map(|img| {
            let ow = img.width() as f64;
            let oh = img.height() as f64;
            let nh = if ow <= 0.0 {
                1
            } else {
                (oh * target_w as f64 / ow).round().max(1.0) as u32
            };
            img.resize_exact(target_w, nh, image::imageops::FilterType::Lanczos3)
        })
        .collect();

    // 3) 构建教程标题头画布
    let mut title_canvas =
        image::ImageBuffer::<image::Rgba<u8>, Vec<u8>>::from_pixel(target_w, TITLE_H, TITLE_BG);

    if let Some(ref f) = font {
        // 教程标题（大字，28px）
        let display_title = if recording.title.is_empty() {
            "未命名教程".to_string()
        } else {
            recording.title.clone()
        };
        let scale_title = ab_glyph::PxScale { x: 28.0, y: 28.0 };
        imageproc::drawing::draw_text_mut(
            &mut title_canvas, TITLE_TEXT, 24, 28, scale_title, f, &display_title,
        );
        // 元信息行（小字，灰色）：应用名 · 步骤数 · 日期
        let meta = format!(
            "{} · {} 步骤 · {}",
            recording.app_name, recording.total_steps, recording.created_at
        );
        let scale_meta = ab_glyph::PxScale { x: 14.0, y: 14.0 };
        imageproc::drawing::draw_text_mut(
            &mut title_canvas, SUBTEXT_COLOR, 24, 58, scale_meta, f, &meta,
        );
    } else {
        // 无字体时用像素块写个简易标记（极端降级）
        for y in 20u32..50 {
            for x in 24u32..(target_w - 24).min(500) {
                title_canvas.put_pixel(x, y, TITLE_TEXT);
            }
        }
    }
    let title_img = image::DynamicImage::ImageRgba8(title_canvas);

    // 4) 为每张截图生成带步骤标题的头部图像
    let mut final_images: Vec<image::DynamicImage> = Vec::new();

    for (idx, img) in scaled.iter().enumerate() {
        let (step_num, action_type, title) = &headers[idx];

        let mut header_canvas =
            image::ImageBuffer::<image::Rgba<u8>, Vec<u8>>::from_pixel(target_w, HEADER_H, HEADER_BG);

        // 底部细线分隔
        for x in 0..target_w {
            header_canvas.put_pixel(x, HEADER_H - 1, image::Rgba([226, 232, 240, 255])); // #e2e8f0
        }

        // 编号圆圈（indigo 实心圆）
        let badge_radius: i32 = 30;
        let badge_cx: i32 = 48;
        let badge_cy: i32 = HEADER_H as i32 / 2;
        for y in (badge_cy - badge_radius)..=(badge_cy + badge_radius) {
            for x in (badge_cx - badge_radius)..=(badge_cx + badge_radius) {
                let dx = x - badge_cx;
                let dy = y - badge_cy;
                if dx * dx + dy * dy <= badge_radius * badge_radius {
                    let ux = x as u32;
                    let uy = y as u32;
                    if ux < target_w && uy < HEADER_H {
                        header_canvas.put_pixel(ux, uy, BADGE_COLOR);
                    }
                }
            }
        }

        // 圆圈内编号（白色）。用 text_size 精确居中，避免固定偏移把白字
        // 画到圆圈外的浅色背景上（白底白字 → 步骤号"看不清"）。
        if let Some(ref f) = font {
            let num_str = step_num.to_string();
            let scale = ab_glyph::PxScale { x: 32.0, y: 32.0 };
            let (num_w, num_h) = imageproc::drawing::text_size(scale, f, &num_str);
            let num_x = badge_cx - num_w as i32 / 2;
            let num_y = badge_cy - num_h as i32 / 2;
            imageproc::drawing::draw_text_mut(
                &mut header_canvas, STEP_NUM_COLOR, num_x, num_y, scale, f, &num_str,
            );
        }

        // 标题文本（32px，垂直居中于头部条）
        if let Some(ref f) = font {
            let display_text = if title.is_empty() {
                if action_type.is_empty() {
                    "步骤".to_string()
                } else {
                    to_action_label(action_type)
                }
            } else {
                title.clone()
            };
            let scale = ab_glyph::PxScale { x: 32.0, y: 32.0 };
            let text_x: i32 = 92; // 圆圈右边距
            let (_, text_h) = imageproc::drawing::text_size(scale, f, &display_text);
            let text_y: i32 = badge_cy - text_h as i32 / 2;
            imageproc::drawing::draw_text_mut(
                &mut header_canvas, TEXT_COLOR, text_x, text_y, scale, f, &display_text,
            );
        }

        let header_img = image::DynamicImage::ImageRgba8(header_canvas);

        // 头部 + 截图 垂直拼接
        let combined_h = HEADER_H + img.height();
        let mut combined = image::ImageBuffer::<image::Rgba<u8>, Vec<u8>>::from_pixel(
            target_w,
            combined_h,
            image::Rgba([255, 255, 255, 255]),
        );
        let header_rgba = header_img.to_rgba8();
        let img_rgba = img.to_rgba8();
        image::imageops::overlay(&mut combined, &header_rgba, 0, 0);
        image::imageops::overlay(&mut combined, &img_rgba, 0, HEADER_H as i64);

        final_images.push(image::DynamicImage::ImageRgba8(combined));
    }

    // 5) 计算总高度（标题头 + 所有步骤头图 + 间隔）并创建最终画布
    let total_h: u32 = TITLE_H
        + final_images.iter().map(|i| i.height()).sum::<u32>()
        + GAP * (final_images.len().saturating_sub(1)) as u32
        + GAP; // 标题头下方也加间隔

    let mut canvas = image::ImageBuffer::<image::Rgba<u8>, Vec<u8>>::from_pixel(
        target_w,
        total_h,
        image::Rgba([255u8, 255u8, 255u8, 255u8]),
    );

    // 先画标题头
    let title_rgba = title_img.to_rgba8();
    image::imageops::overlay(&mut canvas, &title_rgba, 0, 0);
    let mut y: u32 = TITLE_H + GAP;

    // 再逐张画步骤
    for (i, img) in final_images.iter().enumerate() {
        let rgba = img.to_rgba8();
        image::imageops::overlay(&mut canvas, &rgba, 0, y as i64);
        y += img.height();
        if i + 1 < final_images.len() {
            y += GAP;
        }
    }

    canvas
        .save(output_path)
        .map_err(|e| format!("保存长图失败: {}", e))?;
    Ok(())
}

/// 将英文 action_type 转为中文显示标签
fn to_action_label(action_type: &str) -> String {
    match action_type {
        "click" => "点击".to_string(),
        "scroll" => "滚动".to_string(),
        "input" | "type" => "输入".to_string(),
        "key" | "keydown" => "按键".to_string(),
        "paste" => "粘贴".to_string(),
        "drag" => "拖拽".to_string(),
        "wait" => "等待".to_string(),
        "navigate" => "导航".to_string(),
        "voice" => "语音".to_string(),
        other => other.to_string(),
    }
}

// ── DOCX export ──────────────────────────────────────────────

/// 多行描述 → 单个 Run：按 \n 拆行，行间插 TextWrapping 软换行
/// （Word 的 Shift+Enter）。docx XML 里字面 \n 会被 Word 折叠成一行，
/// 必须显式 break。空行只产生 break 不产生空 text 节点。
fn build_docx_desc_run(description: &str) -> Run {
    let mut run = Run::new().size(21);
    for (i, line) in description.split('\n').enumerate() {
        if i > 0 {
            run = run.add_break(BreakType::TextWrapping);
        }
        if !line.is_empty() {
            run = run.add_text(line);
        }
    }
    run
}

/// Generate a self-contained Word document (.docx) with embedded screenshots.
/// Scribe-style layout: title → metadata → step cards with header, description,
/// inline screenshot, and optional tip.
fn export_docx(
    recording: &ExportRecording,
    steps: &[ExportStep],
    output_path: &std::path::Path,
    crop_set: &std::collections::HashSet<&str>,
) -> Result<(), String> {
    const MAX_IMG_WIDTH_EMU: usize = 5_715_000; // ~600px at 96 DPI

    let mut docx = Docx::new();

    // ── Title ─────────────────────────────────────────────────
    docx = docx.add_paragraph(
        Paragraph::new()
            .add_run(Run::new().add_text(&recording.title).bold().size(36))
    );

    // ── Metadata ──────────────────────────────────────────────
    let meta = build_docx_meta_text(&recording.app_name, recording.total_steps, &recording.created_at);
    docx = docx.add_paragraph(
        Paragraph::new()
            .add_run(Run::new().add_text(meta).color("888888").size(20))
    );

    // ── Blank line ────────────────────────────────────────────
    docx = docx.add_paragraph(Paragraph::new());

    // ── Steps ─────────────────────────────────────────────────
    // Compute image sizes: read image dimensions & scale proportionally
    let mut img_dims: Vec<Option<(usize, usize)>> = Vec::with_capacity(steps.len());
    for s in steps {
        if s.screenshot_path.is_empty() {
            img_dims.push(None);
        } else {
            let dim = std::fs::read(&s.screenshot_path)
                .ok()
                .and_then(|data| image::load_from_memory(&data).ok())
                .map(|img| (img.width() as usize, img.height() as usize));
            img_dims.push(dim);
        }
    }

    for (s, dim_opt) in steps.iter().zip(img_dims.iter()) {
        // ── Step header ───────────────────────────────────────
        let header = build_docx_step_header(s.step_number, &s.action_type, &s.title);
        docx = docx.add_paragraph(
            Paragraph::new()
                .add_run(Run::new().add_text(header).bold().size(24))
        );

        // ── Description ───────────────────────────────────────
        if !s.description.is_empty() {
            docx = docx.add_paragraph(Paragraph::new().add_run(build_docx_desc_run(&s.description)));
        }

        // ── Screenshot ────────────────────────────────────────
        if !s.screenshot_path.is_empty() {
            if let Ok(img_bytes) = std::fs::read(&s.screenshot_path) {
                let is_crop = crop_set.contains(s.id.as_str());
                // ⚠️ 尺寸必须跟着 final_bytes 走：裁剪后原图尺寸就失效了
                let (final_bytes, final_dims) = if is_crop && !img_bytes.is_empty() {
                    maybe_crop_screenshot_bytes(img_bytes, s.x, s.y, s.crop_w, s.crop_h, &s.action_type)
                } else {
                    (img_bytes, *dim_opt)
                };
                let (w_emu, h_emu) = compute_image_emu_dims(final_dims, MAX_IMG_WIDTH_EMU);
                let pic = Pic::new(&final_bytes).size(w_emu, h_emu);
                docx = docx.add_paragraph(
                    Paragraph::new()
                        .add_run(Run::new().add_image(pic))
                );
            } else {
                docx = docx.add_paragraph(
                    Paragraph::new()
                        .add_run(Run::new().add_text("(Screenshot not found)").color("aaaaaa").size(20))
                );
            }
        } else {
            docx = docx.add_paragraph(
                Paragraph::new()
                    .add_run(Run::new().add_text("(No screenshot)").color("aaaaaa").size(20))
            );
        }

        // ── Tip ───────────────────────────────────────────────
        if !s.tip.is_empty() {
            docx = docx.add_paragraph(
                Paragraph::new()
                    .add_run(
                        Run::new()
                            .add_text(format!("Tip: {}", s.tip))
                            .color("0066cc")
                            .size(20),
                    )
            );
        }

        // ── Blank spacer ──────────────────────────────────────
        docx = docx.add_paragraph(Paragraph::new());
    }

    // ── Write to file ─────────────────────────────────────────
    let file = std::fs::File::create(output_path)
        .map_err(|e| format!("Failed to create DOCX file: {}", e))?;

    docx
        .build()
        .pack(file)
        .map_err(|e| format!("Failed to write DOCX: {}", e))?;

    Ok(())
}

// ── PDF export ────────────────────────────────────────────────

/// Edge 无头渲染 PDF 的超时上限（进程级，避免大 HTML 卡死导出线程）。
const EDGE_PDF_TIMEOUT: Duration = Duration::from_secs(60);

/// 定位 Microsoft Edge 可执行文件（PDF 渲染引擎）。
///
/// 候选顺序：ProgramFiles / ProgramFiles(x86) / LOCALAPPDATA 环境变量拼接
/// → 常见固定路径（适配系统盘非 C: 或用户级安装）。
fn resolve_edge_path() -> Option<std::path::PathBuf> {
    let mut candidates: Vec<std::path::PathBuf> = Vec::new();
    for var in ["ProgramFiles", "ProgramFiles(x86)", "LOCALAPPDATA"] {
        if let Ok(base) = std::env::var(var) {
            candidates.push(
                std::path::PathBuf::from(base).join("Microsoft/Edge/Application/msedge.exe"),
            );
        }
    }
    candidates.push(std::path::PathBuf::from(
        r"C:\Program Files\Microsoft\Edge\Application\msedge.exe",
    ));
    candidates.push(std::path::PathBuf::from(
        r"C:\Program Files (x86)\Microsoft\Edge\Application\msedge.exe",
    ));
    candidates.into_iter().find(|p| p.exists())
}

/// Generate a PDF by rendering the HTML export via Microsoft Edge headless.
/// The HTML (with base64-embedded screenshots) is written to a temp file,
/// then converted to a self-contained PDF. No printpdf, no external deps.
fn export_pdf(
    recording: &ExportRecording,
    steps: &[ExportStep],
    output_path: &std::path::Path,
    crop_set: &std::collections::HashSet<&str>,
) -> Result<(), String> {
    if steps.is_empty() {
        return Err("No steps to export".to_string());
    }

    // 1. Generate standalone HTML with base64-embedded images
    let html = gen_html_body(recording, steps, crop_set);

    // 2. Write HTML to a temporary file
    let temp_dir = std::env::temp_dir();
    let html_path = temp_dir.join(format!("flowio_pdf_{}.html", recording.id));
    std::fs::write(&html_path, html.as_bytes())
        .map_err(|e| format!("Failed to write temp HTML: {}", e))?;

    // 3. Convert to PDF using Microsoft Edge headless
    let edge_path = resolve_edge_path()
        .ok_or_else(|| "未找到 Microsoft Edge，无法导出 PDF".to_string())?;
    let html_url = format!(
        "file:///{}",
        html_path.to_string_lossy().replace('\\', "/")
    );

    // spawn + try_wait 轮询实现超时：Edge 无头渲染大 HTML 可能卡死，
    // 进程必须能被回收而不是无限阻塞导出线程。
    let mut child = std::process::Command::new(&edge_path)
        .args([
            "--headless=new",
            "--disable-gpu",
            "--no-sandbox",
            "--no-first-run",
            "--disable-extensions",
            "--no-pdf-header-footer",
            &format!("--print-to-pdf={}", output_path.to_string_lossy()),
            &html_url,
        ])
        .stderr(std::process::Stdio::piped())
        .spawn()
        .map_err(|e| format!("Failed to launch Edge: {}", e))?;

    let start = Instant::now();
    let mut timed_out = false;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) => {
                if start.elapsed() > EDGE_PDF_TIMEOUT {
                    let _ = child.kill();
                    timed_out = true;
                    break child
                        .wait()
                        .map_err(|e| format!("Edge wait failed: {}", e))?;
                }
                std::thread::sleep(Duration::from_millis(100));
            }
            Err(e) => return Err(format!("Edge wait failed: {}", e)),
        }
    };

    // 4. Collect Edge stderr（渲染失败信息）+ Clean up temp HTML (best effort)
    let mut stderr = String::new();
    if let Some(mut handle) = child.stderr.take() {
        use std::io::Read;
        let _ = handle.read_to_string(&mut stderr);
    }
    let _ = std::fs::remove_file(&html_path);

    if timed_out {
        return Err(format!(
            "PDF 导出超时：Microsoft Edge 未在 {} 秒内完成渲染，已终止",
            EDGE_PDF_TIMEOUT.as_secs()
        ));
    }

    if !status.success() {
        return Err(format!("Edge PDF conversion failed: {}", stderr.trim()));
    }

    Ok(())
}

// ── Extracted pure logic functions ────────────────────────────

/// Build a single step card header for HTML export.
fn build_step_header_html(step_number: i32, action_type: &str, title: &str) -> String {
    format!(
        "Step {}: <span class=\"step-type\">{}</span> {}",
        step_number,
        htmlescape(action_type),
        htmlescape(title),
    )
}

/// Build the description block for HTML step card (empty → empty string).
/// 多行描述：htmlescape 后把 \n 转为 <br />，保住粘贴文本的换行格式。
fn build_step_desc_html(description: &str) -> String {
    if description.is_empty() {
        String::new()
    } else {
        format!(
            "      <p class=\"step-desc\">{}</p>\n",
            htmlescape(description).replace('\n', "<br />")
        )
    }
}

/// Build the tip block for HTML step card (empty → empty string).
fn build_step_tip_html(tip: &str) -> String {
    if tip.is_empty() {
        String::new()
    } else {
        format!(
            "      <div class=\"step-tip\"><strong>Tip:</strong> {}</div>\n",
            htmlescape(tip)
        )
    }
}

/// Build the "no screenshot" placeholder div.
fn build_screenshot_placeholder_html() -> &'static str {
    "<div class=\"screenshot-placeholder\">No screenshot</div>"
}

/// Build the `<img>` tag for an embedded screenshot with base64 data.
fn build_screenshot_img_html(mime: &str, base64_data: &str, step_number: i32) -> String {
    format!(
        "<img class=\"screenshot\" src=\"data:{};base64,{}\" \
         alt=\"Step {} screenshot\" />",
        mime, base64_data, step_number
    )
}

/// Build the full HTML document with the embedded CSS and step cards.
fn build_html_document(title: &str, total_steps: i32, created_at: &str, step_cards: &str) -> String {
    format!(
        r#"<!DOCTYPE html>
<html lang="zh-CN">
<head>
<meta charset="UTF-8">
<meta name="viewport" content="width=device-width, initial-scale=1.0">
<title>{title}</title>
<style>
  *,*::before,*::after{{box-sizing:border-box;margin:0;padding:0}}
  body{{font-family:-apple-system,BlinkMacSystemFont,"Segoe UI","Microsoft YaHei","PingFang SC","Noto Sans CJK SC","SimSun",Helvetica,Arial,sans-serif;background:#f6f8fa;color:#24292f;line-height:1.6}}
  .header{{background:#24292f;color:#fff;padding:32px 24px 24px;border-bottom:3px solid #0969da}}
  .header h1{{font-size:1.75rem;font-weight:600;margin-bottom:8px}}
  .header .meta{{font-size:0.875rem;opacity:0.75;display:flex;gap:16px;flex-wrap:wrap}}
  .container{{max-width:900px;margin:0 auto;padding:24px 16px}}
  .step-card{{background:#fff;border:1px solid #d0d7de;border-radius:8px;padding:20px;margin-bottom:16px;box-shadow:0 1px 3px rgba(0,0,0,0.04)}}
  .step-header{{font-size:1.05rem;font-weight:600;margin-bottom:8px;color:#1f2328}}
  .step-type{{font-size:0.8rem;color:#656d76;background:#f6f8fa;padding:2px 10px;border-radius:4px;border:1px solid #d0d7de;margin-right:8px;font-weight:normal}}
  .step-desc{{font-size:0.925rem;color:#656d76;margin-bottom:8px}}
  .step-tip{{font-size:0.85rem;color:#0969da;background:#ddf4ff;padding:8px 12px;border-radius:6px;margin-bottom:12px;border-left:3px solid #0969da}}
  .screenshot{{max-width:100%;border-radius:6px;border:1px solid #d0d7de;display:block}}
  .screenshot-placeholder{{color:#8c959f;font-style:italic;padding:24px;background:#f6f8fa;border-radius:6px;text-align:center;border:1px dashed #d0d7de}}
</style>
</head>
<body>
<div class="header">
  <h1>{title}</h1>
  <div class="meta">
    <span>Steps: {total_steps}</span>
    <span>Date: {created_at}</span>
  </div>
</div>
<div class="container">
{step_cards}
</div>
</body>
</html>"#,
        title = htmlescape(title),
        total_steps = total_steps,
        created_at = created_at,
        step_cards = step_cards,
    )
}

/// Build a step header for Markdown export.
fn build_md_step_header(step_number: i32, action_type: &str, title: &str) -> String {
    format!(
        "## Step {} · `{}` · {}",
        step_number, action_type, title
    )
}

/// Build the description block for Markdown (empty → empty string).
/// 段内单换行转 GFM 硬换行（行尾两空格），防止 Markdown 渲染时折叠成一行。
fn build_md_desc_block(description: &str) -> String {
    if description.is_empty() {
        String::new()
    } else {
        format!("{}\n\n", description.replace('\n', "  \n"))
    }
}

/// Build the tip block for Markdown (empty → empty string).
fn build_md_tip_block(tip: &str) -> String {
    if tip.is_empty() {
        String::new()
    } else {
        format!("> **Tip:** {}\n", tip)
    }
}

/// Build the full Markdown document.
fn build_md_document(title: &str, app_name: &str, total_steps: i32, created_at: &str, step_list: &str) -> String {
    format!(
        "# {title}\n\n\
         **App:** {app_name}  ·  **Steps:** {total_steps}  ·  **Date:** {created_at}\n\n\
         ---\n\n\
         {step_list}",
        title = title,
        app_name = app_name,
        total_steps = total_steps,
        created_at = created_at,
        step_list = step_list,
    )
}

/// Build a step header for DOCX export.
fn build_docx_step_header(step_number: i32, action_type: &str, title: &str) -> String {
    format!("Step {} · {} · {}", step_number, action_type, title)
}

/// Build the metadata text line for DOCX export.
fn build_docx_meta_text(app_name: &str, total_steps: i32, created_at: &str) -> String {
    format!("App: {}  ·  Steps: {}  ·  Date: {}", app_name, total_steps, created_at)
}

/// Compute the DOCX image dimensions in EMU (English Metric Units).
/// `dim` is `None` when no screenshot exists; `max_width_emu` is the fallback width.
const EMU_PER_PX: usize = 9525;
const MAX_IMG_PX: usize = 600;

fn compute_image_emu_dims(dim: Option<(usize, usize)>, max_width_emu: usize) -> (u32, u32) {
    match dim {
        Some((w, h)) if w > 0 && h > 0 && w > MAX_IMG_PX => {
            let ratio = MAX_IMG_PX as f64 / w as f64;
            ((MAX_IMG_PX * EMU_PER_PX) as u32, (h as f64 * ratio * EMU_PER_PX as f64) as u32)
        }
        Some((w, h)) if w > 0 && h > 0 => {
            ((w * EMU_PER_PX) as u32, (h * EMU_PER_PX) as u32)
        }
        // 读不到尺寸、或宽高为 0（Word 里会渲染异常）：走兜底比例
        _ => (max_width_emu as u32, (max_width_emu * 3 / 4) as u32),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 真实库 smoke（#[ignore]，手动跑）：把 `%APPDATA%/com.flowio.app/flowio.db`
    /// 复制到 `D:/tmp/flowio_real.db` 后执行
    /// `cargo test --lib skill_md_real_db_smoke -- --ignored --nocapture`，
    /// 会导出固定版/变量版两个捆绑包到 `D:/tmp/skill_fixed/`、`D:/tmp/skill_variable/`
    /// （SKILL.md + screenshots/ 截图目录）。
    /// 顺带验证迁移幂等：对真实库副本跑 init_db 后再读新列。
    #[test]
    #[ignore]
    fn skill_md_real_db_smoke() {
        let path = std::env::var("FLOWIO_REAL_DB")
            .unwrap_or_else(|_| r"D:\tmp\flowio_real.db".to_string());
        let conn = rusqlite::Connection::open_with_flags(
            &path,
            rusqlite::OpenFlags::SQLITE_OPEN_READ_WRITE,
        )
        .expect("open real db copy");
        crate::db::init_db(&conn); // 幂等迁移：补出 is_variable / param_name 列

        let rid: String = match std::env::var("FLOWIO_REAL_REC_ID") {
            Ok(id) if !id.trim().is_empty() => id.trim().to_string(),
            _ => conn
                .query_row(
                    // 取「步数最多」而不是「最新」：最新的往往是 1 步试录，
                    // 生成出来的样例看不出真实教程的数据完备度。
                    "SELECT r.id FROM recordings r JOIN steps s ON s.recording_id = r.id \
                     WHERE r.total_steps > 0 AND r.id IN \
                     (SELECT DISTINCT recording_id FROM steps WHERE action_type = 'type') \
                     GROUP BY r.id ORDER BY COUNT(s.id) DESC LIMIT 1",
                    [],
                    |r| r.get(0),
                )
                .or_else(|_| {
                    conn.query_row(
                        "SELECT id FROM recordings WHERE total_steps > 0 ORDER BY created_at DESC LIMIT 1",
                        [],
                        |r| r.get(0),
                    )
                })
                .expect("pick recording"),
        };
        let (title, app_name, total_steps, created_at): (String, String, i32, String) = conn
            .query_row(
                "SELECT title, app_name, total_steps, created_at FROM recordings WHERE id = ?1",
                params![rid],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
            )
            .unwrap();
        let mut steps: Vec<ExportStep> = Vec::new();
        let mut stmt = conn
            .prepare(
                "SELECT id, order_index, step_number, action_type, title, description, \
                 tip, screenshot_path, x, y, crop_w, crop_h, is_variable, param_name \
                 FROM steps WHERE recording_id = ?1 ORDER BY order_index",
            )
            .unwrap();
        let rows = stmt.query_map(params![rid], |row| {
            Ok(ExportStep {
                id: row.get(0)?,
                order_index: row.get(1)?,
                step_number: row.get(2)?,
                action_type: row.get(3)?,
                title: row.get(4)?,
                description: row.get(5)?,
                tip: row.get(6)?,
                screenshot_path: row.get(7)?,
                x: row.get(8)?,
                y: row.get(9)?,
                crop_w: row.get(10)?,
                crop_h: row.get(11)?,
                is_variable: row.get::<_, i64>(12)? != 0,
                param_name: row.get(13)?,
            })
        }).unwrap();
        for r in rows {
            steps.push(r.unwrap());
        }
        let recording = ExportRecording { id: rid.clone(), title: title.clone(), app_name, total_steps, created_at };

        // 固定版（走 bundle：截图复制到 skill_fixed/screenshots/ 并在步骤下引用）
        let fixed = export_skill_bundle(
            std::path::Path::new(r"D:\tmp\skill_fixed\SKILL.md"),
            &recording,
            &steps,
            &std::collections::HashSet::new(),
            &mut load_skill_machine(&conn, &rid),
        )
        .unwrap();
        println!("== fixed ==\n{}", &fixed);

        // 变量版：把第一个输入步骤标为变量（模拟作者在编辑器里点「变量」）
        if let Some(s) = steps.iter_mut().find(|s| s.action_type == "type") {
            s.is_variable = true;
            s.param_name = "order_id".into();
        }
        let variable = export_skill_bundle(
            std::path::Path::new(r"D:\tmp\skill_variable\SKILL.md"),
            &recording,
            &steps,
            &std::collections::HashSet::new(),
            &mut load_skill_machine(&conn, &rid),
        )
        .unwrap();
        println!("== variable ==\n{}", &variable);
    }

    /// 单元验证：export_skill_bundle 除 SKILL.md / steps.json 外，还应落盘
    /// skill.json 自描述清单（route ②「MCP 技能包」的注册元数据）——让外部
    /// Agent / MCP host 无需解析 SKILL.md 即可登记并执行这个 skill。
    #[test]
    fn export_skill_bundle_writes_manifest() {
        let recording = ExportRecording {
            id: "r1".into(),
            title: "XLOOKUP step by step".into(),
            app_name: "Excel".into(),
            total_steps: 2,
            created_at: "2026-09-08T18:31:23".into(),
        };
        let steps = vec![
            ExportStep {
                id: "s1".into(),
                order_index: 0,
                step_number: 1,
                action_type: "click".into(),
                title: "点击在「函数参数」中".into(),
                description: String::new(),
                tip: String::new(),
                screenshot_path: String::new(),
                x: Some(100),
                y: Some(200),
                crop_w: None,
                crop_h: None,
                is_variable: false,
                param_name: String::new(),
            },
            ExportStep {
                id: "s2".into(),
                order_index: 1,
                step_number: 2,
                action_type: "type".into(),
                title: "在「单元格」中输入".into(),
                description: "=XLOOKUP(A2,B:B,C:C)".into(),
                tip: String::new(),
                screenshot_path: String::new(),
                x: None,
                y: None,
                crop_w: None,
                crop_h: None,
                is_variable: true,
                param_name: "lookup_value".into(),
            },
        ];
        let mut machine: SkillMachine = SkillMachine::new();
        machine.insert(
            1,
            SkillStepData {
                n: 1,
                action: "click".into(),
                intent: "点击在「函数参数」中".into(),
                intent_vague: true,
                window: "函数参数".into(),
                point: Some([100, 200]),
                ..Default::default()
            },
        );
        machine.insert(
            2,
            SkillStepData {
                n: 2,
                action: "type".into(),
                intent: "在「单元格」中输入".into(),
                intent_vague: true,
                window: "Excel".into(),
                text: Some("=XLOOKUP(A2,B:B,C:C)".into()),
                ..Default::default()
            },
        );
        let dir = std::env::temp_dir().join("flowio_skill_manifest_test");
        let _ = std::fs::create_dir_all(&dir);
        let out = dir.join("SKILL.md");
        let crop_set: std::collections::HashSet<&str> = std::collections::HashSet::new();
        let res = export_skill_bundle(&out, &recording, &steps, &crop_set, &mut machine);
        assert!(res.is_ok(), "export_skill_bundle failed: {:?}", res.err());

        let manifest_path = dir.join("skill.json");
        assert!(manifest_path.exists(), "skill.json should be written");
        let text = std::fs::read_to_string(&manifest_path).unwrap();
        let v: serde_json::Value = serde_json::from_str(&text).expect("skill.json must be valid JSON");

        assert_eq!(v["schema"].as_str(), Some("flowio.skill.manifest/v1"));
        // name 现在是 slug + recording-id 后缀（保证注册唯一，避免中文标题都回退 flowio-skill）
        let name = v["name"].as_str().expect("name should be a string");
        assert!(
            name.starts_with("xlookup-step-by-step-"),
            "name should keep slug prefix, got {}",
            name
        );
        assert!(
            name.ends_with("r1"),
            "name should carry recording-id suffix (recording id = r1), got {}",
            name
        );
        assert_eq!(v["files"]["skill"].as_str(), Some("SKILL.md"));
        assert_eq!(v["files"]["steps"].as_str(), Some("steps.json"));
        assert_eq!(v["files"]["screenshots"].as_str(), Some("screenshots/"));
        assert_eq!(v["execution"]["entry"].as_str(), Some("SKILL.md"));
        assert_eq!(v["quality"]["total"].as_i64(), Some(2));
        assert_eq!(v["quality"]["intent_vague"].as_i64(), Some(2));
        assert_eq!(v["steps_summary"]["total"].as_i64(), Some(2));
        let actions = v["steps_summary"]["actions"].as_object().expect("actions object");
        assert!(actions.contains_key("click"), "action histogram should list click");
        assert!(actions.contains_key("type"), "action histogram should list type");
        // 变量步骤 → 清单应含 inputs（消费方需要填的入参）
        let inputs = v["inputs"].as_array().expect("inputs array");
        assert_eq!(inputs.len(), 1, "one variable step → one input");
        assert_eq!(inputs[0]["name"].as_str(), Some("lookup_value"));
        assert_eq!(inputs[0]["sample"].as_str(), Some("=XLOOKUP(A2,B:B,C:C)"));

        // 清理验证产物，避免污染临时目录
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 本文件源码，供静态防线扫描（编译期嵌入，改完文件 cargo test 立即生效）。
    const COMMANDS_SRC: &str = include_str!("commands.rs");

    #[test]
    fn test_export_pdf_no_screenshots() {
        let rec = ExportRecording {
            id: "test-1".into(),
            title: "测试无截图".into(),
            app_name: "TestApp".into(),
            total_steps: 2,
            created_at: "2026-08-09".into(),
        };
        let steps = vec![
            ExportStep {
                id: "test-step".into(),
                order_index: 0,
                step_number: 1,
                action_type: "click".into(),
                title: "第一步".into(),
                description: "这是第一步描述".into(),
                tip: "".into(),
                screenshot_path: "".into(),
                x: None,
                y: None,
                crop_w: None,
                crop_h: None,
                        is_variable: false,
                param_name: String::new(),
            },
            ExportStep {
                id: "test-step".into(),
                order_index: 1,
                step_number: 2,
                action_type: "type".into(),
                title: "第二步".into(),
                description: "这是第二步描述\n包含换行".into(),
                tip: "一个提示".into(),
                screenshot_path: "".into(),
                x: None,
                y: None,
                crop_w: None,
                crop_h: None,
                        is_variable: false,
                param_name: String::new(),
            },
        ];
        let out = std::env::temp_dir().join("flowio_test_no_ss.pdf");
        let crop_set: std::collections::HashSet<&str> = std::collections::HashSet::new();
        let result = export_pdf(&rec, &steps, &out, &crop_set);
        assert!(result.is_ok(), "export_pdf failed: {:?}", result.err());
        assert!(out.exists(), "PDF file not created");
        let size = std::fs::metadata(&out).unwrap().len();
        assert!(size > 100, "PDF too small: {} bytes", size);
        println!("Test PDF (no screenshots): {} bytes at {:?}", size, out);
    }

    #[test]
    fn test_export_pdf_empty_steps() {
        let rec = ExportRecording {
            id: "test-2".into(),
            title: "空步骤".into(),
            app_name: "TestApp".into(),
            total_steps: 0,
            created_at: "2026-08-09".into(),
        };
        let empty: std::collections::HashSet<&str> = std::collections::HashSet::new();
        let result = export_pdf(&rec, &[], std::path::Path::new("unused.pdf"), &empty);
        assert!(result.is_err());
    }

    #[test]
    fn test_sanitize_filename() {
        assert_eq!(sanitize_filename("正常文件名"), "正常文件名");
        assert_eq!(sanitize_filename("a/b\\c:d*e?f\"g<h>i|j"), "a_b_c_d_e_f_g_h_i_j");
        assert_eq!(sanitize_filename(""), "");
        assert_eq!(sanitize_filename("report:2026"), "report_2026");
        assert_eq!(sanitize_filename("..."), "...");
    }

    #[test]
    fn test_htmlescape() {
        assert_eq!(htmlescape("plain"), "plain");
        assert_eq!(htmlescape("<b>&\"'\""), "&lt;b&gt;&amp;&quot;&#39;&quot;");
        assert_eq!(htmlescape(""), "");
    }

    #[test]
    fn test_mime_from_path() {
        assert_eq!(mime_from_path("a.png"), "image/png");
        assert_eq!(mime_from_path("a.PNG"), "image/png");
        assert_eq!(mime_from_path("a.jpg"), "image/jpeg");
        assert_eq!(mime_from_path("a.jpeg"), "image/jpeg");
        assert_eq!(mime_from_path("a.gif"), "image/gif");
        assert_eq!(mime_from_path("a.webp"), "image/webp");
        assert_eq!(mime_from_path("a.bmp"), "image/bmp");
        assert_eq!(mime_from_path("a.unknown"), "image/png"); // 未知扩展名默认 png
        assert_eq!(mime_from_path("noext"), "image/png");
    }

    #[test]
    fn test_gen_html_body_escapes_and_placeholder() {
        let rec = ExportRecording {
            id: "r".into(),
            title: "标题 <script>alert(1)</script>".into(),
            app_name: "App".into(),
            total_steps: 1,
            created_at: "2026-08-09".into(),
        };
        let steps = vec![ExportStep {
            id: "test-step".into(),
            order_index: 0,
            step_number: 1,
            action_type: "click".into(),
            title: "标题 & 内容".into(),
            description: "描述 <b>加粗</b> \"引号\"".into(),
            tip: "提示 '单引号'".into(),
            screenshot_path: "".into(),
            x: None,
            y: None,
            crop_w: None,
            crop_h: None,
                    is_variable: false,
            param_name: String::new(),
        }];
        let crop_set: std::collections::HashSet<&str> = std::collections::HashSet::new();
        let html = gen_html_body(&rec, &steps, &crop_set);
        assert!(html.contains("No screenshot"));
        assert!(html.contains("&lt;script&gt;"));
        assert!(html.contains("&amp;"));
        assert!(html.contains("&lt;b&gt;"));
        assert!(html.contains("&quot;"));
        assert!(html.contains("&#39;"));
        assert!(html.contains("Step 1"));
    }

    #[test]
    fn test_export_markdown_no_screenshot() {
        let rec = ExportRecording {
            id: "r".into(),
            title: "标题".into(),
            app_name: "App".into(),
            total_steps: 2,
            created_at: "2026-08-09".into(),
        };
        let steps = vec![
            ExportStep {
                id: "test-step".into(),
                order_index: 0,
                step_number: 1,
                action_type: "click".into(),
                title: "第一步".into(),
                description: "描述".into(),
                tip: "".into(),
                screenshot_path: "".into(),
                x: None,
                y: None,
                crop_w: None,
                crop_h: None,
                        is_variable: false,
                param_name: String::new(),
            },
            ExportStep {
                id: "test-step".into(),
                order_index: 1,
                step_number: 2,
                action_type: "type".into(),
                title: "第二步".into(),
                description: "".into(),
                tip: "提示".into(),
                screenshot_path: "".into(),
                x: None,
                y: None,
                crop_w: None,
                crop_h: None,
                        is_variable: false,
                param_name: String::new(),
            },
        ];
        let ss_dir = std::env::temp_dir().join("flowio_test_md_ss");
        let crop_set: std::collections::HashSet<&str> = std::collections::HashSet::new();
        let md = export_markdown(&rec, &steps, &ss_dir, &crop_set);
        assert!(md.contains("# 标题"));
        assert!(md.contains("**App:** App"));
        assert!(md.contains("## Step 1"));
        assert!(md.contains("## Step 2"));
        // MD 导出不含任何图片引用
        assert!(!md.contains("![Screenshot]"));
        assert!(!md.contains("No screenshot"));
        assert!(!md.contains("Screenshot not found"));
        assert!(md.contains("> **Tip:** 提示"));
    }

    #[test]
    fn test_export_markdown_ignores_screenshot_files() {
        // MD 导出不再复制或引用任何截图文件，即使 screenshot_path 指向不存在的文件也不报错
        let rec = ExportRecording {
            id: "r2".into(),
            title: "标题2".into(),
            app_name: "App".into(),
            total_steps: 1,
            created_at: "2026-08-09".into(),
        };
        let steps = vec![ExportStep {
            id: "test-step".into(),
            order_index: 0,
            step_number: 1,
            action_type: "click".into(),
            title: "第一步".into(),
            description: "".into(),
            tip: "".into(),
            screenshot_path: "C:/definitely/not/exists/xx.png".into(),
            x: None,
            y: None,
            crop_w: None,
            crop_h: None,
                    is_variable: false,
            param_name: String::new(),
        }];
        let ss_dir = std::env::temp_dir().join("flowio_test_md_ss2");
        let crop_set: std::collections::HashSet<&str> = std::collections::HashSet::new();
        let md = export_markdown(&rec, &steps, &ss_dir, &crop_set);
        // 不应包含任何图片引用或报错文字
        assert!(md.contains("# 标题2"));
        assert!(!md.contains("Screenshot"));
        assert!(!md.contains("!["));
        // 不应创建任何截图文件
        assert!(!ss_dir.join("step_1.png").exists());
    }

    #[test]
    fn test_export_docx_no_screenshots_writes_file() {
        let rec = ExportRecording {
            id: "docx".into(),
            title: "导出测试".into(),
            app_name: "App".into(),
            total_steps: 1,
            created_at: "2026-08-09".into(),
        };
        let steps = vec![ExportStep {
            id: "test-step".into(),
            order_index: 0,
            step_number: 1,
            action_type: "click".into(),
            title: "第一步".into(),
            description: "描述".into(),
            tip: "提示".into(),
            screenshot_path: "".into(),
            x: None,
            y: None,
            crop_w: None,
            crop_h: None,
                    is_variable: false,
            param_name: String::new(),
        }];
        let out = std::env::temp_dir().join("flowio_test_export.docx");
        let crop_set: std::collections::HashSet<&str> = std::collections::HashSet::new();
        export_docx(&rec, &steps, &out, &crop_set).unwrap();
        assert!(out.exists());
        let size = std::fs::metadata(&out).unwrap().len();
        assert!(size > 100);
        let _ = std::fs::remove_file(&out);
    }


    fn write_tmp_png(dir: &std::path::Path, name: &str) -> std::path::PathBuf {
        let path = dir.join(name);
        let img = image::DynamicImage::new_rgb8(8, 8);
        img.save(&path).unwrap();
        path
    }

    #[test]
    fn test_sanitize_filename_boundaries() {
        assert_eq!(sanitize_filename("a:b"), "a_b");
        assert_eq!(sanitize_filename("中文：冒号"), "中文：冒号"); // 全角冒号保留
        assert_eq!(sanitize_filename("CON"), "CON_"); // B7: Windows 保留名追加 _ 规避拒写
        assert_eq!(sanitize_filename("prn.txt"), "prn.txt_");
        assert_eq!(sanitize_filename("aux_reports"), "aux_reports"); // 非精确保留名不动
        assert_eq!(sanitize_filename("COM5"), "COM5_");
        assert_eq!(sanitize_filename("正常报告.txt"), "正常报告.txt");
        assert_eq!(sanitize_filename(" . "), " . ");
        let long = "x".repeat(300);
        assert_eq!(sanitize_filename(&long).len(), 300);
    }

    #[test]
    fn test_htmlescape_unicode_and_backtick() {
        assert_eq!(htmlescape("中文😀"), "中文😀");
        assert_eq!(htmlescape("&<>\"'`"), "&amp;&lt;&gt;&quot;&#39;`");
        assert_eq!(htmlescape("a&amp;b"), "a&amp;amp;b");
    }

    #[test]
    fn test_mime_from_path_extra() {
        assert_eq!(mime_from_path("dir.with.dot/file.JPG"), "image/jpeg");
        assert_eq!(mime_from_path("C:\\Path\\to\\shot.PNG"), "image/png");
        assert_eq!(mime_from_path("a.png?x=1"), "image/png");
        assert_eq!(mime_from_path("a.bmp.png"), "image/png");
        assert_eq!(mime_from_path("a.tiff"), "image/png"); // 未识别默认 png
    }

    #[test]
    fn test_gen_html_body_embeds_real_png() {
        let dir = std::env::temp_dir().join(format!("flowio_test_html_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let png = write_tmp_png(&dir, "shot.png");
        let rec = ExportRecording {
            id: "r".into(),
            title: "标题".into(),
            app_name: "App".into(),
            total_steps: 1,
            created_at: "2026-08-09".into(),
        };
        let steps = vec![ExportStep {
            id: "test-step".into(),
            order_index: 0,
            step_number: 1,
            action_type: "click".into(),
            title: "t".into(),
            description: "".into(),
            tip: "".into(),
            screenshot_path: png.to_string_lossy().to_string(),
            x: None,
            y: None,
            crop_w: None,
            crop_h: None,
                    is_variable: false,
            param_name: String::new(),
        }];
        let crop_set: std::collections::HashSet<&str> = std::collections::HashSet::new();
        let html = gen_html_body(&rec, &steps, &crop_set);
        assert!(html.contains("data:image/png;base64,"));
        assert!(html.contains("Step 1"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_export_markdown_no_image_files_created() {
        // MD 导出不再创建任何截图文件，即使步骤有截图路径
        let dir = std::env::temp_dir().join(format!("flowio_test_md3_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let png = write_tmp_png(&dir, "shot.png");
        let rec = ExportRecording {
            id: "r".into(),
            title: "标题".into(),
            app_name: "App".into(),
            total_steps: 1,
            created_at: "2026-08-09".into(),
        };
        let steps = vec![ExportStep {
            id: "test-step".into(),
            order_index: 0,
            step_number: 1,
            action_type: "click".into(),
            title: "t".into(),
            description: "".into(),
            tip: "".into(),
            screenshot_path: png.to_string_lossy().to_string(),
            x: None,
            y: None,
            crop_w: None,
            crop_h: None,
                    is_variable: false,
            param_name: String::new(),
        }];
        let crop_set: std::collections::HashSet<&str> = std::collections::HashSet::new();
        let md = export_markdown(&rec, &steps, &dir, &crop_set);
        // 不应包含图片引用
        assert!(!md.contains("![Screenshot]"));
        assert!(!md.contains("step_1.png"));
        // 不应创建 step_1.png 文件
        assert!(!dir.join("step_1.png").exists());
        // 但应有正常的文本内容
        assert!(md.contains("# 标题"));
        assert!(md.contains("## Step 1"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_export_docx_with_real_screenshot() {
        let dir = std::env::temp_dir().join(format!("flowio_test_docx2_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let png = write_tmp_png(&dir, "shot.png");
        let rec = ExportRecording {
            id: "r".into(),
            title: "标题".into(),
            app_name: "App".into(),
            total_steps: 1,
            created_at: "2026-08-09".into(),
        };
        let steps = vec![ExportStep {
            id: "test-step".into(),
            order_index: 0,
            step_number: 1,
            action_type: "click".into(),
            title: "t".into(),
            description: "".into(),
            tip: "".into(),
            screenshot_path: png.to_string_lossy().to_string(),
            x: None,
            y: None,
            crop_w: None,
            crop_h: None,
                    is_variable: false,
            param_name: String::new(),
        }];
        let out = dir.join("out.docx");
        let crop_set: std::collections::HashSet<&str> = std::collections::HashSet::new();
        export_docx(&rec, &steps, &out, &crop_set).unwrap();
        assert!(out.exists());
        let size = std::fs::metadata(&out).unwrap().len();
        assert!(size > 100);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_export_markdown_md_special_chars() {
        let rec = ExportRecording {
            id: "r".into(),
            title: "标题 #1 [x] *y*".into(),
            app_name: "App".into(),
            total_steps: 1,
            created_at: "2026-08-09".into(),
        };
        let steps = vec![ExportStep {
            id: "test-step".into(),
            order_index: 0,
            step_number: 1,
            action_type: "click|a".into(),
            title: "**bold** `code`".into(),
            description: "line1\nline2".into(),
            tip: "> quote".into(),
            screenshot_path: "".into(),
            x: None,
            y: None,
            crop_w: None,
            crop_h: None,
                    is_variable: false,
            param_name: String::new(),
        }];
        let ss_dir = std::env::temp_dir().join("flowio_test_md4");
        let crop_set: std::collections::HashSet<&str> = std::collections::HashSet::new();
        let md = export_markdown(&rec, &steps, &ss_dir, &crop_set);
        // 特殊字符原样输出（Markdown 语法由查看器渲染），不转义也不崩溃
        assert!(md.contains("**bold** `code`"));
        assert!(md.contains("click|a"));
        assert!(md.contains("> **Tip:** > quote"));
    }

    #[test]
    fn test_docx_desc_run_multiline_breaks() {
        // 多行描述：行间必须是显式软换行 break（docx 字面 \n 会被 Word 折叠）
        let run = build_docx_desc_run("line1\nline2\n\nline4");
        let json = serde_json::to_string(&run).unwrap();
        assert_eq!(json.matches("\"type\":\"break\"").count(), 3);
        assert!(json.contains("line1"));
        assert!(json.contains("line4"));
        // 单行描述：不应产生 break
        let single = serde_json::to_string(&build_docx_desc_run("only")).unwrap();
        assert!(!single.contains("\"type\":\"break\""));
    }

    #[test]
    fn test_step_desc_html_converts_newlines() {
        let html = build_step_desc_html("第一行\n第二行");
        assert!(html.contains("第一行<br />第二行"));
        assert!(!html.contains("第一行\n第二行"));
        // 空描述返回空串
        assert_eq!(build_step_desc_html(""), "");
    }

    #[test]
    fn test_md_desc_block_hard_break() {
        // GFM 段内硬换行：行尾两空格
        let md = build_md_desc_block("line1\nline2");
        assert!(md.contains("line1  \nline2"));
    }

    fn skill_step(num: i32, action: &str, title: &str, desc: &str, is_var: bool, param: &str) -> ExportStep {
        ExportStep {
            id: format!("s{}", num),
            order_index: num - 1,
            step_number: num,
            action_type: action.into(),
            title: title.into(),
            description: desc.into(),
            tip: String::new(),
            screenshot_path: String::new(),
            x: None,
            y: None,
            crop_w: None,
            crop_h: None,
            is_variable: is_var,
            param_name: param.into(),
        }
    }

    fn skill_recording() -> ExportRecording {
        ExportRecording {
            id: "r1".into(),
            title: "月度对账流程".into(),
            app_name: "WPS 表格".into(),
            total_steps: 3,
            created_at: "2026-09-07".into(),
        }
    }

    #[test]
    fn test_skill_md_fixed_version() {
        let steps = vec![
            skill_step(1, "click", "打开订单表", "", false, ""),
            skill_step(2, "type", "在「订单号」中输入", "SO20260906-001", false, ""),
            skill_step(3, "click", "点击导出按钮", "", false, ""),
        ];
        let md = export_skill_md(&skill_recording(), &steps, &[], &SkillMachine::new());
        assert!(md.starts_with("---\n"));
        assert!(md.contains("name: flowio-skill")); // 中文标题压不出 ASCII → 回退
        assert!(!md.contains("## Inputs")); // 无变量 → 固定版
        assert!(md.contains("输入 `SO20260906-001`")); // sample 原样写死
        assert!(!md.contains("{{"));
        assert!(md.contains("## Steps"));
    }

    #[test]
    fn test_skill_md_variable_version() {
        let steps = vec![
            skill_step(1, "click", "打开订单表", "", false, ""),
            skill_step(2, "type", "在「订单号」中输入", "SO20260906-001", true, "order_id"),
            skill_step(3, "type", "在「月份」中输入", "3", true, "month"),
        ];
        let md = export_skill_md(&skill_recording(), &steps, &[], &SkillMachine::new());
        assert!(md.contains("## Inputs"));
        assert!(md.contains("- `order_id` — 示例值：`SO20260906-001`（来自步骤 2）"));
        assert!(md.contains("- `month` — 示例值：`3`（来自步骤 3）"));
        assert!(md.contains("输入 `{{order_id}}`（示例值：`SO20260906-001`"));
        assert!(md.contains("运行前需提供参数：`order_id`、`month`"));
    }

    #[test]
    fn test_skill_md_param_name_stripped_and_empty_ignored() {
        // param_name 带空白 → trim；is_variable 但 param_name 空 → 不算变量
        let steps = vec![
            skill_step(1, "type", "输入", "abc", true, "  order_id  "),
            skill_step(2, "type", "输入", "xyz", true, ""),
        ];
        let md = export_skill_md(&skill_recording(), &steps, &[], &SkillMachine::new());
        assert!(md.contains("`order_id`"));
        assert!(!md.contains("{{}}"));
        assert!(md.contains("输入 `xyz`")); // 空参数名的步骤按固定值渲染
    }

    #[test]
    fn test_skill_md_image_references() {
        // 捆绑截图：images 非空 → 对应步骤下插入相对路径引用；未提供的步骤不插
        let steps = vec![
            skill_step(1, "click", "打开订单表", "", false, ""),
            skill_step(2, "type", "在「订单号」中输入", "SO20260906-001", false, ""),
            skill_step(3, "click", "点击导出按钮", "", false, ""),
        ];
        let images = vec![
            (1, "screenshots/step-01.png".to_string()),
            (3, "screenshots/step-03.png".to_string()),
        ];
        let md = export_skill_md(&skill_recording(), &steps, &images, &SkillMachine::new());
        assert!(md.contains("![步骤 1 截图](screenshots/step-01.png)"));
        assert!(md.contains("![步骤 3 截图](screenshots/step-03.png)"));
        assert!(!md.contains("step-02"));
        // 空列表 → 纯文本（旧行为），不出任何图片引用
        let md_plain = export_skill_md(&skill_recording(), &steps, &[], &SkillMachine::new());
        assert!(!md_plain.contains("!["));
    }

    #[test]
    fn test_skill_slug_name_ascii() {
        assert_eq!(skill_slug_name("Monthly Report 2026!"), "monthly-report-2026");
        assert_eq!(skill_slug_name("纯中文标题"), "flowio-skill");
        assert_eq!(skill_slug_name(""), "flowio-skill");
    }

    /// 注册名必须**唯一**：多个中文标题教程若都叫 `flowio-skill`，MCP / 注册表会互相覆盖。
    /// 修正：slug 后拼 recording-id 派生的稳定短后缀，同名不同教程也能共存。
    #[test]
    fn skill_unique_name_is_unique_per_recording() {
        fn rec(id: &str, title: &str) -> ExportRecording {
            ExportRecording {
                id: id.into(),
                title: title.into(),
                app_name: String::new(),
                total_steps: 1,
                created_at: String::new(),
            }
        }
        let chinese_a = rec("rec-001", "测试教程");
        let chinese_b = rec("rec-002", "测试教程"); // 同标题不同 id
        let ascii = rec("rec-003", "XLOOKUP tutorial");

        let n1 = skill_unique_name(&chinese_a);
        let n2 = skill_unique_name(&chinese_b);
        let n3 = skill_unique_name(&ascii);

        assert_ne!(n1, n2, "同中文标题、不同 recording 必须不重名");
        assert!(
            n1.starts_with("flowio-skill-"),
            "中文标题 → flowio-skill-<后缀>，got {}",
            n1
        );
        assert!(
            n3.starts_with("xlookup-tutorial-"),
            "ASCII 标题保留 slug 前缀，got {}",
            n3
        );
        // 仍满足 Agent Skills 1.0 name 合法字符集 / 长度
        assert!(n1.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-'));
        assert!(!n1.starts_with('-') && n1.len() <= 64, "name 非法：{}", n1);
        // 同 recording 多次导出 → 名字稳定（注册表幂等）
        assert_eq!(skill_unique_name(&chinese_a), n1);
    }

    // ── SKILL 机器可执行数据 ───────────────────────────────────

    /// 模板标题判别。样本**全部取自真实库**（2026-09-08 审计：19 步里 14 步含糊），
    /// 别改成臆造样本——判错会让 AI 拿着「点击在「X」中」当真去猜控件。
    #[test]
    fn title_lacks_target_detects_real_templates() {
        for t in [
            "点击在「函数参数」中",
            "点击在「插入函数」中",
            "点击在「【WPS表格新手入门…」中",
            "点击在「是否保存文档？」中",
            "在「插入函数」中输入",
            "点击",
            "",
            "   ",
        ] {
            assert!(title_lacks_target("click", t), "应判为含糊：{:?}", t);
        }
        for t in [
            "点击 SUMIF 函数",
            "点击「确定」按钮",
            "在「函数参数」中输入 数值1",
            "拖拽选中",
        ] {
            assert!(!title_lacks_target("click", t), "应判为清晰：{:?}", t);
        }
    }

    /// 键盘类动作的标题天然自足，不能被误判成含糊——
    /// 「Enter 确认」已经把该做什么说尽了，硬要求它写控件名是错的。
    #[test]
    fn title_lacks_target_keyboard_never_vague() {
        assert!(!title_lacks_target("key", "Enter 确认"));
        assert!(!title_lacks_target("hotkey", "快捷键 Alt+Tab · 切换到 录步"));
        assert!(!title_lacks_target("key", "Tab 切换焦点"));
        assert!(!title_lacks_target("hotkey", ""));
        assert!(is_keyboard_action("hotkey") && !is_keyboard_action("click"));
    }

    /// 落点基准：drag 的 x/y 是裁图框中心，真实起点在 start_x/start_y。
    /// 与 executor::delta_reference 同一个坑，两边都必须锁住。
    #[test]
    fn skill_point_drag_uses_start() {
        assert_eq!(
            skill_point("drag", Some(700), Some(400), Some(665), Some(308)),
            Some([665, 308])
        );
        // 旧录制无起点 → 降级用 x/y，绝不返回 None（有个近似值也比没有强）
        assert_eq!(
            skill_point("drag", Some(700), Some(400), None, None),
            Some([700, 400])
        );
        assert_eq!(
            skill_point("click", Some(120), Some(80), None, None),
            Some([120, 80])
        );
        assert_eq!(skill_point("hotkey", None, None, None, None), None);
    }

    fn mstep(n: i32, action: &str, intent: &str, vague: bool) -> SkillStepData {
        SkillStepData {
            n,
            action: action.into(),
            intent: intent.into(),
            intent_vague: vague,
            window: "插入函数".into(),
            point: Some([100 + n, 200 + n]),
            ..Default::default()
        }
    }

    #[test]
    fn skill_quality_counts_coverage() {
        let mut m = SkillMachine::new();
        m.insert(1, mstep(1, "click", "点击在「插入函数」中", true));
        m.insert(2, mstep(2, "click", "点击 SUMIF", false));
        let mut with_shot = mstep(3, "click", "点击 确定", false);
        with_shot.screenshot = Some("screenshots/step-03.jpg".into());
        with_shot.anchor = Some(serde_json::json!({"cls":"Button"}));
        m.insert(3, with_shot);
        assert_eq!(
            skill_quality(&m),
            SkillQuality {
                total: 3,
                vague: 1,
                with_point: 3,
                with_anchor: 1,
                with_shot: 1,
            }
        );
    }

    /// 含糊步骤必须**点名到步号**。只说「有些步骤不准」等于没说，
    /// 消费方无法定位到底该对哪几步改看截图。
    #[test]
    fn skill_notice_lists_vague_step_numbers() {
        let mut m = SkillMachine::new();
        m.insert(9, mstep(9, "click", "点击在「函数参数」中", true));
        m.insert(2, mstep(2, "click", "点击在「插入函数」中", true));
        m.insert(5, mstep(5, "click", "点击 SUMIF", false));
        let q = skill_quality(&m);
        let n = skill_notice(&m, &q);
        assert!(n.contains("第 2、9 步"), "步号需升序点名：{}", n);
        assert!(n.contains("共 3 步中的 2 步"), "{}", n);
        assert!(n.contains("steps.json"), "须指向结构化数据：{}", n);
    }

    /// 数据齐全时不产出免责段落——好数据不该被警告噪音污染。
    #[test]
    fn skill_notice_no_warning_when_all_clear() {
        let mut m = SkillMachine::new();
        m.insert(1, mstep(1, "click", "点击 SUMIF", false));
        let q = skill_quality(&m);
        let n = skill_notice(&m, &q);
        assert!(!n.contains("⚠️"), "无含糊步骤时不该有警告：{}", n);
        assert!(n.contains("执行须知"), "{}", n);
    }

    /// SKILL.md 正文必须带上「目标窗口 + 落点」——标题说不清目标时，
    /// 这两行是消费方唯一可靠的定位依据，不能只藏在 steps.json 里。
    #[test]
    fn skill_md_embeds_machine_locators() {
        let steps = vec![skill_step(1, "click", "点击在「插入函数」中", "", false, "")];
        let mut m = SkillMachine::new();
        m.insert(1, mstep(1, "click", "点击在「插入函数」中", true));
        let out = export_skill_md(&skill_recording(), &steps, &[], &m);
        assert!(out.contains("- 目标窗口：`插入函数`"), "{}", out);
        assert!(out.contains("- 录制落点：`(101, 201)`"), "{}", out);
        assert!(out.contains("请以截图为准定位"), "{}", out);
        assert!(out.contains("## 执行须知"), "{}", out);
    }

    /// 无机器数据时 SKILL.md 必须逐字退回旧格式——老用户导出结果不能因本次
    /// 改动而变样（查表失败也走这条路）。
    #[test]
    fn skill_md_without_machine_stays_legacy() {
        let steps = vec![skill_step(1, "click", "点击 确定", "", false, "")];
        let out = export_skill_md(&skill_recording(), &steps, &[], &SkillMachine::new());
        assert!(!out.contains("## 执行须知"), "{}", out);
        assert!(!out.contains("目标窗口"), "{}", out);
    }

    /// drag 步在正文里要给出起点→终点两个坐标，只给一个点没法复现拖拽。
    #[test]
    fn skill_md_drag_shows_both_ends() {
        let steps = vec![skill_step(1, "drag", "拖拽选中", "", false, "")];
        let mut m = SkillMachine::new();
        let mut d = mstep(1, "drag", "拖拽选中", false);
        d.point = Some([665, 308]);
        d.drag_to = Some([820, 460]);
        m.insert(1, d);
        let out = export_skill_md(&skill_recording(), &steps, &[], &m);
        assert!(
            out.contains("`(665, 308)` → `(820, 460)`（拖拽）"),
            "{}",
            out
        );
    }

    #[test]
    fn test_generate_export_path_extensions() {
        let dir = std::path::Path::new("out");
        assert_eq!(
            generate_export_path(dir, "标题", "html", "20260820_120000"),
            std::path::PathBuf::from("out/标题_20260820_120000.html")
        );
        assert_eq!(
            generate_export_path(dir, "标题", "markdown", "t"),
            std::path::PathBuf::from("out/标题_t.md")
        );
        assert_eq!(
            generate_export_path(dir, "标题", "PDF", "t"),
            std::path::PathBuf::from("out/标题_t.pdf")
        );
        assert_eq!(
            generate_export_path(dir, "标题", "docx", "t"),
            std::path::PathBuf::from("out/标题_t.docx")
        );
        assert_eq!(
            generate_export_path(dir, "标题", "word", "t"),
            std::path::PathBuf::from("out/标题_t.docx")
        );
        // 未知格式回退 txt
        assert_eq!(
            generate_export_path(dir, "标题", "txt", "t"),
            std::path::PathBuf::from("out/标题_t.txt")
        );
        assert_eq!(
            generate_export_path(dir, "标题", "EXCEL", "t"),
            std::path::PathBuf::from("out/标题_t.txt")
        );
        // 空格式也回退 txt
        assert_eq!(
            generate_export_path(dir, "标题", "", "t"),
            std::path::PathBuf::from("out/标题_t.txt")
        );
    }

    #[test]
    fn test_generate_export_path_sanitizes_title() {
        let dir = std::path::Path::new("out");
        let norm = |p: std::path::PathBuf| p.to_string_lossy().replace('\\', "/");
        // Windows 非法字符全部替换为 _
        let p = generate_export_path(dir, "a/b\\c:d*e?f\"g<h>i|j", "markdown", "t");
        assert_eq!(norm(p), "out/a_b_c_d_e_f_g_h_i_j_t.md");
        // 全角冒号保留
        let p2 = generate_export_path(dir, "报告：终版", "pdf", "t");
        assert_eq!(norm(p2), "out/报告：终版_t.pdf");
        // 空标题生成 _t.md
        let p3 = generate_export_path(dir, "", "markdown", "t");
        assert_eq!(norm(p3), "out/_t.md");
        // 未识别格式 "md" 回退 txt（记录现状：前端应传 markdown）
        let p5 = generate_export_path(dir, "标题", "md", "t");
        assert_eq!(norm(p5), "out/标题_t.txt");
        // 超长标题不截断（记录现状，路径长度由 OS 校验）
        let long = "长".repeat(500);
        let p4 = generate_export_path(dir, &long, "html", "t");
        assert!(p4.to_string_lossy().contains("_t.html"));
        assert!(norm(p4).starts_with("out/"));
    }

    // ── Extracted pure function tests ─────────────────────────

    #[test]
    fn test_build_step_header_html_basic_and_escape() {
        assert_eq!(
            build_step_header_html(1, "click", "打开窗口"),
            "Step 1: <span class=\"step-type\">click</span> 打开窗口"
        );
        // 特殊字符转义
        assert_eq!(
            build_step_header_html(2, "<type>&", "标题 <b>"),
            "Step 2: <span class=\"step-type\">&lt;type&gt;&amp;</span> 标题 &lt;b&gt;"
        );
        // 空字段
        assert_eq!(
            build_step_header_html(3, "", ""),
            "Step 3: <span class=\"step-type\"></span> "
        );
        // 超长标题不截断
        let long = "长".repeat(500);
        let h = build_step_header_html(4, "x", &long);
        assert!(h.contains(&long));
        // step_number 边界
        assert!(build_step_header_html(0, "a", "b").starts_with("Step 0:"));
        assert!(build_step_header_html(-1, "a", "b").starts_with("Step -1:"));
    }

    #[test]
    fn test_build_step_desc_html_empty_and_escape() {
        assert_eq!(build_step_desc_html(""), "");
        assert_eq!(
            build_step_desc_html("普通描述"),
            "      <p class=\"step-desc\">普通描述</p>\n"
        );
        // 特殊字符转义
        assert_eq!(
            build_step_desc_html("<b>&\"'"),
            "      <p class=\"step-desc\">&lt;b&gt;&amp;&quot;&#39;</p>\n"
        );
        // 多行描述：\n 转 <br />（HTML 渲染时保留换行）
        assert_eq!(
            build_step_desc_html("line1\nline2"),
            "      <p class=\"step-desc\">line1<br />line2</p>\n"
        );
    }

    #[test]
    fn test_build_step_tip_html_empty_and_escape() {
        assert_eq!(build_step_tip_html(""), "");
        assert_eq!(
            build_step_tip_html("一个提示"),
            "      <div class=\"step-tip\"><strong>Tip:</strong> 一个提示</div>\n"
        );
        assert_eq!(
            build_step_tip_html("<script>alert(1)</script>"),
            "      <div class=\"step-tip\"><strong>Tip:</strong> &lt;script&gt;alert(1)&lt;/script&gt;</div>\n"
        );
    }

    #[test]
    fn test_build_screenshot_placeholder_and_img() {
        assert_eq!(
            build_screenshot_placeholder_html(),
            "<div class=\"screenshot-placeholder\">No screenshot</div>"
        );
        // img 标签
        assert_eq!(
            build_screenshot_img_html("image/png", "abc123", 1),
            "<img class=\"screenshot\" src=\"data:image/png;base64,abc123\" alt=\"Step 1 screenshot\" />"
        );
        // 空 base64 / 空 mime
        assert_eq!(
            build_screenshot_img_html("image/jpeg", "", 0),
            "<img class=\"screenshot\" src=\"data:image/jpeg;base64,\" alt=\"Step 0 screenshot\" />"
        );
        assert_eq!(
            build_screenshot_img_html("", "data", -2),
            "<img class=\"screenshot\" src=\"data:;base64,data\" alt=\"Step -2 screenshot\" />"
        );
    }

    #[test]
    fn test_build_html_document_structure() {
        let doc = build_html_document("标题", 2, "2026-08-20", "<card/>");
        assert!(doc.starts_with("<!DOCTYPE html>"));
        assert!(doc.contains("<title>标题</title>"));
        assert!(doc.contains("<h1>标题</h1>"));
        assert!(doc.contains("<span>Steps: 2</span>"));
        assert!(doc.contains("<span>Date: 2026-08-20</span>"));
        assert!(doc.contains("<card/>"));
        assert!(doc.ends_with("</html>"));
        // title 特殊字符转义
        let doc2 = build_html_document("<script>", 0, "", "");
        assert!(doc2.contains("&lt;script&gt;"));
        // 空 step_cards 合法
        let doc3 = build_html_document("t", 0, "d", "");
        assert!(doc3.contains("<div class=\"container\">\n\n</div>"));
    }

    #[test]
    fn test_build_md_step_header() {
        assert_eq!(build_md_step_header(1, "click", "第一步"), "## Step 1 · `click` · 第一步");
        // Markdown 特殊字符原样（不转义，由渲染器处理）
        assert_eq!(build_md_step_header(2, "a|b`c", "**bold**"), "## Step 2 · `a|b`c` · **bold**");
        assert_eq!(build_md_step_header(0, "", ""), "## Step 0 · `` · ");
    }

    #[test]
    fn test_build_md_desc_tip_blocks() {
        assert_eq!(build_md_desc_block(""), "");
        assert_eq!(build_md_desc_block("描述"), "描述\n\n");
        assert_eq!(build_md_desc_block("line1\nline2"), "line1  \nline2\n\n");
        assert_eq!(build_md_tip_block(""), "");
        assert_eq!(build_md_tip_block("提示"), "> **Tip:** 提示\n");
        // tip 含 > 等特殊字符原样
        assert_eq!(build_md_tip_block("> quote"), "> **Tip:** > quote\n");
    }

    #[test]
    fn test_build_md_document_structure() {
        let md = build_md_document("标题", "App", 1, "2026-08-20", "## Step 1");
        assert!(md.starts_with("# 标题\n\n"));
        assert!(md.contains("**App:** App  ·  **Steps:** 1  ·  **Date:** 2026-08-20"));
        assert!(md.contains("---\n\n## Step 1"));
        // 空 step_list
        let md2 = build_md_document("t", "a", 0, "d", "");
        assert!(md2.contains("---\n\n"));
    }

    #[test]
    fn test_build_docx_step_header_and_meta() {
        assert_eq!(build_docx_step_header(1, "click", "标题"), "Step 1 · click · 标题");
        assert_eq!(build_docx_step_header(5, "", ""), "Step 5 ·  · ");
        assert_eq!(
            build_docx_meta_text("App", 3, "2026-08-20"),
            "App: App  ·  Steps: 3  ·  Date: 2026-08-20"
        );
        assert_eq!(build_docx_meta_text("", 0, ""), "App:   ·  Steps: 0  ·  Date: ");
    }

    #[test]
    fn test_compute_image_emu_dims() {
        // 小图原尺寸缩放（9525 EMU/px）
        assert_eq!(compute_image_emu_dims(Some((100, 50)), 5_715_000), (952_500, 476_250));
        // 恰好 600px 不缩放
        assert_eq!(compute_image_emu_dims(Some((600, 300)), 5_715_000), (5_715_000, 2_857_500));
        // 超宽图等比缩放到 600px 宽
        assert_eq!(compute_image_emu_dims(Some((1200, 600)), 5_715_000), (5_715_000, 2_857_500));
        // 超宽非整数比例（1200x900 → 600x450）
        assert_eq!(compute_image_emu_dims(Some((1200, 900)), 5_715_000), (5_715_000, 4_286_250));
        // 0x0 图：宽高为 0 在 Word 里渲染异常，退回兜底比例（与「读不到尺寸」一致）
        assert_eq!(compute_image_emu_dims(Some((0, 0)), 5_715_000), (5_715_000, 4_286_250));
        assert_eq!(compute_image_emu_dims(Some((0, 100)), 5_715_000), (5_715_000, 4_286_250));
        assert_eq!(compute_image_emu_dims(Some((100, 0)), 5_715_000), (5_715_000, 4_286_250));
        // 无尺寸 → 回退占位（max_width_emu x 3/4）
        assert_eq!(compute_image_emu_dims(None, 5_715_000), (5_715_000, 4_286_250));
        // 自定义 fallback
        assert_eq!(compute_image_emu_dims(None, 100_000), (100_000, 75_000));
        // 超高图（宽 2000 高 4000 → 600x1200）
        assert_eq!(compute_image_emu_dims(Some((2000, 4000)), 5_715_000), (5_715_000, 11_430_000));
    }

    /// 构造一张**非纯色** JPEG，用于裁剪/尺寸回归。
    /// ⚠️ 必须带纹理：空白裁剪守卫（is_blank_crop）会把纯色区域的裁剪回退成整图，
    /// 纯色测试图会让所有裁剪用例意外走整图分支（2026-09-05 加守卫时踩过）。
    fn test_jpeg(w: u32, h: u32) -> Vec<u8> {
        let mut img = image::RgbImage::new(w, h);
        for (x, y, p) in img.enumerate_pixels_mut() {
            // 逐像素交替的确定性纹理：任何步长采样都能采到不同值 → 永远判非纯色
            let v = ((x * 7 + y * 13) % 32) as u8;
            *p = image::Rgb([120u8.wrapping_add(v), 160, 200]);
        }
        let mut buf = Vec::new();
        let enc = image::codecs::jpeg::JpegEncoder::new_with_quality(&mut buf, 90);
        image::DynamicImage::ImageRgb8(img)
            .write_with_encoder(enc)
            .expect("测试图编码失败");
        buf
    }

    /// DOCX 里图片的显示尺寸必须与**实际写进去的字节**一致。
    ///
    /// 不变式：
    ///   1. 显示宽高比 == 图片实际宽高比（不变形）
    ///   2. 显示宽度 ≤ 图片实际宽度（不放大 → 不变模糊）
    ///
    /// 旧行为：裁剪后仍传原图尺寸给 `Pic::new(cropped).size(w, h)`。
    /// 1920×1080 原图裁出 800×600 时，显示框是 16:9 而图片是 4:3 → 横向拉伸 33%；
    /// 裁出 300×200 时更会被放大到 600px 宽（2 倍）变模糊。
    #[test]
    fn docx_image_emu_matches_written_bytes() {
        const EMU_PER_PX: f64 = 9525.0;
        const MAX_EMU: usize = 5_715_000;
        // (图宽, 图高, crop x, y, w, h, action)
        // type 会向外扩展一圈（元素 + 留白），比例反而接近原图、变形小；
        // click 不扩展，裁剪框是什么比例就是什么比例 —— 变形最明显。
        let cases = [
            (1920u32, 1080u32, 560i32, 240i32, 800i32, 600i32, "type"),
            (1920, 1080, 800, 400, 300, 200, "type"),
            (2560, 1440, 960, 480, 640, 480, "type"),
            (1920, 1080, 700, 300, 400, 500, "click"),
            (1920, 1080, 900, 200, 300, 600, "click"),
            (3840, 2160, 1500, 900, 400, 700, "click"),
            (1366, 768, 500, 200, 200, 150, "click"),
        ];
        for (iw, ih, cx, cy, cw, ch, action) in cases {
            let raw = test_jpeg(iw, ih);
            let (bytes, dims) = maybe_crop_screenshot_bytes(
                raw, Some(cx), Some(cy), Some(cw), Some(ch), action,
            );

            // 1) 返回的尺寸必须就是这些字节的真实尺寸
            let real = image::load_from_memory(&bytes).expect("输出应是合法图片");
            let (rw, rh) = (real.width(), real.height());
            assert_eq!(
                dims,
                Some((rw as usize, rh as usize)),
                "返回尺寸与实际字节不符（{iw}x{ih} 裁 {cw}x{ch}）"
            );

            let (w_emu, h_emu) = compute_image_emu_dims(dims, MAX_EMU);

            // 2) 不变形
            let shown_ratio = w_emu as f64 / h_emu as f64;
            let real_ratio = rw as f64 / rh as f64;
            assert!(
                (shown_ratio - real_ratio).abs() < 0.01,
                "显示宽高比 {shown_ratio:.3} ≠ 图片实际 {real_ratio:.3}（{iw}x{ih} 裁出 {rw}x{rh} 会变形）"
            );

            // 3) 不放大
            assert!(
                w_emu as f64 <= rw as f64 * EMU_PER_PX + 1.0,
                "显示宽 {:.0}px > 图片实际宽 {rw}px（被放大，会模糊）",
                w_emu as f64 / EMU_PER_PX
            );

            // 量化输出（cargo test -- --nocapture）：修复前按原图尺寸算会怎样
            let (ow, oh) = compute_image_emu_dims(Some((iw as usize, ih as usize)), MAX_EMU);
            let old_ratio = ow as f64 / oh as f64;
            let old_scale = (ow as f64 / EMU_PER_PX) / rw as f64;
            println!(
                "  {iw}x{ih} {action} 裁 {cw}x{ch} → 实际 {rw}x{rh}｜修复后显示 {:.0}x{:.0}px｜\
                 按原图算 {:.0}x{:.0}px（宽高比 {old_ratio:.3} vs 实际 {real_ratio:.3}，变形 {:+.1}%，缩放 {old_scale:.2}x）",
                w_emu as f64 / EMU_PER_PX,
                h_emu as f64 / EMU_PER_PX,
                ow as f64 / EMU_PER_PX,
                oh as f64 / EMU_PER_PX,
                (old_ratio / real_ratio - 1.0) * 100.0,
            );
        }
    }

    /// 静态防线：DOCX 的图片尺寸必须来自「实际写出去的字节」。
    ///
    /// 上面的行为测试测的是 `maybe_crop_screenshot_bytes` 返回值对不对，
    /// **测不到调用点传了哪个参数** —— 就算把 `final_dims` 改回 `*dim_opt`，
    /// 行为测试照样全绿。只能扫源码补这个洞（同类教训见 check-fit.mjs 第 9/11 段）。
    #[test]
    fn docx_must_size_image_from_written_bytes() {
        // ⚠️ 两道自指陷阱，都踩过：
        // 1) 只扫 `#[cfg(test)]` 之前的生产代码 —— 否则 forbidden 字符串写在防线
        //    自己的源码里，`contains` 恒为真，防线永远不会失败。
        // 2) 即便如此仍把字符串拆开拼接：万一以后有人把这类字符串挪到生产区，
        //    也不至于让断言自我满足。
        // 3) 剥注释：说明文字里会出现被禁用的旧写法。
        let prod: &str = COMMANDS_SRC.split("#[cfg(test)]").next().unwrap_or("");
        let code: String = prod
            .lines()
            .map(|l| l.split("//").next().unwrap_or(""))
            .collect::<Vec<_>>()
            .join("\n");
        let required = concat!("compute_image_emu_dims(", "final_dims");
        let forbidden = concat!("compute_image_emu_dims(", "*dim_opt");
        assert!(
            code.contains(required),
            "DOCX 必须用 final_dims（裁剪后的真实尺寸）算显示尺寸"
        );
        assert!(
            !code.contains(forbidden),
            "DOCX 仍用原图尺寸 *dim_opt 算显示尺寸 —— 裁剪后会被拉伸变形、小图会被放大变模糊"
        );
    }

    /// 未裁剪（或裁剪失败）时，返回尺寸必须仍是原图尺寸。
    #[test]
    fn maybe_crop_returns_full_dims_when_not_cropped() {
        let raw = test_jpeg(1920, 1080);
        let (bytes, dims) = maybe_crop_screenshot_bytes(raw, None, None, None, None, "click");
        assert_eq!(dims, Some((1920, 1080)));
        let real = image::load_from_memory(&bytes).unwrap();
        assert_eq!((real.width(), real.height()), (1920, 1080));

        // 非法字节：解码失败 → 原样返回，尺寸为 None（由调用方走兜底）
        let (bad, dims) = maybe_crop_screenshot_bytes(vec![0u8, 1, 2, 3], None, None, None, None, "click");
        assert_eq!(bad, vec![0u8, 1, 2, 3]);
        assert_eq!(dims, None);
    }

    /// 空白裁剪守卫：裁剪区域几乎纯色（如 WPS 加载中录到的「Program Manager」
    /// 宿主伪元素矩形 → 纯灰空白）时，导出必须回退**整图字节**。
    #[test]
    fn blank_crop_falls_back_to_full_bytes() {
        let raw = test_jpeg_flat(800, 600, [214, 214, 214]);
        let (bytes, dims) = maybe_crop_screenshot_bytes(
            raw.clone(),
            Some(400),
            Some(300),
            Some(420),
            Some(280),
            "type",
        );
        assert_eq!(dims, Some((800, 600)), "纯色区域裁剪应回退整图尺寸");
        assert_eq!(bytes, raw, "回退整图时应原样返回原始字节（不重编码）");
    }

    /// 同构数据但裁剪区域含可见内容（深色块）→ 正常裁剪，不触发守卫。
    #[test]
    fn non_blank_crop_still_crops() {
        let mut img = image::RgbImage::from_pixel(800, 600, image::Rgb([214, 214, 214]));
        for y in 280..320 {
            for x in 380..420 {
                img.put_pixel(x, y, image::Rgb([20, 20, 20]));
            }
        }
        let mut buf = Vec::new();
        let enc = image::codecs::jpeg::JpegEncoder::new_with_quality(&mut buf, 90);
        image::DynamicImage::ImageRgb8(img)
            .write_with_encoder(enc)
            .unwrap();

        let (_, dims) = maybe_crop_screenshot_bytes(buf, Some(400), Some(300), Some(420), Some(280), "click");
        let (w, h) = dims.expect("含内容区域必须正常裁剪");
        assert!(w < 800 && h < 600, "应裁出小于整图的区域，实际 {w}x{h}");
    }

    /// 构造纯色 JPEG（仅供空白守卫测试用；其余测试请用带纹理的 test_jpeg）
    fn test_jpeg_flat(w: u32, h: u32, rgb: [u8; 3]) -> Vec<u8> {
        let img = image::RgbImage::from_pixel(w, h, image::Rgb(rgb));
        let mut buf = Vec::new();
        let enc = image::codecs::jpeg::JpegEncoder::new_with_quality(&mut buf, 90);
        image::DynamicImage::ImageRgb8(img)
            .write_with_encoder(enc)
            .expect("测试图编码失败");
        buf
    }

    /// is_blank_crop 自身：纯色 → true；有深色块 → false；阈值黄金值 24。
    #[test]
    fn is_blank_crop_basic() {
        assert_eq!(BLANK_CROP_DELTA, 24, "与前端 cropBlank.ts 黄金值锁死");
        let flat = image::DynamicImage::ImageRgb8(image::RgbImage::from_pixel(300, 200, image::Rgb([214, 214, 214])));
        assert!(is_blank_crop(&flat), "纯灰应判空白");
        let mut noisy = flat.to_rgb8();
        noisy.put_pixel(150, 100, image::Rgb([214, 214, 250])); // 通道差 36 > 24
        assert!(!is_blank_crop(&image::DynamicImage::ImageRgb8(noisy)), "含可见差异应判非空白");
    }

    // B3 回归 A：PDF/HTML 模板必须带 CJK 字体回退，否则非中文系统中文渲染为豆腐块
    #[test]
    fn test_build_html_document_includes_cjk_font_fallback() {
        let doc = build_html_document("标题", 1, "2026-08-20", "<card/>");
        assert!(
            doc.contains("Microsoft YaHei")
                && doc.contains("Noto Sans CJK SC")
                && doc.contains("SimSun"),
            "HTML font-family 缺少 CJK 回退字体:\n{}",
            doc
        );
        // font-family 声明必须出现在 body 样式（Edge 无头打印 PDF 依赖此处）
        let style_body = doc
            .lines()
            .find(|l| l.contains("font-family"))
            .expect("找不到 font-family 样式行");
        assert!(style_body.contains("Microsoft YaHei"));
    }

    /* ── 裁图几何（export_crop_box）──────────────────────────────────
     * 这套算法在仓库里有 4 份实现（见 export_crop_box 的文档注释）。
     * 下面的黄金值是**前端 cropBoxFromAnchor 的实算输出**，用作跨语言锚点：
     * 任何一端改动而不同步其余三端，这里就会红。
     *
     * 场景：1920x1080 截图，元素矩形中心 (960,540)、尺寸 300x40
     *   type : extra = round(300/3) = 100, w = 300+16+100 = 416, x = 960-208-50 = 702
     *   click: w = 300+16 = 316, x = 960-158 = 802
     */

    /// 元素矩形是否完整落在裁图框内，且四边留白都 >= CROP_PAD。
    /// 这是「裁图不能切掉目标元素」的可执行定义。
    fn element_fully_inside(
        img_w: i32,
        img_h: i32,
        cx: i32,
        cy: i32,
        cw: i32,
        ch: i32,
        action_type: &str,
    ) -> (bool, String) {
        let box_ = match export_crop_box(
            img_w,
            img_h,
            Some(cx),
            Some(cy),
            Some(cw),
            Some(ch),
            action_type,
        ) {
            Some(b) => b,
            None => return (false, "export_crop_box 返回 None".into()),
        };
        let (l, t, w, h) = box_;
        let el = (cx - cw / 2, cy - ch / 2, cw, ch);
        let pads = [
            ("左", el.0 - l),
            ("右", (l + w) - (el.0 + el.2)),
            ("上", el.1 - t),
            ("下", (t + h) - (el.1 + el.3)),
        ];
        let bad: Vec<String> = pads
            .iter()
            .filter(|(_, p)| *p < EXPORT_CROP_PAD)
            .map(|(n, p)| format!("{n}={p}px"))
            .collect();
        (
            bad.is_empty(),
            format!("box={box_:?} 留白 {}", bad.join(" ")),
        )
    }

    #[test]
    fn export_crop_box_golden_matches_frontend() {
        // 与前端 cropBoxFromAnchor({x:960,y:540,cropW:300,cropH:40}) 的实算输出一致
        assert_eq!(
            export_crop_box(1920, 1080, Some(960), Some(540), Some(300), Some(40), "type"),
            Some((702, 512, 416, 56))
        );
        assert_eq!(
            export_crop_box(1920, 1080, Some(960), Some(540), Some(300), Some(40), "click"),
            Some((802, 512, 316, 56))
        );
    }

    #[test]
    fn export_crop_box_golden_edge_element_sizes() {
        // 极小元素（24px）与超宽元素（900px），同样对齐前端实算值
        assert_eq!(
            export_crop_box(1920, 1080, Some(960), Some(540), Some(24), Some(40), "type"),
            Some((932, 512, 48, 56))
        );
        assert_eq!(
            export_crop_box(1920, 1080, Some(960), Some(540), Some(900), Some(40), "type"),
            Some((202, 512, 1216, 56))
        );
    }

    #[test]
    fn export_crop_box_drag_gets_extra_vertical_context() {
        // DRAG 步骤高度方向多留上下文：选区 400×30（表格行拖选），
        // 额外 extraH = max(30*0.6, 100) = 100 → 显示高 = 30+16+100=146。
        // 宽度与 click 一致（无左扩）。
        let (l, t, w, h) = export_crop_box(
            1920, 1080, Some(960), Some(540), Some(400), Some(30), "drag",
        ).unwrap();
        assert_eq!(h, 30 + EXPORT_CROP_PAD * 2 + 100); // 146
        assert_eq!(w, 400 + EXPORT_CROP_PAD * 2); // 416
        // 高度方向中心对齐
        assert_eq!(t, 540 - h / 2);
    }

    #[test]
    fn export_crop_box_type_never_clips_element_right_edge() {
        // v25 的核心防线：元素宽 > 48px 时旧写法（- extra）会把右边缘切掉。
        // 这里枚举常见输入框宽度，断言四边留白都 >= CROP_PAD。
        for cw in [24, 48, 60, 100, 120, 200, 300, 480, 600, 900] {
            for ch in [24, 40, 80] {
                for at in ["type", "click"] {
                    let (ok, msg) = element_fully_inside(1920, 1080, 960, 540, cw, ch, at);
                    assert!(ok, "元素被切：{cw}x{ch} at={at} {msg}");
                }
            }
        }
    }

    #[test]
    fn export_crop_box_type_keeps_label_zone_on_left() {
        // type 的语义：左侧留白应比右侧多出 ~cw/3（label 区），不能两侧一样
        let (l, _t, w, _h) =
            export_crop_box(1920, 1080, Some(960), Some(540), Some(300), Some(40), "type")
                .expect("type 应产生裁图框");
        let elem_left = 960 - 150;
        let elem_right = 960 + 150;
        let pad_left = elem_left - l;
        let pad_right = (l + w) - elem_right;
        assert_eq!(pad_right, EXPORT_CROP_PAD, "右侧留白应恰为 CROP_PAD");
        assert_eq!(
            pad_left,
            EXPORT_CROP_PAD + 100,
            "左侧留白应为 CROP_PAD + extra(100)"
        );
    }

    #[test]
    fn export_crop_box_whole_window_and_missing_anchor() {
        for at in ["scroll", "nav", "screenshot"] {
            assert_eq!(
                export_crop_box(1920, 1080, Some(1), Some(1), Some(2), Some(2), at),
                None,
                "{at} 应返回全图"
            );
        }
        // 无坐标 / 退化图尺寸
        assert_eq!(
            export_crop_box(1920, 1080, None, None, Some(2), Some(2), "click"),
            None
        );
        assert_eq!(
            export_crop_box(0, 0, Some(1), Some(1), Some(2), Some(2), "click"),
            None
        );
        // 负坐标但**没有**元素矩形 → 40% 兜底分支要求坐标非负 → 全图（与前端一致）
        assert_eq!(
            export_crop_box(1920, 1080, Some(-5), Some(10), None, None, "click"),
            None
        );
    }

    #[test]
    fn export_crop_box_negative_center_with_rect_matches_frontend() {
        // 元素中心落在屏幕外（负坐标）但**有**完整矩形时，前端 cropAdaptiveBase64
        // 会正常裁剪（它只在这一分支校验 cropW/cropH > 0，不看坐标正负）。
        // 导出端必须逐像素对齐，否则编辑器显示裁图、导出却是全图。
        //   cw=2 → rw=18, left = -5-9 = -14 → clamp 到 0；top = 10-9 = 1
        assert_eq!(
            export_crop_box(1920, 1080, Some(-5), Some(10), Some(2), Some(2), "click"),
            Some((0, 1, 18, 18))
        );
    }

    #[test]
    fn export_crop_box_fallback_40pct_when_no_element_rect() {
        // 只有点击点：以点击点为中心裁屏幕的 40%，且不加 CROP_PAD
        assert_eq!(
            export_crop_box(1920, 1080, Some(960), Some(540), None, None, "click"),
            Some((576, 324, 768, 432))
        );
    }

    #[test]
    fn export_crop_box_clamps_into_image_bounds() {
        // 元素贴左上角：框必须平移进图内，不能出现负坐标或越界
        for at in ["type", "click"] {
            let (l, t, w, h) =
                export_crop_box(1920, 1080, Some(20), Some(20), Some(300), Some(40), at)
                    .unwrap_or_else(|| panic!("{at} 应产生裁图框"));
            assert!(l >= 0 && t >= 0, "{at} 出现负坐标: ({l},{t})");
            assert!(l + w <= 1920 && t + h <= 1080, "{at} 越界: {l},{t},{w},{h}");
        }
    }

    #[test]
    fn export_crop_box_never_exceeds_image_size() {
        // 元素比图还大：宽高饱和到图尺寸，起点归零
        assert_eq!(
            export_crop_box(400, 300, Some(200), Some(150), Some(3000), Some(2000), "type"),
            Some((0, 0, 400, 300))
        );
    }

    // B3 回归 B：Edge 路径探测不依赖单一硬编码路径（环境变量 + 固定路径候选）
    #[test]
    fn test_resolve_edge_path_candidate_logic() {
        // 探测函数是本机环境相关：若本机装有 Edge 则返回 Some 且指向 msedge.exe；
        // 若为 None（无 Edge）也在允许范围内——但不能 panic。
        let path = resolve_edge_path();
        if let Some(p) = path {
            assert!(
                p.to_string_lossy().to_lowercase().ends_with("msedge.exe"),
                "未指向 msedge.exe: {:?}",
                p
            );
        }
        // 环境变量候选拼接稳定：ProgramFiles 存在时其候选必须被纳入探测
        if let Ok(pf) = std::env::var("ProgramFiles") {
            let candidate =
                std::path::PathBuf::from(pf).join("Microsoft/Edge/Application/msedge.exe");
            assert!(
                resolve_edge_path().is_some() || !candidate.exists(),
                "ProgramFiles 下有 Edge 却未被探测到"
            );
        }
    }

    #[test]
    fn test_export_long_image_with_title_and_steps() {
        // 端到端验证：长图生成含教程标题头 + 步骤头部 + 截图，尺寸正确
        let dir = std::env::temp_dir().join(format!("flowio_test_long_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let png1 = write_tmp_png(&dir, "step1.png");
        let png2 = write_tmp_png(&dir, "step2.png");

        // 用 200x150 的图（比默认 8x8 大，更接近真实截图）
        let img1 = image::DynamicImage::new_rgb8(400, 300);
        img1.save(&png1).unwrap();
        let img2 = image::DynamicImage::new_rgb8(500, 250);
        img2.save(&png2).unwrap();

        let rec = ExportRecording {
            id: "long-test".into(),
            title: "Python 安装教程".into(),
            app_name: "Python.org".into(),
            total_steps: 2,
            created_at: "2026-09-06".into(),
        };
        let steps = vec![
            ExportStep {
                id: "s1".into(),
                order_index: 0,
                step_number: 1,
                action_type: "click".into(),
                title: "点击在「Downloads」中下载按钮".into(),
                description: "点击页面上的 Downloads 链接".into(),
                tip: "".into(),
                screenshot_path: png1.to_string_lossy().to_string(),
                x: None,
                y: None,
                crop_w: None,
                crop_h: None,
                        is_variable: false,
                param_name: String::new(),
            },
            ExportStep {
                id: "s2".into(),
                order_index: 1,
                step_number: 2,
                action_type: "type".into(),
                title: "输入在「搜索框」中 python".into(),
                description: "在搜索框输入关键词".into(),
                tip: "大小写不敏感".into(),
                screenshot_path: png2.to_string_lossy().to_string(),
                x: None,
                y: None,
                crop_w: None,
                crop_h: None,
                        is_variable: false,
                param_name: String::new(),
            },
        ];
        let out = dir.join("long_test.png");
        let crop_set: std::collections::HashSet<&str> = std::collections::HashSet::new();

        export_long_image(&rec, &steps, &out, &crop_set).unwrap();
        assert!(out.exists(), "长图文件未生成");

        let size = std::fs::metadata(&out).unwrap().len();
        assert!(size > 1000, "长图太小（{} bytes），可能渲染失败", size);

        // 验证图片可读取且尺寸合理
        let loaded = image::open(&out).expect("无法读取生成的长图");
        let (w, h) = (loaded.width(), loaded.height());
        // 宽度应为 max(400, 500) = 500（或上限 1440）
        assert!(w >= 400, "宽度 {} 小于最宽截图", w);
        // 高度应 > 标题头(100) + 间隔(24) + 步骤1(56+300) + 间隔(24) + 步骤2(56+250)
        let min_expected_h: u32 = 100 + 24 + (56 + 300) + 24 + (56 + 250);
        assert!(h >= min_expected_h, "高度 {} 小于预期最小值 {}", h, min_expected_h);

        println!("Long image test OK: {}x{}, {} bytes", w, h, size);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
