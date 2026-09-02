use base64::Engine;
use crate::ai::http::{self, Message};
use crate::ai::parse;
use crate::ai::prompt;
use crate::ai::sanitize;
use crate::ai::types::{AiConfig, AiJob, AiResult, StepInput, StepOutput};

/// Padding (px) added around the focused element rect before cropping.
const CROP_PAD: i32 = 24;
/// Minimum crop region side (px); smaller rects are expanded around their center.
const CROP_MIN_SIZE: i32 = 280;
/// Crop region must not exceed this fraction of the screenshot width/height.
const CROP_MAX_SCREEN_RATIO: f32 = 0.9;
/// Fallback crop size when no element rect but click point exists: fraction of screen.
const CROP_FALLBACK_RATIO: f32 = 0.4;

/// JPEG quality for cropped screenshots (85 = good balance of size and fidelity).
const CROP_JPEG_QUALITY: u8 = 85;

/// Choose which screenshot to send to the vision model for a given step.
///
/// Spreadsheet apps (WPS 表格 / Excel / LibreOffice Calc / Numbers …) place the
/// relevant UI — the ribbon, formula bar and function dialog — across the whole
/// window, so a focused 400×400 crop often hides it. For those steps we send the
/// full (already redacted) screenshot instead, trading more tokens for accuracy.
/// Non-spreadsheet steps keep the cheaper focused crop.
fn ai_image_for_step<'a>(step: &'a StepInput, full_marked: &'a str) -> &'a str {
    if crate::ai::prompt::is_spreadsheet_window(&step.window_title) && !full_marked.is_empty() {
        full_marked
    } else {
        &step.screenshot_base64
    }
}

// ── Public pipeline API ───────────────────────────────────────

/// Run the complete four-stage AI pipeline.
///
/// # Stages
/// 1. **Preprocess** — crop screenshots to 400×400 around click point, sanitize text.
/// 2. **Batch call** — iterate steps, call LLM for each, collect raw outputs.
/// 3. **Parse & Validate** — parse JSON, validate required fields.
/// 4. **Persist** — return `AiResult` for the caller to persist.
pub fn run_pipeline(cfg: &AiConfig, job: AiJob) -> Result<AiResult, String> {
    // ── Stage 0: Vision capability gating ─────────────────────
    // 只要有截图输入就必须使用视觉模型（与脱敏结果无关：脱敏可能因 OCR
    // 引擎缺失丢弃图片，但录制本身含截图步骤，文本模型无法正确理解截图。
    // 该检查前置到脱敏之前，避免 OCR 失败导致误判）。
    let has_any_screenshot = job.steps.iter().any(|s| {
        !s.screenshot_base64.is_empty() || !s.before_screenshot_base64.is_empty()
    });
    if has_any_screenshot && !model_supports_vision(&cfg.provider.model) {
        let suggestion = suggest_vision_model(&cfg.provider.id);
        return Err(format!(
            "当前模型「{}」不支持图片识别。录制中的步骤包含截图，需要视觉模型才能分析。请到设置中将模型切换为{}。",
            cfg.provider.model, suggestion
        ));
    }

    // ── Stage 1: Privacy redaction ───────────────────────────
    // 屏幕截图可能含密码、聊天内容、地址栏等敏感像素。外发第三方大模型前
    // 必须先做 OCR 脱敏（与导出一致）。默认全开 + 高斯模糊；OCR 失败时
    // 丢弃该截图而不是裸发（隐私优先于生成质量）。
    let redact_config = crate::redaction::RedactionConfig::default_all();
    let redact_mode = crate::redaction::RedactMode::Blur;
    let mut job = job;
    for s in &mut job.steps {
        match crate::redaction::redact_base64_screenshot(
            &s.screenshot_base64,
            &redact_config,
            redact_mode,
        ) {
            Ok(Some(r)) => s.screenshot_base64 = r,
            Ok(None) => {}
            Err(e) => {
                log::warn!("[ai] 截图脱敏失败，丢弃该图（不裸传）：{}", e);
                s.screenshot_base64 = String::new();
            }
        }
        match crate::redaction::redact_base64_screenshot(
            &s.before_screenshot_base64,
            &redact_config,
            redact_mode,
        ) {
            Ok(Some(r)) => s.before_screenshot_base64 = r,
            Ok(None) => {}
            Err(e) => {
                log::warn!("[ai] 截图脱敏失败，丢弃该图（不裸传）：{}", e);
                s.before_screenshot_base64 = String::new();
            }
        }
    }

    // ── Stage 2: Preprocess ──────────────────────────────────
    // Save original full screenshots before cropping (for context reference).
    let original_marked: Vec<String> = job.steps.iter().map(|s| s.screenshot_base64.clone()).collect();
    let original_before: Vec<String> = job.steps.iter().map(|s| s.before_screenshot_base64.clone()).collect();

    let preprocessed: Vec<StepInput> = job
        .steps
        .into_iter()
        .map(|mut s| {
            s.screenshot_base64 = crop_and_encode_typed(&s.screenshot_base64, s.click_x, s.click_y, s.crop_w, s.crop_h, &s.action_type);
            s.before_screenshot_base64 = crop_and_encode_typed(&s.before_screenshot_base64, s.click_x, s.click_y, s.crop_w, s.crop_h, &s.action_type);
            s.title = sanitize::sanitize_for_ai(&s.title);
            s.description = sanitize::sanitize_for_ai(&s.description);
            // B6: window_title 此前漏洗，与 title/description 一同清洗后才会进入 prompt
            s.window_title = sanitize::sanitize_for_ai(&s.window_title);
            s
        })
        .collect();

    // ── Stage 3: Batch call ──────────────────────────────────
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

        // Build user message with text + optional screenshot.
        // 默认投喂 after 裁剪图（crop_and_encode 处理后的 screenshot_base64）；
        // 表格软件改为投喂 after 全图（original_marked，已脱敏），让模型看清
        // 顶部选项卡 / 公式栏 / 函数对话框（见 ai_image_for_step）。
        // before 全图/裁剪图（original_before / step.before_screenshot_base64）不进入 prompt。
        let ai_image = ai_image_for_step(step, &original_marked[i]);
        let has_marked = !ai_image.is_empty();

        if !has_marked {
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
                serde_json::json!({
                    "type": "image_url",
                    "image_url": {
                        "url": format!("data:image/jpeg;base64,{}", ai_image),
                    },
                }),
            ];

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

/// Crop the screenshot adaptively around the focused element rect, encoding as JPEG.
///
/// Precedence:
/// 1. Element rect (crop_w/crop_h, center x/y): rect expanded by CROP_PAD,
///    enlarged to CROP_MIN_SIZE around the center if smaller, scaled down to fit
///    within CROP_MAX_SCREEN_RATIO of the screenshot while keeping aspect ratio;
/// 2. Click point only: fallback square of CROP_FALLBACK_RATIO × screen size;
/// 3. Otherwise: full image.
/// Region is clamped to image bounds; encoded at JPEG quality 85.
fn crop_and_encode(
    base64_str: &str,
    click_x: Option<i32>,
    click_y: Option<i32>,
    crop_w: Option<i32>,
    crop_h: Option<i32>,
) -> String {
    crop_and_encode_typed(base64_str, click_x, click_y, crop_w, crop_h, "click")
}

/// `ai_crop_region` 的结果。三种状态语义不同，不能合并：
/// - `Rect`：按此矩形裁剪（已 clamp 进图内）
/// - `Full`：没有可用锚点，用整张图，**仍会重编码**
/// - `Outside`：锚点完全落在图外，历史行为是**原样返回原始 base64**（连重编码都不做）。
///   这条是防 panic 的既有约定——锚点在图外时裁出来的图没有意义，见
///   `crop_and_encode_out_of_bounds_coords_no_panic`。
pub(crate) enum AiCropRegion {
    Rect(i32, i32, i32, i32),
    Full,
    Outside,
}

/// AI 裁图区域的纯几何（不碰图片字节）。
///
/// ⚠️ 本文件的常量（PAD=24 / MIN=280 / 缩到屏幕 90%）与导出端、前端**刻意不同**——
/// 喂给 AI 的图需要更大上下文和最小尺寸保证。但 type 步骤「左扩 1/3 宽」的
/// 几何推导是同一套，**左移量必须是 extra/2 而不是 extra**，否则右侧留白变成
/// `PAD - extra/2`，元素宽 > 144px 时右边缘被切。
/// 详见 `export/commands.rs::export_crop_box` 的注释与前端 `src/lib/cropAnchor.ts`。
pub(crate) fn ai_crop_region(
    img_w: i32,
    img_h: i32,
    click_x: Option<i32>,
    click_y: Option<i32>,
    crop_w: Option<i32>,
    crop_h: Option<i32>,
    action_type: &str,
) -> AiCropRegion {
    if img_w <= 0 || img_h <= 0 {
        return AiCropRegion::Outside;
    }
    let raw = match (click_x, click_y, crop_w, crop_h) {
        // 1. Element rect anchored adaptive crop
        (Some(x), Some(y), Some(cw), Some(ch))
            if x >= 0 && y >= 0 && cw > 0 && ch > 0 =>
        {
            let mut rw = (cw + CROP_PAD * 2).max(CROP_MIN_SIZE);
            let mut rh = (ch + CROP_PAD * 2).max(CROP_MIN_SIZE);
            // Scale down (never up) to fit within 90% of screen, keep aspect ratio.
            //
            // ⚠️ 缩放的**下限是元素本身**，不是 90% 屏幕：元素大到超过 90% 屏幕时，
            // 无条件缩到 90% 会把元素边缘切掉——AI 就拿着一张被截断的图写描述
            // （修复前实测：4K 全屏元素左右各切 164px、上下各切 78px）。
            // 故缩放上限取 `max(90% 屏幕, 落库值)`：小元素照旧缩，大元素不缩。
            let max_w = (img_w as f32 * CROP_MAX_SCREEN_RATIO).max(cw as f32);
            let max_h = (img_h as f32 * CROP_MAX_SCREEN_RATIO).max(ch as f32);
            let scale = (1.0f32).min(max_w / rw as f32).min(max_h / rh as f32);
            rw = (rw as f32 * scale).round().max(1.0) as i32;
            rh = (rh as f32 * scale).round().max(1.0) as i32;
            // ⚠️ 等比缩放会**连累**另一个方向：scale 取两方向的最小值，限制更紧的
            // 那一方会把另一方也拉到比元素还小（实测 1920 宽的框被拉成 1884，
            // 反而切掉 1900 宽的元素 16px）。所以缩放后再分别抬回元素尺寸。
            // 「不切元素」优先级高于「保持宽高比」——裁图框的宽高比本来就不影响
            // 输出是否变形（裁多大就是多大，不拉伸）。
            // `cw/ch` 是落库值，已被 `.min(图宽/高)` 截过，故抬回去也不会越界。
            rw = rw.max(cw);
            rh = rh.max(ch);
            // 见测试 `both_consumers_must_contain_the_visible_element_rect`。
            // Stage C: type 步骤左侧多留原始矩形宽的 1/3，把左侧 label 带进裁图。
            // ⚠️ 是 `- extra / 2` 而不是 `- extra`：框宽 rw 本身已含 extra，居中项
            // `x - rw/2` 天然在左右各分到 extra/2。多减一个 extra/2 会过冲，
            // 使右侧留白变成 PAD - extra/2（cw=300 时切掉 26px 右边缘）。
            if action_type == "type" {
                let extra = ((cw as f64) / 3.0).round();
                rw += extra as i32;
                let left = (x as f64 - (rw as f64) / 2.0 - extra / 2.0).round() as i32;
                (left, y - rh / 2, rw, rh)
            } else {
                (x - rw / 2, y - rh / 2, rw, rh)
            }
        }
        // 2. Click point only: fallback ~40% of screen
        (Some(x), Some(y), _, _) if x >= 0 && y >= 0 => {
            let rw = ((img_w as f32) * CROP_FALLBACK_RATIO).round().max(1.0) as i32;
            let rh = ((img_h as f32) * CROP_FALLBACK_RATIO).round().max(1.0) as i32;
            (x - rw / 2, y - rh / 2, rw, rh)
        }
        // 3. No usable anchor: full image (still re-encoded)
        _ => return AiCropRegion::Full,
    };

    let (l, t, w, h) = raw;
    let left = l.max(0);
    let top = t.max(0);
    let w = w.min(img_w - left);
    let h = h.min(img_h - top);
    if w <= 0 || h <= 0 {
        // 锚点完全在图外：调用方应原样返回原始 base64
        return AiCropRegion::Outside;
    }
    AiCropRegion::Rect(left, top, w, h)
}

/// Stage C: 按 action_type 差异化裁剪策略。
/// - scroll / nav / screenshot：整窗截图（不裁，原样返回，保留上下文）
/// - type：输入框 + 左侧 label 区（元素矩形左扩 1/3 宽，让"用户名/密码"等 label 进图）
/// - 其他（click/copy/paste/hotkey/key/shortcut 等）：元素矩形自适应（同 A+B）
fn crop_and_encode_typed(
    base64_str: &str,
    click_x: Option<i32>,
    click_y: Option<i32>,
    crop_w: Option<i32>,
    crop_h: Option<i32>,
    action_type: &str,
) -> String {
    if base64_str.is_empty() {
        return String::new();
    }

    // Stage C: 整窗类型不裁剪，原样返回
    if matches!(action_type, "scroll" | "nav" | "screenshot") {
        return base64_str.to_string();
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
    if img_w <= 0 || img_h <= 0 {
        return base64_str.to_string();
    }

    let cropped = match ai_crop_region(img_w, img_h, click_x, click_y, crop_w, crop_h, action_type)
    {
        AiCropRegion::Rect(left, top, w, h) => {
            img.crop_imm(left as u32, top as u32, w as u32, h as u32)
        }
        AiCropRegion::Full => img,
        // 锚点完全在图外：历史约定是原样返回，不做任何重编码
        AiCropRegion::Outside => return base64_str.to_string(),
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
/// Anthropic (claude-3/3.5), Google (gemini), Zhipu (glm-4v / glm-4.6v),
/// Qwen (qwen-vl / qwen3.x-plus-flash-max), Doubao (doubao-vision), Yi (yi-vision).
///
/// Vision model list is kept in sync with the extension's
/// `modelSupportsVision()` in modelEnum.ts.
fn model_supports_vision(model: &str) -> bool {
    let lower = model.to_lowercase();

    // Qwen 3.x series: qwen3.7-plus, qwen3.5-flash, qwen3.0-max etc.
    // Matches extension regex: qwen3\.[\d.]+-(plus|flash|max)
    if lower.starts_with("qwen3.")
        && (lower.ends_with("-plus") || lower.ends_with("-flash") || lower.ends_with("-max"))
    {
        return true;
    }

    let vision_keywords = [
        "gpt-4o",
        "gpt-4-vision",
        "gpt-4-turbo",
        "claude-3",
        "gemini",
        "glm-4v",
        "glm-4.6v",
        "qwen-vl",
        "doubao-vision",
        "yi-vision",
        "vision",
        "vl",
        // 混元仅视觉版支持图片（hunyuan-lite/standard/turbo 是纯文本模型）
        "hunyuan-vision",
        "multimodal",
    ];
    vision_keywords.iter().any(|kw| lower.contains(kw))
}

/// Suggest a vision-capable model for the given provider.
fn suggest_vision_model(provider_id: &str) -> String {
    match provider_id {
        "flowio-free" | "zhipu" => "「glm-4.6v-flash」".to_string(),
        "qwen" => "「qwen-vl-plus」或「qwen-vl-max」".to_string(),
        "doubao" => "「doubao-vision-pro-32k」".to_string(),
        "yi" => "「yi-vision」".to_string(),
        _ => "支持图片识别的视觉模型（如 gpt-4o、claude-3.5-sonnet 等）".to_string(),
    }
}

#[cfg(test)]
mod pipeline_tests {
    use super::*;

    fn b64(bytes: &[u8]) -> String {
        base64::engine::general_purpose::STANDARD.encode(bytes)
    }

    fn solid_png(w: u32, h: u32, rgb: [u8; 3]) -> Vec<u8> {
        let img = image::RgbImage::from_pixel(w, h, image::Rgb(rgb));
        let mut buf = Vec::new();
        image::DynamicImage::ImageRgb8(img)
            .write_to(&mut std::io::Cursor::new(&mut buf), image::ImageFormat::Png)
            .unwrap();
        buf
    }

    fn decode(s: &str) -> Vec<u8> {
        base64::engine::general_purpose::STANDARD.decode(s).unwrap()
    }

    fn step_with_window(wt: &str, shot: &str) -> StepInput {
        StepInput {
            title: String::new(),
            screenshot_base64: shot.to_string(),
            before_screenshot_base64: String::new(),
            description: String::new(),
            click_x: None,
            click_y: None,
            crop_w: None,
            crop_h: None,
            action_type: String::new(),
            window_title: wt.to_string(),
        }
    }

    #[test]
    fn ai_image_for_step_prefers_full_image_for_spreadsheet() {
        let full = "FULL_REDACTED_BASE64".to_string();
        let cropped = "CROPPED_BASE64".to_string();
        let step = step_with_window("销售表.xlsx - WPS 表格", &cropped);
        assert_eq!(
            ai_image_for_step(&step, &full),
            "FULL_REDACTED_BASE64",
            "表格软件应送全图而非裁剪图"
        );
    }

    #[test]
    fn ai_image_for_step_keeps_crop_for_non_spreadsheet() {
        let full = "FULL_REDACTED_BASE64".to_string();
        let cropped = "CROPPED_BASE64".to_string();
        let step = step_with_window("记事本", &cropped);
        assert_eq!(
            ai_image_for_step(&step, &full),
            "CROPPED_BASE64",
            "非表格软件仍应送裁剪图"
        );
    }

    #[test]
    fn ai_image_for_step_falls_back_to_crop_when_full_empty() {
        let full = String::new();
        let cropped = "CROPPED_BASE64".to_string();
        let step = step_with_window("Excel - 工作簿1", &cropped);
        assert_eq!(
            ai_image_for_step(&step, &full),
            "CROPPED_BASE64",
            "全图为空时即使表格软件也回退到裁剪图"
        );
    }

    // ── detect_window_change ──────────────────────────────────

    #[test]
    fn detect_window_change_empty_strings() {
        assert!(!detect_window_change("", ""));
        assert!(!detect_window_change("", "abc"));
        assert!(!detect_window_change("abc", ""));
    }

    #[test]
    fn detect_window_change_invalid_base64() {
        assert!(!detect_window_change("!!!not-base64!!!", &b64(&[0u8; 60])));
        assert!(!detect_window_change(&b64(&[0u8; 60]), "!!!not-base64!!!"));
    }

    #[test]
    fn detect_window_change_same_bytes() {
        let s = b64(&[42u8; 300]);
        assert!(!detect_window_change(&s, &s));
    }

    #[test]
    fn detect_window_change_big_diff_true() {
        let a = b64(&[0u8; 90]);
        let b = b64(&[255u8; 90]);
        assert!(detect_window_change(&a, &b));
    }

    #[test]
    fn detect_window_change_small_diff_false() {
        let a = b64(&[0u8; 90]);
        let b = b64(&[10u8; 90]); // 平均差 10 ≤ 60
        assert!(!detect_window_change(&a, &b));
    }

    #[test]
    fn detect_window_change_threshold_exact_60_false() {
        let a = b64(&[0u8; 90]);
        let b = b64(&[60u8; 90]); // 平均差 60，>60 才 true
        assert!(!detect_window_change(&a, &b));
    }

    #[test]
    fn detect_window_change_unequal_lengths_no_panic() {
        let a = b64(&[0u8; 300]);
        let b = b64(&[255u8; 31]); // min len = 31，采样 idx 0、30 两个点
        assert!(detect_window_change(&a, &b)); // 不 panic 且能得出结果
    }

    #[test]
    fn detect_window_change_stride_boundary_single_byte() {
        let a = b64(&[0u8]);
        let b = b64(&[255u8]);
        assert!(detect_window_change(&a, &b));
    }

    #[test]
    fn detect_window_change_zero_len_bytes() {
        assert!(!detect_window_change(&b64(&[]), &b64(&[])));
    }

    // ── model_supports_vision ─────────────────────────────────

    #[test]
    fn model_supports_vision_known_vision_models() {
        for m in [
            "gpt-4o",
            "gpt-4o-mini",
            "gpt-4-vision-preview",
            "gpt-4-turbo",
            "claude-3-opus",
            "claude-3.5-sonnet",
            "gemini-1.5-pro",
            "gemini-2.0-flash",
            "glm-4v",
            "glm-4v-plus",
            "glm-4.6v-flash",
            "qwen-vl-plus",
            "qwen-vl-max",
            // Qwen 3.x coding/vision models (aligned with extension's vision regex)
            "qwen3.7-plus",
            "qwen3.5-flash",
            "qwen3.0-max",
            "doubao-vision-pro-32k",
            "yi-vision",
            "hunyuan-vision",
            "openai/gpt-4o-2024-11-20",
        ] {
            assert!(model_supports_vision(m), "expected vision: {}", m);
        }
    }

    #[test]
    fn model_supports_vision_plain_models_false() {
        for m in [
            "gpt-3.5-turbo",
            "deepseek-chat",
            "deepseek-reasoner",
            "qwen-plus",
            "glm-4",
            "glm-4-flash",
            "claude-2",
            "hunyuan-lite",
            "text-davinci-003",
            "ernie-4.0-turbo-8k",
            // qwen3.x without vision suffix = text-only
            "qwen3.7",
            "qwen3",
        ] {
            assert!(!model_supports_vision(m), "expected no vision: {}", m);
        }
    }

    #[test]
    fn model_supports_vision_case_insensitive() {
        assert!(model_supports_vision("GPT-4O"));
        assert!(model_supports_vision("Claude-3.5-Sonnet"));
        assert!(model_supports_vision("Gemini-Pro"));
        assert!(!model_supports_vision("DeepSeek-Chat"));
    }

    #[test]
    fn model_supports_vision_contains_match() {
        assert!(model_supports_vision("openai/gpt-4o-2024-11-20"));
        assert!(model_supports_vision("moonshot-vision-8k"));
    }

    // ── suggest_vision_model ──────────────────────────────────

    #[test]
    fn suggest_vision_model_branches() {
        assert_eq!(suggest_vision_model("flowio-free"), "「glm-4.6v-flash」");
        assert_eq!(suggest_vision_model("zhipu"), "「glm-4.6v-flash」");
        assert!(suggest_vision_model("qwen").contains("qwen-vl"));
        assert!(suggest_vision_model("doubao").contains("doubao-vision"));
        assert_eq!(suggest_vision_model("yi"), "「yi-vision」");
        assert!(suggest_vision_model("unknown-provider").contains("gpt-4o"));
        assert!(suggest_vision_model("").contains("gpt-4o"));
    }

    // ── crop_and_encode ───────────────────────────────────────

    #[test]
    fn crop_and_encode_empty_returns_empty() {
        assert_eq!(crop_and_encode("", None, None, None, None), "");
        assert_eq!(crop_and_encode("", Some(1), Some(2), None, None), "");
    }

    #[test]
    fn crop_and_encode_invalid_base64_returns_original() {
        let s = "@@not-valid-base64@@";
        assert_eq!(crop_and_encode(s, None, None, None, None), s);
    }

    #[test]
    fn crop_and_encode_valid_png_no_coords_full_jpeg() {
        let png = solid_png(64, 64, [10, 20, 30]);
        let out = crop_and_encode(&b64(&png), None, None, None, None);
        assert!(!out.is_empty());
        assert_ne!(out, b64(&png)); // JPEG 而非 PNG 原样
        let bytes = decode(&out);
        assert_eq!(&bytes[..2], &[0xFF, 0xD8], "should be JPEG");
        let img = image::load_from_memory(&bytes).unwrap();
        assert_eq!((img.width(), img.height()), (64, 64));
    }

    #[test]
    fn crop_and_encode_click_only_fallback_40pct() {
        // 无元素矩形：以点击点为中心裁屏幕约 40%（1000*0.4=400）
        let png = solid_png(1000, 1000, [50, 60, 70]);
        let out = crop_and_encode(&b64(&png), Some(500), Some(500), None, None);
        let img = image::load_from_memory(&decode(&out)).unwrap();
        assert_eq!((img.width(), img.height()), (400, 400));
    }

    #[test]
    fn crop_and_encode_rect_adaptive_min_size() {
        // 小矩形 (200x100) + padding → 补足到最小 280×280（保持中心）
        let png = solid_png(1000, 1000, [50, 60, 70]);
        let out = crop_and_encode(&b64(&png), Some(500), Some(500), Some(200), Some(100));
        let img = image::load_from_memory(&decode(&out)).unwrap();
        assert_eq!((img.width(), img.height()), (280, 280));
    }

    #[test]
    fn crop_and_encode_rect_scales_down_keeps_ratio() {
        // 大矩形超出屏幕 90% → 等比缩放保留宽高比。
        //
        // ⚠️ 用例必须是**元素仍在屏幕内**的场景（870 < 1000）：等比缩放的约束
        // 只在「元素放得下」时才有意义。元素大到超出屏幕时走另一条规则
        // ——不缩、不切，见 `crop_and_encode_rect_larger_than_screen_not_cropped`。
        let png = solid_png(1000, 1000, [3, 4, 5]);
        let out = crop_and_encode(&b64(&png), Some(500), Some(500), Some(870), Some(400));
        let img = image::load_from_memory(&decode(&out)).unwrap();
        // rw=918 rh=448 → scale=min(1, 900/918, 900/448)=0.98039… → 900×439
        // 比例 918:448 ≈ 2.05 与 900:439 ≈ 2.05 一致（等比）
        assert_eq!((img.width(), img.height()), (900, 439));
    }

    #[test]
    fn crop_and_encode_rect_larger_than_screen_not_cropped() {
        // 元素比屏幕还大（500x300 落在 400x300 的图里）：可见部分就是整张图，
        // 此时「缩到 90% 保持聚焦」没有意义——缩了只会把元素切掉，AI 会拿着
        // 一张被截断的图写描述。修复前这里裁出 360×229（左右各丢 20px）。
        let png = solid_png(400, 300, [3, 4, 5]);
        let out = crop_and_encode(&b64(&png), Some(200), Some(150), Some(500), Some(300));
        let img = image::load_from_memory(&decode(&out)).unwrap();
        assert_eq!(
            (img.width(), img.height()),
            (400, 300),
            "元素超出屏幕时应裁整图，不能缩到把元素切掉"
        );
    }

    #[test]
    fn crop_and_encode_coords_clamped_at_edge() {
        // 点击 (0,0) 无矩形：fallback 300*0.4=120 → 120×120，以 (0,0) 为左上
        let png = solid_png(300, 300, [1, 2, 3]);
        let out = crop_and_encode(&b64(&png), Some(0), Some(0), None, None);
        let img = image::load_from_memory(&decode(&out)).unwrap();
        assert_eq!((img.width(), img.height()), (120, 120));
    }

    #[test]
    fn crop_and_encode_partial_negative_coord_falls_back_full() {
        // x 有效但 y 为负 → 走全图分支
        let png = solid_png(100, 100, [7, 8, 9]);
        let out = crop_and_encode(&b64(&png), Some(50), Some(-1), None, None);
        let img = image::load_from_memory(&decode(&out)).unwrap();
        assert_eq!((img.width(), img.height()), (100, 100));
    }

    #[test]
    fn crop_and_encode_out_of_bounds_coords_no_panic() {
        // 修复前：x=5000 超出 100x100 图片 → left=4800 > img_w → w 为负 → crop_imm panic
        let png = solid_png(100, 100, [9, 9, 9]);
        let out = crop_and_encode(&b64(&png), Some(5000), Some(5000), None, None);
        // 修复后：裁剪区域完全在图片外 → 原样返回
        assert_eq!(out, b64(&png));
    }

    #[test]
    fn crop_and_encode_one_axis_out_of_bounds_no_panic() {
        let png = solid_png(100, 100, [11, 12, 13]);
        let out = crop_and_encode(&b64(&png), Some(5000), Some(50), None, None);
        // x 方向完全出界 → 修复后原样返回
        assert_eq!(out, b64(&png));
    }

    #[test]
    fn crop_and_encode_typed_type_expands_left_label_zone() {
        // type 步骤：矩形左扩 cw/3=33px → 比 click 更宽（不裁剪时左侧多出 label 区）
        let png = solid_png(800, 600, [9, 8, 7]);
        let base = crop_and_encode_typed(&b64(&png), Some(400), Some(300), Some(100), Some(40), "click");
        let typed = crop_and_encode_typed(&b64(&png), Some(400), Some(300), Some(100), Some(40), "type");
        let base_img = image::load_from_memory(&decode(&base)).unwrap();
        let typed_img = image::load_from_memory(&decode(&typed)).unwrap();
        assert!(typed_img.width() > base_img.width());
    }

    /* ── 裁图几何（ai_crop_region）───────────────────────────────
     * 本端常量（PAD=24 / MIN=280 / 缩到 90%）与导出端、前端**刻意不同**，
     * 但 type 步骤「左扩 1/3 宽」的几何推导是同一套 —— 左移量必须是 extra/2。
     * 旧写法 `- extra` 的过冲阈值更高（PAD=24 → 元素宽 > 144px 才被切），
     * 所以下面用 cw=300 这种常见输入框宽度来锁定。
     *
     * 场景：800x600 截图，元素矩形中心 (400,300)、尺寸 300x40
     *   rw = (300+48).max(280) = 348, rh = (40+48).max(280) = 280
     *   scale = min(1.0, 720/348, 540/280) = 1.0（不缩）
     *   type : extra = 100, rw = 448, left = 400-224-50 = 126, top = 300-140 = 160
     *   click: left = 400-174 = 226, top = 160
     */

    /// 取出 Rect 分支，否则 panic —— 让断言保持简洁
    fn rect(r: AiCropRegion) -> (i32, i32, i32, i32) {
        match r {
            AiCropRegion::Rect(l, t, w, h) => (l, t, w, h),
            AiCropRegion::Full => panic!("期望 Rect，实际 Full"),
            AiCropRegion::Outside => panic!("期望 Rect，实际 Outside"),
        }
    }

    #[test]
    fn ai_crop_region_golden_type_and_click() {
        assert_eq!(
            rect(ai_crop_region(
                800, 600, Some(400), Some(300), Some(300), Some(40), "type"
            )),
            (126, 160, 448, 280)
        );
        assert_eq!(
            rect(ai_crop_region(
                800, 600, Some(400), Some(300), Some(300), Some(40), "click"
            )),
            (226, 160, 348, 280)
        );
    }

    #[test]
    fn ai_crop_region_type_never_clips_element_right_edge() {
        // 核心防线：元素矩形须完整落在框内，四边留白 >= CROP_PAD。
        // 旧写法 `- extra` 在 cw=300 时会把右边缘切掉 26px。
        for cw in [24, 60, 100, 150, 200, 300, 480, 600, 900] {
            for ch in [24, 40, 80, 200] {
                for at in ["type", "click"] {
                    let (l, t, w, h) = rect(ai_crop_region(
                        1920, 1080, Some(960), Some(540), Some(cw), Some(ch), at,
                    ));
                    let (el, et) = (960 - cw / 2, 540 - ch / 2);
                    let pads = [
                        ("左", el - l),
                        ("右", (l + w) - (el + cw)),
                        ("上", et - t),
                        ("下", (t + h) - (et + ch)),
                    ];
                    for (name, p) in pads {
                        assert!(
                            p >= CROP_PAD,
                            "{at} {cw}x{ch} 元素被切：{name}侧留白 {p}px < CROP_PAD({CROP_PAD})"
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn ai_crop_region_type_keeps_label_zone_on_left() {
        let (l, _t, w, _h) = rect(ai_crop_region(
            800, 600, Some(400), Some(300), Some(300), Some(40), "type",
        ));
        let (elem_left, elem_right) = (250, 550);
        assert_eq!((l + w) - elem_right, CROP_PAD, "右侧留白应恰为 CROP_PAD");
        assert_eq!(
            elem_left - l,
            CROP_PAD + 100,
            "左侧留白应为 CROP_PAD + extra(100)"
        );
    }

    #[test]
    fn ai_crop_region_fallback_and_full_cases() {
        // 只有点击点：屏幕 40%
        assert_eq!(
            rect(ai_crop_region(
                800, 600, Some(400), Some(300), None, None, "click"
            )),
            (240, 180, 320, 240)
        );
        // 无坐标 → Full（用全图，但会重编码）
        assert!(
            matches!(
                ai_crop_region(800, 600, None, None, None, None, "click"),
                AiCropRegion::Full
            ),
            "无锚点应为 Full"
        );
        // 退化尺寸 / 锚点完全在图外 → Outside（原样返回，不重编码）
        assert!(
            matches!(
                ai_crop_region(0, 0, Some(1), Some(1), Some(2), Some(2), "click"),
                AiCropRegion::Outside
            ),
            "退化图尺寸应为 Outside"
        );
        assert!(
            matches!(
                ai_crop_region(100, 100, Some(5000), Some(5000), None, None, "click"),
                AiCropRegion::Outside
            ),
            "锚点完全在图外应为 Outside（防 panic 的历史约定）"
        );
    }

    #[test]
    fn ai_crop_region_stays_within_image_bounds() {
        for (cx, cy) in [(20, 20), (960, 540), (1900, 1060), (0, 0)] {
            for at in ["type", "click"] {
                if let AiCropRegion::Rect(l, t, w, h) =
                    ai_crop_region(1920, 1080, Some(cx), Some(cy), Some(300), Some(40), at)
                {
                    assert!(l >= 0 && t >= 0, "{at} 负坐标 ({cx},{cy}) -> ({l},{t})");
                    assert!(
                        l + w <= 1920 && t + h <= 1080,
                        "{at} 越界 ({cx},{cy}) -> {l},{t},{w},{h}"
                    );
                    assert!(w > 0 && h > 0, "{at} 退化尺寸 ({cx},{cy}) -> {w}x{h}");
                }
            }
        }
    }

    #[test]
    fn ai_crop_region_applies_min_size_for_tiny_elements() {
        // 极小元素：框被放大到 CROP_MIN_SIZE，元素自然居中
        let (l, t, w, h) = rect(ai_crop_region(
            800, 600, Some(400), Some(300), Some(10), Some(10), "click",
        ));
        assert_eq!(w, CROP_MIN_SIZE.min(800));
        assert_eq!(h, CROP_MIN_SIZE.min(600));
        assert!(l >= 0 && t >= 0 && l + w <= 800 && t + h <= 600);
    }

    #[test]
    fn crop_and_encode_typed_whole_window_types_return_original() {
        // scroll/nav/screenshot 整窗：返回原 PNG base64，不重编码
        let png = solid_png(100, 80, [1, 2, 3]);
        for at in ["scroll", "nav", "screenshot"] {
            let out = crop_and_encode_typed(&b64(&png), Some(50), Some(40), Some(20), Some(10), at);
            assert_eq!(out, b64(&png));
        }
    }

    /* ── 跨消费端一致性：导出端 vs AI 端 ────────────────────────────
     * 存库锚点由录制端 recorder/commands.rs 生成（它 bake 了自己的 CROP_PAD=24
     * 与 CROP_MIN 420x280），之后被两个消费端**各自独立解释**：
     *   - 导出端 export_crop_box   （PAD=8，贴边平移不缩小）
     *   - AI 端   ai_crop_region   （PAD=24 / MIN=280 / 缩到屏幕 90%）
     *
     * AI 端的常量差异是**刻意**的，声明的意图见 ai_crop_region 的注释：
     * 「喂给 AI 的图需要更大上下文和最小尺寸保证」。这句话可证伪，它蕴含
     * 一条不变式：**AI 看到的区域不该比用户/导出看到的更小**。
     * 若某场景下 AI 框反而更小，AI 就是拿着一张比用户所见更窄的图在写描述。
     */

    /// 复刻录制端存库逻辑（recorder/commands.rs：CROP_PAD=24 / CROP_MIN_W=420 / CROP_MIN_H=280）。
    /// 返回落库的 (x, y, crop_w, crop_h)。
    fn stored_anchor(
        img_w: i32,
        img_h: i32,
        elem_w: i32,
        elem_h: i32,
        cx: i32,
        cy: i32,
    ) -> (i32, i32, i32, i32) {
        const REC_PAD: i32 = 24;
        const REC_MIN_W: i32 = 420;
        const REC_MIN_H: i32 = 280;
        let bw = (elem_w + REC_PAD * 2).max(1).max(REC_MIN_W).min(img_w);
        let bh = (elem_h + REC_PAD * 2).max(1).max(REC_MIN_H).min(img_h);
        let x0 = (cx - bw / 2).clamp(0, img_w - bw);
        let y0 = (cy - bh / 2).clamp(0, img_h - bh);
        (x0 + bw / 2, y0 + bh / 2, bw, bh)
    }

    #[test]
    fn both_consumers_must_contain_the_visible_element_rect() {
        use crate::export::commands::export_crop_box;

        let cases = vec![
            // (img_w, img_h, 元素宽, 元素高, action_type, 场景)
            (1920, 1080, 40, 20, "click", "小按钮"),
            (1920, 1080, 300, 40, "click", "输入框"),
            (1920, 1080, 300, 40, "type", "输入框(type)"),
            (1920, 1080, 600, 400, "click", "大文本域"),
            (1920, 1080, 1800, 60, "click", "整行"),
            (1920, 1080, 1900, 1050, "click", "接近全屏"),
            (3840, 2160, 80, 40, "click", "4K 小按钮"),
            (3840, 2160, 600, 80, "click", "4K 输入框"),
            (3840, 2160, 3600, 2100, "click", "4K 接近全屏"),
            (1366, 768, 300, 40, "click", "低分辨率输入框"),
        ];

        println!(
            "\n{:<16} {:<6} {:>9} {:>10} | {:>11} {:>11} | {:>7} {:>8} {:>8}",
            "场景", "type", "元素", "落库值", "导出框", "AI 框", "AI/导出", "导出切掉", "AI 切掉"
        );
        let mut violations: Vec<String> = Vec::new();

        for (iw, ih, ew, eh, at, name) in cases {
            let (cx, cy) = (iw / 2, ih / 2);
            let (sx, sy, sw, sh) = stored_anchor(iw, ih, ew, eh, cx, cy);

            // 元素在图内的**可见**范围（元素可能比屏幕还大，超出部分不算被切）
            let vis_l = (cx - ew / 2).max(0);
            let vis_r = (cx + ew / 2).min(iw);
            let vis_t = (cy - eh / 2).max(0);
            let vis_b = (cy + eh / 2).min(ih);

            let (ex, ey, ew2, eh2) =
                match export_crop_box(iw, ih, Some(sx), Some(sy), Some(sw), Some(sh), at) {
                    Some(v) => v,
                    None => {
                        println!("{name}: 导出端不裁剪（整窗类型）");
                        continue;
                    }
                };
            let (ax, ay, aw, ah) =
                match ai_crop_region(iw, ih, Some(sx), Some(sy), Some(sw), Some(sh), at) {
                    AiCropRegion::Rect(l, t, w, h) => (l, t, w, h),
                    _ => {
                        println!("{name}: AI 端 Full/Outside");
                        continue;
                    }
                };

            // 某条边被切掉多少像素（0 = 完整包含）。
            // 方向：框界**越过了**元素界才算切 —— 框比元素大（有余量）不算。
            let cut = |l: i32, t: i32, w: i32, h: i32| -> i32 {
                let cut_left = (l - vis_l).max(0); // 框左界在元素左界右侧
                let cut_right = (vis_r - (l + w)).max(0); // 框右界在元素右界左侧
                let cut_top = (t - vis_t).max(0);
                let cut_bottom = (vis_b - (t + h)).max(0);
                cut_left + cut_right + cut_top + cut_bottom
            };
            let cut_export = cut(ex, ey, ew2, eh2);
            let cut_ai = cut(ax, ay, aw, ah);

            let ratio = (aw as f64 * ah as f64) / (ew2 as f64 * eh2 as f64);
            println!(
                "{:<16} {:<6} {:>9} {:>10} | {:>11} {:>11} | {:>6.2}x {:>8} {:>8}",
                name,
                at,
                format!("{ew}x{eh}"),
                format!("{sw}x{sh}"),
                format!("{ew2}x{eh2}"),
                format!("{aw}x{ah}"),
                ratio,
                if cut_export > 0 {
                    format!("{cut_export}px")
                } else {
                    "—".to_string()
                },
                if cut_ai > 0 {
                    format!("{cut_ai}px")
                } else {
                    "—".to_string()
                }
            );

            if cut_export > 0 {
                violations.push(format!(
                    "{name}（{iw}x{ih} {at}，元素 {ew}x{eh}）：导出框 {ew2}x{eh2}@({ex},{ey}) 切掉元素可见部分 {cut_export}px"
                ));
            }
            if cut_ai > 0 {
                violations.push(format!(
                    "{name}（{iw}x{ih} {at}，元素 {ew}x{eh}）：AI 框 {aw}x{ah}@({ax},{ay}) 切掉元素可见部分 {cut_ai}px —— AI 会拿着被截断的图写描述"
                ));
            }

            // 缩放必须保持宽高比（ai_crop_region 的注释承诺 keep aspect ratio）。
            // 只对「框没被 clamp 到图边界」且非 type 的场景断言：
            //   - 触界意味着尺寸已被图截断，比例本来就不由缩放决定；
            //   - type 步骤还要左扩 extra，宽高比必然改变。
            // ⚠️ 这条断言是 `max_w/max_h` 里 `.max(cw/ch)` 的专属防线：
            //    只靠缩放后的 `rw.max(cw)` 虽能避免切元素，却会让比例失真
            //    （实测 1848×299，比例偏离 7%）。
            let touched_edge = ax <= 0 || ay <= 0 || ax + aw >= iw || ay + ah >= ih;
            if at != "type" && !touched_edge {
                let rw0 = (sw + CROP_PAD * 2).max(CROP_MIN_SIZE) as f64;
                let rh0 = (sh + CROP_PAD * 2).max(CROP_MIN_SIZE) as f64;
                let want = rw0 / rh0;
                let got = aw as f64 / ah as f64;
                let dev = (got - want).abs() / want;
                if dev > 0.02 {
                    violations.push(format!(
                        "{name}（{iw}x{ih} {at}）：AI 框 {aw}x{ah} 宽高比 {got:.3} 偏离缩放前 {want:.3} 达 {:.1}% —— 缩放没保持等比",
                        dev * 100.0
                    ));
                }
            }
        }

        assert!(
            violations.is_empty(),
            "裁图框没盖住元素的可见部分：\n  - {}",
            violations.join("\n  - ")
        );
    }
}
