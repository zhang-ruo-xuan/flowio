use base64::Engine;
use docx_rs::*;
use rusqlite::params;
use serde::{Deserialize, Serialize};
use std::sync::Mutex;
use std::time::{Duration, Instant};
use tauri::{Emitter, State};

use crate::redaction::{redact_screenshot_copy, redact_text_with_config, RedactMode};
use crate::DbState;

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
}

// ── Tauri Command ─────────────────────────────────────────────

/// Export a recording to the specified format.
///
/// Supported formats: `"html"`, `"markdown"`, `"pdf"`, `"docx"` (or `"word"`).
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
) -> Result<String, String> {
    // async 命令运行在异步运行时线程，不阻塞 UI 主线程。内部 catch_unwind 防 panic 崩掉 IPC。
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let progress = move |done: usize, total: usize| {
            let _ = app.emit(
                "export://progress",
                serde_json::json!({ "done": done, "total": total }),
            );
        };
        export_recording_inner(&db, &id, &format, output_path, crop_step_ids.as_deref(), Some(&progress))
    }))
    .map_err(|_| "export_recording panicked".to_string())?
}

/// 实际导出逻辑（在独立线程内执行，避免长任务占用 IPC / 阻塞 UI）。
/// 导出前强制自动脱敏：文本替换 + 截图生成脱敏副本，原图与数据库数据保持不动。
pub(crate) fn export_recording_inner(
    db: &State<DbState>,
    id: &str,
    format: &str,
    output_path: Option<String>,
    crop_step_ids: Option<&[String]>,
    progress: Option<&(dyn Fn(usize, usize) + Send + Sync)>,
) -> Result<String, String> {
    let (recording, steps) = load_export_data(db, id)?;
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
            // Create screenshots subdirectory next to the MD file
            let parent = path.parent().unwrap_or(std::path::Path::new("."));
            let ss_dir = parent.join("screenshots");
            std::fs::create_dir_all(&ss_dir)
                .map_err(|e| format!("Failed to create screenshots dir: {}", e))?;
            let content = export_markdown(&recording, &steps, &ss_dir, &crop_set);
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
        other => Err(format!("Unsupported format: {}. Use html, markdown, docx, or pdf.", other)),
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
             tip, screenshot_path, x, y, crop_w, crop_h \
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
            })
        })
        .map_err(|e| e.to_string())?;

    let mut steps = Vec::new();
    for row in rows {
        steps.push(row.map_err(|e| e.to_string())?);
    }

    Ok((recording, steps))
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
        "pdf" => "pdf",
        "docx" | "word" => "docx",
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

fn maybe_crop_screenshot_bytes(
    raw_bytes: Vec<u8>,
    x: Option<i32>,
    y: Option<i32>,
    crop_w: Option<i32>,
    crop_h: Option<i32>,
    action_type: &str,
) -> Vec<u8> {
    // 整窗类型不裁剪（与前端 Stage C 一致）
    if matches!(action_type, "scroll" | "nav" | "screenshot") {
        return raw_bytes;
    }
    // 前端逻辑：连坐标都没有才显示全图，这里保持一致
    let has_point = matches!((x, y), (Some(a), Some(b)) if a >= 0 && b >= 0);
    if !has_point {
        return raw_bytes;
    }
    let mut img = match image::load_from_memory(&raw_bytes) {
        Ok(i) => i,
        Err(_) => return raw_bytes,
    };
    let img_w = img.width() as i32;
    let img_h = img.height() as i32;

    // 分支选择（与前端 cropAdaptiveBase64 一致）：
    // ① 有元素矩形（中心+尺寸）→ 按矩形外扩裁剪；② 只有坐标 → 点击点为中心裁 40%。
    // 前端 40% 兜底不加 CROP_PAD，此处用 add_pad 区分。
    let (cx, cy, rw0, rh0, add_pad) = match (x, y, crop_w, crop_h) {
        (Some(a), Some(b), Some(c), Some(d)) if c > 0 && d > 0 => (a, b, c, d, true),
        (Some(a), Some(b), _, _) => {
            let fw = ((img_w as f64) * 0.4).round() as i32;
            let fh = ((img_h as f64) * 0.4).round() as i32;
            (a, b, fw, fh, false)
        }
        _ => return raw_bytes,
    };

    // ① 外扩 CROP_PAD；type 步骤左扩 1/3 宽（仅矩形分支，与前端一致）
    let mut rw = if add_pad { rw0 + EXPORT_CROP_PAD * 2 } else { rw0 };
    let rh = if add_pad { rh0 + EXPORT_CROP_PAD * 2 } else { rh0 };
    let left;
    let top;
    if add_pad && action_type == "type" {
        let extra = rw0 / 3;
        rw += extra;
        left = cx - rw / 2 - extra;
    } else {
        left = cx - rw / 2;
    }
    top = cy - rh / 2;

    // 贴边平移进图内（不缩小裁剪框，避免内容被切掉一半）
    let rw = rw.min(img_w).max(1);
    let rh = rh.min(img_h).max(1);
    let left = left.clamp(0, img_w - rw);
    let top = top.clamp(0, img_h - rh);

    let cropped = img.crop(left as u32, top as u32, rw as u32, rh as u32);
    let mut buf: Vec<u8> = Vec::new();
    // 输出 JPEG 质量 85（与前端 canvas.toDataURL("image/jpeg", 0.85) 一致）
    let enc = image::codecs::jpeg::JpegEncoder::new_with_quality(&mut buf, 85);
    if cropped.write_with_encoder(enc).is_err() {
        return raw_bytes;
    }
    buf
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
                    maybe_crop_screenshot_bytes(raw, s.x, s.y, s.crop_w, s.crop_h, &s.action_type)
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

/// Generate a Scribe-style Markdown document:
///   - Clean title + metadata row
///   - Numbered step cards with badge-style header, description, screenshot, tip
///   - Screenshots copied to a `screenshots/` subdirectory and referenced via relative paths
fn export_markdown(recording: &ExportRecording, steps: &[ExportStep], ss_dir: &std::path::Path, crop_set: &std::collections::HashSet<&str>) -> String {
    let step_list: String = steps
        .iter()
        .map(|s| {
            // ── Screenshot (copy to subdir, relative path) ─────
            let screenshot_line = if s.screenshot_path.is_empty() {
                String::from("> *(No screenshot)*")
            } else {
                let src = std::path::Path::new(&s.screenshot_path);
                let ext = src
                    .extension()
                    .unwrap_or(std::ffi::OsStr::new("jpg"));
                let filename = format!("step_{}.{}", s.step_number, ext.to_string_lossy());
                let dest = ss_dir.join(&filename);
                let is_crop = crop_set.contains(s.id.as_str());
                if is_crop {
                    // 裁剪模式：读取→裁剪→写入
                    match std::fs::read(src).ok().filter(|b| !b.is_empty()) {
                        Some(raw) => {
                            let cropped = maybe_crop_screenshot_bytes(raw, s.x, s.y, s.crop_w, s.crop_h, &s.action_type);
                            match std::fs::write(&dest, &cropped) {
                                Ok(_) => format!("![Screenshot](./screenshots/{})", filename),
                                Err(_) => format!("> *(Screenshot export failed)*"),
                            }
                        }
                        None => format!("> *(Screenshot not found: {})*", s.screenshot_path.replace('\\', "/")),
                    }
                } else {
                    // 全图模式：直接复制原文件
                    match std::fs::copy(src, &dest) {
                        Ok(_) => format!("![Screenshot](./screenshots/{})", filename),
                        Err(_) => format!(
                            "> *(Screenshot not found: {})*",
                            s.screenshot_path.replace('\\', "/")
                        ),
                    }
                }
            };

            // ── Step header (Scribe-style badge) ───────────────
            let header = build_md_step_header(s.step_number, &s.action_type, &s.title);

            // ── Description ─────────────────────────────────────
            let desc_block = build_md_desc_block(&s.description);

            // ── Tip (styled blockquote) ─────────────────────────
            let tip_block = build_md_tip_block(&s.tip);

            format!(
                "{header}\n\n{desc_block}{screenshot}\n\n{tip_block}",
                header = header,
                desc_block = desc_block,
                screenshot = screenshot_line,
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

// ── DOCX export ──────────────────────────────────────────────

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
            docx = docx.add_paragraph(
                Paragraph::new()
                    .add_run(Run::new().add_text(&s.description).size(21))
            );
        }

        // ── Screenshot ────────────────────────────────────────
        if !s.screenshot_path.is_empty() {
            if let Ok(img_bytes) = std::fs::read(&s.screenshot_path) {
                let is_crop = crop_set.contains(s.id.as_str());
                let final_bytes = if is_crop && !img_bytes.is_empty() {
                    maybe_crop_screenshot_bytes(img_bytes, s.x, s.y, s.crop_w, s.crop_h, &s.action_type)
                } else {
                    img_bytes
                };
                let (w_emu, h_emu) = compute_image_emu_dims(*dim_opt, MAX_IMG_WIDTH_EMU);
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
fn build_step_desc_html(description: &str) -> String {
    if description.is_empty() {
        String::new()
    } else {
        format!(
            "      <p class=\"step-desc\">{}</p>\n",
            htmlescape(description)
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
fn build_md_desc_block(description: &str) -> String {
    if description.is_empty() {
        String::new()
    } else {
        format!("{}\n\n", description)
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
        Some((w, h)) if w > MAX_IMG_PX => {
            let ratio = MAX_IMG_PX as f64 / w as f64;
            ((MAX_IMG_PX * EMU_PER_PX) as u32, (h as f64 * ratio * EMU_PER_PX as f64) as u32)
        }
        Some((w, h)) => {
            ((w * EMU_PER_PX) as u32, (h * EMU_PER_PX) as u32)
        }
        None => {
            (max_width_emu as u32, (max_width_emu * 3 / 4) as u32)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
            },
        ];
        let ss_dir = std::env::temp_dir().join("flowio_test_md_ss");
        let crop_set: std::collections::HashSet<&str> = std::collections::HashSet::new();
        let md = export_markdown(&rec, &steps, &ss_dir, &crop_set);
        assert!(md.contains("# 标题"));
        assert!(md.contains("**App:** App"));
        assert!(md.contains("## Step 1"));
        assert!(md.contains("## Step 2"));
        assert!(md.contains("*(No screenshot)*"));
        assert!(md.contains("> **Tip:** 提示"));
        assert!(!md.contains("![Screenshot]"));
    }

    #[test]
    fn test_export_markdown_missing_screenshot_file() {
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
        }];
        let ss_dir = std::env::temp_dir().join("flowio_test_md_ss2");
        let crop_set: std::collections::HashSet<&str> = std::collections::HashSet::new();
        let md = export_markdown(&rec, &steps, &ss_dir, &crop_set);
        assert!(md.contains("Screenshot not found"));
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
        }];
        let crop_set: std::collections::HashSet<&str> = std::collections::HashSet::new();
        let html = gen_html_body(&rec, &steps, &crop_set);
        assert!(html.contains("data:image/png;base64,"));
        assert!(html.contains("Step 1"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_export_markdown_copies_real_screenshot() {
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
        }];
        let ss_dir = dir.join("screenshots");
        std::fs::create_dir_all(&ss_dir).unwrap();
        let crop_set: std::collections::HashSet<&str> = std::collections::HashSet::new();
        let md = export_markdown(&rec, &steps, &ss_dir, &crop_set);
        assert!(md.contains("![Screenshot](./screenshots/step_1.png)"));
        assert!(ss_dir.join("step_1.png").exists());
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
        // 多行描述保留换行
        assert_eq!(
            build_step_desc_html("line1\nline2"),
            "      <p class=\"step-desc\">line1\nline2</p>\n"
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
        assert_eq!(build_md_desc_block("line1\nline2"), "line1\nline2\n\n");
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
        // 0x0 图
        assert_eq!(compute_image_emu_dims(Some((0, 0)), 5_715_000), (0, 0));
        // 无尺寸 → 回退占位（max_width_emu x 3/4）
        assert_eq!(compute_image_emu_dims(None, 5_715_000), (5_715_000, 4_286_250));
        // 自定义 fallback
        assert_eq!(compute_image_emu_dims(None, 100_000), (100_000, 75_000));
        // 超高图（宽 2000 高 4000 → 600x1200）
        assert_eq!(compute_image_emu_dims(Some((2000, 4000)), 5_715_000), (5_715_000, 11_430_000));
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
}
