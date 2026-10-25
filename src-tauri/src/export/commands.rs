use base64::Engine;
use ab_glyph::Font;
use docx_rs::*;
use rusqlite::params;
use serde::{Deserialize, Serialize};
use std::sync::Mutex;
use std::time::{Duration, Instant};
use tauri::{Emitter, State};

use crate::export::pptx;
use crate::export::xlsx::build_xlsx_bytes;
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
    // ── 文档块（第十四轮）────────────────────────────────────────
    // block_kind：'step'（默认，真步骤）/ 'heading'（大标题块）/ 'summary'（流程摘要块）
    //             / 'intro'（简介块，第十九轮），后三者不占编号
    // block_level：标题层级 1..4
    // note_kind：步骤「补充说明」的类型 ''（普通）/ 'mark'（标注）/ 'warn'（警告）
    // title_html：大标题的富文本正文（白名单内联 HTML 子集，与编辑器同一份）
    #[serde(default)]
    block_kind: String,
    #[serde(default)]
    block_level: i32,
    #[serde(default)]
    note_kind: String,
    #[serde(default)]
    title_html: String,
    /// 图片描述（第十六轮）：紧贴截图下方渲染的一句话说明；空串 = 不渲染
    #[serde(default)]
    image_caption: String,
    // ── 步骤媒体（第二十轮）──────────────────────────────────────
    /// 'image'（含 gif 动图）/ 'video'（mp4/webm）。导出对两者的区别只有一处：
    /// 视频在 HTML 里嵌 `<video>`，其余格式一律用首帧静态图。
    #[serde(default)]
    media_kind: String,
    /// 视频首帧路径。**加载时会把 video 步骤的 `screenshot_path` 换成它**，
    /// 这样长图 / Word / Markdown / 纯文本四条链路一行都不用改（它们只认 screenshot_path）。
    #[serde(default)]
    media_poster_path: String,
    /// video 步骤的原始视频文件路径（导出 GIF 动图时需要）。
    #[serde(default)]
    media_source_path: String,
    /// 步骤来源网址（对齐插件 `step.url`）：该步骤发生的页面地址。
    /// 桌面端录制多为本机操作，存量/本机录制为空串 = 不渲染 `🔗` 行（天然兼容）。
    #[serde(default)]
    url: String,
    /// 干净底片（对齐插件 `step.afterScreenshotClean`，v0.65.247）：录制时
    /// 烧录鼠标标记之前另存的「同一帧无标注整图」。仅 click/drag 等真烧了
    /// 标记的步骤才有；导出「鼠标标注」关掉（marks=false）时用它替换
    /// screenshot_path，避免像素里残留录制时烧入的圆点/红框。
    #[serde(default)]
    before_screenshot_path: String,
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
    lang: Option<String>,
    options: Option<serde_json::Value>,
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
    // 前端不传输出路径时统一落到固定导出目录（<文档>\录步导出），不再弹保存框。
    let export_dir = resolve_export_dir(&app).ok();
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
            export_dir.as_deref(),
            Some(&progress),
            lang.as_deref(),
            options.as_ref(),
        )
    }))
    .map_err(|_| "export_recording panicked".to_string())?;

    // 全局完成提醒（任意页面弹窗 + Windows 系统通知，带教程名；成功时携带文件路径供前端点击打开）
    match &result {
        Ok(path) => crate::job_notify::emit_job_done(
            &app,
            &id,
            &title,
            crate::job_notify::JobOp::Export,
            true,
            &format!("已导出为 {}", path),
            Some(path),
        ),
        Err(e) => crate::job_notify::emit_job_done(
            &app,
            &id,
            &title,
            crate::job_notify::JobOp::Export,
            false,
            e,
            None,
        ),
    }
    result
}

/// 批量导出：把选中的若干录制各导出一个文件/文件夹，统一装进临时目录后压缩成 ZIP。
/// 每个录制独占一个子文件夹，最终返回 ZIP 文件的绝对路径。
#[tauri::command]
pub async fn batch_export_recordings(
    db: State<'_, DbState>,
    app: tauri::AppHandle,
    ids: Vec<String>,
    format: String,
    output_path: Option<String>,
    lang: Option<String>,
    options: Option<serde_json::Value>,
) -> Result<String, String> {
    let export_dir = resolve_export_dir(&app).ok();
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        batch_export_recordings_inner(
            &db,
            &ids,
            &format,
            output_path.as_deref(),
            export_dir.as_deref(),
            lang.as_deref(),
            options.as_ref(),
        )
    }))
    .map_err(|_| "batch_export_recordings panicked".to_string())?
}

#[allow(clippy::too_many_arguments)]
fn batch_export_recordings_inner(
    db: &State<DbState>,
    ids: &[String],
    format: &str,
    output_path: Option<&str>,
    default_dir: Option<&std::path::Path>,
    lang: Option<&str>,
    options: Option<&serde_json::Value>,
) -> Result<String, String> {
    if ids.is_empty() {
        return Err("未选择任何录制".to_string());
    }

    // 没给路径时落到固定导出目录（<文档>\录步导出），文件名带时间戳
    let out_path = match output_path {
        Some(p) if !p.trim().is_empty() => std::path::PathBuf::from(p),
        _ => {
            let dir = default_dir
                .map(|d| d.to_path_buf())
                .unwrap_or_else(|| std::path::PathBuf::from("output"));
            std::fs::create_dir_all(&dir)
                .map_err(|e| format!("创建输出目录失败: {}", e))?;
            let ts = chrono::Utc::now().format("%Y%m%d_%H%M%S").to_string();
            dir.join(format!("录步批量导出_{}.zip", ts))
        }
    };
    if let Some(parent) = out_path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("创建输出目录失败: {}", e))?;
    }

    let timestamp = chrono::Utc::now().format("%Y%m%d_%H%M%S").to_string();
    let temp_dir = std::env::temp_dir().join(format!("flowio_batch_{}_{}", timestamp, uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&temp_dir)
        .map_err(|e| format!("创建临时目录失败: {}", e))?;

    let mut used = std::collections::HashSet::new();
    let mut failed: Vec<String> = Vec::new();
    let mut ok = 0usize;

    for id in ids {
        let title: String = {
            let conn = db.0.lock().map_err(|e| e.to_string())?;
            conn.query_row(
                "SELECT title FROM recordings WHERE id = ?1",
                rusqlite::params![id],
                |row| row.get::<_, String>(0),
            )
            .unwrap_or_else(|_| "未命名录制".to_string())
        };

        let safe = sanitize_filename(&title);
        let base = if safe.trim().is_empty() {
            "flowio-guide".to_string()
        } else {
            safe.clone()
        };

        let mut folder_name = base.clone();
        let mut n = 2;
        while used.contains(&folder_name) {
            folder_name = format!("{} ({})", base, n);
            n += 1;
        }
        used.insert(folder_name.clone());

        let sub_dir = temp_dir.join(&folder_name);
        std::fs::create_dir_all(&sub_dir)
            .map_err(|e| format!("创建子目录失败: {}", e))?;

        let ext = extension_for_format(format);
        // 与插件 exportFilename 对齐：文件名主体统一带 -flowio 后缀
        let file_name = format!("{}-flowio.{}", base, ext);
        let file_path = sub_dir.join(file_name);

        match export_recording_inner(
            db,
            id,
            format,
            Some(file_path.to_string_lossy().to_string()),
            None,
            None,
            lang,
            options,
        ) {
            Ok(_) => ok += 1,
            Err(e) => failed.push(format!("{}: {}", title, e)),
        }
    }

    if ok == 0 {
        let _ = std::fs::remove_dir_all(&temp_dir);
        return Err(format!("全部导出失败: {}", failed.join("; ")));
    }

    zip_directory(&temp_dir, &out_path)?;
    let _ = std::fs::remove_dir_all(&temp_dir);

    if failed.is_empty() {
        Ok(out_path.to_string_lossy().to_string())
    } else {
        Ok(format!("{} (部分失败: {})", out_path.to_string_lossy().to_string(), failed.join("; ")))
    }
}

fn extension_for_format(format: &str) -> &'static str {
    match format.to_lowercase().as_str() {
        "html" => "html",
        "markdown" | "md" => "md",
        "pdf" => "pdf",
        "docx" | "word" => "docx",
        "txt" => "txt",
        "csv" => "csv",
        "xlsx" => "xlsx",
        "skill" => "md",
        "long" => "png",
        "images" => "zip",
        "pptx" => "pptx",
        "quick_video" | "recording_video" => "mp4",
        _ => "bin",
    }
}

fn zip_directory(src_dir: &std::path::Path, dst_file: &std::path::Path) -> Result<(), String> {
    let file = std::fs::File::create(dst_file)
        .map_err(|e| format!("创建压缩包失败: {}", e))?;
    let mut zip = zip::ZipWriter::new(file);
    let options = zip::write::FileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated);
    let base_len = src_dir.to_string_lossy().len();
    zip_dir_recursive(&mut zip, src_dir, base_len, options)?;
    zip.finish().map_err(|e| format!("完成压缩包失败: {}", e))?;
    Ok(())
}

fn zip_dir_recursive(
    zip: &mut zip::ZipWriter<std::fs::File>,
    dir: &std::path::Path,
    base_len: usize,
    options: zip::write::FileOptions<()>,
) -> Result<(), String> {
    for entry in std::fs::read_dir(dir).map_err(|e| format!("读取目录失败: {}", e))? {
        let entry = entry.map_err(|e| format!("读取条目失败: {}", e))?;
        let path = entry.path();
        let name = path.to_string_lossy();
        let rel = &name[base_len..].trim_start_matches('\\').trim_start_matches('/');
        let archive_name = rel.replace('\\', "/");
        if path.is_file() {
            zip.start_file(archive_name, options)
                .map_err(|e| format!("添加文件到压缩包失败: {}", e))?;
            let mut f = std::fs::File::open(&path).map_err(|e| format!("打开文件失败: {}", e))?;
            std::io::copy(&mut f, zip).map_err(|e| format!("写入压缩数据失败: {}", e))?;
        } else if path.is_dir() {
            zip.add_directory(archive_name, options)
                .map_err(|e| format!("添加目录到压缩包失败: {}", e))?;
            zip_dir_recursive(zip, &path, base_len, options)?;
        }
    }
    Ok(())
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
    default_dir: Option<&std::path::Path>,
    progress: Option<&(dyn Fn(usize, usize) + Send + Sync)>,
    lang: Option<&str>,
    options: Option<&serde_json::Value>,
) -> Result<String, String> {
    let (mut recording, mut steps) = load_export_data(db, id)?;
    // 「鼠标标注」收口（对齐插件 `opts.marks`，v0.65.247）：marks=false 时
    // 导出必须用**干净底片**（录制烧标记前另存的同一帧无标注整图），否则
    // 像素里会残留录制时烧入的圆点/红框。桌面端该图存于 before_screenshot_path。
    // 缺省 true（保留标记），与插件 marks 缺省语义一致 —— 不能用 opt_bool
    // （它缺省 false 会把缺省语义反过来）。
    // 干净底片缺失（老数据 / 手动导入步骤）时保持原图，与插件 cleanShotFor
    // 降级语义一致。注意在脱敏之前替换，保证脱敏也作用于干净图、不漏码。
    let show_marks = options
        .and_then(|o| o.get("mouseAnnotation"))
        .and_then(|v| v.as_bool())
        .unwrap_or(true);
    if !show_marks {
        for s in steps.iter_mut() {
            if !s.before_screenshot_path.trim().is_empty() {
                s.screenshot_path = std::mem::take(&mut s.before_screenshot_path);
            }
        }
    }
    if let Some(lang) = lang {
        apply_translation_override(db, id, lang, &mut recording, &mut steps)?;
    }
    // 重点图倍率：用户在编辑器里按步调过就记住，没调过一律 125%（以操作落点为原点）
    let zoom_map = load_focus_zoom_map(&db.0, id);
    // Phase C: 导出前自动脱敏（开关键 redact_before_export 默认关闭）——
    // 仅当用户在「设置 → 脱敏」中开启「导出前自动脱敏」时才执行；
    // 文本替换 + 截图生成脱敏副本，原图与数据库数据保持不动。
    let mut steps = prepare_export_steps(db, steps, progress)?;
    // 「显示来源网址」开关（对齐插件 `opts.links !== false`，**缺省显示**）：
    // 关掉时统一清空步骤 url。在这里一处收口，HTML/PDF/Word/Markdown/CSV/TXT
    // 全部格式同时生效，免得每个导出函数各判一次、日后新增格式再漏一处。
    // 注意：这里不能用 opt_bool（它缺省 false，会把缺省语义反过来）。
    let show_links = options
        .and_then(|o| o.get("links"))
        .and_then(|v| v.as_bool())
        .unwrap_or(true);
    // CSV 例外：插件的 CSV 恒含 URL 列、不受 links 开关控制，这里保持一致。
    if !show_links && !format.eq_ignore_ascii_case("csv") {
        for s in steps.iter_mut() {
            s.url.clear();
        }
    }

    // 页脚二维码（对齐插件 `opts.publicUrl` / `opts.qr`）：
    //   publicUrl —— 教程的线上地址（桌面端没有发布链路，由用户手动填公开链接；
    //                 空串 = 没发布过，二维码不出现，与插件「无公开地址不出码」一致）；
    //   qr        —— 页脚是否带「扫码看线上版」二维码，缺省 true（对齐插件缺省语义）。
    let public_url = options
        .and_then(|o| o.get("publicUrl"))
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let qr_enabled = options
        .and_then(|o| o.get("qr"))
        .and_then(|v| v.as_bool())
        .unwrap_or(true);

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
        // 没给路径时落到固定导出目录（<文档>\录步导出）；
        // 拿不到 AppHandle（理论上不会）才退回相对 output/。
        let output_dir = match default_dir {
            Some(d) => d.to_path_buf(),
            None => std::path::PathBuf::from("output"),
        };
        std::fs::create_dir_all(&output_dir)
            .map_err(|e| format!("Failed to create output directory: {}", e))?;
        unique_export_path(&output_dir, &recording.title, &format)
    };

    match format.to_lowercase().as_str() {
        "html" => {
            let content = export_html(&recording, &steps, &zoom_map, &public_url, qr_enabled);
            std::fs::write(&path, content)
                .map_err(|e| format!("Failed to write HTML: {}", e))?;
            Ok(path.to_string_lossy().to_string())
        }
        "markdown" => {
            // Screenshots placed alongside the MD file (no subdirectory)
            let parent = path.parent().unwrap_or(std::path::Path::new("."));
            let content = export_markdown(&recording, &steps, &parent, &zoom_map);
            std::fs::write(&path, content)
                .map_err(|e| format!("Failed to write Markdown: {}", e))?;
            Ok(path.to_string_lossy().to_string())
        }
        "pdf" => {
            export_pdf(&recording, &steps, &path, &zoom_map, &public_url, qr_enabled)?;
            Ok(path.to_string_lossy().to_string())
        }
        "docx" | "word" => {
            export_docx(&recording, &steps, &path, &zoom_map, &public_url, qr_enabled, true)?;
            Ok(path.to_string_lossy().to_string())
        }
        "txt" => {
            let content = export_txt(&recording, &steps);
            std::fs::write(&path, content)
                .map_err(|e| format!("Failed to write TXT: {}", e))?;
            Ok(path.to_string_lossy().to_string())
        }
        "csv" => {
            let content = export_csv(&recording, &steps);
            // UTF-8 BOM：Excel / WPS 打开中文 CSV 不乱码（与插件导出同一约定）
            let mut bytes = Vec::with_capacity(content.len() + 3);
            bytes.extend_from_slice("\u{feff}".as_bytes());
            bytes.extend_from_slice(content.as_bytes());
            std::fs::write(&path, bytes).map_err(|e| format!("Failed to write CSV: {}", e))?;
            Ok(path.to_string_lossy().to_string())
        }
        "xlsx" => {
            // Excel（对齐插件 v0.65.249 的 XLSX 导出）：真 OOXML 包，
            // 列与 CSV 同口径（序号/类型/标题/说明/URL/敏感），表头橙底白字、冻结首行。
            // 序号语义与 CSV 完全一致：文档块不占编号。
            let mut rows: Vec<Vec<(String, bool)>> = vec![vec![
                ("序号".to_string(), true),
                ("类型".to_string(), true),
                ("标题".to_string(), true),
                ("说明".to_string(), true),
                ("URL".to_string(), true),
                ("敏感".to_string(), true),
            ]];
            let mut num: i32 = 0;
            for s in &steps {
                let no = if is_doc_block(s) {
                    String::new()
                } else {
                    num += 1;
                    num.to_string()
                };
                rows.push(vec![
                    (no, false),
                    (csv_type_label(s), false),
                    (s.title.clone(), false),
                    (s.description.clone(), false),
                    (s.url.clone(), false),
                    ("否".to_string(), false),
                ]);
            }
            let bytes = build_xlsx_bytes("步骤清单", &rows)?;
            std::fs::write(&path, bytes)
                .map_err(|e| format!("Failed to write XLSX: {}", e))?;
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
            export_skill_bundle(&path, &recording, &steps, &zoom_map, &mut machine)
        }
        "long" => {
            let include_cursor = opt_bool(options, "mouseAnnotation");
            let image_zoom = opt_f64(options, "imageZoom").unwrap_or(1.0);
            let gif = opt_bool(options, "gif");
            export_long_image(&recording, &steps, &path, &zoom_map, include_cursor, image_zoom, gif)?;
            Ok(path.to_string_lossy().to_string())
        }
        "images" => {
            let include_cursor = opt_bool(options, "mouseAnnotation");
            let image_zoom = opt_f64(options, "imageZoom").unwrap_or(1.0);
            let gif = opt_bool(options, "gif");
            export_individual_images(&recording, &steps, &path, &zoom_map, include_cursor, image_zoom, gif)?;
            Ok(path.to_string_lossy().to_string())
        }
        "pptx" => {
            let include_cursor = opt_bool(options, "mouseAnnotation");
            let include_toc = opt_bool(options, "includeToc");
            export_pptx(&recording, &steps, &path, &zoom_map, include_cursor, include_toc)?;
            Ok(path.to_string_lossy().to_string())
        }
        "quick_video" => {
            let include_cursor = opt_bool(options, "mouseAnnotation");
            let gif = opt_bool(options, "gif");
            let vopt = parse_video_options(options);
            export_video(&recording, &steps, &path, &zoom_map, include_cursor, true, &vopt, gif)?;
            Ok(path.to_string_lossy().to_string())
        }
        "recording_video" => {
            let include_cursor = opt_bool(options, "mouseAnnotation");
            let gif = opt_bool(options, "gif");
            let vopt = parse_video_options(options);
            export_video(&recording, &steps, &path, &zoom_map, include_cursor, false, &vopt, gif)?;
            Ok(path.to_string_lossy().to_string())
        }
        other => Err(format!("Unsupported format: {}. Use html, markdown, txt, csv, docx, pdf, pptx, images, quick_video, recording_video, skill, or long.", other)),
    }
}

fn opt_bool(options: Option<&serde_json::Value>, key: &str) -> bool {
    options
        .and_then(|o| o.get(key))
        .and_then(|v| v.as_bool())
        .unwrap_or(false)
}

fn opt_f64(options: Option<&serde_json::Value>, key: &str) -> Option<f64> {
    options.and_then(|o| o.get(key)).and_then(|v| v.as_f64())
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
             tip, screenshot_path, x, y, crop_w, crop_h, is_variable, param_name, \
             block_kind, block_level, note_kind, title_html, image_caption, \
             COALESCE(media_kind, 'image'), COALESCE(media_poster_path, ''), \
             COALESCE(url, ''), COALESCE(before_screenshot_path, '') \
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
                block_kind: row
                    .get::<_, Option<String>>(14)?
                    .unwrap_or_else(|| "step".to_string()),
                block_level: row.get::<_, Option<i32>>(15)?.unwrap_or(1),
                note_kind: row.get::<_, Option<String>>(16)?.unwrap_or_default(),
                title_html: row.get::<_, Option<String>>(17)?.unwrap_or_default(),
                image_caption: row.get::<_, Option<String>>(18)?.unwrap_or_default(),
                media_kind: row.get::<_, Option<String>>(19)?.unwrap_or_else(|| "image".into()),
                media_poster_path: row.get::<_, Option<String>>(20)?.unwrap_or_default(),
                media_source_path: String::new(),
                url: row.get::<_, Option<String>>(21)?.unwrap_or_default(),
                before_screenshot_path: row
                    .get::<_, Option<String>>(22)?
                    .unwrap_or_default(),
            })
        })
        .map_err(|e| e.to_string())?;

    let mut steps = Vec::new();
    for row in rows {
        steps.push(row.map_err(|e| e.to_string())?);
    }

    // ── 第二十轮：视频步骤改用「首帧」──────────────────────────────
    // 长图 / Word / Markdown / 纯文本四条链路全都只认 `screenshot_path` 这一张图
    // （`std::fs::read` 之后直接丢给图片解码器）。这里一次性把 video 步骤换成它的
    // 首帧静态图 ⇒ 下游一行都不用改；首帧缺失（极端情况）就清空路径按「没图」处理，
    // 绝不把 .mp4 丢给 image 解码器。`media_kind` 保留，HTML 那侧据此标一个「视频」角标。
    for s in steps.iter_mut() {
        if s.media_kind.eq_ignore_ascii_case("video") {
            // 保留原始视频路径，供「视频转 GIF」导出使用
            s.media_source_path = s.screenshot_path.clone();
            s.screenshot_path = if s.media_poster_path.is_empty() {
                String::new()
            } else {
                s.media_poster_path.clone()
            };
        }
    }

    // ── 导出侧兜底重编号 ─────────────────────────────────────────
    // 历史删除只 DELETE 不重排，DB 里 step_number 可能留空洞（1,2,4,5…）。
    // 导出按 order_index 升序已排好，这里就地重编为连续 1..N，保证任何旧
    // 录制导出（SKILL.md/steps.json/HTML/PDF）序号都连续（2026-09-09 用户实测）。
    // order_index 同步压实，保持两字段口径一致（与 editor 合并流程同款）。
    //
    // 🔴 第十四轮修正：大标题块（block_kind='heading'）**不占编号**。
    //    此前这里把**所有行**一律编成 1..N，于是每个大标题都会被当成一个
    //    真步骤、带着空截图混进 HTML/PDF/DOCX/Markdown（用户可见的错版）。
    //    现在只给真步骤递增 step_number，大标题恒为 0。
    let mut num: i32 = 0;
    for (i, s) in steps.iter_mut().enumerate() {
        s.order_index = i as i32;
        if s.block_kind.is_empty() || s.block_kind.eq_ignore_ascii_case("step") {
            num += 1;
            s.step_number = num;
        } else {
            s.step_number = 0;
        }
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
        "csv" => "csv",
        "pdf" => "pdf",
        "docx" | "word" => "docx",
        "long" => "png",
        "images" => "zip", // 逐张图片打成「图片包」（zip），与插件交付一致
        "pptx" => "pptx",
        "quick_video" | "recording_video" => "mp4",
        "skill" => "md",
        _ => "txt",
    };
    output_dir.join(format!("{}_{}.{}", sanitized_title, timestamp, ext))
}

/// 固定导出目录：`<文档>\录步导出`。用户从「打开导出目录」按钮进来的就是这个目录。
pub(crate) fn resolve_export_dir(app: &tauri::AppHandle) -> Result<std::path::PathBuf, String> {
    use tauri::Manager;
    let base = app
        .path()
        .document_dir()
        .map_err(|e| format!("无法定位系统「文档」目录: {}", e))?;
    let dir = base.join("录步导出");
    std::fs::create_dir_all(&dir).map_err(|e| format!("创建导出目录失败: {}", e))?;
    Ok(dir)
}

/// 返回固定导出目录（顺手创建），前端用来提示/展示路径。
#[tauri::command]
pub fn get_export_dir(app: tauri::AppHandle) -> Result<String, String> {
    Ok(resolve_export_dir(&app)?.to_string_lossy().to_string())
}

/// 用资源管理器打开固定导出目录。
#[tauri::command]
pub fn open_export_dir(app: tauri::AppHandle) -> Result<String, String> {
    let dir = resolve_export_dir(&app)?;
    tauri_plugin_opener::open_path(&dir, None::<&str>)
        .map_err(|e| format!("打开导出目录失败: {}", e))?;
    Ok(dir.to_string_lossy().to_string())
}

/// 不覆盖已有文件：`标题.docx` 存在时顺延成 `标题 (2).docx`；图片包（`images`）同理。
fn unique_export_path(dir: &std::path::Path, title: &str, format: &str) -> std::path::PathBuf {
    let mut stem = sanitize_filename(title);
    if stem.trim().is_empty() {
        // 与插件 exportFilename 对齐：空标题统一回落 flowio-guide
        stem = "flowio-guide".to_string();
    }
    let ext = match format.to_lowercase().as_str() {
        "html" => "html",
        "markdown" | "md" => "md",
        "txt" => "txt",
        "pdf" => "pdf",
        "docx" | "word" => "docx",
        "long" => "png",
        "images" => "zip",
        "pptx" => "pptx",
        "quick_video" | "recording_video" => "mp4",
        "skill" => "md",
        _ => "txt",
    };
    let build = |n: usize| -> std::path::PathBuf {
        let name = if n <= 1 {
            stem.clone()
        } else {
            format!("{} ({})", stem, n)
        };
        // 与插件 exportFilename 对齐：文件名主体统一带 -flowio 后缀
        if ext.is_empty() {
            dir.join(format!("{}-flowio", name))
        } else {
            dir.join(format!("{}-flowio.{}", name, ext))
        }
    };
    let mut p = build(1);
    let mut n = 2usize;
    while p.exists() && n <= 999 {
        p = build(n);
        n += 1;
    }
    p
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

/// 导出裁剪：**与前端 cropBoxFromAnchor / 插件 computeCropRect 逻辑完全一致**，
/// 保证导出图 = 编辑器里看到的图。
///
/// 坐标语义（与录制端 centered_crop_box 一致）：x/y = 裁剪框**中心**，
/// crop_w/h = 元素矩形尺寸（仅作「有元素」标志，不参与几何）。
/// - 整窗类型（scroll/nav/screenshot）：不裁剪，原样返回；
/// - 有坐标（中心+尺寸）：恒 50%×50% 截图窗口（倍率 2×），以中心定位，
///   越界贴边平移夹回图内（不缩小窗口）；
/// - 无坐标：与前端一致返回全图（前端此场景同样显示全图）。
///
/// 5b 对齐：几何从「元素 + PAD 外扩 / type 左扩 1/3 / drag extraH」切换为
/// 插件端 `flowio-extension/src/shared/cropRegion.ts` 的 computeCropRect 口径；
/// 兜底倍率同步 40% → 50%（FOCUS_VISIBLE_FRAC=0.5）。

/// 导出裁剪框的纯几何（不碰图片字节），便于单测。
/// 实现委托给 `crop_region_50`（三端唯一纯几何：前端 / 导出 / AI）。
pub(crate) fn export_crop_box(
    img_w: i32,
    img_h: i32,
    x: Option<i32>,
    y: Option<i32>,
    crop_w: Option<i32>,
    crop_h: Option<i32>,
    action_type: &str,
) -> Option<(i32, i32, i32, i32)> {
    crop_region_50(img_w, img_h, x, y, crop_w, crop_h, action_type)
}

/// 裁图几何唯一纯函数（三端共用同一口径，5b 对齐插件 computeCropRect）：
/// **中心 + 恒 50% 窗口（FOCUS_VISIBLE_FRAC=0.5，倍率 2×）+ 贴边平移夹回**。
///
/// 消费端同步清单（任何一端改动必须同步其余端）：
///   1. `src/lib/cropAnchor.ts::cropBoxFromAnchor`（前端，PAD/type/drag 已随 5b 删除）
///   2. 本函数（导出 docx/html/pdf）
///   3. `src-tauri/src/ai/pipeline.rs::ai_crop_region`（AI 管线，常量已随 5b 对齐）
/// 录制端 `recorder/commands.rs` 仍落库「中心 + 尺寸」，尺寸仅作标志。
/// 一致性由 `npm run check:crop`（第 6 段黄金值）与 cargo 侧 `both_consumers_*` 测试共同兜底。
///
/// 分支语义（与前端 cropAdaptiveBase64 / 插件 computeCropRect 逐条对齐）：
/// - 整窗类型（scroll/nav/screenshot）→ None（前端 Stage C 原样返回）；
/// - 有元素矩形（crop_w>0 && crop_h>0）→ 恒 50% 窗口、锚点中心定位；
/// - 只有坐标且非负 → 点击点为中心、同样恒 50% 窗口（前端兜底 40% 已随 5b 改为 50%）；
/// - 其余 → None（前端同样显示全图）。
///
/// ⚠️ 与前端一致：**不校验坐标正负**，元素中心落在屏幕外时两侧行为必须一致，
/// 否则编辑器显示裁图而导出变全图。
pub(crate) fn crop_region_50(
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

    // 分支选择：有元素矩形（中心+尺寸）→ 锚点分支；只有坐标且非负 → 点击点分支。
    // ⚠️ 不校验坐标正负——前端同样不校验，元素中心落在屏幕外时两侧行为必须一致。
    let (cx, cy) = match (x, y, crop_w, crop_h) {
        (Some(a), Some(b), Some(c), Some(d)) if c > 0 && d > 0 => (a, b),
        (Some(a), Some(b), _, _) if a >= 0 && b >= 0 => (a, b),
        _ => return None,
    };

    // 恒 50% 窗口（FOCUS_VISIBLE_FRAC=0.5，倍率 2×），round 与前端 Math.round 对齐
    let rw = ((img_w as f64) * 0.5).round().max(1.0) as i32;
    let rh = ((img_h as f64) * 0.5).round().max(1.0) as i32;

    // 贴边平移夹回图内（不缩小窗口，避免内容被切掉一半）
    let rw = rw.min(img_w).max(1);
    let rh = rh.min(img_h).max(1);
    let left = (cx as f64 - (rw as f64) / 2.0).round() as i32;
    let top = (cy as f64 - (rh as f64) / 2.0).round() as i32;
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

// ── 重点图（focus shot）────────────────────────────────────────────
// 「全图 / 裁图」这一对概念已废弃（2026-09-19）：截图一律是重点图——
// 以操作落点（录制时记下的点击坐标）为原点把整图放大 125%，超出画面的
// 边缘被容器裁掉。用户可以按步微调倍率（1.0–2.0），倍率随视图偏好落库。
//
// 前端用 CSS transform 做这件事（不回流、滚动位置不跳），导出端没有 CSS，
// 于是反算出「放大后仍然可见的那块区域」并真的裁出来 —— 两者必须等价，
// 否则编辑器里看到的和导出的不是同一张图。
//
// 推导：整图 W×H、倍率 z、落点 (cx, cy)。元素盒 [0,W]×[0,H] 逆变换后
// 可见区域宽高为 W/z、H/z；落点在区域内的相对位置与其在整图内一致
// ⇒ left = cx·(1 − 1/z)，top = cy·(1 − 1/z)。

/// 重点图默认倍率（必须与前端 `StepEditor.tsx::SHOT_ZOOM_DEFAULT` 一致）
pub(crate) const FOCUS_ZOOM_DEFAULT: f64 = 1.25;

/// 重点图倍率上限（与插件 `shared/keyFrame.ts::MAX_KEY_FRAME_SCALE` 一致）
pub(crate) const MAX_KEY_FRAME_SCALE: f64 = 1.5;

/// 重点图等效裁框 `(left, top, w, h)`（原图像素坐标）。
/// 倍率 ≤ 1、落点缺失或越界 → None（调用方用整图）。
pub(crate) fn focus_shot_box(
    img_w: i32,
    img_h: i32,
    x: Option<i32>,
    y: Option<i32>,
    zoom: f64,
) -> Option<(i32, i32, i32, i32)> {
    if !zoom.is_finite() || zoom <= 1.0 + 1e-6 || img_w <= 0 || img_h <= 0 {
        return None;
    }
    let (cx, cy) = match (x, y) {
        (Some(a), Some(b)) => (a, b),
        _ => return None,
    };
    if cx < 0 || cy < 0 || cx > img_w || cy > img_h {
        return None;
    }
    let z = zoom.min(4.0);
    let w = (((img_w as f64) / z).round().max(1.0) as i32).min(img_w).max(1);
    let h = (((img_h as f64) / z).round().max(1.0) as i32).min(img_h).max(1);
    let k = 1.0 - 1.0 / z;
    let left = ((cx as f64) * k).round() as i32;
    let top = ((cy as f64) * k).round() as i32;
    Some((left.clamp(0, img_w - w), top.clamp(0, img_h - h), w, h))
}

/// 重点图裁切，返回 `(字节, 真实像素尺寸)`；倍率 ≤ 1 或落点缺失 → 原样返回整图。
pub(crate) fn maybe_focus_shot_bytes(
    raw_bytes: Vec<u8>,
    x: Option<i32>,
    y: Option<i32>,
    zoom: f64,
) -> (Vec<u8>, Option<(usize, usize)>) {
    let mut img = match image::load_from_memory(&raw_bytes) {
        Ok(i) => i,
        Err(_) => return (raw_bytes, None),
    };
    let (img_w, img_h) = (img.width() as i32, img.height() as i32);
    let full_dims = Some((img.width() as usize, img.height() as usize));
    let (left, top, rw, rh) = match focus_shot_box(img_w, img_h, x, y, zoom) {
        Some(v) => v,
        None => return (raw_bytes, full_dims),
    };
    let cropped = img.crop(left as u32, top as u32, rw as u32, rh as u32);
    let mut buf: Vec<u8> = Vec::new();
    // 输出 JPEG 质量 85（与前端 canvas.toDataURL("image/jpeg", 0.85) 一致）
    let enc = image::codecs::jpeg::JpegEncoder::new_with_quality(&mut buf, 85);
    if cropped.write_with_encoder(enc).is_err() {
        return (raw_bytes, full_dims);
    }
    (buf, Some((rw as usize, rh as usize)))
}

/// 从视图偏好（settings 表 `flowio_view::<recording_id>`）读出每步的重点图倍率。
/// 偏好缺失 / JSON 损坏 → 空表，调用方按 [`FOCUS_ZOOM_DEFAULT`] 兜底。
pub(crate) fn load_focus_zoom_map(
    db: &std::sync::Mutex<rusqlite::Connection>,
    recording_id: &str,
) -> std::collections::HashMap<String, f64> {
    let mut out = std::collections::HashMap::new();
    let value: Option<String> = match db.lock() {
        Ok(conn) => conn
            .query_row(
                "SELECT value FROM settings WHERE key = ?1",
                params![crate::editor::commands::view_setting_key(recording_id)],
                |r| r.get(0),
            )
            .ok(),
        Err(_) => None,
    };
    let Some(value) = value else {
        return out;
    };
    if let Ok(v) = serde_json::from_str::<serde_json::Value>(&value) {
        if let Some(obj) = v.get("shotZoom").and_then(|z| z.as_object()) {
            for (k, z) in obj {
                if let Some(f) = z.as_f64() {
                    out.insert(k.clone(), f);
                }
            }
        }
    }
    out
}

/// 某一步的重点图倍率：用户调过就用他的，没调过用 125% 默认。
pub(crate) fn zoom_for(zoom_map: &std::collections::HashMap<String, f64>, step_id: &str) -> f64 {
    match zoom_map.get(step_id) {
        Some(z) if z.is_finite() && *z > 0.0 => *z,
        _ => FOCUS_ZOOM_DEFAULT,
    }
}

// ── HTML export ───────────────────────────────────────────────

/// Generate the inner HTML body block (step cards).
/// Images are embedded as base64 data URIs for standalone output.
fn gen_html_body(
    recording: &ExportRecording,
    steps: &[ExportStep],
    zoom_map: &std::collections::HashMap<String, f64>,
    public_url: &str,
    qr_enabled: bool,
) -> String {
    let mut blocks: Vec<String> = Vec::with_capacity(steps.len());
    let mut num: i32 = 0;
    for (i, s) in steps.iter().enumerate() {
        // ── 大标题块：正文里的标题行，不占编号、没有截图 ──
        if is_heading_block(s) {
            blocks.push(build_heading_html(s, &(i + 1).to_string()));
            continue;
        }
        // ── 流程摘要块：正文最前面的摘要卡，同样不占编号 ──
        if is_summary_block(s) {
            let html = build_summary_html(&s.description);
            if !html.is_empty() {
                blocks.push(html);
            }
            continue;
        }
        // ── 简介块：用户自己写的一段引子，不占编号 ──
        if is_intro_block(s) {
            let html = build_intro_html(&s.description);
            if !html.is_empty() {
                blocks.push(html);
            }
            continue;
        }
        num += 1;
        {
            // 重点图（对齐插件 `isKeyFrame` / `keyShotWidth` 语义）：只有**用户显式调过
            // 倍率且 > 1** 的步才算重点（与长图同一口径）。容器按 keyShotWidth 探出
            // （width 百分数 + 负 margin 居中），img 铺满容器 —— 对齐插件 buildHtml 的 imgBlock。
            let key_on = match zoom_map.get(&s.id) {
                Some(z) if z.is_finite() && *z > 1.0 => true,
                _ => false,
            };
            let key_style = if key_on {
                let ratio = zoom_map
                    .get(&s.id)
                    .map(|z| (z.min(MAX_KEY_FRAME_SCALE) * 100.0).round())
                    .unwrap_or(125.0);
                let (w, bleed) = key_shot_width(100.0, ratio);
                let mut st = format!(" style=\"width:{}%\"", fmt_pct(w));
                if bleed > 0.0 {
                    st.push_str(&format!(";max-width:none;margin-left:-{}%", fmt_pct(bleed)));
                }
                Some(st)
            } else {
                None
            };

            let screenshot_html = if s.screenshot_path.is_empty() {
                build_screenshot_placeholder_html().to_string()
            } else {
                let raw = std::fs::read(&s.screenshot_path).ok().unwrap_or_default();
                if raw.is_empty() {
                    // 路径非空但读取失败（临时截图被清理等）：显式占位。
                    // 之前静默产出空 base64 的 <img>，整份 PDF 观感接近全空白。
                    build_screenshot_placeholder_html().to_string()
                } else {
                    // 重点图：按该步倍率裁出「放大后仍可见」的那块区域
                    let data = maybe_focus_shot_bytes(raw, s.x, s.y, zoom_for(zoom_map, &s.id)).0;
                    let encoded = base64::engine::general_purpose::STANDARD.encode(&data);
                    let mime = mime_from_path(&s.screenshot_path);
                    let img_tag = build_screenshot_img_html(mime, &encoded, s.step_number);
                    match &key_style {
                        Some(st) => format!(
                            "<div class=\"shot key\"{st}>{img_tag}</div>",
                            st = st,
                            img_tag = img_tag
                        ),
                        None => format!("<div class=\"shot\">{img_tag}</div>", img_tag = img_tag),
                    }
                }
            };

            // 步骤头（编号圆圈 + 类型 + 重点徽章）→ 标题 → 截图 → 图片描述 → 说明 → 来源网址 → 提示，
            // 顺序与预览逐项对齐。锚点 id="step-(i+1)" 与标题块同一计数（目录跳转目标）。
            let header = build_step_header_html(num, &s.action_type, &s.title, key_on);
            let title_html = build_step_title_html(&s.title);
            let desc_block = build_step_desc_html_kind(&s.description, &s.note_kind);
            let url_block = build_step_url_html(&s.url);
            let tip_block = build_step_tip_html(&s.tip);
            let caption_html = build_caption_html(&s.image_caption);

            blocks.push(format!(
                r#"    <div class="step-card" id="step-{anchor}">
      <div class="step-header">{header}</div>
{title_html}      {screenshot_html}
{caption_html}{desc_block}{url_block}{tip_block}    </div>"#,
                anchor = i + 1,
                header = header,
                title_html = title_html,
                desc_block = desc_block,
                url_block = url_block,
                tip_block = tip_block,
                caption_html = caption_html,
                screenshot_html = screenshot_html,
            ));
        }
    }
    // 目录导航（对齐插件 buildHtml 的 tocHtml）：steps>1 时生成，锚点指回各 step-N。
    // 插在正文块最前（容器内与步骤同宽同 padding，与插件「toc 与正文同一版心」一致）。
    let toc_html = build_toc_html(steps);
    let step_cards = if toc_html.is_empty() {
        blocks.join("\n")
    } else {
        format!("{}{}", toc_html, blocks.join("\n"))
    };

    // 页头「步骤数」用**真步骤数**而非 DB 的 total_steps：历史删除不同步该字段
    // （2026-09-09 用户实测），且大标题块本就不该计入（第十四轮）。
    build_html_document(
        &recording.title,
        real_step_count(steps),
        &recording.created_at,
        &step_cards,
        public_url,
        qr_enabled,
    )
}

/// Generate standalone HTML with base64-embedded screenshots.
fn export_html(
    recording: &ExportRecording,
    steps: &[ExportStep],
    zoom_map: &std::collections::HashMap<String, f64>,
    public_url: &str,
    qr_enabled: bool,
) -> String {
    gen_html_body(recording, steps, zoom_map, public_url, qr_enabled)
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

/// Generate a clean Markdown document:
///   - Title + metadata row
///   - Numbered step cards with header, description, tip
///   - Screenshots embedded as base64 data URLs (single portable file)
///
/// 与插件 `buildMarkdown` 对齐：截图不再只写图注，而是按该步倍率裁好
/// 后以 data URL 内嵌；不额外产出截图文件目录。
fn export_markdown(recording: &ExportRecording, steps: &[ExportStep], _ss_dir: &std::path::Path, zoom_map: &std::collections::HashMap<String, f64>) -> String {
    let mut parts: Vec<String> = Vec::with_capacity(steps.len());
    let mut num: i32 = 0;
    for s in steps {
        // 大标题块 → Markdown 标题（H1~H4 映射到 ##~#####，让 MD 的大纲
        // 与左栏目录/预览完全同构；文档自身标题占用 #）。
        if is_heading_block(s) {
            let lv = heading_level(s) + 1;
            let inner = rich_inline_html(&s.title, &s.title_html);
            parts.push(format!("{} {}", "#".repeat(lv as usize), inner));
            continue;
        }
        // 流程摘要块：正文最前面的摘要（总览段 + 要点列表）
        if is_summary_block(s) {
            let md = build_md_summary(&s.description);
            if !md.is_empty() {
                parts.push(md);
            }
            continue;
        }
        // 简介块：用户自己写的一段引子（MD 里就是普通段落，换行原样保留）
        if is_intro_block(s) {
            let t = s.description.trim();
            if !t.is_empty() {
                parts.push(format!("{}\n", t));
            }
            continue;
        }
        num += 1;
        let header = build_md_step_header(num, &s.action_type, &s.title);
        let shot_block = md_screenshot_block(s, zoom_map);
        let caption_block = if s.image_caption.trim().is_empty() {
            String::new()
        } else {
            format!("*{}*\n\n", s.image_caption.trim())
        };
        let desc_block = build_md_desc_block_kind(&s.description, &s.note_kind);
        let url_block = build_md_url_block(&s.url);
        let tip_block = build_md_tip_block(&s.tip);
        // 顺序与预览一致：标题 → 截图 → 图片描述 → 补充说明 → 来源网址 → 提示
        // （来源网址紧跟在说明之后，与插件 buildMarkdown 的写法一致）
        parts.push(format!(
            "{header}\n\n{shot_block}{caption_block}{desc_block}{url_block}{tip_block}",
            header = header,
            shot_block = shot_block,
            caption_block = caption_block,
            desc_block = desc_block,
            url_block = url_block,
            tip_block = tip_block,
        ));
    }
    let step_list = parts.join("\n---\n\n");

    build_md_document(
        &recording.title,
        &recording.app_name,
        real_step_count(steps),
        &recording.created_at,
        &step_list,
    )
}

/// Markdown 步骤截图块：与 HTML/PDF 完全同源的取图逻辑（按该步倍率裁出
/// 重点区域），再以 base64 data URL 内嵌，保证导出单文件可移植。
///
/// 截图缺失或读取失败时返回空串——不产出空 `![]()` 标签（与 HTML 侧
/// 占位策略不同：MD 无占位图语义，直接省略更干净）。
fn md_screenshot_block(s: &ExportStep, zoom_map: &std::collections::HashMap<String, f64>) -> String {
    if s.screenshot_path.is_empty() {
        return String::new();
    }
    let raw = match std::fs::read(&s.screenshot_path) {
        Ok(r) if !r.is_empty() => r,
        _ => return String::new(),
    };
    let data = maybe_focus_shot_bytes(raw, s.x, s.y, zoom_for(zoom_map, &s.id)).0;
    if data.is_empty() {
        return String::new();
    }
    let encoded = base64::engine::general_purpose::STANDARD.encode(&data);
    let mime = mime_from_path(&s.screenshot_path);
    format!("![](data:{};base64,{})\n\n", mime, encoded)
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
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
struct SkillStepData {
    /// 步号（与 SKILL.md 的编号、screenshots/step-NN 一一对应）。
    #[serde(default)]
    n: i32,
    /// 动作类型：click / dblclick / rightclick / type / paste / drag / scroll / key / hotkey。
    #[serde(default)]
    action: String,
    /// 人读意图（步骤标题）。
    #[serde(default)]
    intent: String,
    /// 该意图**没说清点了什么**（形如「点击在「X」中」只报窗口）。
    /// 置 true 时消费方应以截图为准判断目标，别信这行文字。
    #[serde(default)]
    intent_vague: bool,
    /// 目标窗口标题（录制时的前台窗口）。
    #[serde(default)]
    window: String,
    /// 录制时的落点（屏幕逻辑坐标）。drag 用真实起点而非裁图框中心。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    point: Option<[i32; 2]>,
    /// 拖拽终点。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    drag_to: Option<[i32; 2]>,
    /// 滚轮增量（正=向上）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    scroll_delta: Option<i32>,
    /// 结构化组合键，如 `Ctrl+Shift+S`。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    hotkey: Option<String>,
    /// 待输入文本（type / paste 步）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    text: Option<String>,
    /// 控件锚点（类名 + 窗口内归一化比例），比绝对坐标抗窗口移动。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    anchor: Option<serde_json::Value>,
    /// 该步截图在包内的相对路径（bundle 落盘时回填）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
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
#[derive(Debug, Clone, Default, PartialEq, serde::Deserialize)]
struct SkillQuality {
    #[serde(default)]
    total: usize,
    /// 意图含糊（只报窗口 / 裸动词 / 空）的步数。JSON key 用可读性更强的 `intent_vague`。
    #[serde(rename = "intent_vague", default)]
    vague: usize,
    /// 有落点坐标的步数。
    #[serde(default)]
    with_point: usize,
    /// 有控件锚点的步数。
    #[serde(default)]
    with_anchor: usize,
    /// 有截图的步数。JSON key 与导出一致用 `with_screenshot`。
    #[serde(rename = "with_screenshot", default)]
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
         FROM steps WHERE recording_id = ?1 \
           AND COALESCE(block_kind, 'step') = 'step' \
         ORDER BY order_index",
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
    // 键与 load_export_data 的兜底重编号保持同一口径：按 order_index 顺序 1..N。
    // DB 的 step_number 在历史删除后可能有空洞，直接用会与展示序号错位。
    let mut seq = 0i32;
    for row in rows.filter_map(|r| r.ok()) {
        let (_, action, title, window, x, y, sx, sy, ex, ey, scroll, hotkey, anchor, desc) = row;
        seq += 1;
        let n = seq;
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
        real_step_count(steps)
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
    // 文档块（大标题 / 流程摘要 / 简介）不是可执行步骤：不进编号列表
    // （否则会写出一条「0. **标题**」；第十九轮起统一用 is_doc_block 判定，
    // 之前只挡了大标题，摘要块会漏进来变成 0 号步骤）
    for s in steps.iter().filter(|s| !is_doc_block(s)) {
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
    zoom_map: &std::collections::HashMap<String, f64>,
    machine: &mut SkillMachine,
) -> Result<String, String> {
    let parent = output_path
        .parent()
        .unwrap_or(std::path::Path::new("."));
    let ss_dir = parent.join("screenshots");
    let mut images: Vec<(i32, String)> = Vec::new();
    // 始终创建 screenshots/ 目录（即便本次没有任何截图）：skill.json 的
    // files.screenshots 永远指向它，目录缺失会让包在加载/校验时被判坏
    //（load_skill_package 会据此拒绝登记）。空目录也是合法捆绑包。
    std::fs::create_dir_all(&ss_dir)
        .map_err(|e| format!("Failed to create screenshots directory: {}", e))?;
    for s in steps {
        if s.screenshot_path.is_empty() {
            continue;
        }
        let raw = match std::fs::read(&s.screenshot_path) {
            Ok(b) if !b.is_empty() => b,
            _ => continue,
        };
        // 重点图：按该步倍率裁出「放大后仍可见」的那块区域
        let bytes = maybe_focus_shot_bytes(raw, s.x, s.y, zoom_for(zoom_map, &s.id)).0;
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

// ── Skill 包加载 / 校验（route ②「MCP 技能包」的读侧）─────────────
//
// `export_skill_bundle` 是写侧：把录制落盘成 SKILL.md + screenshots/ + steps.json
// + skill.json。光有写侧还不够——一个 MCP host / 外部 Agent 要「登记并执行」这个
// skill，得先把包**读回来并校验**。`read_skill_package` 就是读侧核心：解析 skill.json、
// 核对它引用的文件确实存在、读回步数，返回规范化结构。对外的 Tauri 命令
// `load_skill_package(String)` 直接封装它（JS 传字符串路径）。与写侧对称，构成
// 导出 ↔ 注册 的可往返契约。

#[derive(Debug, Clone, Default, serde::Deserialize)]
struct SkillManifestFiles {
    #[serde(default)]
    skill: String,
    #[serde(default)]
    screenshots: String,
    #[serde(default)]
    steps: String,
}

#[derive(Debug, Clone, serde::Deserialize)]
struct SkillManifest {
    schema: String,
    name: String,
    title: String,
    #[serde(default)]
    app: String,
    #[serde(default)]
    recorded_at: String,
    #[serde(default)]
    files: SkillManifestFiles,
    #[serde(default)]
    quality: SkillQuality,
}

#[derive(Debug, Clone, serde::Deserialize)]
struct SkillStepsFile {
    #[serde(default)]
    steps: Vec<SkillStepData>,
}

/// 规范化后的 skill 包视图，供注册表 / 执行器消费。
#[derive(Debug, Clone)]
struct SkillPackage {
    name: String,
    title: String,
    app: String,
    recorded_at: String,
    skill_md: std::path::PathBuf,
    screenshots_dir: std::path::PathBuf,
    steps_file: Option<std::path::PathBuf>,
    step_count: usize,
    quality: SkillQuality,
}

impl SkillPackage {
    /// 注册表友好摘要（可被 Tauri 命令或 MCP 桥直接序列化出去）。
    fn summary(&self) -> serde_json::Value {
        serde_json::json!({
            "name": self.name,
            "title": self.title,
            "app": self.app,
            "recorded_at": self.recorded_at,
            "skill_md": self.skill_md.to_string_lossy(),
            "screenshots_dir": self.screenshots_dir.to_string_lossy(),
            "steps_file": self.steps_file.as_ref().map(|p| p.to_string_lossy()),
            "step_count": self.step_count,
            "quality": {
                "total": self.quality.total,
                "intent_vague": self.quality.vague,
                "with_point": self.quality.with_point,
                "with_anchor": self.quality.with_anchor,
                "with_shot": self.quality.with_shot,
            },
        })
    }
}

/// 读取并校验一个导出的 skill 包目录。任一引用缺失 / schema 不符都返回 Err——
/// 调用方（MCP host）据此拒绝登记坏包，而非带着半截文件去执行。
/// （`#[tauri::command] load_skill_package` 是其对外封装，接收 JS 传来的字符串路径，
///  本函数是其纯 Rust 内核，接收 `&Path`，单测直接调用它。）
fn read_skill_package(dir: &std::path::Path) -> Result<SkillPackage, String> {
    let manifest_path = dir.join("skill.json");
    let text = std::fs::read_to_string(&manifest_path)
        .map_err(|e| format!("读取 skill.json 失败: {}", e))?;
    let manifest: SkillManifest =
        serde_json::from_str(&text).map_err(|e| format!("解析 skill.json 失败: {}", e))?;
    if manifest.schema != "flowio.skill.manifest/v1" {
        return Err(format!("skill.json schema 不符，期望 flowio.skill.manifest/v1，实得 {}", manifest.schema));
    }
    // 校验被引用的文件确实存在（缺一个都不许登记）。
    let skill_path = dir.join(&manifest.files.skill);
    if manifest.files.skill.is_empty() || !skill_path.is_file() {
        return Err(format!("skill.json 引用的 SKILL.md（{}）不存在", manifest.files.skill));
    }
    let ss_dir = dir.join(&manifest.files.screenshots);
    if manifest.files.screenshots.is_empty() || !ss_dir.is_dir() {
        return Err(format!("screenshots 目录（{}）不存在", manifest.files.screenshots));
    }
    let mut steps_file: Option<std::path::PathBuf> = None;
    let mut step_count = 0usize;
    if !manifest.files.steps.is_empty() {
        let sp = dir.join(&manifest.files.steps);
        if !sp.is_file() {
            return Err(format!("skill.json 引用的 steps.json（{}）不存在", manifest.files.steps));
        }
        if let Ok(stext) = std::fs::read_to_string(&sp) {
            if let Ok(file) = serde_json::from_str::<SkillStepsFile>(&stext) {
                step_count = file.steps.len();
            }
        }
        steps_file = Some(sp);
    }
    Ok(SkillPackage {
        name: manifest.name,
        title: manifest.title,
        app: manifest.app,
        recorded_at: manifest.recorded_at,
        skill_md: skill_path,
        screenshots_dir: ss_dir,
        steps_file,
        step_count,
        quality: manifest.quality,
    })
}

/// 对外 Tauri 命令：把一个已导出的 skill 包目录（JS 传来的字符串路径）读回并校验，
/// 返回注册表友好摘要（`SkillPackage::summary()`）。任一引用缺失 / schema 不符都返回
/// Err——MCP host / 外部 Agent 据此拒绝登记坏包，而非带着半截文件去执行。
///
/// 纯文件读取、同步、快，无需异步运行时。
#[tauri::command]
pub fn load_skill_package(dir: String) -> Result<serde_json::Value, String> {
    let p = std::path::Path::new(&dir);
    let pkg = read_skill_package(p)?;
    Ok(pkg.summary())
}

// ── CSV export ────────────────────────────────────────────────

/// CSV 单元格转义：含逗号/引号/换行时整体加引号，内部引号翻倍。
/// 与插件 `csvCell` 同规则，Excel / WPS 可直接打开。
fn csv_cell(s: &str) -> String {
    let v = s.replace('"', "\"\"");
    if v.contains(',') || v.contains('"') || v.contains('\n') || v.contains('\r') {
        format!("\"{}\"", v)
    } else {
        v
    }
}

/// CSV「类型」列取值：文档块给中文块名，普通步骤沿用 action_type。
pub(crate) fn csv_type_label(s: &ExportStep) -> String {
    if is_heading_block(s) {
        "标题".to_string()
    } else if is_summary_block(s) {
        "摘要".to_string()
    } else if is_intro_block(s) {
        "简介".to_string()
    } else {
        s.action_type.clone()
    }
}

/// 生成 CSV 文本（表格格式，含 BOM 由写文件侧添加）。
/// 列与插件 `buildCsv` 对齐：序号 / 类型 / 标题 / 说明 / URL / 敏感。
/// 截图是二进制，不进入 CSV。
///
/// 已对齐插件口径（2026-09-25）：
///   1. 序号 = 数组索引全编号（文档块 doc_block 也编号，与插件 `i + 1` 一致）；
///   2. URL 列取步骤来源网址（`steps.url`，空串 = 空单元格）；敏感列暂恒为「否」——
///      桌面端 steps 表尚无步骤级 sensitive 字段（已核查），待字段落地后回填。
fn export_csv(recording: &ExportRecording, steps: &[ExportStep]) -> String {
    let _ = recording;
    let mut out = String::from("序号,类型,标题,说明,URL,敏感\r\n");
    for (idx, s) in steps.iter().enumerate() {
        let no = (idx + 1).to_string();
        out.push_str(&format!(
            "{},{},{},{},{},{}\r\n",
            csv_cell(&no),
            csv_cell(&csv_type_label(s)),
            csv_cell(&s.title),
            csv_cell(&s.description),
            csv_cell(&s.url),
            "否",
        ));
    }
    out
}

// ── TXT export ────────────────────────────────────────────────

/// 纯文本导出：标题 + 元信息 + 编号步骤（标题 / 描述 / 提示）。
/// 文本导出不含截图，故忽略 zoom_map。
///
/// 已按插件 `buildTxt` 口径对齐（2026-09-25）：
///   - 头部 `═══ 标题 ═══` + 录制时间 + `步骤数 | 时长`；
///   - 步骤行 `[N] 标题` + `类型：xxx`；
///   - 结尾 `── 由 录步 Flowio 导出 ──`。
/// 桌面端领先项（图片描述 / 标注 / 警告 / 链接 / 提示行）保留不删。
fn export_txt(recording: &ExportRecording, steps: &[ExportStep]) -> String {
    let mut out = String::new();
    out.push_str(&format!("═══ {} ═══\n", recording.title.trim()));
    out.push_str(&format!(
        "录制时间：{}\n步骤数：{}",
        recording.created_at,
        real_step_count(steps),
    ));
    // 时长：桌面端 recordings 表未落录制总时长字段（仅 total_steps），
    // 不编造分钟数，等字段落地后再补 `| 时长：约 N 分钟`。
    out.push_str("\n\n");
    for s in steps {
        // 大标题块：缩进体现层级，与目录/预览的层次一致
        if is_heading_block(s) {
            let lv = heading_level(s);
            out.push_str(&format!(
                "{}【{}】\n\n",
                "  ".repeat((lv - 1) as usize),
                s.title
            ));
            continue;
        }
        // 流程摘要块：纯文本里用「摘要」标签把总览与要点列出来
        if is_summary_block(s) {
            let (intro, bullets) = parse_summary(&s.description);
            if !intro.is_empty() {
                out.push_str(&format!("摘要：{}\n", intro));
            }
            for (label, body) in &bullets {
                if label.is_empty() {
                    out.push_str(&format!("  · {}\n", body));
                } else {
                    out.push_str(&format!("  · {}：{}\n", label, body));
                }
            }
            if !intro.is_empty() || !bullets.is_empty() {
                out.push('\n');
            }
            continue;
        }
        // 简介块：纯文本里原样输出那段引子
        if is_intro_block(s) {
            let t = s.description.trim();
            if !t.is_empty() {
                out.push_str(&format!("{}\n\n", t));
            }
            continue;
        }
        out.push_str(&format!("[{}] {}\n", s.step_number, s.title));
        if !s.action_type.is_empty() {
            out.push_str(&format!("类型：{}\n", to_action_label(&s.action_type)));
        }
        if !s.image_caption.trim().is_empty() {
            out.push_str(&format!("图片描述：{}\n", s.image_caption.trim()));
        }
        if !s.description.is_empty() {
            let prefix = match s.note_kind.as_str() {
                "mark" => "标注：",
                "warn" => "警告：",
                _ => "",
            };
            out.push_str(&format!("{}{}\n", prefix, s.description));
        }
        if !s.url.trim().is_empty() {
            out.push_str(&format!("链接：{}\n", s.url.trim()));
        }
        if !s.tip.is_empty() {
            out.push_str(&format!("提示：{}\n", s.tip));
        }
        out.push('\n');
    }
    out.push_str("── 由 录步 Flowio 导出 ──\n");
    out
}

// ── Long image (stitched) export ──────────────────────────────

/// 长图：把「有截图的步骤」竖向拼成一张 PNG。
///
/// ★ 已按插件口径重做（对齐 flowio-extension 的 lib/export.ts::buildLongImage
///   与 shared/keyFrame.ts::longImageLayout）：
///   1. **纯截图拼接** —— 不再有教程标题头、步骤标题条、文字横幅，画面里只有截图；
///   2. 基准宽度 baseW = min(各图最大自然宽, 1440)，图与图之间 24px 白色间隔；
///   3. **重点图**（用户显式调过倍率且 > 1）按倍率放大、画布跟着变宽，普通图居中；
///   4. 重点图下方补一条「为什么是重点」说明条（淡橙底 + 左侧橙色竖条 + 标题 +
///      最多 4 行描述），排版常量与插件的 NOTE_* 同口径。
fn export_long_image(
    recording: &ExportRecording,
    steps: &[ExportStep],
    output_path: &std::path::Path,
    zoom_map: &std::collections::HashMap<String, f64>,
    include_cursor: bool,
    image_zoom: f64,
    _gif: bool,
) -> Result<(), String> {
    let _ = recording; // 插件口径的长图不使用教程标题头
    /// 步骤间隔（与插件 buildLongImage 的 GAP 一致）
    const GAP: u32 = 24;
    /// 基准宽度上限（与插件 MAX_W 一致）
    const MAX_W: u32 = 1440;
    /// 重点图倍率上限（与插件 MAX_KEY_FRAME_SCALE 一致）
    const MAX_SCALE: f64 = 1.5;
    // ── 说明条排版常量（与插件 NOTE_* 同一套数字）──
    const NOTE_PAD: u32 = 16;
    const NOTE_TITLE_SIZE: f32 = 20.0;
    const NOTE_DESC_SIZE: f32 = 16.0;
    const NOTE_TITLE_LH: u32 = 28;
    const NOTE_DESC_LH: u32 = 24;
    const NOTE_MAX_DESC_LINES: usize = 4;
    const NOTE_BG: image::Rgba<u8> = image::Rgba([255, 247, 242, 255]); // #fff7f2
    const NOTE_BAR: image::Rgba<u8> = image::Rgba([255, 107, 53, 255]); // #ff6b35
    const NOTE_TITLE_COLOR: image::Rgba<u8> = image::Rgba([17, 24, 39, 255]); // #111827
    const NOTE_DESC_COLOR: image::Rgba<u8> = image::Rgba([55, 65, 81, 255]); // #374151
    const WHITE: image::Rgba<u8> = image::Rgba([255, 255, 255, 255]);

    let font = get_font();

    // 1) 收集「有截图的步骤」：与插件一致 —— 文档块（大标题 / 摘要 / 简介）不占位、不渲染
    struct Prepped {
        img: image::DynamicImage,
        scale: f64,
        title: String,
        desc: String,
    }
    let mut items: Vec<Prepped> = Vec::new();
    for s in steps {
        if is_doc_block(s) || s.screenshot_path.is_empty() {
            continue;
        }
        let raw = match std::fs::read(&s.screenshot_path) {
            Ok(b) if !b.is_empty() => b,
            _ => continue,
        };
        let mut img = match image::load_from_memory(&raw) {
            Ok(i) => i,
            Err(_) => continue,
        };
        if include_cursor {
            draw_step_cursor(&mut img, s.x, s.y);
        }
        // 重点图倍率：只有**用户显式标记过且 > 1** 的步才算重点（对齐插件 keyFrame 语义）。
        // 桌面端 shotZoom 的缺省 1.25 是编辑器的「显示态」默认，若照搬，长图会变成
        // 每张图都挂说明条 —— 那不是插件的长图。
        let scale = match zoom_map.get(&s.id) {
            Some(z) if z.is_finite() && *z > 1.0 => z.min(MAX_SCALE),
            _ => 1.0,
        };
        let title = if !s.title.trim().is_empty() {
            s.title.trim().to_string()
        } else if !s.action_type.is_empty() {
            to_action_label(&s.action_type)
        } else {
            String::new()
        };
        items.push(Prepped {
            img,
            scale,
            title,
            desc: s.description.trim().to_string(),
        });
    }
    if items.is_empty() {
        return Err("该教程没有任何截图，无法导出长图".to_string());
    }

    // 2) 版式（对齐 longImageLayout）：基准宽度 → 画布宽度 → 每格 x / y / 说明条高度
    let zoom_ratio = image_zoom.max(0.1).min(2.0);
    let base_w = ((items.iter().map(|i| i.img.width()).max().unwrap_or(960) as f64) * zoom_ratio)
        .round()
        .max(1.0) as u32;
    let base_w = base_w.min(MAX_W).max(1);
    let max_scale = items.iter().map(|i| i.scale).fold(1.0_f64, f64::max);
    let canvas_w = (((base_w as f64) * max_scale).round().max(1.0)) as u32;

    struct Slot {
        w: u32,
        h: u32,
        x: u32,
        y: u32,
        note_h: u32,
    }
    let mut slots: Vec<Slot> = Vec::with_capacity(items.len());
    let mut cursor_y: u32 = 0;
    for it in items.iter() {
        let src_w = it.img.width().max(1);
        let src_h = it.img.height().max(1);
        let w = (((base_w as f64) * it.scale).round().max(1.0)) as u32;
        let h = (((src_h as f64) * (w as f64) / (src_w as f64)).round().max(1.0)) as u32;
        // 说明条只挂在重点图上；标题与描述都为空则不出条（别在长图里塞空条）
        let note_h = if it.scale > 1.0 {
            let inner_w = w.saturating_sub(NOTE_PAD * 2).max(40);
            let title_h = if it.title.is_empty() { 0 } else { NOTE_TITLE_LH };
            let desc_lines = if it.desc.is_empty() {
                0
            } else {
                wrap_cjk(&it.desc, NOTE_DESC_SIZE, inner_w)
                    .len()
                    .min(NOTE_MAX_DESC_LINES) as u32
            };
            if title_h == 0 && desc_lines == 0 {
                0
            } else {
                NOTE_PAD * 2 + title_h + desc_lines * NOTE_DESC_LH
            }
        } else {
            0
        };
        let x = canvas_w.saturating_sub(w) / 2;
        slots.push(Slot { w, h, x, y: cursor_y, note_h });
        cursor_y += h + note_h + GAP;
    }
    let canvas_h = cursor_y.saturating_sub(GAP).max(1);

    // 3) 画布：白底 → 逐格贴图 → 说明条
    let mut canvas =
        image::ImageBuffer::<image::Rgba<u8>, Vec<u8>>::from_pixel(canvas_w, canvas_h, WHITE);
    for (i, it) in items.iter().enumerate() {
        let slot = &slots[i];
        let scaled = if slot.w != it.img.width() || slot.h != it.img.height() {
            it.img.resize_exact(slot.w, slot.h, image::imageops::FilterType::Lanczos3)
        } else {
            it.img.clone()
        };
        image::imageops::overlay(&mut canvas, &scaled.to_rgba8(), slot.x as i64, slot.y as i64);
        if slot.note_h == 0 {
            continue;
        }
        let ny = slot.y + slot.h;
        // 淡橙底 + 左侧 4px 橙条（与导出件的重点色同一套）
        for px in slot.x..(slot.x + slot.w).min(canvas_w) {
            for py in ny..(ny + slot.note_h).min(canvas_h) {
                canvas.put_pixel(px, py, NOTE_BG);
            }
        }
        for px in slot.x..(slot.x + 4).min(canvas_w) {
            for py in ny..(ny + slot.note_h).min(canvas_h) {
                canvas.put_pixel(px, py, NOTE_BAR);
            }
        }
        let Some(ref f) = font else { continue };
        let tx = (slot.x + NOTE_PAD) as i32;
        let inner_w = slot.w.saturating_sub(NOTE_PAD * 2).max(40);
        let mut ty = (ny + NOTE_PAD) as i32;
        if !it.title.is_empty() {
            let scale_title = ab_glyph::PxScale { x: NOTE_TITLE_SIZE, y: NOTE_TITLE_SIZE };
            // 说明条标题只有一行：排不下直接截断（与插件 drawRuns 同口径，宁可少字不撑破版面）
            let mut shown = String::new();
            for ch in it.title.chars() {
                let candidate = format!("{}{}", shown, ch);
                let (tw, _) = imageproc::drawing::text_size(scale_title, f, &candidate);
                if tw > inner_w {
                    break;
                }
                shown = candidate;
            }
            imageproc::drawing::draw_text_mut(&mut canvas, NOTE_TITLE_COLOR, tx, ty, scale_title, f, &shown);
            ty += NOTE_TITLE_LH as i32;
        }
        if !it.desc.is_empty() {
            let scale_desc = ab_glyph::PxScale { x: NOTE_DESC_SIZE, y: NOTE_DESC_SIZE };
            for (k, ln) in wrap_cjk(&it.desc, NOTE_DESC_SIZE, inner_w)
                .iter()
                .take(NOTE_MAX_DESC_LINES)
                .enumerate()
            {
                imageproc::drawing::draw_text_mut(
                    &mut canvas,
                    NOTE_DESC_COLOR,
                    tx,
                    ty + (k as i32) * (NOTE_DESC_LH as i32),
                    scale_desc,
                    f,
                    ln,
                );
            }
        }
    }

    canvas
        .save(output_path)
        .map_err(|e| format!("保存长图失败: {}", e))?;
    Ok(())
}


///
/// 字号按预览的 28/23/19/17px 等比放大到长图正文尺度（步骤标题 32px / 预览 16px
/// ⇒ 约 1.45 倍），保证层级关系与预览一致：H1 > H2 > H3 > H4 > 步骤标题。
fn heading_banner(
    target_w: u32,
    s: &ExportStep,
    font: Option<&ab_glyph::FontArc>,
) -> image::DynamicImage {
    let lv = heading_level(s);
    let px: f32 = match lv {
        1 => 41.0,
        2 => 33.0,
        3 => 28.0,
        _ => 25.0,
    };
    let h = px as u32 + 30;
    let mut canvas = image::ImageBuffer::<image::Rgba<u8>, Vec<u8>>::from_pixel(
        target_w,
        h,
        image::Rgba([255, 255, 255, 255]),
    );
    if let Some(f) = font {
        let text = if s.title.trim().is_empty() {
            "（空标题）"
        } else {
            s.title.trim()
        };
        let scale = ab_glyph::PxScale { x: px, y: px };
        imageproc::drawing::draw_text_mut(
            &mut canvas,
            image::Rgba([26, 26, 46, 255]), // #1a1a2e，与预览标题同色
            24,
            12,
            scale,
            f,
            text,
        );
    }
    image::DynamicImage::ImageRgba8(canvas)
}

/// 文档块（简介 / 流程摘要）在长图里的横幅文字。
/// 简介直接用正文；摘要复用 `parse_summary`，把要点拼成「· 名字：说明」。
fn doc_block_text(s: &ExportStep) -> String {
    if is_intro_block(s) {
        return s.description.trim().to_string();
    }
    let (intro, bullets) = parse_summary(&s.description);
    let mut out = intro;
    for (label, body) in bullets {
        if !out.is_empty() {
            out.push('\n');
        }
        if label.is_empty() {
            out.push_str(&format!("· {}", body));
        } else {
            out.push_str(&format!("· {}：{}", label, body));
        }
    }
    out
}

/// 按目标宽度折行（没有字体度量就按字号估算：中日韩字符算 1 个字宽、
/// 其余算半字宽）。返回每行的文本。
fn wrap_cjk(text: &str, px: f32, max_w: u32) -> Vec<String> {
    let per_line = ((max_w as f32) / px).max(4.0) as usize;
    let mut lines: Vec<String> = Vec::new();
    for raw in text.split('\n') {
        let mut cur = String::new();
        let mut used: f32 = 0.0;
        for ch in raw.chars() {
            let w = if (ch as u32) < 0x2E80 { 0.55 } else { 1.0 };
            if used + w > per_line as f32 && !cur.is_empty() {
                lines.push(std::mem::take(&mut cur));
                used = 0.0;
            }
            cur.push(ch);
            used += w;
        }
        lines.push(cur); // 空行也保留（段落分隔）
    }
    lines
}

/// 一段纯文字的横幅（简介 / 摘要用）。空内容返回 None —— 别在长图里塞白条。
fn text_banner(
    target_w: u32,
    text: &str,
    font: Option<&ab_glyph::FontArc>,
) -> Option<image::DynamicImage> {
    let t = text.trim();
    if t.is_empty() {
        return None;
    }
    let px: f32 = 26.0;
    let line_h = (px * 1.5) as u32;
    let lines = wrap_cjk(t, px, target_w.saturating_sub(48));
    let h = line_h * lines.len() as u32 + 24;
    let mut canvas = image::ImageBuffer::<image::Rgba<u8>, Vec<u8>>::from_pixel(
        target_w,
        h,
        image::Rgba([255, 255, 255, 255]),
    );
    if let Some(f) = font {
        let scale = ab_glyph::PxScale { x: px, y: px };
        for (i, line) in lines.iter().enumerate() {
            if line.is_empty() {
                continue;
            }
            imageproc::drawing::draw_text_mut(
                &mut canvas,
                image::Rgba([63, 70, 82, 255]), // #3f4652，与预览 .sop-preview-intro 同色
                24,
                12 + (i as u32 * line_h) as i32,
                scale,
                f,
                line,
            );
        }
    }
    Some(image::DynamicImage::ImageRgba8(canvas))
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

// ── DOCX 富文本（第十四轮）────────────────────────────────────

/// 解析中的内联样式状态（只保留白名单子集里出现过的属性）。
#[derive(Clone, Default)]
struct InlineStyleState {
    bold: bool,
    italic: bool,
    underline: bool,
    strike: bool,
    color: Option<String>,
    bg: Option<String>,
}

/// 按给定样式构造一个 Word run。
fn docx_style_run(
    text: &str,
    st: &InlineStyleState,
    base_size: usize,
    base_color: &str,
    brk: bool,
) -> Run {
    let mut run = Run::new().size(base_size);
    if brk {
        run = run.add_break(BreakType::TextWrapping);
    }
    if !text.is_empty() {
        run = run.add_text(text.to_string());
    }
    if st.bold {
        run = run.bold();
    }
    if st.italic {
        run = run.italic();
    }
    if st.underline {
        run = run.underline("single");
    }
    if st.strike {
        run = run.strike();
    }
    let c = st.color.clone().unwrap_or_else(|| base_color.to_string());
    if c != base_color {
        run = run.color(c);
    }
    if let Some(bg) = &st.bg {
        run = run.shading(Shading::new().fill(bg.clone()).shd_type(ShdType::Solid));
    }
    run
}

/// 富文本标题 → Word run 序列（斜体 / 下划线 / 删除线 / 字色 / 底色）。
///
/// 只认白名单子集（与 `sanitize_inline_html` 同一套语法）；解析不出内容时
/// 退回单段纯文本 run，绝不产出半个文档。
fn build_docx_rich_runs(plain: &str, html: &str, base_size: usize, base_color: &str) -> Vec<Run> {
    let src = if html.trim().is_empty() {
        String::new()
    } else {
        sanitize_inline_html(html).unwrap_or_default()
    };
    if src.trim().is_empty() {
        return vec![docx_style_run(
            plain,
            &InlineStyleState::default(),
            base_size,
            base_color,
            false,
        )];
    }

    const TAGS: [(&str, u8); 8] = [
        ("<b>", 0),
        ("</b>", 1),
        ("<i>", 2),
        ("</i>", 3),
        ("<u>", 4),
        ("</u>", 5),
        ("<s>", 6),
        ("</s>", 7),
    ];

    let mut out: Vec<Run> = Vec::new();
    let mut st = InlineStyleState::default();
    let mut buf = String::new();
    let mut brk = false;
    let mut i = 0usize;
    let s = src.as_str();

    while i < s.len() {
        let rest = &s[i..];

        if rest.starts_with("</span>") {
            if !buf.is_empty() || brk {
                out.push(docx_style_run(&buf, &st, base_size, base_color, brk));
                buf.clear();
                brk = false;
            }
            st.color = None;
            st.bg = None;
            i += "</span>".len();
            continue;
        }
        if rest.starts_with("<span style=\"") {
            if !buf.is_empty() || brk {
                out.push(docx_style_run(&buf, &st, base_size, base_color, brk));
                buf.clear();
                brk = false;
            }
            let head = "<span style=\"".len();
            if let Some(end) = rest[head..].find('"') {
                for part in rest[head..head + end].split(';') {
                    let Some((k, v)) = part.split_once(':') else {
                        continue;
                    };
                    let (k, v) = (k.trim(), v.trim());
                    match k {
                        "color" => st.color = Some(v.trim_start_matches('#').to_string()),
                        "background-color" => {
                            st.bg = Some(v.trim_start_matches('#').to_string())
                        }
                        "font-style" if v == "italic" => st.italic = true,
                        "font-weight" if v == "bold" => st.bold = true,
                        "text-decoration" => {
                            if v.contains("underline") {
                                st.underline = true;
                            }
                            if v.contains("line-through") {
                                st.strike = true;
                            }
                        }
                        _ => {}
                    }
                }
                i += head + end + 2;
                continue;
            }
            return vec![docx_style_run(
                plain,
                &InlineStyleState::default(),
                base_size,
                base_color,
                false,
            )];
        }
        if rest.starts_with("<br>") || rest.starts_with("</br>") {
            if !buf.is_empty() {
                out.push(docx_style_run(&buf, &st, base_size, base_color, brk));
                buf.clear();
                brk = false;
            }
            brk = true;
            i += if rest.starts_with("<br>") { 4 } else { 5 };
            continue;
        }

        let mut hit = false;
        for (tag, code) in TAGS.iter() {
            if rest.starts_with(*tag) {
                if !buf.is_empty() || brk {
                    out.push(docx_style_run(&buf, &st, base_size, base_color, brk));
                    buf.clear();
                    brk = false;
                }
                match code {
                    0 => st.bold = true,
                    1 => st.bold = false,
                    2 => st.italic = true,
                    3 => st.italic = false,
                    4 => st.underline = true,
                    5 => st.underline = false,
                    6 => st.strike = true,
                    _ => st.strike = false,
                }
                i += tag.len();
                hit = true;
                break;
            }
        }
        if hit {
            continue;
        }

        match s[i..].chars().next() {
            Some(ch) => {
                buf.push(ch);
                i += ch.len_utf8();
            }
            None => break,
        }
    }
    if !buf.is_empty() || brk {
        out.push(docx_style_run(&buf, &st, base_size, base_color, brk));
    }
    if out.is_empty() {
        out.push(docx_style_run(
            plain,
            &InlineStyleState::default(),
            base_size,
            base_color,
            false,
        ));
    }
    out
}

/// 大标题在 Word 里的字号（半磅）：H1 20pt / H2 16pt / H3 13pt / H4 11pt。
fn docx_heading_size(level: i32) -> usize {
    match level {
        1 => 40,
        2 => 32,
        3 => 26,
        _ => 22,
    }
}

/// Generate a self-contained Word document (.docx) with embedded screenshots.
/// Scribe-style layout: title → metadata → step cards with header, description,
/// inline screenshot, and optional tip.
fn export_docx(
    recording: &ExportRecording,
    steps: &[ExportStep],
    output_path: &std::path::Path,
    zoom_map: &std::collections::HashMap<String, f64>,
    public_url: &str,
    qr_enabled: bool,
    include_toc: bool,
) -> Result<(), String> {
    let mut docx = Docx::new();

    // ── 页脚（对齐插件 `export.ts::footerQrHtml` + `opts.qr`）─────────────
    // 有公开地址且未关二维码 → 页脚放「扫码看线上版」二维码位图（Word 不认 SVG）；
    // 生成失败只丢码不中断导出（与 HTML 侧同语义）。落款始终与插件一致。
    let mut footer = Footer::new();
    if qr_enabled {
        let url = public_url.trim();
        if !url.is_empty() {
            if let Ok(png) = crate::export::qr::qr_png_bytes(url, 8) {
                // 88px 展示（EMU = px × 9525），与 HTML 页脚二维码尺寸一致
                let side_emu = 88u32 * 9525;
                let pic = Pic::new(&png).size(side_emu, side_emu);
                footer = footer.add_paragraph(
                    Paragraph::new()
                        .add_run(Run::new().add_image(pic))
                        .align(AlignmentType::Center),
                );
            } else {
                eprintln!("[export] DOCX 页脚二维码生成失败，跳过二维码（导出继续）");
            }
        }
    }
    let footer_text = format!("本指南由 录步 Flowio 自动生成 · {}", recording.created_at);
    footer = footer.add_paragraph(
        Paragraph::new()
            .add_run(Run::new().add_text(footer_text).color("8c959f").size(18))
            .align(AlignmentType::Center),
    );
    docx = docx.footer(footer);

    // ── 品牌区（对齐插件 `buildWordHtml` 首部成套品牌视觉）─────────
    // ① 浅橙胶囊「操作指南」+ 品牌名「录步 Flowio」；
    // ② 大标题（docx-rs 下划线色随 run 色，整体用品牌橙强调）；
    // ③ 元信息；④ 落款「由 录步 Flowio 导出 · flowio.cn」。
    docx = docx.add_paragraph(
        Paragraph::new()
            .add_run(
                Run::new()
                    .add_text("操作指南")
                    .bold()
                    .size(18)
                    .color("c2410c")
                    .shading(Shading::new().fill("ffe9de").shd_type(ShdType::Solid)),
            )
            .add_run(Run::new().add_text("  录步 Flowio").bold().size(18).color("ff6b35")),
    );
    docx = docx.add_paragraph(
        Paragraph::new()
            .add_run(
                Run::new()
                    .add_text(recording.title.trim())
                    .bold()
                    .size(36)
                    .color("ff6b35")
                    .underline("single"),
            )
            .line_spacing(LineSpacing::new().before(120)),
    );
    let meta = build_docx_meta_text(&recording.app_name, recording.total_steps, &recording.created_at);
    docx = docx.add_paragraph(
        Paragraph::new().add_run(Run::new().add_text(meta).color("888888").size(20)),
    );
    docx = docx.add_paragraph(
        Paragraph::new()
            .add_run(Run::new().add_text("由 录步 Flowio 导出 · flowio.cn").color("8c959f").size(18))
            .line_spacing(LineSpacing::new().after(160)),
    );

    // ── 目录（include_toc 控制；对齐插件 `buildWordTocHtml`）────────
    // 单层 table 纸面化目录 + `_Toc_sN` 锚点跳转；摘要/简介不占条目，
    // 大标题块与步骤标题按出现顺序编号（与步骤正文锚点一一对应）。
    let mut anchor_idx: Vec<usize> = vec![0usize; steps.len()];
    {
        let mut n: usize = 0;
        for (i, s) in steps.iter().enumerate() {
            if is_summary_block(s) || is_intro_block(s) {
                continue;
            }
            n += 1;
            anchor_idx[i] = n;
        }
    }
    if include_toc && anchor_idx.iter().any(|&v| v > 0) {
        docx = docx.add_paragraph(
            Paragraph::new().add_run(Run::new().add_text("目录").bold().size(26).color("1a1a2e")),
        );
        let mut toc_rows: Vec<TableRow> = Vec::new();
        for (i, s) in steps.iter().enumerate() {
            if anchor_idx[i] == 0 {
                continue;
            }
            let label = if s.title.trim().is_empty() {
                format!("{}", anchor_idx[i])
            } else {
                format!("{}　{}", anchor_idx[i], s.title.trim())
            };
            let link = Hyperlink::new(format!("_Toc_s{}", anchor_idx[i]), HyperlinkType::Anchor)
                .add_run(Run::new().add_text(label).size(20).color("1a1a2e"));
            toc_rows.push(TableRow::new(vec![TableCell::new().add_paragraph(
                Paragraph::new().add_hyperlink(link),
            )]));
        }
        docx = docx.add_table(Table::new(toc_rows));
        docx = docx.add_paragraph(Paragraph::new());
    }

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

    for (i, (s, dim_opt)) in steps.iter().zip(img_dims.iter()).enumerate() {
        // ── 大标题块：按层级出标题段（富文本落到 run 属性）────────
        if is_heading_block(s) {
            let size = docx_heading_size(heading_level(s));
            let mut para = Paragraph::new();
            if anchor_idx[i] > 0 {
                para = para.add_bookmark_start(9000 + anchor_idx[i], format!("_Toc_s{}", anchor_idx[i]));
            }
            for r in build_docx_rich_runs(&s.title, &s.title_html, size, "1a1a2e") {
                para = para.add_run(r.bold());
            }
            if anchor_idx[i] > 0 {
                para = para.add_bookmark_end(9000 + anchor_idx[i]);
            }
            docx = docx.add_paragraph(para);
            continue;
        }

        // ── 流程摘要块：总览段 + 要点段（不占编号）────────────────
        if is_summary_block(s) {
            let (intro, bullets) = parse_summary(&s.description);
            if !intro.is_empty() {
                docx = docx.add_paragraph(
                    Paragraph::new().add_run(Run::new().add_text(intro).size(22).color("24292f")),
                );
            }
            for (label, body) in &bullets {
                let mut run = Run::new().add_text(if label.is_empty() {
                    format!("· {}", body)
                } else {
                    format!("· {}：{}", label, body)
                });
                run = run.size(22).color("24292f");
                docx = docx.add_paragraph(Paragraph::new().add_run(run));
            }
            docx = docx.add_paragraph(Paragraph::new());
            continue;
        }

        // ── 简介块：一段引子（不占编号）──────────────────────────
        if is_intro_block(s) {
            let t = s.description.trim();
            if !t.is_empty() {
                docx = docx.add_paragraph(
                    Paragraph::new().add_run(Run::new().add_text(t).size(22).color("3f4652")),
                );
                docx = docx.add_paragraph(Paragraph::new());
            }
            continue;
        }

        // ── Step header（动作标签胶囊 + `_Toc_sN` 锚点）────────
        let header = build_docx_step_header(s.step_number, &s.action_type, &s.title);
        let mut header_para = Paragraph::new();
        if anchor_idx[i] > 0 {
            header_para =
                header_para.add_bookmark_start(9000 + anchor_idx[i], format!("_Toc_s{}", anchor_idx[i]));
        }
        header_para = header_para.add_run(Run::new().add_text(header).bold().size(24));
        if !s.action_type.is_empty() {
            header_para = header_para.add_run(
                Run::new()
                    .add_text(format!("  {}", to_action_label(&s.action_type)))
                    .size(20)
                    .color("6d28d9")
                    .shading(Shading::new().fill("ede9fe").shd_type(ShdType::Solid)),
            );
        }
        if anchor_idx[i] > 0 {
            header_para = header_para.add_bookmark_end(9000 + anchor_idx[i]);
        }
        docx = docx.add_paragraph(header_para);

        // ── Description（标注/警告带底色，与预览同色）──────────
        if !s.description.is_empty() {
            let mut run = build_docx_desc_run(&s.description);
            if let Some((bg, fg, _bar)) = note_palette(&s.note_kind) {
                run = run
                    .color(fg)
                    .shading(Shading::new().fill(bg).shd_type(ShdType::Solid));
            }
            docx = docx.add_paragraph(Paragraph::new().add_run(run));
        }

        // ── Screenshot ────────────────────────────────────────
        if !s.screenshot_path.is_empty() {
            if let Ok(img_bytes) = std::fs::read(&s.screenshot_path) {
                // 重点图：按该步倍率裁出「放大后仍可见」的那块区域
                // ⚠️ 尺寸必须跟着 final_bytes 走：裁剪后原图尺寸就失效了
                let (final_bytes, final_dims) =
                    maybe_focus_shot_bytes(img_bytes, s.x, s.y, zoom_for(zoom_map, &s.id));
                let (w_emu, h_emu) = docx_image_emu_dims(final_dims);
                let pic = Pic::new(&final_bytes).size(w_emu, h_emu);
                docx = docx.add_paragraph(
                    Paragraph::new()
                        .add_run(Run::new().add_image(pic))
                );
            } else {
                docx = docx.add_paragraph(
                    Paragraph::new()
                        .add_run(Run::new().add_text("（截图文件缺失）").color("aaaaaa").size(20))
                );
            }
        } else {
            docx = docx.add_paragraph(
                Paragraph::new()
                    .add_run(Run::new().add_text("（这一步没有截图）").color("aaaaaa").size(20))
            );
        }

        // ── 图片描述：紧贴截图下方的小灰字（空则不渲染）─────────
        if !s.image_caption.trim().is_empty() {
            docx = docx.add_paragraph(
                Paragraph::new().add_run(
                    Run::new()
                        .add_text(s.image_caption.trim())
                        .color("6b7280")
                        .size(18),
                ),
            );
        }

        // ── 来源网址：说明之后、提示之前（对齐插件「每步显示来源网址 🔗」）──
        if !s.url.trim().is_empty() {
            docx = docx.add_paragraph(
                Paragraph::new().add_run(
                    Run::new()
                        .add_text(format!("🔗 {}", s.url.trim()))
                        .color("6b7280")
                        .size(18),
                ),
            );
        }

        // ── Tip ───────────────────────────────────────────────
        if !s.tip.is_empty() {
            docx = docx.add_paragraph(
                Paragraph::new()
                    .add_run(
                        Run::new()
                            .add_text(format!("提示：{}", s.tip))
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

/// 用指定 headless 模式跑一次 Edge 打印。返回 (退出状态, stderr)；超时返回 Err。
fn run_edge_pdf(
    edge_path: &std::path::Path,
    headless_flag: &str,
    profile_dir: &std::path::Path,
    html_url: &str,
    output_pdf: &str,
) -> Result<(std::process::ExitStatus, String), String> {
    // spawn + try_wait 轮询实现超时：Edge 无头渲染大 HTML 可能卡死，
    // 进程必须能被回收而不是无限阻塞导出线程。
    let mut edge_cmd = std::process::Command::new(edge_path);
    no_console_window(&mut edge_cmd);
    let mut child = edge_cmd
        .args([
            headless_flag,
            "--disable-gpu",
            "--no-sandbox",
            "--no-first-run",
            "--disable-extensions",
            "--no-pdf-header-footer",
            // 独立 profile：与用户正开着的 Edge 完全隔离，杜绝进程交接/配置锁冲突
            &format!("--user-data-dir={}", profile_dir.to_string_lossy()),
            &format!("--print-to-pdf={}", output_pdf),
            html_url,
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

    let mut stderr = String::new();
    if let Some(mut handle) = child.stderr.take() {
        use std::io::Read;
        let _ = handle.read_to_string(&mut stderr);
    }
    if timed_out {
        return Err(format!(
            "PDF 导出超时：Microsoft Edge 未在 {} 秒内完成渲染，已终止",
            EDGE_PDF_TIMEOUT.as_secs()
        ));
    }
    Ok((status, stderr))
}

/// 产物有效性：文件存在且大于「纯空白页」阈值（空白 PDF 实测 ~1.6KB，
/// 正常 PDF 必含页头文字 + 内嵌字体子集，远大于该值）。
fn pdf_output_ok(output_path: &std::path::Path) -> bool {
    std::fs::metadata(output_path)
        .map(|m| m.len() > 2048)
        .unwrap_or(false)
}

/// Generate a PDF by rendering the HTML export via Microsoft Edge headless.
/// The HTML (with base64-embedded screenshots) is written to a temp file,
/// then converted to a self-contained PDF. No printpdf, no external deps.
fn export_pdf(
    recording: &ExportRecording,
    steps: &[ExportStep],
    output_path: &std::path::Path,
    zoom_map: &std::collections::HashMap<String, f64>,
    public_url: &str,
    qr_enabled: bool,
) -> Result<(), String> {
    if steps.is_empty() {
        return Err("No steps to export".to_string());
    }

    // 1. Generate standalone HTML with base64-embedded images
    let html = gen_html_body(recording, steps, zoom_map, public_url, qr_enabled);

    // 2. Write HTML to a temporary file
    let temp_dir = std::env::temp_dir();
    let html_path = temp_dir.join(format!("flowio_pdf_{}.html", recording.id));
    std::fs::write(&html_path, html.as_bytes())
        .map_err(|e| format!("Failed to write temp HTML: {}", e))?;

    // 3. Convert to PDF using Microsoft Edge headless
    let edge_path = resolve_edge_path()
        .ok_or_else(|| "未找到 Microsoft Edge，无法导出 PDF".to_string())?;
    // file URL 必须走 percent-encoding：手拼 file:/// 遇中文/空格路径会加载失败，
    // 且 Edge 加载失败仍返回成功码 → 静默产出空白 PDF（2026-09-09 加固）。
    let html_url = url::Url::from_file_path(&html_path)
        .map(|u| u.to_string())
        .unwrap_or_else(|_| format!("file:///{}", html_path.to_string_lossy().replace('\\', "/")));
    let profile_dir = temp_dir.join(format!("flowio_edge_profile_{}", std::process::id()));

    let output = output_path.to_string_lossy().to_string();
    let mut result = run_edge_pdf(&edge_path, "--headless=new", &profile_dir, &html_url, &output);
    // 静默空白防护：进程正常退出但没产出有效 PDF → 换旧版 headless 开关重试一次
    if result.is_ok() && !pdf_output_ok(output_path) {
        result = run_edge_pdf(&edge_path, "--headless", &profile_dir, &html_url, &output);
    }

    match result {
        Err(e) => {
            let _ = std::fs::remove_dir_all(&profile_dir);
            Err(format!(
                "{}（临时 HTML 已保留供诊断：{}）",
                e,
                html_path.to_string_lossy()
            ))
        }
        Ok((status, stderr)) => {
            if !status.success() {
                let _ = std::fs::remove_dir_all(&profile_dir);
                return Err(format!("Edge PDF conversion failed: {}", stderr.trim()));
            }
            if !pdf_output_ok(output_path) {
                // 两次都没产出有效 PDF：报错并把原始 HTML 留给用户/开发者，
                // 绝不再静默成功（「导出成功」+ 打开全空白最坑人）。
                let _ = std::fs::remove_dir_all(&profile_dir);
                return Err(format!(
                    "PDF 导出异常：Edge 正常退出但未生成有效 PDF（产物缺失或为空白页）。\
                     原始 HTML 已保留在 {}，请将该文件连同此提示反馈给开发者。{}",
                    html_path.to_string_lossy(),
                    if stderr.trim().is_empty() {
                        String::new()
                    } else {
                        format!(" Edge 输出：{}", stderr.trim())
                    }
                ));
            }
            let _ = std::fs::remove_file(&html_path);
            let _ = std::fs::remove_dir_all(&profile_dir);
            Ok(())
        }
    }
}

// ── 文档块 / 富文本（第十四轮）────────────────────────────────

/// 大标题块判定。
fn is_heading_block(s: &ExportStep) -> bool {
    s.block_kind.eq_ignore_ascii_case("heading")
}

/// 简介块判定（block_kind = 'intro'，第十九轮）。
/// 用户在正文里自己写的引子：不占编号、没有截图，正文存 `description`。
fn is_intro_block(s: &ExportStep) -> bool {
    s.block_kind.eq_ignore_ascii_case("intro")
}

/// 文档块（大标题 / 流程摘要 / 简介）：都不占编号、都不计入「共 N 个步骤」。
fn is_doc_block(s: &ExportStep) -> bool {
    is_heading_block(s) || is_summary_block(s) || is_intro_block(s)
}

/// 标题层级，夹到 1..4（与编辑器 H1~H4 一致）。
fn heading_level(s: &ExportStep) -> i32 {
    s.block_level.clamp(1, 4)
}

/// 真步骤数：大标题块不占编号，也不计入「Steps: N」。
fn real_step_count(steps: &[ExportStep]) -> i32 {
    steps.iter().filter(|s| !is_heading_block(s)).count() as i32
}

/// 判断 #rgb / #rrggbb / #rrggbbaa。
fn is_hex_color(v: &str) -> bool {
    match v.strip_prefix('#') {
        Some(h) => {
            (h.len() == 3 || h.len() == 6 || h.len() == 8) && h.chars().all(|c| c.is_ascii_hexdigit())
        }
        None => false,
    }
}

/// 只放行 4 个 CSS 属性，且取值必须落在固定集合内。
fn sanitize_style(style: &str) -> Option<String> {
    let mut out: Vec<String> = Vec::new();
    for part in style.split(';') {
        let part = part.trim();
        if part.is_empty() {
            continue;
        }
        let (k, v) = part.split_once(':')?;
        let k = k.trim().to_ascii_lowercase();
        let v = v.trim().to_ascii_lowercase();
        let ok = match k.as_str() {
            "color" | "background-color" => is_hex_color(&v),
            "font-style" => v == "italic" || v == "normal",
            "font-weight" => v == "bold" || v == "normal" || v == "700",
            "text-decoration" => matches!(
                v.as_str(),
                "underline"
                    | "line-through"
                    | "none"
                    | "underline line-through"
                    | "line-through underline"
            ),
            _ => false,
        };
        if !ok {
            return None;
        }
        out.push(format!("{}:{}", k, v));
    }
    if out.is_empty() {
        None
    } else {
        Some(out.join(";"))
    }
}

/// 富文本标题的白名单校验器。
///
/// 编辑器只会写出下面这几种内联标签（规范化后的 HTML 子集），所以这里采用
/// **「不完全匹配就整体退回纯文本」** 的策略：不修补、不做容错解析 ——
/// 手写 HTML 解析器的容错分支正是 XSS 的温床，而 DB 里的内容属于不可信输入
/// （用户可能在 contenteditable 里粘贴任意 HTML，也可能直接改库）。
fn sanitize_inline_html(html: &str) -> Option<String> {
    const SIMPLE: [&str; 10] = [
        "<b>", "</b>", "<i>", "</i>", "<u>", "</u>", "<s>", "</s>", "<br>", "</br>",
    ];
    let bytes = html.as_bytes();
    let mut out = String::with_capacity(html.len());
    let mut i = 0usize;
    while i < html.len() {
        if !html.is_char_boundary(i) {
            return None;
        }
        if bytes[i] == b'<' {
            let rest = &html[i..];
            let mut hit: Option<&str> = None;
            for tag in SIMPLE.iter() {
                if rest.starts_with(*tag) {
                    hit = Some(*tag);
                    break;
                }
            }
            if let Some(tag) = hit {
                out.push_str(tag);
                i += tag.len();
                continue;
            }
            if rest.starts_with("</span>") {
                out.push_str("</span>");
                i += "</span>".len();
                continue;
            }
            if rest.starts_with("<span style=\"") {
                let head = "<span style=\"".len();
                let end = rest[head..].find('"')?;
                let safe = sanitize_style(&rest[head..head + end])?;
                out.push_str("<span style=\"");
                out.push_str(&safe);
                out.push_str("\">");
                i += head + end + 2;
                continue;
            }
            return None;
        }
        if bytes[i] == b'>' {
            return None;
        }
        let ch = html[i..].chars().next()?;
        out.push(ch);
        i += ch.len_utf8();
    }
    Some(out)
}

/// 大标题的展示用 HTML：白名单通过就用富文本，否则退化为转义后的纯文本。
fn rich_inline_html(plain: &str, html: &str) -> String {
    if html.trim().is_empty() {
        return htmlescape(plain);
    }
    match sanitize_inline_html(html) {
        Some(s) if !s.trim().is_empty() => s,
        _ => htmlescape(plain),
    }
}

/// 说明类型配色（与编辑器 `.sop-edit-note` / 预览逐值一致）：
/// (底色, 字色, 左色条)
fn note_palette(kind: &str) -> Option<(&'static str, &'static str, &'static str)> {
    match kind {
        "mark" => Some(("#e6f1fb", "#0c447c", "#378add")),
        "warn" => Some(("#faeeda", "#633806", "#ba7517")),
        _ => None,
    }
}

/// 大标题块的 HTML（h1~h4）。字号与编辑器 `.sop-heading-t1~t4` 逐像素对齐。
/// `anchor_id` 非空时挂 `id="step-N"`（与插件 `buildHtml` 的标题块锚点同语义，
/// 目录分组标签跳转到此）。
fn build_heading_html(s: &ExportStep, anchor_id: &str) -> String {
    let lv = heading_level(s);
    let inner = rich_inline_html(&s.title, &s.title_html);
    let id_attr = if anchor_id.is_empty() {
        String::new()
    } else {
        format!(" id=\"step-{}\"", anchor_id)
    };
    format!(
        "    <h{lv} class=\"doc-heading h{lv}\"{id_attr}>{inner}</h{lv}>\n",
        lv = lv,
        inner = inner,
        id_attr = id_attr,
    )
}

// ── 流程摘要块 / 图片描述（第十六轮）──────────────────────────

/// 摘要块判定（block_kind = 'summary'）。不占编号、没有截图，排正文最前。
fn is_summary_block(s: &ExportStep) -> bool {
    s.block_kind.eq_ignore_ascii_case("summary")
}

/// 把摘要正文拆成「总览段 + 要点列表」。
/// 约定（与编辑器/预览同一份）：首行是总览；`- **要点**：说明` 为一条要点；
/// 其它以 `-`/`•` 开头的行按「无标签要点」处理；其余普通行并入总览段。
fn parse_summary(text: &str) -> (String, Vec<(String, String)>) {
    let mut intro: Vec<String> = Vec::new();
    let mut bullets: Vec<(String, String)> = Vec::new();
    for raw in text.lines() {
        let line = raw.trim();
        if line.is_empty() {
            continue;
        }
        let rest = line
            .strip_prefix('-')
            .or_else(|| line.strip_prefix('•'))
            .or_else(|| line.strip_prefix('*'));
        if let Some(rest) = rest {
            let item = rest.trim();
            if let Some(start) = item.find("**") {
                if let Some(rel) = item[start + 2..].find("**") {
                    let label = item[start + 2..start + 2 + rel].trim().to_string();
                    let body = item[start + 2 + rel + 2..]
                        .trim()
                        .trim_start_matches(['：', ':'])
                        .trim()
                        .to_string();
                    bullets.push((label, body));
                    continue;
                }
            }
            bullets.push((String::new(), item.to_string()));
        } else {
            intro.push(line.to_string());
        }
    }
    (intro.join(""), bullets)
}

/// 摘要卡片 HTML：与预览 `.sop-preview-summary` 同一版式（浅灰圆角卡 + 要点列表）。
fn build_summary_html(text: &str) -> String {
    let (intro, bullets) = parse_summary(text);
    if intro.is_empty() && bullets.is_empty() {
        return String::new();
    }
    let mut inner = String::new();
    if !intro.is_empty() {
        inner.push_str(&format!("      <p>{}</p>\n", htmlescape(&intro)));
    }
    if !bullets.is_empty() {
        inner.push_str("      <ul>\n");
        for (label, body) in &bullets {
            if label.is_empty() {
                inner.push_str(&format!("        <li>{}</li>\n", htmlescape(body)));
            } else {
                inner.push_str(&format!(
                    "        <li><b>{}</b>：{}</li>\n",
                    htmlescape(label),
                    htmlescape(body)
                ));
            }
        }
        inner.push_str("      </ul>\n");
    }
    format!("    <div class=\"doc-summary\">\n{}</div>\n", inner)
}

/// 简介块 HTML：一段引子正文（保留换行），与预览 `.sop-preview-intro` 同版式。
/// 空内容返回空串 —— 不产出一个空 div 占位。
fn build_intro_html(text: &str) -> String {
    let t = text.trim();
    if t.is_empty() {
        return String::new();
    }
    format!("    <div class=\"doc-intro\">{}</div>\n", htmlescape(t))
}

/// 图片描述（图注）：紧贴截图下方的一行小灰字；空串返回空。
fn build_caption_html(caption: &str) -> String {
    if caption.trim().is_empty() {
        return String::new();
    }
    format!(
        "      <div class=\"step-caption\">{}</div>\n",
        htmlescape(caption.trim())
    )
}

/// 摘要卡片 Markdown：总览段 + `- **要点**：说明` 列表。
fn build_md_summary(text: &str) -> String {
    let (intro, bullets) = parse_summary(text);
    if intro.is_empty() && bullets.is_empty() {
        return String::new();
    }
    let mut parts: Vec<String> = Vec::new();
    if !intro.is_empty() {
        parts.push(intro);
    }
    for (label, body) in &bullets {
        if label.is_empty() {
            parts.push(format!("- {}", body));
        } else {
            parts.push(format!("- **{}**：{}", label, body));
        }
    }
    parts.join("\n")
}

// ── Extracted pure logic functions ────────────────────────────

/// Build a single step card header for HTML export.
///
/// 与预览一致：编号圆圈 + 「步骤」+ 类型徽标；**标题单独成行**（见 step-title），
/// 不再像旧版那样把标题塞进这一行（那是「导出跟预览两套版式」的根源之一）。
fn build_step_header_html(step_number: i32, action_type: &str, title: &str, key_on: bool) -> String {
    let _ = title; // 标题改由 build_step_title_html 单独渲染，保留形参兼容旧调用
    // 重点图徽章（对齐插件 step-head 的 `.key-badge`）：只在重点步显示
    let badge = if key_on {
        "<span class=\"key-badge\">重点</span>"
    } else {
        ""
    };
    if action_type.is_empty() {
        format!(
            "<span class=\"step-num-circle\">{}</span>步骤{}",
            step_number, badge
        )
    } else {
        format!(
            "<span class=\"step-num-circle\">{}</span>步骤<span class=\"step-type\">{}</span>{}",
            step_number,
            htmlescape(action_type),
            badge
        )
    }
}

/// 步骤标题行（预览里的 `.sop-preview-step-title`）。
fn build_step_title_html(title: &str) -> String {
    if title.trim().is_empty() {
        String::new()
    } else {
        format!("      <h3 class=\"step-title\">{}</h3>\n", htmlescape(title))
    }
}

/// Build the description block for HTML step card (empty → empty string).
/// 多行描述：htmlescape 后把 \n 转为 <br />，保住粘贴文本的换行格式。
fn build_step_desc_html(description: &str) -> String {
    build_step_desc_html_kind(description, "")
}

/// 说明块（描述 / 标注 / 警告）。
///
/// note_kind 决定底色 + 左色条，取值与编辑器 `.sop-edit-note.is-mark/.is-warn`
/// 完全一致 —— 这样「预览看到的」和「导出的」是同一套颜色。
fn build_step_desc_html_kind(description: &str, note_kind: &str) -> String {
    if description.is_empty() {
        return String::new();
    }
    let body = htmlescape(description).replace('\n', "<br />");
    match note_palette(note_kind) {
        Some((bg, fg, bar)) => format!(
            "      <p class=\"step-desc is-{kind}\" style=\"background:{bg};color:{fg};\
             border-left:3px solid {bar}\"\">{body}</p>\n",
            kind = htmlescape(note_kind),
            bg = bg,
            fg = fg,
            bar = bar,
            body = body,
        ),
        None => format!("      <p class=\"step-desc\">{}</p>\n", body),
    }
}

/// Build the tip block for HTML step card (empty → empty string).
fn build_step_tip_html(tip: &str) -> String {
    if tip.is_empty() {
        String::new()
    } else {
        format!(
            "      <div class=\"step-tip\"><strong>提示：</strong>{}</div>\n",
            htmlescape(tip)
        )
    }
}

/// 来源网址块（对齐插件 HTML 的 `🔗 <url>` 行）：空串/纯空白 → 不产出行。
fn build_step_url_html(url: &str) -> String {
    let u = url.trim();
    if u.is_empty() {
        String::new()
    } else {
        format!("      <div class=\"step-url\">🔗 {}</div>\n", htmlescape(u))
    }
}

/// Build the "no screenshot" placeholder div.
fn build_screenshot_placeholder_html() -> &'static str {
    "<div class=\"screenshot-placeholder\">（这一张没有截图）</div>"
}

/// Build the `<img>` tag for an embedded screenshot with base64 data.
fn build_screenshot_img_html(mime: &str, base64_data: &str, step_number: i32) -> String {
    format!(
        "<img class=\"screenshot\" src=\"data:{};base64,{}\" \
         alt=\"步骤 {} 截图\" />",
        mime, base64_data, step_number
    )
}

/// CSS 百分数（去掉多余尾零：125.00 → 125；12.50 → 12.5）。
fn fmt_pct(n: f64) -> String {
    let s = format!("{:.2}", n);
    s.trim_end_matches('0').trim_end_matches('.').to_string()
}

/// 重点图容器宽度（对齐插件 `shared/keyFrame.ts::keyShotWidth`）：
/// `width = round(basePct*ratio/100*100)/100`；`bleed = round(max(0,(width-100)/2)*100)/100`。
/// 桌面端 HTML 无「缩小」档（basePct 恒 100）。
fn key_shot_width(base_pct: f64, ratio: f64) -> (f64, f64) {
    let r2 = |n: f64| (n * 100.0).round() / 100.0;
    let width = r2(base_pct * ratio / 100.0);
    let bleed = r2((width - 100.0).max(0.0) / 2.0);
    (width, bleed)
}

/// 目录导航（对齐插件 `export.ts::buildHtml` 的 tocHtml）：
/// 仅 `steps.len() > 1` 时生成；锚点指回正文各 `#step-N`。
/// 标题块（H1~H4）是目录里的**分组标签**：不印序号、橙色加重、上方留白；
/// 普通步骤带补零两位的橙色序号列（`toc-no`）—— 与 Word / PPT 目录同一套语义。
fn build_toc_html(steps: &[ExportStep]) -> String {
    if steps.len() <= 1 {
        return String::new();
    }
    let mut entries: Vec<String> = Vec::with_capacity(steps.len());
    for (i, s) in steps.iter().enumerate() {
        let n = i + 1;
        let label = if s.title.trim().is_empty() {
            format!("步骤 {}", n)
        } else {
            s.title.trim().to_string()
        };
        let label = htmlescape(&label);
        if is_heading_block(s) {
            entries.push(format!(
                "          <li class=\"toc-group\"><a href=\"#step-{}\"><span class=\"toc-text\">{}</span></a></li>",
                n, label
            ));
        } else {
            entries.push(format!(
                "          <li><a href=\"#step-{}\"><span class=\"toc-no\">{:02}</span><span class=\"toc-text\">{}</span></a></li>",
                n, n, label
            ));
        }
    }
    format!(
        "    <nav class=\"toc\">\n        <div class=\"toc-head\"><span class=\"toc-title\">目录</span><span class=\"toc-meta\">共 {} 步</span></div>\n        <div class=\"toc-rule\"></div>\n        <ol class=\"toc-list\">\n{}\n        </ol>\n      </nav>\n",
        steps.len(),
        entries.join("\n")
    )
}

/// Build the full HTML document with the embedded CSS and step cards.
fn build_html_document(
    title: &str,
    total_steps: i32,
    created_at: &str,
    step_cards: &str,
    public_url: &str,
    qr_enabled: bool,
) -> String {
    format!(
        r#"<!DOCTYPE html>
<html lang="zh-CN">
<head>
<meta charset="UTF-8">
<meta name="viewport" content="width=device-width, initial-scale=1.0">
<title>{title}</title>
<style>
  *,*::before,*::after{{box-sizing:border-box;margin:0;padding:0}}
  body{{font-family:-apple-system,BlinkMacSystemFont,"Segoe UI","Microsoft YaHei","PingFang SC","Noto Sans CJK SC","SimSun",Helvetica,Arial,sans-serif;background:#fff;color:#111827;line-height:1.6}}
  /* ★ 纸面品牌头（对齐插件 `export.ts` 的 `.doc-header` 系列）：去掉深色大卡片（打印费墨），
     改成「操作指南」徽章 + 大标题 + 品牌橙题头条 + 灰色 meta 行 + 细分隔线。 */
  .doc-header{{max-width:900px;margin:0 auto 26px;padding:0 16px 20px;border-bottom:1px solid #e5e7eb;-webkit-print-color-adjust:exact;print-color-adjust:exact}}
  .doc-eyebrow{{display:flex;align-items:center;justify-content:space-between;margin-bottom:16px}}
  .doc-badge{{background:#ffe9de;color:#c2410c;font-size:12px;font-weight:700;padding:3px 12px;border-radius:999px;letter-spacing:3px;-webkit-print-color-adjust:exact;print-color-adjust:exact}}
  .doc-brand{{font-size:12px;color:#6b7280}}
  .doc-header h1{{font-size:34px;font-weight:700;color:#111827;line-height:1.35;word-break:break-word}}
  .doc-titlebar{{width:56px;height:4px;background:#ff6b35;border-radius:2px;margin:12px 0 14px;-webkit-print-color-adjust:exact;print-color-adjust:exact}}
  .doc-header .meta{{font-size:12px;color:#6b7280;display:flex;flex-wrap:wrap;gap:14px}}
  .container{{max-width:900px;margin:0 auto;padding:24px 16px}}
  /* ★ 纸面目录（对齐插件 v0.65.359 的 toc 重做）：无外框无底纹，只有「目录」二字 + 橙色细线 + 两栏 */
  .toc{{margin:0 0 32px}}
  .toc-head{{display:flex;align-items:baseline;justify-content:space-between;gap:12px}}
  .toc-title{{font-size:16px;font-weight:700;color:#111827;letter-spacing:4px}}
  .toc-meta{{font-size:12px;color:#6b7280}}
  .toc-rule{{height:2px;background:#ff6b35;border-radius:1px;margin:10px 0 2px;-webkit-print-color-adjust:exact;print-color-adjust:exact}}
  .toc-list{{margin:0;padding:0;list-style:none}}
  .toc-list li{{break-inside:avoid;border-bottom:1px solid #f1f3f5}}
  .toc-list li:last-child{{border-bottom:none}}
  .toc-list a{{display:flex;align-items:baseline;gap:14px;padding:7px 2px;text-decoration:none}}
  .toc-no{{flex:0 0 22px;font-size:12px;font-weight:700;color:#ff6b35;letter-spacing:.5px;font-variant-numeric:tabular-nums}}
  .toc-text{{font-size:14px;color:#374151;word-break:break-word}}
  .toc-list a:hover .toc-text{{color:#ff6b35;text-decoration:underline}}
  .toc-list li.toc-group{{border-bottom:none}}
  .toc-list li.toc-group a{{padding:14px 2px 3px}}
  .toc-list li.toc-group .toc-text{{font-size:14px;font-weight:700;color:#c2410c}}
  .step-card{{background:#fff;border:1px solid #d0d7de;border-radius:8px;padding:20px;margin-bottom:16px;box-shadow:0 1px 3px rgba(0,0,0,0.04)}}
  .doc-heading{{color:#1a1a2e;font-weight:600;letter-spacing:0.01em;margin:28px 0 12px}}
  .doc-heading.h1{{font-size:28px}}
  .doc-heading.h2{{font-size:23px}}
  .doc-heading.h3{{font-size:19px}}
  .doc-heading.h4{{font-size:17px}}
  .step-header{{display:flex;align-items:center;gap:8px;font-size:1.05rem;font-weight:600;margin-bottom:8px;color:#1a1a2e}}
  .step-num-circle{{display:inline-flex;align-items:center;justify-content:center;width:24px;height:24px;border-radius:50%;background:#ff6b35;color:#fff;font-size:13px;font-weight:600;line-height:1;flex:none}}
  .step-title{{font-size:16px;font-weight:500;color:#1a1a2e;margin:0 0 16px 0}}
  .step-type{{font-size:0.8rem;color:#656d76;background:#f6f8fa;padding:2px 10px;border-radius:4px;border:1px solid #d0d7de;margin-right:8px;font-weight:normal}}
  .step-desc{{font-size:14px;line-height:1.7;color:#24292f;margin-bottom:12px;white-space:pre-wrap}}
  .step-desc.is-mark{{background:#e6f1fb;color:#0c447c;border-radius:0 6px 6px 0;padding:8px 12px}}
  .step-desc.is-warn{{background:#faeeda;color:#633806;border-radius:0 6px 6px 0;padding:8px 12px}}
  .step-tip{{font-size:0.85rem;color:#0969da;background:#ddf4ff;padding:8px 12px;border-radius:6px;margin-bottom:12px;border-left:3px solid #0969da}}
  .screenshot{{max-width:100%;border-radius:6px;border:1px solid #d0d7de;display:block}}
  /* ★ 图块容器（对齐插件 `.shot`）：普通步同样套容器，重点步整体放大（边框/圆角一起变大） */
  .shot{{position:relative;border:1px solid #e5e7eb;border-radius:10px;overflow:hidden;background:#f9fafb;max-width:100%}}
  .shot img{{display:block;width:100%;height:auto;margin:0 auto}}
  .shot .screenshot{{border:none;border-radius:0}}
  .step-card.key .shot{{border-color:#ff6b35;box-shadow:0 4px 18px rgba(255,107,53,.16)}}
  .key-badge{{background:#ff6b35;color:#fff;font-size:11px;font-weight:700;padding:2px 9px;border-radius:999px;flex:0 0 auto}}
  .step-card.key .step-num-circle{{background:#ff6b35}}
  @media (max-width:900px){{.step-card.key .shot{{width:100% !important;max-width:100% !important;margin-left:0 !important}}}}
  .screenshot-placeholder{{color:#8c959f;font-style:italic;padding:24px;background:#f6f8fa;border-radius:6px;text-align:center;border:1px dashed #d0d7de}}
  .step-caption{{font-size:12.5px;color:#6b7280;line-height:1.6;margin-top:6px;white-space:pre-wrap}}
  .step-url{{font-size:12.5px;color:#6b7280;line-height:1.6;margin-top:6px;word-break:break-all}}
  .doc-summary{{background:#f7f8fc;border:1px solid #e6e8f0;border-radius:12px;padding:14px 18px;margin:0 0 16px}}
  .doc-summary p{{font-size:14px;line-height:1.75;color:#24292f;margin:0}}
  .doc-summary ul{{margin:8px 0 0;padding-left:20px}}
  .doc-summary li{{font-size:14px;line-height:1.75;color:#24292f;margin:2px 0}}
  .doc-intro{{font-size:14.5px;line-height:1.8;color:#3f4652;white-space:pre-wrap;margin:0 0 18px}}
  .footer{{margin-top:36px;text-align:center;color:#8c959f;font-size:12px}}
  .footer-qr{{display:block;margin:0 auto 8px;width:88px;height:88px}}
  /* ★ 页脚二维码（对齐插件 `export.ts` 的 `.qr` 系列）：扫码看线上版，教程更新码不用换 */
  .qr{{display:flex;align-items:center;justify-content:center;gap:14px;margin:0 auto 16px;padding:12px 16px;border:1px solid #d0d7de;border-radius:12px;max-width:420px;text-align:left}}
  .qr svg{{display:block;width:84px;height:84px;flex:0 0 auto}}
  .qr-text{{min-width:0}}
  .qr-title{{font-size:13px;font-weight:700;color:#1a1a2e}}
  .qr-url{{font-size:11px;color:#6b7280;word-break:break-all;margin-top:2px}}
  .qr-hint{{font-size:11px;color:#8c959f;margin-top:2px;opacity:.85}}
  .qr{{page-break-inside:avoid}}
  @media print{{
    @page{{margin:0}}
    body{{background:#fff}}
    .doc-header{{-webkit-print-color-adjust:exact;print-color-adjust:exact}}
    .step-card{{-webkit-print-color-adjust:exact;print-color-adjust:exact}}
    .step-card{{page-break-inside:avoid}}
    .shot{{page-break-inside:avoid}}
  }}
</style>
</head>
<body>
<div class="doc-header">
  <div class="doc-eyebrow"><span class="doc-badge">操作指南</span><span class="doc-brand">录步 Flowio</span></div>
  <h1>{title}</h1>
  <div class="doc-titlebar"></div>
  <div class="meta">
    <span>{created_at}</span>
    <span>共 {total_steps} 个步骤</span>
  </div>
</div>
<div class="container">
{step_cards}
</div>
{footer}
</body>
</html>"#,
        title = htmlescape(title),
        total_steps = total_steps,
        created_at = created_at,
        step_cards = step_cards,
        footer = build_html_footer(created_at, public_url, qr_enabled),
    )
}

/// HTML 导出的页脚落款（对齐插件 `export.ts` 的 `<footer class="footer">`）。
///
/// 插件版在「有公开地址（publicUrl）且未关二维码」时，会于落款**上方**插一块
/// 「扫码看线上版」二维码（矢量 SVG，打印不糊）。桌面端没有发布链路，这里按插件
/// 同样的三条边界实现：① 开关关闭 / 没填公开地址 → 什么都不出，不留空框；
/// ② 地址异常导致二维码生成失败 → 只丢二维码，不能带崩整份导出；
/// ③ 地址是用户可控字符串 → 一律 escHtml。落款文案与插件逐字一致。
fn build_html_footer(created_at: &str, public_url: &str, qr_enabled: bool) -> String {
    // 二维码块（对齐插件 `footerQrHtml`）：有公开地址且未关二维码才注入，失败只丢码。
    let qr_slot = build_footer_qr_html(public_url, qr_enabled);
    format!(
        "  <footer class=\"footer\">{}本指南由 录步 Flowio 自动生成 · {}</footer>\n",
        qr_slot,
        htmlescape(created_at),
    )
}

/// 页脚二维码块（对齐插件 `export.ts::footerQrHtml`）。
///
/// 三条边界与插件一致：`qr===false` 不出；`publicUrl` 空串（没发布过）不出；
/// 二维码生成失败只丢码不出（不阻塞导出）。生成失败时把原因打到 stderr 方便排查，
/// 正文仍正常输出 —— 与插件 try/catch 后返回空串的「不能带崩整份导出」语义一致。
fn build_footer_qr_html(public_url: &str, qr_enabled: bool) -> String {
    if !qr_enabled {
        return String::new();
    }
    let url = public_url.trim();
    if url.is_empty() {
        return String::new();
    }
    let svg = match crate::export::qr::qr_svg(url, 3) {
        Ok(svg) => svg,
        Err(e) => {
            eprintln!("[export] 页脚二维码生成失败，跳过二维码（导出继续）：{}", e);
            return String::new();
        }
    };
    format!(
        "  <div class=\"qr\">\n\
            <div class=\"qr-img\">{svg}</div>\n\
            <div class=\"qr-text\">\n\
              <div class=\"qr-title\">📱 扫码看线上版</div>\n\
              <div class=\"qr-url\">{url}</div>\n\
              <div class=\"qr-hint\">教程更新后这个码不用换，永远扫到最新版</div>\n\
            </div>\n\
          </div>\n",
        svg = svg,
        url = htmlescape(url),
    )
}

/// Build a step header for Markdown export.
/// 步骤标题行（对齐插件 `buildMarkdown`：`## 步骤 N：标题` 真二级标题，
/// `#` 留给教程名与标题块）。action_type 不再拼进标题（插件同口径，
/// 动作为 HTML/Word 的胶囊位，MD 不输出）。
fn build_md_step_header(step_number: i32, _action_type: &str, title: &str) -> String {
    if title.trim().is_empty() {
        format!("## 步骤 {}", step_number)
    } else {
        format!("## 步骤 {}：{}", step_number, title)
    }
}

/// Build the description block for Markdown (empty → empty string).
/// 段内单换行转 GFM 硬换行（行尾两空格），防止 Markdown 渲染时折叠成一行。
fn build_md_desc_block(description: &str) -> String {
    build_md_desc_block_kind(description, "")
}

/// 说明块。标注/警告用引用块 + 与预览同色的内联 HTML 底色（GFM 允许内联 HTML），
/// 这样在支持 HTML 的渲染器里看到的颜色与预览一致，纯文本渲染器也能读出语义。
fn build_md_desc_block_kind(description: &str, note_kind: &str) -> String {
    if description.is_empty() {
        return String::new();
    }
    let body = description.replace('\n', "  \n");
    match note_palette(note_kind) {
        Some((bg, fg, bar)) => {
            let label = if note_kind == "warn" { "警告" } else { "标注" };
            format!(
                "<p style=\"background:{bg};color:{fg};border-left:3px solid {bar};\
                 padding:8px 12px\"><strong>{label}：</strong>{body}</p>\n\n",
                bg = bg,
                fg = fg,
                bar = bar,
                label = label,
                body = body,
            )
        }
        None => format!("{}\n\n", body),
    }
}

/// Build the tip block for Markdown (empty → empty string).
fn build_md_tip_block(tip: &str) -> String {
    if tip.is_empty() {
        String::new()
    } else {
        format!("> **提示：** {}\n", tip)
    }
}

/// 来源网址块（对齐插件 `buildMarkdown` 的 `🔗 <url>` 一行）。
/// 空串/纯空白 → 不产出任何内容（本机录制与存量数据天然不渲染）。
fn build_md_url_block(url: &str) -> String {
    let u = url.trim();
    if u.is_empty() {
        String::new()
    } else {
        format!("🔗 {}\n\n", u)
    }
}

/// Build the full Markdown document.
/// 对齐插件 `buildMarkdown`：`# 标题` + 引用块元信息 + 步骤正文 + 结尾落款。
/// 桌面端未落录制总时长字段，故引用块不输出时长段（不编造分钟数）。
fn build_md_document(title: &str, _app_name: &str, total_steps: i32, created_at: &str, step_list: &str) -> String {
    format!(
        "# {title}\n\n\
         > 生成于 {created_at} · 共 {total_steps} 步 · 由 录步 Flowio 生成\n\n\
         {step_list}\n\n\
         ---\n\n\
         *由 录步 Flowio 生成*",
        title = title,
        created_at = created_at,
        total_steps = total_steps,
        step_list = step_list,
    )
}

/// Build a step header for DOCX export.
fn build_docx_step_header(step_number: i32, action_type: &str, title: &str) -> String {
    if action_type.is_empty() {
        format!("步骤 {} · {}", step_number, title)
    } else {
        format!("步骤 {} · {} · {}", step_number, action_type, title)
    }
}

/// Build the metadata text line for DOCX export.
fn build_docx_meta_text(app_name: &str, total_steps: i32, created_at: &str) -> String {
    format!(
        "应用：{}  ·  共 {} 个步骤  ·  {}",
        app_name, total_steps, created_at
    )
}

/// Compute the DOCX image dimensions in EMU (English Metric Units).
/// `dim` is `None` when no screenshot exists; `max_width_emu` is the fallback width.
const EMU_PER_PX: usize = 9525;
// 对齐插件 buildWordHtml：imgW = round(672*zoom)（A4 版心铺满一整行）。
// 旧值 600 会把调用方传入的 672px 上限（MAX_IMG_WIDTH_EMU=6_400_800）盖成 600，
// 与插件口径不一致 —— 改回 672。
const MAX_IMG_PX: usize = 672;

/// docx 图片尺寸：对齐插件 `buildWordHtml` 的 `imgW = max(120, round(672*zoom))`。
/// 桌面端 Word 无全局 zoom 选项（等价 zoom=1）⇒ **一律铺满 A4 版心 672px**，
/// 不足 672px 的截图同样放大到版心宽（插件口径，非「只缩不放」），高度按原宽保比例。
/// 与 PPTX（`compute_image_emu_dims` 区域适配、放大上限 2x）语义不同，故独立成函数。
fn docx_image_emu_dims(dim: Option<(usize, usize)>) -> (u32, u32) {
    let full = (MAX_IMG_PX * EMU_PER_PX) as u32;
    match dim {
        Some((w, h)) if w > 0 && h > 0 => {
            let ratio = MAX_IMG_PX as f64 / w as f64;
            (full, (h as f64 * ratio * EMU_PER_PX as f64) as u32)
        }
        // 读不到尺寸、或宽高为 0（Word 里会渲染异常）：走兜底比例
        _ => (full, (full * 3 / 4) as u32),
    }
}

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

// ── 新增导出格式：逐张图片 / PPTX / 速览视频 / 录屏视频 ──────────
// 与长图同一套图像预处理：maybe_focus_shot_bytes（125% 重点取景）+ 文档块跳过；
// 「鼠标标注」开时用 draw_step_cursor 画红色光标 + 十字。

/// 在截图上画「鼠标标注」：红色圆圈 + 十字线，锚在该步的点击落点。
fn draw_step_cursor(img: &mut image::DynamicImage, x: Option<i32>, y: Option<i32>) {
    use image::GenericImage as _; // image 0.25：put_pixel 在 GenericImage trait 上
    let (Some(cx), Some(cy)) = (x, y) else { return };
    let (w, h) = (img.width() as i32, img.height() as i32);
    if cx < 0 || cy < 0 || cx >= w || cy >= h {
        return;
    }
    let radius = 14i32;
    let color = image::Rgba([250, 81, 81, 255]); // 柔红 #FA5151
    // 空心圆（中点圆算法，只描边不填充）
    for t in 0..360 {
        let rad = (t as f64).to_radians();
        let px = cx + (radius as f64 * rad.cos()).round() as i32;
        let py = cy + (radius as f64 * rad.sin()).round() as i32;
        if px >= 0 && py >= 0 && px < w && py < h {
            img.put_pixel(px as u32, py as u32, color);
        }
    }
    // 十字线（圆心 ±6px，不进圆内太多）
    for d in -6..=6 {
        for (px, py) in [(cx + d, cy), (cx, cy + d)] {
            if px >= 0 && py >= 0 && px < w && py < h {
                img.put_pixel(px as u32, py as u32, color);
            }
        }
    }
}

/// 逐张图片交付：每张带截图的步骤单独出一张 PNG，连同一份 `说明.txt` 打成一个 zip。
///
/// ★ 已按插件口径重做（对齐 flowio-extension 的 pages/editor.tsx::exportImageFiles）：
///   · 包内文件名 `步骤{编号}.png`（编号用步骤序号，与插件的 `步骤{order}` 同口径）；
///   · `说明.txt` 首行「由 Flowio 录步 导出 —— {教程标题}」，空行之后逐条
///     「步骤N：标题」，有描述再补一行两空格缩进的描述；
///   · 整个包用 **store** 模式打包（不压缩，与插件 zipStore 同口径）。
fn export_individual_images(
    recording: &ExportRecording,
    steps: &[ExportStep],
    output_path: &std::path::Path,
    zoom_map: &std::collections::HashMap<String, f64>,
    include_cursor: bool,
    image_zoom: f64,
    gif: bool,
) -> Result<(), String> {
    // ⚠️ image_zoom 已钉死 1（UI 撤比例设置，与插件一致）：不再缩放单图，保留形参兼容调用方。
    let _ = image_zoom;
    let zoom_ratio = 1.0;
    let mut entries: Vec<(String, Vec<u8>)> = Vec::new();
    let display_title = if recording.title.trim().is_empty() {
        "未命名教程".to_string()
    } else {
        recording.title.trim().to_string()
    };
    let mut caption: Vec<String> = vec![
        format!("由 Flowio 录步 导出 —— {}", display_title),
        String::new(),
    ];

    // GIF 需要一个真实文件落点：用完即清（下面统一收尾）
    let gif_dir = std::env::temp_dir().join(format!("flowio_images_{}", std::process::id()));

    for s in steps {
        if is_doc_block(s) {
            continue;
        }
        let order = s.step_number;
        let mut added = false;

        // 视频步骤且开启「动图」⇒ 尝试输出 GIF（与其它格式同一个开关）
        if gif && s.media_kind.eq_ignore_ascii_case("video") && !s.media_source_path.is_empty() {
            let src = std::path::Path::new(&s.media_source_path);
            if src.exists() {
                let _ = std::fs::create_dir_all(&gif_dir);
                let out = gif_dir.join(format!("step-{}.gif", order));
                match video_to_gif(src, &out, zoom_ratio) {
                    Ok(()) => match std::fs::read(&out) {
                        Ok(data) if !data.is_empty() => {
                            entries.push((format!("步骤{}.gif", order), data));
                            added = true;
                        }
                        _ => {}
                    },
                    Err(e) => eprintln!("视频转 GIF 失败（{}）: {}", s.media_source_path, e),
                }
            }
        }

        if !added && !s.screenshot_path.is_empty() {
            if let Ok(raw) = std::fs::read(&s.screenshot_path) {
                if !raw.is_empty() {
                    // 重点图裁切口径与其它导出保持一致
                    let bytes = maybe_focus_shot_bytes(raw, s.x, s.y, zoom_for(zoom_map, &s.id)).0;
                    if let Ok(mut img) = image::load_from_memory(&bytes) {
                        if include_cursor {
                            draw_step_cursor(&mut img, s.x, s.y);
                        }
                        if (zoom_ratio - 1.0).abs() > 0.001 {
                            let new_w = ((img.width() as f64) * zoom_ratio).round().max(1.0) as u32;
                            let new_h = ((img.height() as f64) * zoom_ratio).round().max(1.0) as u32;
                            img = img.resize_exact(new_w, new_h, image::imageops::FilterType::Lanczos3);
                        }
                        let mut buf: Vec<u8> = Vec::new();
                        if img
                            .write_to(&mut std::io::Cursor::new(&mut buf), image::ImageFormat::Png)
                            .is_ok()
                        {
                            entries.push((format!("步骤{}.png", order), buf));
                            added = true;
                        }
                    }
                }
            }
        }

        // 说明.txt 只登记**真正进了包**的步骤（与插件同口径）
        if added {
            let title = s.title.trim();
            caption.push(if title.is_empty() {
                format!("步骤{}", order)
            } else {
                format!("步骤{}：{}", order, title)
            });
            let desc = s.description.trim();
            if !desc.is_empty() {
                caption.push(format!("  {}", desc));
            }
        }
    }

    if entries.is_empty() {
        return Err("该教程没有任何截图，无法导出图片".to_string());
    }
    entries.push(("说明.txt".to_string(), caption.join("\n").into_bytes()));

    let result = write_zip_store(output_path, &entries);
    if gif_dir.exists() {
        let _ = std::fs::remove_dir_all(&gif_dir);
    }
    result
}

/// 把若干 `(包内文件名, 字节)` 写成一个 **store 模式**（不压缩）的 zip。
///
/// 图片本身已是压缩格式，再 deflate 一遍只白费 CPU —— 插件端 zipStore 也是这么做的。
pub(crate) fn write_zip_store(
    dst: &std::path::Path,
    entries: &[(String, Vec<u8>)],
) -> Result<(), String> {
    use std::io::Write as _;
    let file = std::fs::File::create(dst).map_err(|e| format!("创建压缩包失败: {}", e))?;
    let mut zip = zip::ZipWriter::new(file);
    for (name, data) in entries {
        let options: zip::write::FileOptions<'_, ()> = zip::write::FileOptions::default()
            .compression_method(zip::CompressionMethod::Stored);
        zip.start_file(name.to_string(), options)
            .map_err(|e| format!("写入压缩包条目 {} 失败: {}", name, e))?;
        zip.write_all(data)
            .map_err(|e| format!("写入压缩包数据 {} 失败: {}", name, e))?;
    }
    zip.finish().map_err(|e| format!("收尾压缩包失败: {}", e))?;
    Ok(())
}


/// PPTX：每步一张幻灯片，手写 OOXML 生成（B 批对齐插件 buildPptx）。
/// 版式：封面（品牌色带+大标题+落款）/ 目录（序号网格，按条目数自动分栏）/
/// 步骤页（橙色徽章+动作胶囊+截图投影+页脚网址·页码）/ 章节页（heading 块）。
fn export_pptx(
    recording: &ExportRecording,
    steps: &[ExportStep],
    output_path: &std::path::Path,
    zoom_map: &std::collections::HashMap<String, f64>,
    include_cursor: bool,
    include_toc: bool,
) -> Result<(), String> {
    let mut pptx_steps: Vec<pptx::PptxStep> = Vec::new();
    let mut any_visual = false;

    for s in steps {
        let is_doc = is_doc_block(s);
        let heading_level = if is_heading_block(s) { Some(heading_level(s)) } else { None };

        // 截图：读原图 → 焦点/裁剪 → 可选光标 → 重新编码 PNG（与旧链路一致）
        let image = if s.screenshot_path.is_empty() {
            None
        } else {
            let raw = match std::fs::read(&s.screenshot_path) {
                Ok(b) if !b.is_empty() => b,
                _ => Vec::new(),
            };
            if raw.is_empty() {
                None
            } else {
                let bytes = maybe_focus_shot_bytes(raw, s.x, s.y, zoom_for(zoom_map, &s.id)).0;
                image::load_from_memory(&bytes)
                    .ok()
                    .map(|mut img| {
                        if include_cursor {
                            draw_step_cursor(&mut img, s.x, s.y);
                        }
                        let mut buf = std::io::Cursor::new(Vec::new());
                        let _ = img.write_to(&mut buf, image::ImageFormat::Png);
                        buf.into_inner()
                    })
            }
        };

        if is_doc && image.is_none() && s.title.trim().is_empty() && s.description.trim().is_empty() {
            continue; // 完全空白的文档块，跳过
        }
        if image.is_some() || !s.title.trim().is_empty() || !s.description.trim().is_empty() {
            any_visual = true;
        }

        pptx_steps.push(pptx::PptxStep {
            title: s.title.clone(),
            title_html: s.title_html.clone(),
            heading_level,
            description: s.description.clone(),
            action_label: if is_doc { String::new() } else { to_action_label(&s.action_type) },
            url: s.url.clone(),
            image,
            step_number: if is_doc { 0 } else { s.step_number },
        });
    }

    if !any_visual {
        return Err("该教程没有任何内容，无法导出 PPT".to_string());
    }

    let entries = pptx::build_pptx_entries(&recording.title, &recording.created_at, &pptx_steps, include_toc)?;
    write_zip_store(output_path, &entries)
}

/// 视频（速览 / 录屏）：步骤截图串成 slideshow MP4，依赖 ffmpeg。
/// quick=true → 每步 1 秒（速览）；false → 每步 3 秒（录屏节奏）。
// ── 视频导出的「其他设置」（第四十八轮）────────────────────────
//
// 设置项与竞品对齐：字幕样式 / 字幕大小 / 配音 / 镜头缩放 / 视频封面 /
// 速度 / 去除水印 / 自定义品牌水印 / 高清。
// 其中字幕与品牌水印**直接烧进每一帧 PNG**（复用已有中文字体加载器，
// 比 ffmpeg drawtext 的转义/字体路径稳），ffmpeg 只负责分辨率、推镜、变速、
// 音频混合与编码。
#[derive(Debug, Clone, serde::Deserialize)]
#[serde(rename_all = "camelCase", default)]
struct VideoOptions {
    /// 字幕样式：`bg`=字幕带背板（默认）/ `plain`=纯文字加描边
    /// （对齐插件 SUB_STYLES；`none` 仅为旧存档兼容，界面已不再提供该档）
    subtitle_style: String,
    /// 字幕大小：`small` / `medium` / `large`
    subtitle_size: String,
    /// 配音：用系统中文语音朗读每步标题，并按时轴贴回视频
    voiceover: bool,
    /// 镜头缩放：画面缓慢推近（Ken Burns）
    ken_burns: bool,
    /// 视频封面：额外导出一张首帧封面图 `<文件名>_封面.jpg`
    cover: bool,
    /// 播放倍速：1 / 1.25 / 1.5（对齐插件 SPEED_CHOICES）
    speed: f64,
    /// 去除水印：勾选则不烧右下角品牌水印（对齐插件 watermark="off"）；
    /// 缺省 false = 保留水印（对齐插件默认档 "on"）。
    remove_watermark: bool,
    /// 自定义品牌水印文字（留空 = 用默认品牌名「录步 Flowio」，对齐插件 wmText）
    brand_watermark: String,
    /// 高清：输出 2 倍分辨率 + 更高码率
    hd: bool,
}

impl Default for VideoOptions {
    fn default() -> Self {
        Self {
            subtitle_style: "bg".to_string(),
            subtitle_size: "medium".to_string(),
            voiceover: true,
            ken_burns: true,
            cover: true,
            speed: 1.0,
            remove_watermark: false,
            brand_watermark: String::new(),
            hd: false,
        }
    }
}

fn parse_video_options(options: Option<&serde_json::Value>) -> VideoOptions {
    options
        .and_then(|v| serde_json::from_value::<VideoOptions>(v.clone()).ok())
        .unwrap_or_default()
}

/// 半透明矩形填充（imageproc 的 draw_filled_rect_mut 是直接覆盖，
/// 这里手动做 alpha 混合，让字幕背板/水印不糊成一坨黑）。
fn blend_rect(
    img: &mut image::RgbaImage,
    x0: i32,
    y0: i32,
    x1: i32,
    y1: i32,
    color: [u8; 4],
) {
    let a = color[3] as f32 / 255.0;
    let (w, h) = (img.width() as i32, img.height() as i32);
    for y in y0.max(0)..y1.min(h) {
        for x in x0.max(0)..x1.min(w) {
            let p = img.get_pixel_mut(x as u32, y as u32);
            for i in 0..3 {
                p.0[i] = (p.0[i] as f32 * (1.0 - a) + color[i] as f32 * a).round() as u8;
            }
            p.0[3] = 255;
        }
    }
}

/// 字号按「1280 宽」为基准等比换算，保证 1920 宽的截图字幕不会显得太小。
fn video_text_px(img_w: u32, base: f32) -> f32 {
    (base * (img_w as f32 / 1280.0)).max(14.0)
}

/// 把步骤标题画成字幕（底部居中）。
fn draw_subtitle(img: &mut image::DynamicImage, text: &str, style: &str, size: &str) {
    let text = text.trim();
    if style == "none" || text.is_empty() {
        return;
    }
    let Some(font) = get_font() else { return };
    let mut rgba = img.to_rgba8();
    let (w, h) = (rgba.width(), rgba.height());
    let base_px = match size {
        "small" => 24.0,
        "large" => 44.0,
        _ => 33.0,
    };
    let px = video_text_px(w, base_px);
    let scale = ab_glyph::PxScale { x: px, y: px };
    let (tw, th) = imageproc::drawing::text_size(scale, &font, text);
    let pad_x = (px * 0.62) as i32;
    let pad_y = (px * 0.34) as i32;
    let x = ((w as i32 - tw as i32) / 2).max(pad_x + 4);
    let y = (h as i32 - th as i32 - (h as i32 / 14)).max(0);

    if style == "bg" {
        blend_rect(
            &mut rgba,
            x - pad_x,
            y - pad_y,
            x + tw as i32 + pad_x,
            y + th as i32 + pad_y,
            [12, 16, 24, 185],
        );
    } else {
        // 纯文字：四周描边，避免落在浅色截图上读不清
        for (dx, dy) in [(-2, 0), (2, 0), (0, -2), (0, 2)] {
            imageproc::drawing::draw_text_mut(
                &mut rgba,
                image::Rgba([0, 0, 0, 200]),
                x + dx,
                y + dy,
                scale,
                &font,
                text,
            );
        }
    }
    imageproc::drawing::draw_text_mut(
        &mut rgba,
        image::Rgba([255, 255, 255, 255]),
        x,
        y,
        scale,
        &font,
        text,
    );
    *img = image::DynamicImage::ImageRgba8(rgba);
}

/// 品牌水印默认文字：自定义留空时用它（对齐插件 `wmText` 空串回落默认品牌名）。
const DEFAULT_WATERMARK_TEXT: &str = "录步 Flowio";

/// 自定义品牌水印：右下角，浅色字 + 深色投影。
/// `text` 为空/全空白时回落默认品牌名（对齐插件语义，不再直接跳过）。
fn draw_brand_watermark(img: &mut image::DynamicImage, text: &str) {
    let trimmed = text.trim();
    let text = if trimmed.is_empty() { DEFAULT_WATERMARK_TEXT } else { trimmed };
    let Some(font) = get_font() else { return };
    let mut rgba = img.to_rgba8();
    let (w, h) = (rgba.width(), rgba.height());
    let px = video_text_px(w, 26.0);
    let scale = ab_glyph::PxScale { x: px, y: px };
    let (tw, th) = imageproc::drawing::text_size(scale, &font, text);
    let margin = (w as f32 * 0.022).max(16.0) as i32;
    let x = (w as i32 - tw as i32 - margin).max(0);
    let y = (h as i32 - th as i32 - margin).max(0);
    imageproc::drawing::draw_text_mut(
        &mut rgba,
        image::Rgba([0, 0, 0, 170]),
        x + 2,
        y + 2,
        scale,
        &font,
        text,
    );
    imageproc::drawing::draw_text_mut(
        &mut rgba,
        image::Rgba([255, 255, 255, 220]),
        x,
        y,
        scale,
        &font,
        text,
    );
    *img = image::DynamicImage::ImageRgba8(rgba);
}

/// 用系统语音合成（SAPI）把每步字幕读成 wav，返回按步骤序号的音频文件列表。
/// 一次 PowerShell 进程内合成全部句子（逐步启动进程会慢到不可用）。
/// 失败返回 Err，调用方降级为「无配音导出」，不影响视频本身。
fn synth_voiceover(lines: &[String], dir: &std::path::Path) -> Result<Vec<std::path::PathBuf>, String> {
    if lines.is_empty() {
        return Err("没有可朗读的步骤".to_string());
    }
    let list_path = dir.join("vo_lines.txt");
    // 空行也用占位，保证行号 == 步骤序号
    let body = lines
        .iter()
        .map(|l| if l.trim().is_empty() { "　".to_string() } else { l.trim().to_string() })
        .collect::<Vec<_>>()
        .join("\n");
    std::fs::write(&list_path, body).map_err(|e| format!("写入配音文本失败: {}", e))?;

    let ps_path = dir.join("synth.ps1");
    let script = r#"param([string]$ListFile, [string]$OutDir)
$ErrorActionPreference = 'Stop'
Add-Type -AssemblyName System.Speech
$synth = New-Object System.Speech.Synthesis.SpeechSynthesizer
foreach ($v in $synth.GetInstalledVoices()) {
  if ($v.VoiceInfo.Culture.Name -like 'zh*') { $synth.SelectVoice($v.VoiceInfo.Name); break }
}
$synth.Rate = 1
$lines = Get-Content -LiteralPath $ListFile -Encoding UTF8
$i = 0
foreach ($ln in $lines) {
  $out = Join-Path $OutDir ('vo_{0:d4}.wav' -f $i)
  $synth.SetOutputToWaveFile($out)
  $synth.Speak([string]$ln)
  $synth.SetOutputToNull()
  $i++
}
$synth.Dispose()
Write-Output 'SYNTH_OK'
"#;
    std::fs::write(&ps_path, script).map_err(|e| format!("写入配音脚本失败: {}", e))?;

    let mut ps_cmd = std::process::Command::new("powershell");
    no_console_window(&mut ps_cmd);
    let status = ps_cmd
        .args([
            "-NoProfile",
            "-ExecutionPolicy",
            "Bypass",
            "-File",
            &ps_path.to_string_lossy(),
            "-ListFile",
            &list_path.to_string_lossy(),
            "-OutDir",
            &dir.to_string_lossy(),
        ])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map_err(|e| format!("启动语音合成失败: {}", e))?;
    if !status.success() {
        return Err("语音合成进程异常退出".to_string());
    }

    let mut out: Vec<std::path::PathBuf> = Vec::new();
    for i in 0..lines.len() {
        let p = dir.join(format!("vo_{:04}.wav", i));
        if !p.exists() {
            return Err(format!("第 {} 句语音未生成", i + 1));
        }
        out.push(p);
    }
    Ok(out)
}

// ──────────────────────────────────────────────────────────────
// 速览视频「运镜/时间轴」纯函数（对齐插件 flowio-extension/src/lib/reel.ts v0.65.180）
// 坐标一律为分数坐标（0..1，相对图片自身像素），与插件 RectF 同一口径。
// 为什么抽纯函数：视频只能「肉眼验收」，把几何 + 时间轴钉死成不变式，
// export_video 只负责照着算好的帧去画。
// ──────────────────────────────────────────────────────────────

/// 分数坐标矩形（相对图片像素，0..1）。
#[derive(Debug, Clone, Copy, PartialEq)]
struct ReelRectF {
    x: f64,
    y: f64,
    w: f64,
    h: f64,
}

/// 每步停留时长（插件 REEL_DEFAULTS.holdMs = 1800）。
const REEL_DEFAULT_HOLD_MS: i64 = 1800;
/// 步骤间运镜时长（插件 REEL_DEFAULTS.moveMs = 700）。
const REEL_DEFAULT_MOVE_MS: i64 = 700;
/// 焦点框外扩比例（插件 REEL_DEFAULTS.pad = 0.3，四周各留 30% 上下文）。
const REEL_DEFAULT_PAD: f64 = 0.3;
/// 视野相对「整图可见」的最大放大倍率（插件 REEL_DEFAULTS.maxZoom = 3，防推糊）。
const REEL_DEFAULT_MAX_ZOOM: f64 = 3.0;
/// 录制帧率（插件 REEL_DEFAULTS.fps = 30；桌面端沿用 24，只影响帧粒度不影响时长语义）。
const REEL_FPS: f64 = 24.0;
/// 总时长目标：步骤多时自动压缩每步停留（插件 REEL_TARGET_MS = 75_000）。
const REEL_TARGET_MS: i64 = 75_000;
/// 每步停留下限：低于此值看不清，宁可超时长也不再压（插件 REEL_MIN_HOLD_MS = 900）。
const REEL_MIN_HOLD_MS: i64 = 900;

fn reel_clamp(v: f64, lo: f64, hi: f64) -> f64 {
    if v < lo {
        lo
    } else if v > hi {
        hi
    } else {
        v
    }
}

/// 把焦点框变成可用镜头视野（插件 fitView 四步：归一化→外扩→套宽高比→夹紧）。
fn fit_reel_view(box_f: Option<ReelRectF>, aspect: f64, pad: f64, max_zoom: f64) -> ReelRectF {
    let a = if aspect.is_finite() && aspect > 0.0 {
        aspect
    } else {
        16.0 / 9.0
    };
    let w_limit = if a >= 1.0 { 1.0 } else { a };
    let h_limit = w_limit / a;
    let full = ReelRectF {
        x: 0.0,
        y: 0.0,
        w: w_limit,
        h: h_limit,
    };
    let Some(b) = box_f else { return full };
    let bw = if b.w.is_finite() { b.w.abs() } else { 0.0 };
    let bh = if b.h.is_finite() { b.h.abs() } else { 0.0 };
    if bw <= 0.0 || bh <= 0.0 {
        return full;
    }
    // ① 归一化到图内
    let bx = reel_clamp(if b.x.is_finite() { b.x } else { 0.0 }, 0.0, 1.0);
    let by = reel_clamp(if b.y.is_finite() { b.y } else { 0.0 }, 0.0, 1.0);
    let bx1 = reel_clamp(bx + bw, bx, 1.0);
    let by1 = reel_clamp(by + bh, by, 1.0);
    let zoom_cap = if max_zoom.is_finite() && max_zoom > 1.0 {
        max_zoom
    } else {
        1.0
    };
    let p = if pad.is_finite() { pad } else { 0.0 };
    // ② 外扩 + ③ 套画布宽高比（取较大一边，保证焦点整块都在画面里）
    let mut w = (bx1 - bx) * (1.0 + 2.0 * p);
    let mut h = (by1 - by) * (1.0 + 2.0 * p);
    if w / h < a {
        w = h * a;
    } else {
        h = w / a;
    }
    // ④ 倍率上限（下界）→ 整图（上界）
    let w_floor = w_limit / zoom_cap;
    if w < w_floor {
        w = w_floor;
        h = w / a;
    }
    if w > w_limit {
        w = w_limit;
        h = h_limit;
    }
    let cx = (bx + bx1) / 2.0;
    let cy = (by + by1) / 2.0;
    ReelRectF {
        x: reel_clamp(cx - w / 2.0, 0.0, 1.0 - w),
        y: reel_clamp(cy - h / 2.0, 0.0, 1.0 - h),
        w,
        h,
    }
}

/// smoothstep：端点必须精确 0/1，否则首尾差半帧「开头抖一下」。
fn reel_ease_in_out(t: f64) -> f64 {
    if !t.is_finite() || t <= 0.0 {
        return 0.0;
    }
    if t >= 1.0 {
        return 1.0;
    }
    t * t * (3.0 - 2.0 * t)
}

/// 两个视野线性插值（a、b 宽高比相同 ⇒ 结果也同）。
fn reel_lerp_view(a: ReelRectF, b: ReelRectF, t: f64) -> ReelRectF {
    let k = reel_clamp(if t.is_finite() { t } else { 0.0 }, 0.0, 1.0);
    ReelRectF {
        x: a.x + (b.x - a.x) * k,
        y: a.y + (b.y - a.y) * k,
        w: a.w + (b.w - a.w) * k,
        h: a.h + (b.h - a.h) * k,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ReelPhase {
    Move,
    Hold,
}

#[derive(Debug, Clone, Copy)]
struct ReelSegment {
    kind: ReelPhase,
    step_index: usize,
    from: ReelRectF,
    to: ReelRectF,
    start_ms: i64,
    end_ms: i64,
}

#[derive(Debug, Clone)]
struct ReelTimeline {
    segments: Vec<ReelSegment>,
    views: Vec<ReelRectF>,
    total_ms: i64,
}

/// 时间轴 = 每步「运镜 → 停留」交替；第 0 步从整图推入（插件 buildTimeline）。
/// 运镜对齐步与步的关系，停留对齐这一步本身——合成一段会看晕。
fn build_reel_timeline(
    boxes: &[Option<ReelRectF>],
    aspect: f64,
    pad: f64,
    max_zoom: f64,
    move_ms: i64,
    hold_ms: i64,
) -> Result<ReelTimeline, String> {
    let a = if aspect.is_finite() && aspect > 0.0 {
        aspect
    } else {
        16.0 / 9.0
    };
    let w_limit = if a >= 1.0 { 1.0 } else { a };
    let full = ReelRectF {
        x: 0.0,
        y: 0.0,
        w: w_limit,
        h: w_limit / a,
    };
    let views: Vec<ReelRectF> = boxes
        .iter()
        .map(|b| fit_reel_view(*b, a, pad, max_zoom))
        .collect();
    let mv = move_ms.max(0);
    let hd = hold_ms.max(0);
    let mut segments: Vec<ReelSegment> = Vec::new();
    let mut t: i64 = 0;
    for (i, to) in views.iter().enumerate() {
        let from = if i == 0 { full } else { views[i - 1] };
        if mv > 0 {
            segments.push(ReelSegment {
                kind: ReelPhase::Move,
                step_index: i,
                from,
                to: *to,
                start_ms: t,
                end_ms: t + mv,
            });
            t += mv;
        }
        segments.push(ReelSegment {
            kind: ReelPhase::Hold,
            step_index: i,
            from: *to,
            to: *to,
            start_ms: t,
            end_ms: t + hd,
        });
        t += hd;
    }
    if t <= 0 {
        return Err("速览视频：时间轴长度为 0（每步停留与运镜不能同时为 0）".to_string());
    }
    Ok(ReelTimeline {
        segments,
        views,
        total_ms: t,
    })
}

/// 取 t 时刻的画面（插件 sampleAt）：hold 段完全静止，move 段 smoothstep。
fn sample_reel_at(tl: &ReelTimeline, t_ms: i64) -> (usize, ReelPhase, f64, ReelRectF) {
    let t = reel_clamp(t_ms as f64, 0.0, tl.total_ms as f64) as i64;
    let mut seg = *tl.segments.last().expect("速览视频：时间轴为空");
    for s in &tl.segments {
        if t >= s.start_ms && t < s.end_ms {
            seg = *s;
            break;
        }
    }
    let span = seg.end_ms - seg.start_ms;
    let p = if span > 0 {
        (t - seg.start_ms) as f64 / span as f64
    } else {
        1.0
    };
    let e = if seg.kind == ReelPhase::Hold {
        1.0
    } else {
        reel_ease_in_out(p)
    };
    (seg.step_index, seg.kind, e, reel_lerp_view(seg.from, seg.to, e))
}

/// 按 fps 折算总时长需要的帧数（插件 frameCount，最少 2 帧）。
fn reel_frame_count(total_ms: i64, fps: f64) -> usize {
    let f = if fps.is_finite() && fps > 0.0 { fps } else { 30.0 };
    (total_ms as f64 / 1000.0 * f).round().max(2.0) as usize
}

/// 步骤多时压缩每步停留，把总时长往 REEL_TARGET_MS 收（插件 adaptOptions）。
/// 有下限 REEL_MIN_HOLD_MS：宁可超时长，也不做成一闪而过。运镜时长不动。
fn reel_adapt_hold_ms(step_count: usize, move_ms: i64, base_hold_ms: i64) -> i64 {
    let n = step_count as i64;
    if n <= 0 {
        return base_hold_ms;
    }
    let allowed = (REEL_TARGET_MS as f64 / n as f64).floor() as i64 - move_ms;
    reel_clamp(
        allowed as f64,
        REEL_MIN_HOLD_MS as f64,
        base_hold_ms as f64,
    )
    .round() as i64
}

/// 导出视频：逐帧按 reel 视野渲染（对齐插件 reel.ts 运镜/节奏），
/// 支持字幕 / 品牌水印 / 变速 / 高清 / 配音 / 封面 / 整体 GIF。
#[allow(clippy::too_many_arguments)]
fn export_video(
    _recording: &ExportRecording,
    steps: &[ExportStep],
    output_path: &std::path::Path,
    _zoom_map: &std::collections::HashMap<String, f64>,
    include_cursor: bool,
    quick: bool,
    vopt: &VideoOptions,
    gif: bool,
) -> Result<(), String> {
    let temp_dir = std::env::temp_dir().join(format!("flowio_video_{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&temp_dir)
        .map_err(|e| format!("创建视频临时目录失败: {}", e))?;

    // ── 原始视频步骤优先交付（对齐插件「录屏视频=原始录像」语义）────────
    // 插件侧录屏是 MediaRecorder 原始录像，导出时直接交付原片；桌面端教程若含
    // 「视频」步骤（media_kind=video 且 media_source_path 指向真实录像），导出同样
    // 优先交付原始录像：单个直接拷贝（不依赖 ffmpeg），多个尝试 concat 无损拼接
    // （同编码才有效；有 ffmpeg 才尝试），拼接失败时回落截图幻灯片合成（既有能力
    // 不回归）。纯截图教程维持幻灯片。
    let video_sources: Vec<&str> = steps
        .iter()
        .filter(|s| s.media_kind == "video" && !s.media_source_path.trim().is_empty())
        .map(|s| s.media_source_path.trim())
        .collect();
    if !video_sources.is_empty() {
        let exists: Vec<&std::path::Path> = video_sources
            .iter()
            .map(|p| std::path::Path::new(p))
            .filter(|p| p.exists())
            .collect();
        if !exists.is_empty() {
            if exists.len() == 1 {
                // 单个原始视频：直接拷贝交付
                std::fs::copy(exists[0], output_path)
                    .map_err(|e| format!("复制原始录像失败: {}", e))?;
                let _ = std::fs::remove_dir_all(&temp_dir);
                return Ok(());
            }
            // 多个原始视频：concat demuxer 无损拼接（同编码/同分辨率才有效）
            let concat_list = temp_dir.join("concat.txt");
            let mut list_content = String::new();
            for p in &exists {
                let abs = std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf());
                list_content.push_str(&format!(
                    "file '{}'\n",
                    abs.to_string_lossy().replace('\\', "/")
                ));
            }
            let mut concat_ok = false;
            if std::fs::write(&concat_list, list_content).is_ok() {
                if let Some(ffmpeg) = which_ffmpeg() {
                    let status = std::process::Command::new(&ffmpeg)
                        .args(["-y", "-f", "concat", "-safe", "0", "-i"])
                        .arg(&concat_list)
                        .args(["-c", "copy"])
                        .arg(output_path)
                        .status();
                    concat_ok = matches!(status, Ok(st) if st.success());
                }
            }
            if concat_ok {
                let _ = std::fs::remove_dir_all(&temp_dir);
                return Ok(());
            }
            eprintln!("[export] 原始录像拼接失败，回落截图幻灯片合成");
        }
    }

    // 视频导出依赖 ffmpeg（合成/推镜/字幕/水印阶段）。未安装时给出清晰提示。
    let ffmpeg = which_ffmpeg()
        .ok_or("未检测到 ffmpeg，无法导出视频。请先安装 ffmpeg 并加入 PATH")?;

    let speed = if vopt.speed > 0.05 { vopt.speed } else { 1.0 };

    // ── ① 读源图 + 每步焦点框（分数坐标，crop_region_50 口径 = 插件 resolveStepShot().box）──
    //    zoom_map 不再用于视频：焦点框来自落库 crop 字段（5b 已对齐插件 computeCropRect），
    //    与 HTML/Word/MD 同一出口，避免视频继续用旧 focus_shot_box 口径产生偏差。
    struct ReelSrc {
        path: std::path::PathBuf,
        box_f: Option<ReelRectF>,
        title: String,
        step_number: i32,
        x: Option<i32>,
        y: Option<i32>,
        img_w: u32,
        img_h: u32,
    }
    let mut srcs: Vec<ReelSrc> = Vec::new();
    for s in steps {
        if is_doc_block(s) || s.screenshot_path.is_empty() {
            continue;
        }
        let raw = match std::fs::read(&s.screenshot_path) {
            Ok(b) if !b.is_empty() => b,
            _ => continue,
        };
        let img = match image::load_from_memory(&raw) {
            Ok(i) => i,
            Err(_) => continue,
        };
        let (iw, ih) = (img.width(), img.height());
        let box_f = export_crop_box(
            iw as i32,
            ih as i32,
            s.x,
            s.y,
            s.crop_w,
            s.crop_h,
            &s.action_type,
        )
        .map(|(l, t, w, h)| ReelRectF {
            x: l as f64 / iw as f64,
            y: t as f64 / ih as f64,
            w: w as f64 / iw as f64,
            h: h as f64 / ih as f64,
        });
        let src_path = temp_dir.join(format!("src-{:04}.png", s.step_number));
        img.save(&src_path)
            .map_err(|e| format!("保存视频源图失败: {}", e))?;
        srcs.push(ReelSrc {
            path: src_path,
            box_f,
            title: s.title.trim().to_string(),
            step_number: s.step_number,
            x: s.x,
            y: s.y,
            img_w: iw,
            img_h: ih,
        });
    }

    if srcs.is_empty() {
        let _ = std::fs::remove_dir_all(&temp_dir);
        return Err("该教程没有任何截图，无法导出视频".to_string());
    }

    // ── ② 输出尺寸：高清 = 2 倍（H.264 要求宽高为偶数）──────────────────
    let scale_factor = if vopt.hd { 2 } else { 1 };
    let out_w = (srcs[0].img_w * scale_factor).max(2) & !1;
    let out_h = (srcs[0].img_h * scale_factor).max(2) & !1;

    // ── ③ 时间轴（插件 buildTimeline / adaptOptions 口径）───────────────
    //    pan 焦点：运镜目标 = 每步焦点框（fit_reel_view 外扩 + 套宽高比 + 夹紧）
    //    cover 节奏：move(700ms)→hold(1800ms) 交替，第 0 步从整图推入；
    //                步骤多时 adaptOptions 把停留往 75s 总时长收（下限 900ms）
    //    quick 预览：不做运镜（move=0），每步只停 900ms（保留原 1.0s/步的快出语义）
    let move_ms: i64 = if quick { 0 } else { REEL_DEFAULT_MOVE_MS };
    let hold_ms: i64 = if quick {
        REEL_MIN_HOLD_MS
    } else {
        reel_adapt_hold_ms(srcs.len(), REEL_DEFAULT_MOVE_MS, REEL_DEFAULT_HOLD_MS)
    };
    let tl = build_reel_timeline(
        &srcs.iter().map(|s| s.box_f).collect::<Vec<_>>(),
        out_w as f64 / out_h as f64,
        REEL_DEFAULT_PAD,
        REEL_DEFAULT_MAX_ZOOM,
        move_ms,
        hold_ms,
    )?;
    let frame_n = reel_frame_count(tl.total_ms, REEL_FPS);

    // ── ④ 逐帧渲染：按 sampleAt 视野从原图裁切 + 缩放，替代旧 zoompan 中心推近 ──
    let mut frames: Vec<std::path::PathBuf> = Vec::new();
    for fi in 0..frame_n {
        let t_ms = (fi as f64 * 1000.0 / REEL_FPS).round() as i64;
        let (idx, _kind, _e, view) = sample_reel_at(&tl, t_ms);
        let src = &srcs[idx];
        let mut img = match image::open(&src.path) {
            Ok(i) => i,
            Err(e) => return Err(format!("读取视频源图失败: {}", e)),
        };
        let (iw, ih) = (img.width(), img.height());
        let left = ((view.x * iw as f64).round() as i32).clamp(0, iw as i32 - 1);
        let top = ((view.y * ih as f64).round() as i32).clamp(0, ih as i32 - 1);
        let cw = ((view.w * iw as f64).round() as i32).clamp(1, iw as i32 - left);
        let ch = ((view.h * ih as f64).round() as i32).clamp(1, ih as i32 - top);
        let mut frame = img.crop_imm(left as u32, top as u32, cw as u32, ch as u32);
        frame = frame.resize_exact(out_w, out_h, image::imageops::FilterType::Lanczos3);
        if include_cursor {
            // 光标是原图坐标：换算到裁切后的帧坐标，视野外则不画
            if let (Some(cx), Some(cy)) = (src.x, src.y) {
                let rx = (cx as f64 - left as f64) / cw as f64;
                let ry = (cy as f64 - top as f64) / ch as f64;
                if (0.0..=1.0).contains(&rx) && (0.0..=1.0).contains(&ry) {
                    draw_step_cursor(
                        &mut frame,
                        Some((rx * out_w as f64).round() as i32),
                        Some((ry * out_h as f64).round() as i32),
                    );
                }
            }
        }
        // 字幕 + 品牌水印烧进帧（不受编码器缩放影响读不清）
        draw_subtitle(&mut frame, &src.title, &vopt.subtitle_style, &vopt.subtitle_size);
        if !vopt.remove_watermark {
            draw_brand_watermark(&mut frame, &vopt.brand_watermark);
        }
        let fp = temp_dir.join(format!("frame-{:04}.png", fi));
        frame
            .save(&fp)
            .map_err(|e| format!("保存视频帧失败: {}", e))?;
        frames.push(fp);
    }

    // 封面：首帧（已含字幕/水印）另存一张 jpg，便于设置视频封面/缩略图
    if vopt.cover {
        let stem = output_path
            .file_stem()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_else(|| "video".to_string());
        let parent = output_path.parent().unwrap_or(std::path::Path::new("."));
        let cover_path = parent.join(format!("{}_封面.jpg", stem));
        if let Ok(img) = image::open(&frames[0]) {
            let _ = img.save(&cover_path);
        }
    }

    // ── ⑤ ffmpeg 编码：帧已按视野渲染成最终尺寸，只做编码 + 变速 ──
    let mut vf = String::from("format=yuv420p");
    if (speed - 1.0).abs() > 0.01 {
        vf.push_str(&format!(",setpts=PTS/{}", speed));
    }
    let total_dur = (tl.total_ms as f64 / 1000.0 / speed).max(0.2);
    let crf = if vopt.hd { "18" } else { "20" };

    // 配音：每步一个 wav，按该步「停留段」起始时间 adelay 后混成一条音轨
    //（读者在该步停留时听旁白；起始时间随 speed 同步缩放）
    let voice = if vopt.voiceover {
        synth_voiceover(&srcs.iter().map(|s| s.title.clone()).collect::<Vec<_>>(), &temp_dir).ok()
    } else {
        None
    };

    let mut cmd = std::process::Command::new(&ffmpeg);
    cmd.arg("-y")
        .arg("-framerate").arg(format!("{}", REEL_FPS))
        .arg("-i").arg(temp_dir.join("frame-%04d.png"));

    if let Some(ref wavs) = voice {
        for w in wavs {
            cmd.arg("-i").arg(w);
        }
        let mut fc = format!("[0:v]{}[v];", vf);
        let mut labels = String::new();
        let mut hold_starts: Vec<i64> = Vec::new();
        for seg in &tl.segments {
            if seg.kind == ReelPhase::Hold {
                hold_starts.push(seg.start_ms);
            }
        }
        for (i, _) in wavs.iter().enumerate() {
            let start_ms = hold_starts
                .get(i)
                .map(|ms| (*ms as f64 / speed).round() as i64)
                .unwrap_or(0);
            fc.push_str(&format!(
                "[{}:a]adelay={}|{}[a{}];",
                i + 1,
                start_ms,
                start_ms,
                i
            ));
            labels.push_str(&format!("[a{}]", i));
        }
        fc.push_str(&format!(
            "{}amix=inputs={}:normalize=0[a]",
            labels,
            wavs.len()
        ));
        // ⚠️ `-filter_complex_script` 在 ffmpeg 7+ 已被移除（本机 9.0.1 报
        // "Unrecognized option"），内联传参全版本通用，且几十步也只 ~2KB。
        cmd.arg("-filter_complex").arg(&fc)
            .arg("-map").arg("[v]")
            .arg("-map").arg("[a]");
    } else {
        cmd.arg("-vf").arg(&vf);
    }

    cmd.arg("-c:v").arg("libx264")
        .arg("-crf").arg(crf)
        .arg("-pix_fmt").arg("yuv420p");
    if voice.is_some() {
        cmd.arg("-c:a").arg("aac").arg("-b:a").arg("128k");
    }
    cmd.arg("-t").arg(format!("{:.3}", total_dur))
        .arg(output_path);

    no_console_window(&mut cmd);
    let status = cmd
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map_err(|e| format!("启动 ffmpeg 失败: {}", e))?;

    if !status.success() {
        let _ = std::fs::remove_dir_all(&temp_dir);
        return Err(format!("ffmpeg 退出码: {}", status.code().unwrap_or(-1)));
    }

    // ── ⑥ GIF 整体通道：gif 开关开启时，对合成好的视频整体转 GIF（复用 video_to_gif）──
    if gif {
        let gif_path = output_path.with_extension("gif");
        video_to_gif(output_path, &gif_path, 1.0)
            .map_err(|e| format!("整体转 GIF 失败: {}", e))?;
    }

    let _ = std::fs::remove_dir_all(&temp_dir);
    Ok(())
}

/// 用 ffmpeg 把视频片段转成 GIF 动图，并按 zoom 缩放宽度（高度同比）。
fn video_to_gif(
    src: &std::path::Path,
    dst: &std::path::Path,
    zoom: f64,
) -> Result<(), String> {
    let ffmpeg = which_ffmpeg().ok_or("未检测到 ffmpeg，无法导出 GIF 动图")?;
    let scale_expr = format!("iw*{}:-1:flags=lanczos", zoom);
    let vf = format!(
        "fps=10,scale={},split[s0][s1];[s0]palettegen=max_colors=128[p];[s1][p]paletteuse=dither=bayer",
        scale_expr
    );
    let mut gif_cmd = std::process::Command::new(ffmpeg);
    no_console_window(&mut gif_cmd);
    let status = gif_cmd
        .arg("-y")
        .arg("-i")
        .arg(src)
        .arg("-vf")
        .arg(&vf)
        .arg("-loop")
        .arg("0")
        .arg(dst)
        .status()
        .map_err(|e| format!("ffmpeg 执行失败: {}", e))?;
    if !status.success() {
        return Err("ffmpeg 转 GIF 失败".to_string());
    }
    Ok(())
}

/// Windows 上 GUI 子系统的 app spawn 控制台程序（ffmpeg / powershell / Edge）时，
/// 系统会新建一个黑色控制台窗口闪出来。统一加 CREATE_NO_WINDOW 让它静默运行。
fn no_console_window(cmd: &mut std::process::Command) -> &mut std::process::Command {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    cmd
}

fn which_ffmpeg() -> Option<std::path::PathBuf> {
    let candidates = ["ffmpeg.exe", "ffmpeg"];
    for c in candidates {
        let mut probe = std::process::Command::new(c);
        probe.arg("-version");
        no_console_window(&mut probe);
        if let Ok(p) = probe.status() {
            if p.success() {
                return Some(std::path::PathBuf::from(c));
            }
        }
    }
    None
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
                block_kind: "step".to_string(),
                block_level: 1,
                note_kind: String::new(),
                title_html: String::new(),
                image_caption: String::new(),
                media_kind: "image".into(),
                media_poster_path: String::new(),
                media_source_path: String::new(),
                url: String::new(),
                before_screenshot_path: String::new(),
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
            &std::collections::HashMap::new(),
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
            &std::collections::HashMap::new(),
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
                block_kind: "step".to_string(),
                block_level: 1,
                note_kind: String::new(),
                title_html: String::new(),
                image_caption: String::new(),
                media_kind: "image".into(),
                media_poster_path: String::new(),
                media_source_path: String::new(),
                url: String::new(),
                before_screenshot_path: String::new(),
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
                block_kind: "step".to_string(),
                block_level: 1,
                note_kind: String::new(),
                title_html: String::new(),
                image_caption: String::new(),
                media_kind: "image".into(),
                media_poster_path: String::new(),
                media_source_path: String::new(),
                url: String::new(),
                before_screenshot_path: String::new(),
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
        let zoom_map: std::collections::HashMap<String, f64> = std::collections::HashMap::new();
        let res = export_skill_bundle(&out, &recording, &steps, &zoom_map, &mut machine);
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

    /// 导出 ⇄ 加载 可往返：export_skill_bundle 写出的包，load_skill_package 能原样读回并校验。
    #[test]
    fn load_skill_package_roundtrips_with_export() {
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
                block_kind: "step".to_string(),
                block_level: 1,
                note_kind: String::new(),
                title_html: String::new(),
                image_caption: String::new(),
                media_kind: "image".into(),
                media_poster_path: String::new(),
                media_source_path: String::new(),
                url: String::new(),
                before_screenshot_path: String::new(),
            },
            ExportStep {
                id: "s2".into(),
                order_index: 1,
                step_number: 2,
                action_type: "type".into(),
                title: "在「单元格」中输入".into(),
                description: "=XLOOKUP(...)".into(),
                tip: String::new(),
                screenshot_path: String::new(),
                x: None,
                y: None,
                crop_w: None,
                crop_h: None,
                is_variable: false,
                param_name: String::new(),
                block_kind: "step".to_string(),
                block_level: 1,
                note_kind: String::new(),
                title_html: String::new(),
                image_caption: String::new(),
                media_kind: "image".into(),
                media_poster_path: String::new(),
                media_source_path: String::new(),
                url: String::new(),
                before_screenshot_path: String::new(),
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
                text: Some("=XLOOKUP(...)".into()),
                ..Default::default()
            },
        );
        let dir = std::env::temp_dir().join("flowio_skill_load_rt");
        let _ = std::fs::create_dir_all(&dir);
        let out = dir.join("SKILL.md");
        let zoom_map: std::collections::HashMap<String, f64> = std::collections::HashMap::new();
        let res = export_skill_bundle(&out, &recording, &steps, &zoom_map, &mut machine);
        assert!(res.is_ok(), "export failed: {:?}", res.err());

        let pkg = read_skill_package(&dir)
            .expect("read_skill_package should succeed on a freshly exported bundle");
        assert_eq!(pkg.name, skill_unique_name(&recording), "name 应与导出时一致");
        assert_eq!(pkg.title, "XLOOKUP step by step");
        assert_eq!(pkg.app, "Excel");
        assert_eq!(pkg.step_count, 2, "steps.json 步数应被读回");
        assert_eq!(pkg.quality.total, 2);
        assert_eq!(pkg.quality.vague, 2);
        assert!(pkg.skill_md.is_file());
        assert!(pkg.screenshots_dir.is_dir());
        assert!(pkg
            .steps_file
            .as_ref()
            .map(|p| p.is_file())
            .unwrap_or(false));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn load_skill_package_rejects_missing_manifest() {
        let dir = std::env::temp_dir().join("flowio_skill_load_nomanifest");
        let _ = std::fs::create_dir_all(&dir);
        let res = read_skill_package(&dir);
        assert!(res.is_err(), "无 skill.json 必须拒绝登记");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn load_skill_package_rejects_bad_schema() {
        let dir = std::env::temp_dir().join("flowio_skill_load_badschema");
        let _ = std::fs::create_dir_all(&dir);
        let _ = std::fs::create_dir_all(dir.join("screenshots"));
        std::fs::write(dir.join("SKILL.md"), "# x\n").unwrap();
        std::fs::write(
            dir.join("skill.json"),
            r#"{"schema":"wrong/v1","name":"n","title":"t","files":{"skill":"SKILL.md","screenshots":"screenshots/","steps":"steps.json"}}"#,
        )
        .unwrap();
        let res = read_skill_package(&dir);
        assert!(res.is_err(), "schema 不符必须拒绝登记");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn load_skill_package_rejects_missing_skill_md() {
        let dir = std::env::temp_dir().join("flowio_skill_load_noskillmd");
        let _ = std::fs::create_dir_all(&dir);
        let _ = std::fs::create_dir_all(dir.join("screenshots"));
        std::fs::write(
            dir.join("skill.json"),
            r#"{"schema":"flowio.skill.manifest/v1","name":"n","title":"t","files":{"skill":"SKILL.md","screenshots":"screenshots/","steps":""}}"#,
        )
        .unwrap();
        let res = read_skill_package(&dir);
        assert!(res.is_err(), "引用的 SKILL.md 缺失必须拒绝登记");
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
                block_kind: "step".to_string(),
                block_level: 1,
                note_kind: String::new(),
                title_html: String::new(),
                image_caption: String::new(),
                media_kind: "image".into(),
                media_poster_path: String::new(),
                media_source_path: String::new(),
                url: String::new(),
                before_screenshot_path: String::new(),
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
                block_kind: "step".to_string(),
                block_level: 1,
                note_kind: String::new(),
                title_html: String::new(),
                image_caption: String::new(),
                media_kind: "image".into(),
                media_poster_path: String::new(),
                media_source_path: String::new(),
                url: String::new(),
                before_screenshot_path: String::new(),
            },
        ];
        let out = std::env::temp_dir().join("flowio_test_no_ss.pdf");
        let zoom_map: std::collections::HashMap<String, f64> = std::collections::HashMap::new();
        let result = export_pdf(&rec, &steps, &out, &zoom_map, "", true);
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
        let empty: std::collections::HashMap<String, f64> = std::collections::HashMap::new();
        let result = export_pdf(&rec, &[], std::path::Path::new("unused.pdf"), &empty, "", true);
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
            block_kind: "step".to_string(),
            block_level: 1,
            note_kind: String::new(),
            title_html: String::new(),
            image_caption: String::new(),
            media_kind: "image".into(),
            media_poster_path: String::new(),
            media_source_path: String::new(),
            url: String::new(),
            before_screenshot_path: String::new(),
        }];
        let zoom_map: std::collections::HashMap<String, f64> = std::collections::HashMap::new();
        let html = gen_html_body(&rec, &steps, &zoom_map, "", true);
        assert!(html.contains("（这一张没有截图）"));
        assert!(html.contains("&lt;script&gt;"));
        assert!(html.contains("&amp;"));
        assert!(html.contains("&lt;b&gt;"));
        assert!(html.contains("&quot;"));
        assert!(html.contains("&#39;"));
        assert!(html.contains("<span class=\"step-num-circle\">1</span>步骤"));
    }

    /* ── 第十六轮：流程摘要块 + 图片描述（预览=导出）────────────── */

    fn summary_block(text: &str) -> ExportStep {
        let mut s = heading_block(1, "", "");
        s.block_kind = "summary".to_string();
        s.description = text.to_string();
        s.title_html = String::new();
        s
    }

    /// 造一个简介块（第十九轮）
    fn intro_block(text: &str) -> ExportStep {
        let mut s = heading_block(1, "", "");
        s.block_kind = "intro".to_string();
        s.description = text.to_string();
        s.title_html = String::new();
        s
    }

    fn rec_1() -> ExportRecording {
        ExportRecording {
            id: "r".into(),
            title: "教程标题".into(),
            app_name: "App".into(),
            total_steps: 1,
            created_at: "2026-09-19".into(),
        }
    }

    #[test]
    fn test_parse_summary_splits_intro_and_bullets() {
        let text = "本流程共 3 个步骤，分为 2 个部分。\n- **上架前准备**：打开控制台、确认版本\n- **发布**：点击发布按钮\n- 结尾检查一遍";
        let (intro, bullets) = parse_summary(text);
        assert_eq!(intro, "本流程共 3 个步骤，分为 2 个部分。");
        assert_eq!(bullets.len(), 3);
        assert_eq!(bullets[0], ("上架前准备".to_string(), "打开控制台、确认版本".to_string()));
        assert_eq!(bullets[1].0, "发布");
        assert_eq!(bullets[2].0, "", "无标签要点也要保留");
        assert_eq!(bullets[2].1, "结尾检查一遍");
    }

    #[test]
    fn test_html_renders_summary_and_caption() {
        let mut st = plain_step(1, "打开控制台", "补充说明", "");
        st.image_caption = "红圈处是版本号".to_string();
        let steps = vec![
            summary_block("本流程演示了上架前的准备与发布。\n- **上架前准备**：确认版本号"),
            st,
        ];
        let html = gen_html_body(&rec_1(), &steps, &std::collections::HashMap::new(), "", true);
        assert!(html.contains("class=\"doc-summary\""), "摘要卡没渲染");
        assert!(html.contains("<b>上架前准备</b>：确认版本号"), "要点没渲染");
        assert!(html.contains("class=\"step-caption\">红圈处是版本号"), "图片描述没渲染");
        assert!(html.contains(".doc-summary{"), "摘要卡样式缺失");
        // 摘要块不占编号：第一步仍是 1 号
        assert!(html.contains("<span class=\"step-num-circle\">1</span>步骤"));
        assert!(!html.contains("step-num-circle\">2<"), "摘要块被当成步骤了");
    }

    /// 第十九轮：简介块四种导出格式都要出，且不占编号
    #[test]
    fn test_intro_block_renders_in_all_formats_without_number() {
        let steps = vec![
            intro_block("这份教程适合第一次上手的人，跟着做完能学会导出。"),
            plain_step(1, "打开控制台", "补充说明", ""),
        ];
        let html = gen_html_body(&rec_1(), &steps, &std::collections::HashMap::new(), "", true);
        assert!(html.contains("class=\"doc-intro\""), "HTML 简介块没渲染: {html}");
        assert!(html.contains("跟着做完能学会导出"), "HTML 简介正文丢了");
        assert!(html.contains(".doc-intro{"), "导出 HTML 缺 .doc-intro 样式");
        // 简介块不占编号：第一步仍是 1 号
        assert!(html.contains("<span class=\"step-num-circle\">1</span>步骤"));
        assert!(!html.contains("step-num-circle\">2<"), "简介块被当成步骤了");

        let md = export_markdown(
            &rec_1(),
            &steps,
            std::path::Path::new("."),
            &std::collections::HashMap::new(),
        );
        assert!(md.contains("跟着做完能学会导出"), "Markdown 简介丢了");
        assert!(!md.contains("**步骤 0"), "简介块不该出「步骤 0」");

        let txt = export_txt(&rec_1(), &steps);
        assert!(txt.contains("跟着做完能学会导出"), "TXT 简介丢了");
        assert!(!txt.contains("步骤 0："), "TXT 里简介块不该被当成步骤");

        // 长图：简介块要有文字横幅（以前摘要块因为没有截图被静默丢掉）
        assert!(doc_block_text(&intro_block("简介正文")) == "简介正文");
        assert!(doc_block_text(&summary_block("总览\n- **要点**：说明")) == "总览\n· 要点：说明");
        assert!(text_banner(600, "   ", None).is_none(), "空内容不该产白条");
        let b = text_banner(600, "一段很长的简介".repeat(20).as_str(), None).unwrap();
        assert!(b.height() > 30, "长文本应当折行成多行横幅");
        // 折行：CJK 按 1 字宽、ASCII 按半字宽
        let lines = wrap_cjk("abcdefghij", 10.0, 50);
        assert!(lines.len() > 1, "半角字符也要折行: {lines:?}");
    }

    #[test]
    fn test_md_and_txt_render_summary_and_caption() {
        let mut st = plain_step(1, "打开控制台", "补充说明", "");
        st.image_caption = "红圈处是版本号".to_string();
        let steps = vec![
            summary_block("本流程演示了上架前的准备与发布。\n- **上架前准备**：确认版本号"),
            st.clone(),
        ];
        let md = export_markdown(
            &rec_1(),
            &steps,
            std::path::Path::new("."),
            &std::collections::HashMap::new(),
        );
        assert!(md.contains("本流程演示了上架前的准备与发布。"));
        assert!(md.contains("- **上架前准备**：确认版本号"));
        assert!(md.contains("*红圈处是版本号*"), "MD 图注缺失");

        let txt = export_txt(&rec_1(), &steps);
        assert!(txt.contains("摘要：本流程演示了上架前的准备与发布。"));
        assert!(txt.contains("  · 上架前准备：确认版本号"));
        assert!(txt.contains("图片描述：红圈处是版本号"));
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
                block_kind: "step".to_string(),
                block_level: 1,
                note_kind: String::new(),
                title_html: String::new(),
                image_caption: String::new(),
                media_kind: "image".into(),
                media_poster_path: String::new(),
                media_source_path: String::new(),
                url: String::new(),
                before_screenshot_path: String::new(),
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
                block_kind: "step".to_string(),
                block_level: 1,
                note_kind: String::new(),
                title_html: String::new(),
                image_caption: String::new(),
                media_kind: "image".into(),
                media_poster_path: String::new(),
                media_source_path: String::new(),
                url: String::new(),
                before_screenshot_path: String::new(),
            },
        ];
        let ss_dir = std::env::temp_dir().join("flowio_test_md_ss");
        let zoom_map: std::collections::HashMap<String, f64> = std::collections::HashMap::new();
        let md = export_markdown(&rec, &steps, &ss_dir, &zoom_map);
        assert!(md.contains("# 标题"));
        // 对齐插件 buildMarkdown：meta 行「> 生成于 …」、步骤头「## 步骤 N」
        assert!(md.contains("> 生成于 2026-08-09 · 共 2 步 · 由 录步 Flowio 生成"));
        assert!(md.contains("## 步骤 1"));
        assert!(md.contains("## 步骤 2"));
        // 截图路径为空 → 不产出任何图片引用（更不留占位文字）
        assert!(!md.contains("![]("));
        assert!(!md.contains("![Screenshot]"));
        assert!(!md.contains("（这一张没有截图）"));
        assert!(!md.contains("Screenshot not found"));
        assert!(md.contains("> **提示：** 提示"));
    }

    #[test]
    fn test_export_markdown_missing_screenshot_file_silently_skipped() {
        // 截图文件缺失（临时截图被清理等）时静默省略引用，不报错、不产出空 ![]()
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
            block_kind: "step".to_string(),
            block_level: 1,
            note_kind: String::new(),
            title_html: String::new(),
            image_caption: String::new(),
            media_kind: "image".into(),
            media_poster_path: String::new(),
            media_source_path: String::new(),
            url: String::new(),
            before_screenshot_path: String::new(),
        }];
        let ss_dir = std::env::temp_dir().join("flowio_test_md_ss2");
        let zoom_map: std::collections::HashMap<String, f64> = std::collections::HashMap::new();
        let md = export_markdown(&rec, &steps, &ss_dir, &zoom_map);
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
            block_kind: "step".to_string(),
            block_level: 1,
            note_kind: String::new(),
            title_html: String::new(),
            image_caption: String::new(),
            media_kind: "image".into(),
            media_poster_path: String::new(),
            media_source_path: String::new(),
            url: String::new(),
            before_screenshot_path: String::new(),
        }];
        let out = std::env::temp_dir().join("flowio_test_export.docx");
        let zoom_map: std::collections::HashMap<String, f64> = std::collections::HashMap::new();
        export_docx(&rec, &steps, &out, &zoom_map, "", true, true).unwrap();
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
            block_kind: "step".to_string(),
            block_level: 1,
            note_kind: String::new(),
            title_html: String::new(),
            image_caption: String::new(),
            media_kind: "image".into(),
            media_poster_path: String::new(),
            media_source_path: String::new(),
            url: String::new(),
            before_screenshot_path: String::new(),
        }];
        let zoom_map: std::collections::HashMap<String, f64> = std::collections::HashMap::new();
        let html = gen_html_body(&rec, &steps, &zoom_map, "", true);
        assert!(html.contains("data:image/png;base64,"));
        assert!(html.contains("<span class=\"step-num-circle\">1</span>步骤"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_export_markdown_embeds_screenshot_without_extra_files() {
        // 与插件 buildMarkdown 对齐：截图以 base64 data URL 内嵌，不再往磁盘额外写截图文件
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
            block_kind: "step".to_string(),
            block_level: 1,
            note_kind: String::new(),
            title_html: String::new(),
            image_caption: String::new(),
            media_kind: "image".into(),
            media_poster_path: String::new(),
            media_source_path: String::new(),
            url: String::new(),
            before_screenshot_path: String::new(),
        }];
        let zoom_map: std::collections::HashMap<String, f64> = std::collections::HashMap::new();
        let md = export_markdown(&rec, &steps, &dir, &zoom_map);
        // 核心新行为：截图按 data URL 内嵌
        assert!(md.contains("![](data:image/"));
        assert!(md.contains(";base64,"));
        // 内嵌而非引用文件：不出现文件名，也不落额外截图文件
        assert!(!md.contains("![Screenshot]"));
        assert!(!md.contains("step_1.png"));
        // 不应创建 step_1.png 文件
        assert!(!dir.join("step_1.png").exists());
        // 但应有正常的文本内容
        assert!(md.contains("# 标题"));
        assert!(md.contains("## 步骤 1"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_export_csv_structure_and_escaping() {
        // 列结构与插件 buildCsv 对齐；文档块不占编号；含逗号/引号/换行的单元格整体加引号
        let rec = ExportRecording {
            id: "r".into(),
            title: "标题".into(),
            app_name: "App".into(),
            total_steps: 2,
            created_at: "2026-08-09".into(),
        };
        let mk = |id: &str, order: i32, num: i32, action: &str, title: &str, desc: &str, kind: &str, url: &str| ExportStep {
            id: id.into(),
            order_index: order,
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
            is_variable: false,
            param_name: String::new(),
            block_kind: kind.to_string(),
            block_level: 1,
            note_kind: String::new(),
            title_html: String::new(),
            image_caption: String::new(),
            media_kind: "image".into(),
            media_poster_path: String::new(),
            media_source_path: String::new(),
            url: url.into(),
            before_screenshot_path: String::new(),
        };
        let steps = vec![
            mk("s1", 0, 1, "", "准备阶段", "", "heading", ""),
            mk(
                "s2",
                1,
                2,
                "click",
                "点击\"开始\"",
                "含,逗号\n与换行",
                "step",
                "https://flowio.cn/g/abc?x=1,2",
            ),
        ];
        let csv = export_csv(&rec, &steps);
        let mut lines = csv.split("\r\n");
        assert_eq!(lines.next().unwrap(), "序号,类型,标题,说明,URL,敏感");
        // 文档块：序号全编号（对齐插件 i+1），类型列给中文块名；无来源网址 → URL 列空
        assert_eq!(lines.next().unwrap(), "1,标题,准备阶段,,,否");
        // 普通步骤：序号继续递增；引号翻倍 + 整格加引号；URL 带值（含逗号 → 整格加引号）
        assert_eq!(
            lines.next().unwrap(),
            "2,click,\"点击\"\"开始\"\"\",\"含,逗号\n与换行\",\"https://flowio.cn/g/abc?x=1,2\",否"
        );
    }

    #[test]
    fn test_export_md_and_txt_include_step_url() {
        // 对齐插件：Markdown 出 `🔗 <url>`，TXT 出「链接：<url>」；空网址两处都不出
        let rec = ExportRecording {
            id: "r".into(),
            title: "标题".into(),
            app_name: "App".into(),
            total_steps: 2,
            created_at: "2026-08-09".into(),
        };
        let mk = |order: i32, num: i32, title: &str, url: &str| ExportStep {
            id: format!("s{}", order),
            order_index: order,
            step_number: num,
            action_type: "click".into(),
            title: title.into(),
            description: String::new(),
            tip: String::new(),
            screenshot_path: String::new(),
            x: None,
            y: None,
            crop_w: None,
            crop_h: None,
            is_variable: false,
            param_name: String::new(),
            block_kind: "step".to_string(),
            block_level: 1,
            note_kind: String::new(),
            title_html: String::new(),
            image_caption: String::new(),
            media_kind: "image".into(),
            media_poster_path: String::new(),
            media_source_path: String::new(),
            url: url.into(),
            before_screenshot_path: String::new(),
        };
        let steps = vec![mk(0, 1, "有网址", "https://flowio.cn/g/abc"), mk(1, 2, "无网址", "")];
        let ss_dir = std::env::temp_dir().join("flowio_test_md_url");
        let zoom_map: std::collections::HashMap<String, f64> = std::collections::HashMap::new();
        let md = export_markdown(&rec, &steps, &ss_dir, &zoom_map);
        assert!(md.contains("🔗 https://flowio.cn/g/abc"));
        assert_eq!(md.matches("🔗").count(), 1);
        let txt = export_txt(&rec, &steps);
        assert!(txt.contains("链接：https://flowio.cn/g/abc"));
        assert_eq!(txt.matches("链接：").count(), 1);
        let html = export_html(&rec, &steps, &zoom_map, "", true);
        assert!(html.contains("class=\"step-url\">🔗 https://flowio.cn/g/abc"));
        assert_eq!(html.matches("class=\"step-url\"").count(), 1);
    }

    // ── 速览视频 reel 纯函数（对齐插件 reel.ts 几何/时间轴不变式）──────

    #[test]
    fn reel_fit_view_basic_invariants() {
        // 无焦点 → 整图（16:9 画布，wLimit=1, h=9/16）
        let v = fit_reel_view(None, 16.0 / 9.0, 0.3, 3.0);
        assert!((v.w - 1.0).abs() < 1e-9);
        assert!((v.h - 9.0 / 16.0).abs() < 1e-9);
        assert_eq!(v.x, 0.0);
        assert_eq!(v.y, 0.0);
        // 焦点框在中心，pad 0.3 外扩 → 视野套 16:9（取较大边），焦点整块在画面内
        let b = ReelRectF { x: 0.4, y: 0.4, w: 0.2, h: 0.2 };
        let v = fit_reel_view(Some(b), 16.0 / 9.0, 0.3, 3.0);
        assert!((v.w / v.h - 16.0 / 9.0).abs() < 1e-9);
        // 视野包住外扩后的焦点框
        let pw = 0.2 * 1.6;
        let ph = 0.2 * 1.6;
        assert!(v.w >= pw - 1e-9 && v.h >= ph - 1e-9);
        assert!(v.x <= 0.4 - 1e-9 && v.y <= 0.4 - 1e-9);
        assert!(v.x + v.w >= 0.6 + 1e-9 && v.y + v.h >= 0.6 + 1e-9);
        // 视野不越界
        assert!(v.x >= 0.0 && v.y >= 0.0 && v.x + v.w <= 1.0 + 1e-9 && v.y + v.h <= 1.0 + 1e-9);
        // 极大框 → 顶满整图
        let big = ReelRectF { x: 0.0, y: 0.0, w: 0.99, h: 0.99 };
        let v = fit_reel_view(Some(big), 16.0 / 9.0, 0.3, 3.0);
        assert!((v.w - 1.0).abs() < 1e-9);
        // 极小框 → 不小于 1/maxZoom（防推糊）
        let tiny = ReelRectF { x: 0.499, y: 0.499, w: 0.002, h: 0.002 };
        let v = fit_reel_view(Some(tiny), 16.0 / 9.0, 0.3, 3.0);
        assert!(v.w >= 1.0 / 3.0 - 1e-9);
    }

    #[test]
    fn reel_build_timeline_first_segment_from_full_and_total_length() {
        let boxes = vec![
            None,
            Some(ReelRectF { x: 0.4, y: 0.4, w: 0.2, h: 0.2 }),
            None,
        ];
        let tl = build_reel_timeline(&boxes, 16.0 / 9.0, 0.3, 3.0, 700, 1800).unwrap();
        // 段数 = n*2 = 6（move+hold 交替）
        assert_eq!(tl.segments.len(), 6);
        // 第 0 段是 move，from=整图
        assert_eq!(tl.segments[0].kind, ReelPhase::Move);
        assert_eq!(tl.segments[0].step_index, 0);
        assert!((tl.segments[0].from.w - 1.0).abs() < 1e-9);
        // 时间轴连续且总长 = n*(move+hold)
        let mut prev_end = 0;
        for seg in &tl.segments {
            assert_eq!(seg.start_ms, prev_end);
            assert!(seg.end_ms > seg.start_ms);
            prev_end = seg.end_ms;
        }
        assert_eq!(tl.total_ms, 3 * (700 + 1800));
    }

    #[test]
    fn reel_sample_hold_is_static_and_move_endpoints_exact() {
        let boxes = vec![
            None,
            Some(ReelRectF { x: 0.5, y: 0.5, w: 0.2, h: 0.2 }),
        ];
        let tl = build_reel_timeline(&boxes, 16.0 / 9.0, 0.3, 3.0, 700, 1800).unwrap();
        // hold 段中任意时刻 = 终值（完全静止）
        let (_, kind, e, v) = sample_reel_at(&tl, 700 + 900);
        assert_eq!(kind, ReelPhase::Hold);
        assert_eq!(e, 1.0);
        assert_eq!(v, tl.views[0]);
        // move 段端点精确 0/1
        let (_, _, e0, v0) = sample_reel_at(&tl, 0);
        assert_eq!(e0, 0.0);
        assert!((v0.w - tl.segments[0].from.w).abs() < 1e-9);
        // t 超界夹到末尾
        let (_, kind2, e2, v2) = sample_reel_at(&tl, tl.total_ms + 1000);
        assert_eq!(kind2, ReelPhase::Hold);
        assert_eq!(e2, 1.0);
        assert_eq!(v2, tl.views[1]);
    }

    #[test]
    fn reel_adapt_hold_compresses_toward_target_with_floor() {
        // 步骤多 → 压到下限 900ms
        assert_eq!(reel_adapt_hold_ms(60, 700, 1800), 900);
        // 步骤少 → 保持基准 1800ms
        assert_eq!(reel_adapt_hold_ms(5, 700, 1800), 1800);
        // 20 步：allowed = floor(75000/20)-700 = 3050 → 仍 > 1800 → 1800
        assert_eq!(reel_adapt_hold_ms(20, 700, 1800), 1800);
        // 40 步：floor(75000/40)-700 = 1175 → 1175
        assert_eq!(reel_adapt_hold_ms(40, 700, 1800), 1175);
        // 极端：move 大于目标单步预算 → 仍守住下限
        assert_eq!(reel_adapt_hold_ms(200, 1000, 1800), 900);
    }

    #[test]
    fn reel_frame_count_at_least_two() {
        assert_eq!(reel_frame_count(0, 24.0), 2);
        assert_eq!(reel_frame_count(1000, 24.0), 24);
        assert_eq!(reel_frame_count(500, 24.0), 12);
    }

    /// 品牌水印两档语义：缺省 = 显示水印（对齐插件 watermark="on" 默认档）；
    /// 同时盯住前端 camelCase 字段名，防止 removeWatermark / brandWatermark 失配后静默失效。
    #[test]
    fn test_video_options_watermark_semantics() {
        let d = parse_video_options(None);
        assert!(!d.remove_watermark, "缺省应保留水印");
        assert!(d.brand_watermark.is_empty(), "缺省用默认品牌名");

        let opts = serde_json::json!({
            "removeWatermark": true,
            "brandWatermark": "我的品牌",
        });
        let v = parse_video_options(Some(&opts));
        assert!(v.remove_watermark, "勾选后应去除水印");
        assert_eq!(v.brand_watermark, "我的品牌");
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
            block_kind: "step".to_string(),
            block_level: 1,
            note_kind: String::new(),
            title_html: String::new(),
            image_caption: String::new(),
            media_kind: "image".into(),
            media_poster_path: String::new(),
            media_source_path: String::new(),
            url: String::new(),
            before_screenshot_path: String::new(),
        }];
        let out = dir.join("out.docx");
        let zoom_map: std::collections::HashMap<String, f64> = std::collections::HashMap::new();
        export_docx(&rec, &steps, &out, &zoom_map, "", true, true).unwrap();
        assert!(out.exists());
        let size = std::fs::metadata(&out).unwrap().len();
        assert!(size > 100);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_export_docx_qr_footer_injects_png() {
        use std::io::Read;
        let rec = ExportRecording {
            id: "r".into(),
            title: "标题".into(),
            app_name: "App".into(),
            total_steps: 1,
            created_at: "2026-08-09".into(),
        };
        let dir = std::env::temp_dir().join("flowio_test_docx_qr");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let png = write_tmp_png(&dir, "s.png");
        let steps = vec![ExportStep {
            id: "qr-step".into(),
            order_index: 0,
            step_number: 1,
            title: "步骤".into(),
            action_type: "click".into(),
            description: "".into(),
            tip: "".into(),
            screenshot_path: png.to_string_lossy().to_string(),
            x: None,
            y: None,
            crop_w: None,
            crop_h: None,
            is_variable: false,
            param_name: String::new(),
            block_kind: "step".to_string(),
            block_level: 1,
            note_kind: String::new(),
            title_html: String::new(),
            image_caption: String::new(),
            media_kind: "image".into(),
            media_poster_path: String::new(),
            media_source_path: String::new(),
            url: String::new(),
            before_screenshot_path: String::new(),
        }];
        let out = dir.join("qr.docx");
        let zoom_map: std::collections::HashMap<String, f64> = std::collections::HashMap::new();
        export_docx(&rec, &steps, &out, &zoom_map, "https://flowio.cn/g/demo", true, true).unwrap();
        assert!(out.exists());
        // 解包 zip：页脚 part 存在且引用了媒体图；正文 sectPr 挂 footerReference
        let zip = std::fs::read(&out).unwrap();
        let mut archive = zip::ZipArchive::new(std::io::Cursor::new(zip)).unwrap();
        let names: Vec<String> = archive.file_names().map(|s| s.to_string()).collect();
        assert!(
            names.iter().any(|n| n == "word/footer1.xml"),
            "footer part missing: {:?}",
            names
        );
        let media_count = names
            .iter()
            .filter(|n| n.starts_with("word/media/") && n.ends_with(".png"))
            .count();
        assert!(media_count >= 1, "footer qr media missing: {:?}", names);
        let mut footer_xml = String::new();
        archive
            .by_name("word/footer1.xml")
            .unwrap()
            .read_to_string(&mut footer_xml)
            .unwrap();
        assert!(
            footer_xml.contains("r:embed") && footer_xml.contains("录步 Flowio"),
            "footer should carry image ref and attribution: {}",
            &footer_xml[..footer_xml.len().min(300)]
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_export_video_copies_single_raw_video_without_ffmpeg() {
        // 对齐插件「录屏视频=原始录像」：单视频步骤 → 直接拷贝原片交付，不依赖 ffmpeg
        let rec = ExportRecording {
            id: "r".into(),
            title: "标题".into(),
            app_name: "App".into(),
            total_steps: 1,
            created_at: "2026-08-09".into(),
        };
        let dir = std::env::temp_dir().join(format!("flowio_test_video_copy_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let src = dir.join("raw.webm");
        std::fs::write(&src, b"fake-webm-bytes-12345").unwrap();
        let out = dir.join("out.mp4");
        let steps = vec![ExportStep {
            id: "v".into(),
            order_index: 0,
            step_number: 1,
            title: "录屏".into(),
            action_type: "record".into(),
            description: "".into(),
            tip: "".into(),
            screenshot_path: "".into(),
            x: None,
            y: None,
            crop_w: None,
            crop_h: None,
            is_variable: false,
            param_name: String::new(),
            block_kind: "step".to_string(),
            block_level: 1,
            note_kind: String::new(),
            title_html: String::new(),
            image_caption: String::new(),
            media_kind: "video".into(),
            media_poster_path: String::new(),
            media_source_path: src.to_string_lossy().to_string(),
            url: String::new(),
            before_screenshot_path: String::new(),
        }];
        let zoom_map: std::collections::HashMap<String, f64> = std::collections::HashMap::new();
        export_video(&rec, &steps, &out, &zoom_map, false, false, &VideoOptions::default(), false).unwrap();
        assert_eq!(std::fs::read(&out).unwrap(), b"fake-webm-bytes-12345");
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
            block_kind: "step".to_string(),
            block_level: 1,
            note_kind: String::new(),
            title_html: String::new(),
            image_caption: String::new(),
            media_kind: "image".into(),
            media_poster_path: String::new(),
            media_source_path: String::new(),
            url: String::new(),
            before_screenshot_path: String::new(),
        }];
        let ss_dir = std::env::temp_dir().join("flowio_test_md4");
        let zoom_map: std::collections::HashMap<String, f64> = std::collections::HashMap::new();
        let md = export_markdown(&rec, &steps, &ss_dir, &zoom_map);
        // 特殊字符原样输出（Markdown 语法由查看器渲染），不转义也不崩溃
        assert!(md.contains("**bold** `code`"));
        // 对齐插件 buildMarkdown：步骤头只含 title，action_type 不再出现
        assert!(!md.contains("click|a"));
        assert!(md.contains("> **提示：** > quote"));
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
            block_kind: "step".to_string(),
            block_level: 1,
            note_kind: String::new(),
            title_html: String::new(),
            image_caption: String::new(),
            media_kind: "image".into(),
            media_poster_path: String::new(),
            media_source_path: String::new(),
            url: String::new(),
            before_screenshot_path: String::new(),
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
            build_step_header_html(1, "click", "打开窗口", false),
            "<span class=\"step-num-circle\">1</span>步骤<span class=\"step-type\">click</span>"
        );
        // 特殊字符转义
        assert_eq!(
            build_step_header_html(2, "<type>&", "标题 <b>", false),
            "<span class=\"step-num-circle\">2</span>步骤<span class=\"step-type\">&lt;type&gt;&amp;</span>"
        );
        // 空字段
        assert_eq!(
            build_step_header_html(3, "", "", false),
            "<span class=\"step-num-circle\">3</span>步骤"
        );
        // 重点图徽章（对齐插件 `.key-badge`）：追加在类型徽标之后
        assert_eq!(
            build_step_header_html(4, "click", "重点步", true),
            "<span class=\"step-num-circle\">4</span>步骤<span class=\"step-type\">click</span><span class=\"key-badge\">重点</span>"
        );
        assert_eq!(
            build_step_header_html(5, "", "", true),
            "<span class=\"step-num-circle\">5</span>步骤<span class=\"key-badge\">重点</span>"
        );
        // 标题不再进这一行（第十四轮：与预览一致，标题单独成行）
        assert_eq!(
            build_step_title_html("标题 <b>"),
            "      <h3 class=\"step-title\">标题 &lt;b&gt;</h3>\n"
        );
        assert_eq!(build_step_title_html("   "), "");
        // step_number 边界
        assert!(build_step_header_html(0, "a", "b", false)
            .starts_with("<span class=\"step-num-circle\">0</span>步骤"));
        assert!(build_step_header_html(-1, "a", "b", false)
            .starts_with("<span class=\"step-num-circle\">-1</span>步骤"));
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
            // 中文全角冒号后不加空格（与预览 .sop-preview-tip 的写法一致）
            build_step_tip_html("一个提示"),
            "      <div class=\"step-tip\"><strong>提示：</strong>一个提示</div>\n"
        );
        assert_eq!(
            build_step_tip_html("<script>alert(1)</script>"),
            "      <div class=\"step-tip\"><strong>提示：</strong>&lt;script&gt;alert(1)&lt;/script&gt;</div>\n"
        );
    }

    #[test]
    fn test_build_screenshot_placeholder_and_img() {
        assert_eq!(
            build_screenshot_placeholder_html(),
            "<div class=\"screenshot-placeholder\">（这一张没有截图）</div>"
        );
        // img 标签
        assert_eq!(
            build_screenshot_img_html("image/png", "abc123", 1),
            "<img class=\"screenshot\" src=\"data:image/png;base64,abc123\" alt=\"步骤 1 截图\" />"
        );
        // 空 base64 / 空 mime
        assert_eq!(
            build_screenshot_img_html("image/jpeg", "", 0),
            "<img class=\"screenshot\" src=\"data:image/jpeg;base64,\" alt=\"步骤 0 截图\" />"
        );
        assert_eq!(
            build_screenshot_img_html("", "data", -2),
            "<img class=\"screenshot\" src=\"data:;base64,data\" alt=\"步骤 -2 截图\" />"
        );
    }

    #[test]
    fn test_build_toc_html_structure() {
        // 单步不生成目录（与插件 `tocOn && steps.length > 1` 同口径）
        assert_eq!(build_toc_html(&[plain_step(1, "打开", "d", "")]), "");
        // 多步：普通步骤补零两位序号，标题块为分组标签（不印序号）
        let steps = vec![
            heading_block(1, "第一章", ""),
            plain_step(1, "打开设置", "d", ""),
            plain_step(2, "保存", "d", ""),
        ];
        let toc = build_toc_html(&steps);
        assert!(toc.contains("<span class=\"toc-title\">目录</span>"));
        assert!(toc.contains("<span class=\"toc-meta\">共 3 步</span>"));
        assert!(toc.contains(
            "<li class=\"toc-group\"><a href=\"#step-1\"><span class=\"toc-text\">第一章</span></a></li>"
        ));
        assert!(toc.contains(
            "<li><a href=\"#step-2\"><span class=\"toc-no\">02</span><span class=\"toc-text\">打开设置</span></a></li>"
        ));
        assert!(toc.contains(
            "<li><a href=\"#step-3\"><span class=\"toc-no\">03</span><span class=\"toc-text\">保存</span></a></li>"
        ));
        // 空标题回退「步骤 N」；标题转义
        let toc2 = build_toc_html(&[
            plain_step(1, "   ", "d", ""),
            plain_step(2, "<b>&", "d", ""),
        ]);
        assert!(toc2.contains(">步骤 1</span>"));
        assert!(toc2.contains("&lt;b&gt;&amp;"));
    }

    #[test]
    fn test_key_shot_width_matches_plugin() {
        // 对齐插件 keyFrame.ts::keyShotWidth：125% → 向两侧各探出 12.5%
        assert_eq!(key_shot_width(100.0, 125.0), (125.0, 12.5));
        // 150% → 25%
        assert_eq!(key_shot_width(100.0, 150.0), (150.0, 25.0));
        // 不放大 → 无溢出
        assert_eq!(key_shot_width(100.0, 100.0), (100.0, 0.0));
        // 缩小档不产生负溢出（桌面端无缩小档，仍按插件口径兜底）
        assert_eq!(key_shot_width(100.0, 62.5), (62.5, 0.0));
        // fmt_pct 去尾零
        assert_eq!(fmt_pct(125.0), "125");
        assert_eq!(fmt_pct(12.5), "12.5");
        assert_eq!(fmt_pct(100.0), "100");
    }

    #[test]
    fn test_build_html_document_structure() {
        let doc = build_html_document("标题", 2, "2026-08-20", "<card/>", "", true);
        assert!(doc.starts_with("<!DOCTYPE html>"));
        assert!(doc.contains("<title>标题</title>"));
        assert!(doc.contains("<h1>标题</h1>"));
        assert!(doc.contains("<span>共 2 个步骤</span>"));
        assert!(doc.contains("<span>2026-08-20</span>"));
        assert!(doc.contains("<card/>"));
        assert!(doc.ends_with("</html>"));
        // title 特殊字符转义
        let doc2 = build_html_document("<script>", 0, "", "", "", true);
        assert!(doc2.contains("&lt;script&gt;"));
        // 空 step_cards 合法
        let doc3 = build_html_document("t", 0, "d", "", "", true);
        assert!(doc3.contains("<div class=\"container\">\n\n</div>"));
    }

    #[test]
    fn test_build_md_step_header() {
        // 对齐插件 buildMarkdown：`## 步骤 ${i+1}：${title}`（H1 留给教程名）
        assert_eq!(build_md_step_header(1, "click", "第一步"), "## 步骤 1：第一步");
        // Markdown 特殊字符原样（不转义，由渲染器处理）
        assert_eq!(
            build_md_step_header(2, "a|b`c", "**bold**"),
            "## 步骤 2：**bold**"
        );
        // 无标题：退化成「## 步骤 N」（插件 title 为空时同款）
        assert_eq!(build_md_step_header(0, "", ""), "## 步骤 0");
    }

    #[test]
    fn test_build_md_desc_tip_blocks() {
        assert_eq!(build_md_desc_block(""), "");
        assert_eq!(build_md_desc_block("描述"), "描述\n\n");
        assert_eq!(build_md_desc_block("line1\nline2"), "line1  \nline2\n\n");
        assert_eq!(build_md_tip_block(""), "");
        assert_eq!(build_md_tip_block("提示"), "> **提示：** 提示\n");
        // tip 含 > 等特殊字符原样
        assert_eq!(build_md_tip_block("> quote"), "> **提示：** > quote\n");
    }

    #[test]
    fn test_build_md_document_structure() {
        let md = build_md_document("标题", "App", 1, "2026-08-20", "## Step 1");
        assert!(md.starts_with("# 标题\n\n"));
        // 对齐插件 buildMarkdown：meta 为「> 生成于 {date} · 共 N 步 · 由 录步 Flowio 生成」
        assert!(md.contains("> 生成于 2026-08-20 · 共 1 步 · 由 录步 Flowio 生成"));
        assert!(md.contains("## Step 1\n\n---"));
        // 空 step_list
        let md2 = build_md_document("t", "a", 0, "d", "");
        assert!(md2.contains("---\n\n"));
    }

    #[test]
    fn test_build_docx_step_header_and_meta() {
        assert_eq!(build_docx_step_header(1, "click", "标题"), "步骤 1 · click · 标题");
        assert_eq!(build_docx_step_header(5, "", ""), "步骤 5 · ");
        assert_eq!(
            build_docx_meta_text("App", 3, "2026-08-20"),
            "应用：App  ·  共 3 个步骤  ·  2026-08-20"
        );
        assert_eq!(build_docx_meta_text("", 0, ""), "应用：  ·  共 0 个步骤  ·  ");
    }

    #[test]
    fn test_docx_image_emu_dims_always_full_width() {
        // docx 对齐插件 buildWordHtml：恒 672px 铺满版心（zoom=1），小图也放大，仅保比例
        let full = (672 * 9525) as u32;
        assert_eq!(docx_image_emu_dims(Some((100, 50))), (full, full / 2));
        assert_eq!(docx_image_emu_dims(Some((672, 672))), (full, full));
        assert_eq!(docx_image_emu_dims(Some((1200, 600))), (full, full / 2));
        assert_eq!(docx_image_emu_dims(Some((1200, 900))), (full, full * 3 / 4));
        assert_eq!(docx_image_emu_dims(Some((2000, 4000))), (full, full * 2));
        // 读不到尺寸 / 宽高为 0：兜底 4:3
        assert_eq!(docx_image_emu_dims(None), (full, full * 3 / 4));
        assert_eq!(docx_image_emu_dims(Some((0, 0))), (full, full * 3 / 4));
    }

    #[test]
    fn test_compute_image_emu_dims() {
        // 小图原尺寸缩放（9525 EMU/px）
        assert_eq!(compute_image_emu_dims(Some((100, 50)), 5_715_000), (952_500, 476_250));
        // 恰好 600px 不缩放（600 < 672 上限）
        assert_eq!(compute_image_emu_dims(Some((600, 300)), 5_715_000), (5_715_000, 2_857_500));
        // 超宽图等比缩放到 672px 宽（MAX_IMG_PX 已从 600 对齐插件 672）
        assert_eq!(compute_image_emu_dims(Some((1200, 600)), 5_715_000), (6_400_800, 3_200_400));
        // 超宽非整数比例（1200x900 → 672x504）
        assert_eq!(compute_image_emu_dims(Some((1200, 900)), 5_715_000), (6_400_800, 4_800_600));
        // 0x0 图：宽高为 0 在 Word 里渲染异常，退回兜底比例（与「读不到尺寸」一致）
        assert_eq!(compute_image_emu_dims(Some((0, 0)), 5_715_000), (5_715_000, 4_286_250));
        assert_eq!(compute_image_emu_dims(Some((0, 100)), 5_715_000), (5_715_000, 4_286_250));
        assert_eq!(compute_image_emu_dims(Some((100, 0)), 5_715_000), (5_715_000, 4_286_250));
        // 无尺寸 → 回退占位（max_width_emu x 3/4）
        assert_eq!(compute_image_emu_dims(None, 5_715_000), (5_715_000, 4_286_250));
        // 自定义 fallback
        assert_eq!(compute_image_emu_dims(None, 100_000), (100_000, 75_000));
        // 超高图（宽 2000 高 4000 → 672x1344）
        assert_eq!(compute_image_emu_dims(Some((2000, 4000)), 5_715_000), (6_400_800, 12_801_600));
    }

    /* ── 第十四轮：文档块 / 富文本标题 / 预览=导出 ───────────────── */

    /// 造一个大标题块
    fn heading_block(level: i32, title: &str, html: &str) -> ExportStep {
        ExportStep {
            id: format!("h-{}", title),
            order_index: 0,
            step_number: 0,
            action_type: String::new(),
            title: title.to_string(),
            description: String::new(),
            tip: String::new(),
            screenshot_path: String::new(),
            x: None,
            y: None,
            crop_w: None,
            crop_h: None,
            is_variable: false,
            param_name: String::new(),
            block_kind: "heading".to_string(),
            block_level: level,
            note_kind: String::new(),
            title_html: html.to_string(),
            image_caption: String::new(),
            media_kind: "image".into(),
            media_poster_path: String::new(),
            media_source_path: String::new(),
            url: String::new(),
            before_screenshot_path: String::new(),
        }
    }

    /// 造一个真步骤（可带说明类型）
    fn plain_step(num: i32, title: &str, desc: &str, note: &str) -> ExportStep {
        ExportStep {
            id: format!("s-{}", num),
            order_index: num - 1,
            step_number: num,
            action_type: "click".to_string(),
            title: title.to_string(),
            description: desc.to_string(),
            tip: String::new(),
            screenshot_path: String::new(),
            x: None,
            y: None,
            crop_w: None,
            crop_h: None,
            is_variable: false,
            param_name: String::new(),
            block_kind: "step".to_string(),
            block_level: 1,
            note_kind: note.to_string(),
            title_html: String::new(),
            image_caption: String::new(),
            media_kind: "image".into(),
            media_poster_path: String::new(),
            media_source_path: String::new(),
            url: String::new(),
            before_screenshot_path: String::new(),
        }
    }

    #[test]
    fn test_is_heading_block_and_level_clamp() {
        assert!(is_heading_block(&heading_block(1, "x", "")));
        // 大小写不敏感：手改库写成 Heading 也算
        let mut odd = heading_block(1, "x", "");
        odd.block_kind = "Heading".to_string();
        assert!(is_heading_block(&odd));
        assert!(!is_heading_block(&plain_step(1, "x", "", "")));
        // 真步骤的 block_kind 是 'step'
        assert_eq!(plain_step(1, "x", "", "").block_kind, "step");

        // 层级夹到 1..4（越界不 panic，也不写出 h9）
        assert_eq!(heading_level(&heading_block(0, "x", "")), 1);
        assert_eq!(heading_level(&heading_block(-3, "x", "")), 1);
        assert_eq!(heading_level(&heading_block(4, "x", "")), 4);
        assert_eq!(heading_level(&heading_block(99, "x", "")), 4);
    }

    #[test]
    fn test_real_step_count_skips_headings() {
        let steps = vec![
            plain_step(1, "a", "", ""),
            heading_block(1, "小节", ""),
            plain_step(2, "b", "", ""),
            plain_step(3, "c", "", ""),
            heading_block(2, "小节 2", ""),
        ];
        assert_eq!(real_step_count(&steps), 3);
        assert_eq!(real_step_count(&[]), 0);
        assert_eq!(real_step_count(&[heading_block(1, "只有标题", "")]), 0);
    }

    #[test]
    fn test_sanitize_inline_html_whitelist() {
        // 通过：白名单标签
        assert_eq!(
            sanitize_inline_html("<b>粗</b><i>斜</i><u>下</u><s>删</s>"),
            Some("<b>粗</b><i>斜</i><u>下</u><s>删</s>".to_string())
        );
        // 通过：hex 颜色 / 高亮 / 其它三个属性
        assert_eq!(
            sanitize_inline_html("<span style=\"color:#c0392b\">红</span>"),
            Some("<span style=\"color:#c0392b\">红</span>".to_string())
        );
        assert_eq!(
            sanitize_inline_html("<span style=\"background-color:#faeeda\">亮</span>"),
            Some("<span style=\"background-color:#faeeda\">亮</span>".to_string())
        );
        assert_eq!(
            sanitize_inline_html(
                "<span style=\"font-style:italic;font-weight:bold;text-decoration:underline\">x</span>"
            ),
            Some(
                "<span style=\"font-style:italic;font-weight:bold;text-decoration:underline\">x</span>"
                    .to_string()
            )
        );
        // 拒绝：非白名单标签 / 内联事件 / 未转义尖括号
        assert_eq!(sanitize_inline_html("<script>alert(1)</script>"), None);
        assert_eq!(sanitize_inline_html("<b onclick=\"x\">t</b>"), None);
        assert_eq!(sanitize_inline_html("a > b"), None);
        assert_eq!(sanitize_inline_html("<a href=\"#\">t</a>"), None);
        // 拒绝：颜色不是 hex（rgb() 必须先在前端转成 hex 才落库）
        assert_eq!(sanitize_inline_html("<span style=\"color:red\">x</span>"), None);
        assert_eq!(
            sanitize_inline_html("<span style=\"color:rgb(1,2,3)\">x</span>"),
            None
        );
        // 空串 = 无格式，直接过
        assert_eq!(sanitize_inline_html(""), Some(String::new()));
    }

    #[test]
    fn test_rich_inline_html_falls_back_to_plain() {
        // 没有富文本 → 纯文本（转义）
        assert_eq!(rich_inline_html("纯文本", ""), "纯文本");
        assert_eq!(rich_inline_html("a<b", ""), "a&lt;b");
        // 富文本不合法 → 整体退回纯文本，而不是丢掉格式继续拼
        assert_eq!(rich_inline_html("纯文本", "<script>x</script>"), "纯文本");
        assert_eq!(rich_inline_html("a<b", "<span style=\"color:red\">x</span>"), "a&lt;b");
        // 合法富文本 → 用富文本
        assert_eq!(rich_inline_html("忽略我", "<b>用它</b>"), "<b>用它</b>");
    }

    #[test]
    fn test_build_heading_html_levels_and_escape() {
        assert_eq!(
            build_heading_html(&heading_block(1, "开始", ""), ""),
            "    <h1 class=\"doc-heading h1\">开始</h1>\n"
        );
        assert_eq!(
            build_heading_html(&heading_block(4, "收尾", "<s>收尾</s>"), ""),
            "    <h4 class=\"doc-heading h4\"><s>收尾</s></h4>\n"
        );
        // 越界层级夹到 1..4
        assert!(build_heading_html(&heading_block(9, "x", ""), "")
            .starts_with("    <h4 class=\"doc-heading h4\">"));
        assert!(build_heading_html(&heading_block(0, "x", ""), "")
            .starts_with("    <h1 class=\"doc-heading h1\">"));
        // 纯文本里的尖括号必须转义
        assert_eq!(
            build_heading_html(&heading_block(2, "a<b", ""), ""),
            "    <h2 class=\"doc-heading h2\">a&lt;b</h2>\n"
        );
    }

    #[test]
    fn test_note_palette_matches_preview() {
        // 与编辑器 .sop-edit-note.is-mark/.is-warn、预览 .sop-preview-desc 逐值一致
        assert_eq!(note_palette("mark"), Some(("#e6f1fb", "#0c447c", "#378add")));
        assert_eq!(note_palette("warn"), Some(("#faeeda", "#633806", "#ba7517")));
        assert_eq!(note_palette(""), None);
        assert_eq!(note_palette("note"), None);
        assert_eq!(note_palette("desc"), None);
    }

    #[test]
    fn test_step_desc_html_kind_colors() {
        let mark = build_step_desc_html_kind("注意", "mark");
        assert!(mark.contains("class=\"step-desc is-mark\""));
        assert!(mark.contains("background:#e6f1fb;color:#0c447c;"));
        assert!(mark.contains("border-left:3px solid #378add"));
        let warn = build_step_desc_html_kind("危险", "warn");
        assert!(warn.contains("class=\"step-desc is-warn\""));
        assert!(warn.contains("background:#faeeda;color:#633806;"));
        assert!(warn.contains("border-left:3px solid #ba7517"));
        // 普通描述不带底色，与旧行为完全一致
        assert_eq!(
            build_step_desc_html_kind("普通", ""),
            build_step_desc_html("普通")
        );
        // 空描述：任何类型都不出块
        assert_eq!(build_step_desc_html_kind("", "mark"), "");
    }

    #[test]
    fn test_export_txt_and_md_skip_heading_numbers() {
        let rec = skill_recording();
        let steps = vec![
            heading_block(2, "第二节", ""),
            plain_step(1, "点一下", "说明", ""),
            heading_block(4, "第四节", ""),
            plain_step(2, "再点一下", "注意", "warn"),
        ];
        assert_eq!(real_step_count(&steps), 2);

        let txt = export_txt(&rec, &steps);
        assert!(txt.contains("步骤数：2"), "txt meta: {}", txt);
        assert!(txt.contains("  【第二节】"), "txt 二级标题应缩进两格: {}", txt);
        assert!(txt.contains("【第四节】"));
        assert!(txt.contains("[1] 点一下"));
        assert!(txt.contains("[2] 再点一下"));
        assert!(txt.contains("警告：注意"));
        assert!(!txt.contains("步骤 0"));

        let empty_zoom = std::collections::HashMap::new();
        let md = export_markdown(&rec, &steps, std::path::Path::new("."), &empty_zoom);
        assert!(md.contains("### 第二节"), "md H2 → ###: {}", md);
        assert!(md.contains("##### 第四节"), "md H4 → #####: {}", md);
        assert!(md.contains("## 步骤 1：点一下"));
        assert!(!md.contains("步骤 0"));
        assert!(md.contains("共 2 步 · 由 录步 Flowio 生成"));
    }

    #[test]
    fn test_skill_md_skips_heading_blocks() {
        // 大标题块不是可执行步骤：SKILL.md 的编号列表里不能出现它
        let steps = vec![
            skill_step(1, "click", "点一下", "", false, ""),
            heading_block(2, "第二节", ""),
            skill_step(2, "click", "再点一下", "", false, ""),
        ];
        let md = export_skill_md(&skill_recording(), &steps, &[], &SkillMachine::new());
        assert!(!md.contains("第二节"), "大标题不该进 SKILL.md: {}", md);
        assert!(md.contains("1. **点一下**"));
        assert!(md.contains("2. **再点一下**"));
        // frontmatter 的步数只数真步骤
        assert!(md.contains("共 2 步"), "frontmatter: {}", md);
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
        let required = concat!("docx_image_emu_dims(", "final_dims");
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
        let doc = build_html_document("标题", 1, "2026-08-20", "<card/>", "", true);
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

    /* ── 裁图几何（export_crop_box / crop_region_50）────────────────
     * 5b 对齐插件 computeCropRect：中心 + 恒 50% 窗口 + 贴边平移夹回。
     * 下面的黄金值与前端 cropBoxFromAnchor（check-crop.mjs 第 6 段）
     * 以及 AI 端 ai_crop_region（pipeline.rs）共用同一组数：任一端改动
     * 而不同步其余端，这里就会红。
     *
     * 场景：1920x1080 截图，锚点中心 (960,540)
     *   → 恒 50% 窗口：w=960, h=540, x=960-480=480, y=540-270=270
     *   type/click/drag 无差异（元素尺寸不参与几何）
     */

    #[test]
    fn export_crop_box_golden_matches_frontend() {
        // 与前端 cropBoxFromAnchor({x:960,y:540,cropW:300,cropH:40}, t, 1920, 1080)
        // 及 AI ai_crop_region 的实算输出一致；type/click/drag 无差异
        for at in ["type", "click", "drag"] {
            assert_eq!(
                export_crop_box(1920, 1080, Some(960), Some(540), Some(300), Some(40), at),
                Some((480, 270, 960, 540)),
                "{at} 应恒 50% 窗口且与元素尺寸无关"
            );
        }
    }

    #[test]
    fn export_crop_box_element_size_does_not_affect_geometry() {
        // 极小元素（24px）与超宽元素（900px）：窗口不变（旧 PAD/左扩体系已废弃）
        for (cw, ch) in [(24, 40), (900, 40), (3000, 2000)] {
            assert_eq!(
                export_crop_box(1920, 1080, Some(960), Some(540), Some(cw), Some(ch), "type"),
                Some((480, 270, 960, 540)),
                "元素 {cw}x{ch} 不应改变 50% 窗口"
            );
        }
    }

    #[test]
    fn export_crop_box_golden_edge_corners() {
        // 贴边平移夹回：中心在 (20,20) / (-5,10) 时窗口贴左上角；
        // 4K 与低分屏分别验证倍率 2× 与 round 取整
        assert_eq!(
            export_crop_box(1920, 1080, Some(20), Some(20), Some(300), Some(40), "click"),
            Some((0, 0, 960, 540))
        );
        assert_eq!(
            export_crop_box(1920, 1080, Some(-5), Some(10), Some(2), Some(2), "click"),
            Some((0, 0, 960, 540))
        );
        assert_eq!(
            export_crop_box(3840, 2160, Some(1920), Some(1080), Some(300), Some(40), "click"),
            Some((960, 540, 1920, 1080))
        );
        assert_eq!(
            export_crop_box(1366, 768, Some(683), Some(384), Some(300), Some(40), "click"),
            Some((342, 192, 683, 384))
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
        // 负坐标但**没有**元素矩形 → 50% 兜底分支要求坐标非负 → 全图（与前端一致）
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
        //   rw=960 → left = round(-5-480) = -485 → clamp 0；top = round(10-270) = -260 → clamp 0
        assert_eq!(
            export_crop_box(1920, 1080, Some(-5), Some(10), Some(2), Some(2), "click"),
            Some((0, 0, 960, 540))
        );
    }

    #[test]
    fn export_crop_box_fallback_50pct_when_no_element_rect() {
        // 只有点击点：以点击点为中心裁屏幕的 50%（兜底倍率 40% → 50%，5b 对齐）
        assert_eq!(
            export_crop_box(1920, 1080, Some(960), Some(540), None, None, "click"),
            Some((480, 270, 960, 540))
        );
    }

    #[test]
    fn export_crop_box_clamps_into_image_bounds() {
        // 元素贴左上角：框必须平移进图内，不能出现负坐标或越界
        for at in ["type", "click"] {
            let (l, t, w, h) =
                export_crop_box(1920, 1080, Some(20), Some(20), Some(300), Some(40), at)
                    .unwrap_or_else(|| panic!("{at} 应产生裁图框"));
            assert_eq!((l, t, w, h), (0, 0, 960, 540), "{at} 应贴边夹回");
        }
    }

    #[test]
    fn export_crop_box_never_exceeds_image_size() {
        // 窗口恒为图的 50%，永不越界（贴边夹回保证起点非负且宽高饱和到图尺寸）
        assert_eq!(
            export_crop_box(400, 300, Some(200), Some(150), Some(3000), Some(2000), "type"),
            Some((100, 75, 200, 150))
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
    fn test_export_long_image_plain_stitch() {
        // 插件口径的长图 = **纯截图拼接**：没有教程标题头、没有步骤标题条。
        // baseW = min(各图最大自然宽, 1440)；图之间 24px；总高 = 各图等比高之和 + 间隔。
        let dir = std::env::temp_dir().join(format!("flowio_test_long_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let png1 = write_tmp_png(&dir, "step1.png");
        let png2 = write_tmp_png(&dir, "step2.png");
        image::DynamicImage::new_rgb8(400, 300).save(&png1).unwrap();
        image::DynamicImage::new_rgb8(500, 250).save(&png2).unwrap();

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
                block_kind: "step".to_string(),
                block_level: 1,
                note_kind: String::new(),
                title_html: String::new(),
                image_caption: String::new(),
                media_kind: "image".into(),
                media_poster_path: String::new(),
                media_source_path: String::new(),
                url: String::new(),
                before_screenshot_path: String::new(),
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
                block_kind: "step".to_string(),
                block_level: 1,
                note_kind: String::new(),
                title_html: String::new(),
                image_caption: String::new(),
                media_kind: "image".into(),
                media_poster_path: String::new(),
                media_source_path: String::new(),
                url: String::new(),
                before_screenshot_path: String::new(),
            },
        ];
        let out = dir.join("long_plain.png");
        let zoom_map: std::collections::HashMap<String, f64> = std::collections::HashMap::new();
        export_long_image(&rec, &steps, &out, &zoom_map, false, 1.0, false).unwrap();
        assert!(out.exists(), "长图文件未生成");

        let loaded = image::open(&out).expect("无法读取生成的长图");
        let (w, h) = (loaded.width(), loaded.height());
        // baseW = min(max(400,500),1440) = 500；s1 = 500×375、s2 = 500×250；高 = 375+24+250
        assert_eq!(w, 500, "宽度应等于基准宽度（无标题头，不再受其影响）");
        assert_eq!(h, 649, "高度应只由「图高 + 间隔」组成");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_export_long_image_focus_scales_and_notes() {
        // 重点图（显式倍率 > 1）：画布跟着变宽、该图放大，普通图居中；
        // 重点图下方挂一条说明条 —— 高度必须把说明条算进去。
        let dir = std::env::temp_dir().join(format!("flowio_test_long_focus_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let png1 = write_tmp_png(&dir, "step1.png");
        image::DynamicImage::new_rgb8(800, 600).save(&png1).unwrap();

        let rec = ExportRecording {
            id: "long-focus".into(),
            title: "重点图教程".into(),
            app_name: "Flowio".into(),
            total_steps: 1,
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
                block_kind: "step".to_string(),
                block_level: 1,
                note_kind: String::new(),
                title_html: String::new(),
                image_caption: String::new(),
                media_kind: "image".into(),
                media_poster_path: String::new(),
                media_source_path: String::new(),
                url: String::new(),
                before_screenshot_path: String::new(),
            },
        ];
        let out = dir.join("long_focus.png");
        let mut zoom_map: std::collections::HashMap<String, f64> = std::collections::HashMap::new();
        zoom_map.insert("s1".to_string(), 1.25);
        export_long_image(&rec, &steps, &out, &zoom_map, false, 1.0, false).unwrap();

        let loaded = image::open(&out).expect("无法读取生成的长图");
        let (w, h) = (loaded.width(), loaded.height());
        assert_eq!(w, 1000, "重点图应把画布撑到 800×1.25");
        // 图高 = round(600 × 1000 / 800) = 750；说明条 = 上下内边距 32 + 标题行 28 + 描述 1 行 24
        assert_eq!(h, 750 + 84, "说明条高度没算进去");
        let _ = std::fs::remove_dir_all(&dir);
    }

}


#[cfg(test)]
mod focus_shot_tests {
    use super::*;

    /// 重点图裁框必须与前端 CSS `transform: scale(z)` + `transform-origin: 操作落点`
    /// 等价：编辑器里看到的画面 = 导出出去的图。
    ///
    /// 手算：1280×720、落点 (900,300)、z=1.25
    ///   可见区域 = 1280/1.25 × 720/1.25 = 1024×576
    ///   left = 900·(1 − 1/1.25) = 180 ；top = 300·0.2 = 60
    #[test]
    fn focus_box_matches_css_transform() {
        assert_eq!(
            focus_shot_box(1280, 720, Some(900), Some(300), 1.25),
            Some((180, 60, 1024, 576))
        );
    }

    /// 倍率 1（用户手动还原原图）→ 不裁，导出整图
    #[test]
    fn focus_box_zoom_one_is_full_image() {
        assert_eq!(focus_shot_box(1280, 720, Some(900), Some(300), 1.0), None);
    }

    /// 没有点击坐标（整窗类型 scroll/nav）→ 没法定位重点，导出整图
    #[test]
    fn focus_box_without_point_is_full_image() {
        assert_eq!(focus_shot_box(1280, 720, None, None, 1.25), None);
        assert_eq!(focus_shot_box(1280, 720, Some(-5), Some(10), 1.25), None);
        assert_eq!(focus_shot_box(1280, 720, Some(9999), Some(10), 1.25), None);
    }

    /// 落点贴边：裁框整体平移进图内，绝不越界、也绝不缩小（缩小就等于偷偷改倍率）
    #[test]
    fn focus_box_clamps_into_bounds() {
        let (l, t, w, h) = focus_shot_box(1280, 720, Some(0), Some(0), 1.25).unwrap();
        assert_eq!((l, t, w, h), (0, 0, 1024, 576));
        let (l2, t2, w2, h2) = focus_shot_box(1280, 720, Some(1280), Some(720), 1.25).unwrap();
        assert_eq!((w2, h2), (1024, 576));
        assert!(l2 + w2 <= 1280 && t2 + h2 <= 720);
    }

    /// 倍率缺省 → 默认 125%
    #[test]
    fn zoom_for_defaults_to_125_percent() {
        let m = std::collections::HashMap::new();
        assert_eq!(zoom_for(&m, "any-step"), 1.25);
        let mut m2 = std::collections::HashMap::new();
        m2.insert("s1".to_string(), 2.0);
        assert_eq!(zoom_for(&m2, "s1"), 2.0);
        assert_eq!(zoom_for(&m2, "s2"), 1.25);
    }
}
