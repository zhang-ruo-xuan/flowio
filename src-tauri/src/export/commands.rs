use base64::Engine;
use docx_rs::*;
use rusqlite::params;
use serde::{Deserialize, Serialize};
use tauri::State;

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
    order_index: i32,
    step_number: i32,
    action_type: String,
    title: String,
    description: String,
    tip: String,
    screenshot_path: String,
}

// ── Tauri Command ─────────────────────────────────────────────

/// Export a recording to the specified format.
///
/// Supported formats: `"html"`, `"markdown"`, `"pdf"`, `"docx"` (or `"word"`).
/// If `output_path` is provided, write to that exact path; otherwise auto-generate
/// a path under the `output/` directory.
/// Returns the absolute path to the generated file.
#[tauri::command]
pub fn export_recording(
    db: State<DbState>,
    id: String,
    format: String,
    output_path: Option<String>,
) -> Result<String, String> {
    let (recording, steps) = load_export_data(&db, &id)?;

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
        let sanitized_title = sanitize_filename(&recording.title);
        let timestamp = chrono::Utc::now().format("%Y%m%d_%H%M%S");
        let ext = match format.to_lowercase().as_str() {
            "html" => "html",
            "markdown" => "md",
            "pdf" => "pdf",
            "docx" | "word" => "docx",
            _ => "txt",
        };
        output_dir.join(format!("{}_{}.{}", sanitized_title, timestamp, ext))
    };

    match format.to_lowercase().as_str() {
        "html" => {
            let content = export_html(&recording, &steps);
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
            let content = export_markdown(&recording, &steps, &ss_dir);
            std::fs::write(&path, content)
                .map_err(|e| format!("Failed to write Markdown: {}", e))?;
            Ok(path.to_string_lossy().to_string())
        }
        "pdf" => {
            export_pdf(&recording, &steps, &path)?;
            Ok(path.to_string_lossy().to_string())
        }
        "docx" | "word" => {
            export_docx(&recording, &steps, &path)?;
            Ok(path.to_string_lossy().to_string())
        }
        other => Err(format!("Unsupported format: {}. Use html, markdown, docx, or pdf.", other)),
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
            "SELECT order_index, step_number, action_type, title, description, \
             tip, screenshot_path FROM steps WHERE recording_id = ?1 ORDER BY order_index",
        )
        .map_err(|e| e.to_string())?;

    let rows = stmt
        .query_map(params![id], |row| {
            Ok(ExportStep {
                order_index: row.get(0)?,
                step_number: row.get(1)?,
                action_type: row.get(2)?,
                title: row.get(3)?,
                description: row.get(4)?,
                tip: row.get(5)?,
                screenshot_path: row.get(6)?,
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
    name.chars()
        .map(|c| match c {
            '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|' => '_',
            _ => c,
        })
        .collect()
}

// ── HTML export ───────────────────────────────────────────────

/// Generate the inner HTML body block (step cards).
/// Images are embedded as base64 data URIs for standalone output.
fn gen_html_body(
    recording: &ExportRecording,
    steps: &[ExportStep],
) -> String {
    let step_cards: String = steps
        .iter()
        .map(|s| {
            let screenshot_html = if s.screenshot_path.is_empty() {
                String::from(
                    "<div class=\"screenshot-placeholder\">No screenshot</div>",
                )
            } else {
                let encoded = std::fs::read(&s.screenshot_path)
                    .ok()
                    .map(|data| {
                        base64::engine::general_purpose::STANDARD.encode(&data)
                    })
                    .unwrap_or_default();
                let mime = mime_from_path(&s.screenshot_path);
                format!(
                    "<img class=\"screenshot\" src=\"data:{};base64,{}\" \
                     alt=\"Step {} screenshot\" />",
                    mime,
                    encoded,
                    s.step_number
                )
            };

            // Step header: "Step N: [type] title"
            let header = format!(
                "Step {}: <span class=\"step-type\">{}</span> {}",
                s.step_number,
                htmlescape(&s.action_type),
                htmlescape(&s.title),
            );

            let desc_block = if s.description.is_empty() {
                String::new()
            } else {
                format!(
                    "      <p class=\"step-desc\">{}</p>\n",
                    htmlescape(&s.description)
                )
            };

            let tip_block = if s.tip.is_empty() {
                String::new()
            } else {
                format!(
                    "      <div class=\"step-tip\"><strong>Tip:</strong> {}</div>\n",
                    htmlescape(&s.tip)
                )
            };

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

    format!(
        r#"<!DOCTYPE html>
<html lang="zh-CN">
<head>
<meta charset="UTF-8">
<meta name="viewport" content="width=device-width, initial-scale=1.0">
<title>{title}</title>
<style>
  *,*::before,*::after{{box-sizing:border-box;margin:0;padding:0}}
  body{{font-family:-apple-system,BlinkMacSystemFont,"Segoe UI",Helvetica,Arial,sans-serif;background:#f6f8fa;color:#24292f;line-height:1.6}}
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
        title = htmlescape(&recording.title),
        total_steps = recording.total_steps,
        created_at = &recording.created_at,
        step_cards = step_cards,
    )
}

/// Generate standalone HTML with base64-embedded screenshots.
fn export_html(recording: &ExportRecording, steps: &[ExportStep]) -> String {
    gen_html_body(recording, steps)
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
fn export_markdown(recording: &ExportRecording, steps: &[ExportStep], ss_dir: &std::path::Path) -> String {
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
                match std::fs::copy(src, &dest) {
                    Ok(_) => {
                        format!("![Screenshot](./screenshots/{})", filename)
                    }
                    Err(_) => format!(
                        "> *(Screenshot not found: {})*",
                        s.screenshot_path.replace('\\', "/")
                    ),
                }
            };

            // ── Step header (Scribe-style badge) ───────────────
            let header = format!(
                "## Step {step_number} · `{action_type}` · {title}",
                step_number = s.step_number,
                action_type = &s.action_type,
                title = &s.title,
            );

            // ── Description ─────────────────────────────────────
            let desc_block = if s.description.is_empty() {
                String::new()
            } else {
                format!("{}\n\n", &s.description)
            };

            // ── Tip (styled blockquote) ─────────────────────────
            let tip_block = if s.tip.is_empty() {
                String::new()
            } else {
                format!("> **Tip:** {}\n", &s.tip)
            };

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

    format!(
        "# {title}\n\n\
         **App:** {app_name}  ·  **Steps:** {total_steps}  ·  **Date:** {created_at}\n\n\
         ---\n\n\
         {step_list}",
        title = &recording.title,
        app_name = &recording.app_name,
        total_steps = recording.total_steps,
        created_at = &recording.created_at,
        step_list = step_list,
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
) -> Result<(), String> {
    const MAX_IMG_WIDTH_EMU: usize = 5_715_000; // ~600px at 96 DPI

    let mut docx = Docx::new();

    // ── Title ─────────────────────────────────────────────────
    docx = docx.add_paragraph(
        Paragraph::new()
            .add_run(Run::new().add_text(&recording.title).bold().size(36))
    );

    // ── Metadata ──────────────────────────────────────────────
    let meta = format!(
        "App: {}  ·  Steps: {}  ·  Date: {}",
        recording.app_name, recording.total_steps, recording.created_at
    );
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
        let header = format!(
            "Step {} · {} · {}",
            s.step_number, s.action_type, s.title
        );
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
                let (w_emu, h_emu) = if let Some((w, h)) = dim_opt {
                    if *w > 600 {
                        let ratio = 600.0 / *w as f64;
                        (
                            (600.0 * 9525.0) as u32,
                            (*h as f64 * ratio * 9525.0) as u32,
                        )
                    } else {
                        ((*w * 9525) as u32, (*h * 9525) as u32)
                    }
                } else {
                    (MAX_IMG_WIDTH_EMU as u32, (MAX_IMG_WIDTH_EMU * 3 / 4) as u32)
                };
                let pic = Pic::new(&img_bytes).size(w_emu, h_emu);
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

/// Generate a PDF by rendering the HTML export via Microsoft Edge headless.
/// The HTML (with base64-embedded screenshots) is written to a temp file,
/// then converted to a self-contained PDF. No printpdf, no external deps.
fn export_pdf(
    recording: &ExportRecording,
    steps: &[ExportStep],
    output_path: &std::path::Path,
) -> Result<(), String> {
    if steps.is_empty() {
        return Err("No steps to export".to_string());
    }

    // 1. Generate standalone HTML with base64-embedded images
    let html = gen_html_body(recording, steps);

    // 2. Write HTML to a temporary file
    let temp_dir = std::env::temp_dir();
    let html_path = temp_dir.join(format!("flowio_pdf_{}.html", recording.id));
    std::fs::write(&html_path, html.as_bytes())
        .map_err(|e| format!("Failed to write temp HTML: {}", e))?;

    // 3. Convert to PDF using Microsoft Edge headless
    let edge_path = r"C:\Program Files (x86)\Microsoft\Edge\Application\msedge.exe";
    let html_url = format!(
        "file:///{}",
        html_path.to_string_lossy().replace('\\', "/")
    );

    let output = std::process::Command::new(edge_path)
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
        .output()
        .map_err(|e| format!("Failed to launch Edge: {}", e))?;

    // 4. Clean up temp HTML (best effort)
    let _ = std::fs::remove_file(&html_path);

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!("Edge PDF conversion failed: {}", stderr));
    }

    Ok(())
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
                order_index: 0,
                step_number: 1,
                action_type: "click".into(),
                title: "第一步".into(),
                description: "这是第一步描述".into(),
                tip: "".into(),
                screenshot_path: "".into(),
            },
            ExportStep {
                order_index: 1,
                step_number: 2,
                action_type: "type".into(),
                title: "第二步".into(),
                description: "这是第二步描述\n包含换行".into(),
                tip: "一个提示".into(),
                screenshot_path: "".into(),
            },
        ];
        let out = std::env::temp_dir().join("flowio_test_no_ss.pdf");
        let result = export_pdf(&rec, &steps, &out);
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
        let result = export_pdf(&rec, &[], std::path::Path::new("unused.pdf"));
        assert!(result.is_err());
    }
}
