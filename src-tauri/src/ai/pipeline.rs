use base64::Engine;
use crate::ai::http::{self, Message};
use crate::ai::parse;
use crate::ai::prompt;
use crate::ai::sanitize;
use crate::ai::types::{AiConfig, AiJob, AiResult, StepInput, StepOutput};

/// Crop radius: half the side length of the square sent to the vision model.
const CROP_HALF_SIDE: i32 = 200;

/// JPEG quality for cropped screenshots (85 = good balance of size and fidelity).
const CROP_JPEG_QUALITY: u8 = 85;

// ── Public pipeline API ───────────────────────────────────────

/// Run the complete four-stage AI pipeline.
///
/// # Stages
/// 1. **Preprocess** — crop screenshots to 400×400 around click point, sanitize text.
/// 2. **Batch call** — iterate steps, call LLM for each, collect raw outputs.
/// 3. **Parse & Validate** — parse JSON, validate required fields.
/// 4. **Persist** — return `AiResult` for the caller to persist.
pub fn run_pipeline(cfg: &AiConfig, job: AiJob) -> Result<AiResult, String> {
    // ── Stage 1: Preprocess ──────────────────────────────────
    // Save original full screenshots before cropping (for context reference).
    let original_marked: Vec<String> = job.steps.iter().map(|s| s.screenshot_base64.clone()).collect();
    let original_before: Vec<String> = job.steps.iter().map(|s| s.before_screenshot_base64.clone()).collect();

    let preprocessed: Vec<StepInput> = job
        .steps
        .into_iter()
        .map(|mut s| {
            s.screenshot_base64 = crop_and_encode(&s.screenshot_base64, s.click_x, s.click_y);
            s.before_screenshot_base64 = crop_and_encode(&s.before_screenshot_base64, s.click_x, s.click_y);
            s.title = sanitize::sanitize_for_ai(&s.title);
            s.description = sanitize::sanitize_for_ai(&s.description);
            s
        })
        .collect();

    // ── Check: screenshots require a vision-capable model ────
    let has_screenshots = preprocessed.iter().any(|s| !s.screenshot_base64.is_empty() || !s.before_screenshot_base64.is_empty());
    if has_screenshots && !model_supports_vision(&cfg.provider.model) {
        let suggestion = suggest_vision_model(&cfg.provider.id);
        return Err(format!(
            "当前模型「{}」不支持图片识别。录制中的步骤包含截图，需要视觉模型才能分析。请到设置中将模型切换为{}。",
            cfg.provider.model, suggestion
        ));
    }

    // ── Stage 2: Batch call ──────────────────────────────────
    let system_prompt = prompt::build_system_prompt();
    let mut raw_outputs: Vec<Option<String>> = Vec::with_capacity(preprocessed.len());
    let mut errors: Vec<String> = Vec::new();

    for (i, step) in preprocessed.iter().enumerate() {
        let step_number = i + 1;
        let mut step_prompt = prompt::build_step_prompt(step, step_number);

        // Detect dramatic window change between before/after screenshots
        // and prepend a hard override instruction when detected.
        if detect_window_change(&original_before[i], &original_marked[i]) {
            step_prompt = format!(
                "【强制指令】此步骤的 before 和 after 截图完全不同——用户执行的是窗口管理操作（关闭窗口、切换窗口、打开新应用等），而不是在 before 截图的界面上操作。你必须以录制的原始动作类型和原始标题为准来撰写描述，严禁根据 before 截图中的网页内容猜测操作（如投稿、点赞等）。\n\n{}",
                step_prompt
            );
        }

        let mut messages = vec![Message {
            role: "system".to_string(),
            content: serde_json::Value::String(system_prompt.clone()),
        }];

        // Build user message with text + optional screenshots
        let has_before = !step.before_screenshot_base64.is_empty();
        let has_marked = !step.screenshot_base64.is_empty();

        if !has_before && !has_marked {
            messages.push(Message {
                role: "user".to_string(),
                content: serde_json::Value::String(step_prompt),
            });
        } else {
            let mut content_parts: Vec<serde_json::Value> = vec![
                serde_json::json!({
                    "type": "text",
                    "text": step_prompt,
                }),
            ];

            // Determine if cropping actually happened (valid coordinates → crop was applied).
            let has_valid_coords = step.click_x.map_or(false, |x| x >= 0)
                && step.click_y.map_or(false, |y| y >= 0);

            if has_before {
                // Prepend full before-screenshot for global context (only when crop was applied).
                if has_valid_coords && !original_before[i].is_empty() {
                    content_parts.push(serde_json::json!({
                        "type": "image_url",
                        "image_url": {
                            "url": format!("data:image/png;base64,{}", original_before[i]),
                        },
                    }));
                }
                content_parts.push(serde_json::json!({
                    "type": "image_url",
                    "image_url": {
                        "url": format!("data:image/jpeg;base64,{}", step.before_screenshot_base64),
                    },
                }));
            }

            if has_marked {
                // Prepend full marked-screenshot for global context (only when crop was applied).
                if has_valid_coords && !original_marked[i].is_empty() {
                    content_parts.push(serde_json::json!({
                        "type": "image_url",
                        "image_url": {
                            "url": format!("data:image/png;base64,{}", original_marked[i]),
                        },
                    }));
                }
                content_parts.push(serde_json::json!({
                    "type": "image_url",
                    "image_url": {
                        "url": format!("data:image/jpeg;base64,{}", step.screenshot_base64),
                    },
                }));
            }

            messages.push(Message {
                role: "user".to_string(),
                content: serde_json::Value::Array(content_parts),
            });
        }

        match http::call_llm(cfg, messages) {
            Ok(raw) => raw_outputs.push(Some(raw)),
            Err(e) => {
                errors.push(format!("Step {} LLM error: {}", step_number, e));
                raw_outputs.push(None);
            }
        }
    }

    // ── Stage 3: Parse & Validate ────────────────────────────
    let mut steps: Vec<StepOutput> = Vec::with_capacity(raw_outputs.len());

    for (i, raw_opt) in raw_outputs.iter().enumerate() {
        let step_number = i + 1;
        let raw = match raw_opt {
            Some(r) => r,
            None => continue,
        };

        match parse::parse_step_output(raw, &preprocessed[i].action_type) {
            Ok(output) => {
                // Validate required fields
                if output.title.trim().is_empty() {
                    errors.push(format!(
                        "Step {} validation: title is empty",
                        step_number
                    ));
                    continue;
                }
                if output.description.trim().is_empty() {
                    errors.push(format!(
                        "Step {} validation: description is empty",
                        step_number
                    ));
                    continue;
                }
                steps.push(output);
            }
            Err(e) => {
                errors.push(format!("Step {} parse error: {}", step_number, e));
            }
        }
    }

    // ── Stage 4: Persist (return to caller) ──────────────────
    Ok(AiResult { steps, errors })
}

// ── Internal helpers ──────────────────────────────────────────

/// Detect whether the before/after screenshot pair represents a dramatic window change
/// (e.g. closing a browser, switching applications, opening a new window).
///
/// Uses base64-decoded raw bytes with stride sampling to approximate pixel comparison
/// without depending on an image decoding crate. Samples every 30 bytes (≈ 10 pixels ×
/// 3 channels), computes the mean absolute difference, and flags a "window change" when
/// the average difference exceeds 60/255.
fn detect_window_change(before_b64: &str, after_b64: &str) -> bool {
    if before_b64.is_empty() || after_b64.is_empty() {
        return false;
    }

    let before_bytes = match base64::engine::general_purpose::STANDARD.decode(before_b64) {
        Ok(b) => b,
        Err(_) => return false,
    };
    let after_bytes = match base64::engine::general_purpose::STANDARD.decode(after_b64) {
        Ok(b) => b,
        Err(_) => return false,
    };

    // Use the shorter length so we don't index out of bounds.
    let len = before_bytes.len().min(after_bytes.len());

    // Stride: 30 bytes ≈ 10 pixels × 3 channels (RGB).
    const STRIDE: usize = 30;

    let mut total_diff: u64 = 0;
    let mut count: u64 = 0;

    let mut idx = 0;
    while idx < len {
        let d = if before_bytes[idx] > after_bytes[idx] {
            (before_bytes[idx] - after_bytes[idx]) as u64
        } else {
            (after_bytes[idx] - before_bytes[idx]) as u64
        };
        total_diff += d;
        count += 1;
        idx += STRIDE;
    }

    if count == 0 {
        return false;
    }

    let avg_diff = total_diff / count;
    avg_diff > 60
}

/// Crop the screenshot to 400×400 centered on the click point, encoding as JPEG.
///
/// When coordinates are available: crop a square around (x, y), clamped to image bounds,
/// and encode at quality 85. Without coordinates: fall back to full-image JPEG encoding
/// at quality 85 (no aggressive down-scaling — 400×400 crops are tiny and full screenshots
/// get naturally bounded by JPEG compression).
fn crop_and_encode(base64_str: &str, click_x: Option<i32>, click_y: Option<i32>) -> String {
    if base64_str.is_empty() {
        return String::new();
    }

    let data = match base64::engine::general_purpose::STANDARD.decode(base64_str) {
        Ok(d) => d,
        Err(_) => return base64_str.to_string(),
    };

    let img = match image::load_from_memory(&data) {
        Ok(i) => i,
        Err(_) => return base64_str.to_string(),
    };

    let (img_w, img_h) = (img.width() as i32, img.height() as i32);

    // Determine crop region — if coordinates available, center 400×400 around the click
    let cropped = match (click_x, click_y) {
        (Some(x), Some(y)) if x >= 0 && y >= 0 => {
            let left = (x - CROP_HALF_SIDE).max(0);
            let top = (y - CROP_HALF_SIDE).max(0);
            let w = (CROP_HALF_SIDE * 2).min(img_w - left);
            let h = (CROP_HALF_SIDE * 2).min(img_h - top);
            img.crop_imm(left as u32, top as u32, w as u32, h as u32)
        }
        _ => img,
    };

    // Encode as JPEG with quality control
    let mut buf: Vec<u8> = Vec::new();
    {
        let mut encoder =
            image::codecs::jpeg::JpegEncoder::new_with_quality(&mut buf, CROP_JPEG_QUALITY);
        if encoder
            .encode(cropped.as_bytes(), cropped.width(), cropped.height(), cropped.color().into())
            .is_ok()
        {
            return base64::engine::general_purpose::STANDARD.encode(&buf);
        }
    }
    base64_str.to_string()
}

/// Check whether a model name suggests vision/multimodal capabilities.
///
/// Uses substring matching against known vision model identifiers.
/// Covers major providers: OpenAI (gpt-4o, gpt-4-turbo),
/// Anthropic (claude-3/3.5), Google (gemini), Zhipu (glm-4v),
/// Qwen (qwen-vl), Doubao (doubao-vision), Yi (yi-vision).
fn model_supports_vision(model: &str) -> bool {
    let lower = model.to_lowercase();
    let vision_keywords = [
        "gpt-4o",
        "gpt-4-vision",
        "gpt-4-turbo",
        "claude-3",
        "gemini",
        "glm-4v",
        "qwen-vl",
        "doubao-vision",
        "yi-vision",
        "vision",
        "vl",
        "hunyuan",
        "multimodal",
    ];
    vision_keywords.iter().any(|kw| lower.contains(kw))
}

/// Suggest a vision-capable model for the given provider.
fn suggest_vision_model(provider_id: &str) -> String {
    match provider_id {
        "zhipu" => "「glm-4v」".to_string(),
        "qwen" => "「qwen-vl-plus」或「qwen-vl-max」".to_string(),
        "doubao" => "「doubao-vision-pro-32k」".to_string(),
        "yi" => "「yi-vision」".to_string(),
        _ => "支持图片识别的视觉模型（如 gpt-4o、claude-3.5-sonnet 等）".to_string(),
    }
}
