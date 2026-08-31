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

        // Build user message with text + optional screenshot
        // 只投喂 after 裁剪图（crop_and_encode 处理后的 screenshot_base64）一张图；
        // before 全图/裁剪图（original_before / step.before_screenshot_base64）与
        // after 全图（original_marked）不再进入 prompt。
        let has_marked = !step.screenshot_base64.is_empty();

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
                        "url": format!("data:image/jpeg;base64,{}", step.screenshot_base64),
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

    // Determine crop region
    let region: Option<(i32, i32, i32, i32)> = match (click_x, click_y, crop_w, crop_h) {
        // 1. Element rect anchored adaptive crop
        (Some(x), Some(y), Some(cw), Some(ch))
            if x >= 0 && y >= 0 && cw > 0 && ch > 0 =>
        {
            let mut rw = cw + CROP_PAD * 2;
            let mut rh = ch + CROP_PAD * 2;
            rw = rw.max(CROP_MIN_SIZE);
            rh = rh.max(CROP_MIN_SIZE);
            // Scale down (never up) to fit within 90% of screen, keep aspect ratio
            let scale = (1.0f32)
                .min((img_w as f32 * CROP_MAX_SCREEN_RATIO) / rw as f32)
                .min((img_h as f32 * CROP_MAX_SCREEN_RATIO) / rh as f32);
            rw = (rw as f32 * scale).round().max(1.0) as i32;
            rh = (rh as f32 * scale).round().max(1.0) as i32;
            // Stage C: type 步骤左扩原始矩形宽的 1/3，把左侧 label（如"用户名/密码"）带进裁剪图
            if action_type == "type" {
                let extra = cw / 3;
                rw += extra;
                Some((x - rw / 2 - extra, y - rh / 2, rw, rh))
            } else {
                Some((x - rw / 2, y - rh / 2, rw, rh))
            }
        }
        // 2. Click point only: fallback ~40% of screen
        (Some(x), Some(y), _, _) if x >= 0 && y >= 0 => {
            let rw = ((img_w as f32) * CROP_FALLBACK_RATIO).round().max(1.0) as i32;
            let rh = ((img_h as f32) * CROP_FALLBACK_RATIO).round().max(1.0) as i32;
            Some((x - rw / 2, y - rh / 2, rw, rh))
        }
        // 3. No usable anchor: full image
        _ => None,
    };

    let cropped = match region {
        Some((l, t, w, h)) => {
            let left = l.max(0);
            let top = t.max(0);
            let w = w.min(img_w - left);
            let h = h.min(img_h - top);
            if w <= 0 || h <= 0 {
                // Crop region fully outside image: return original unchanged
                return base64_str.to_string();
            }
            img.crop_imm(left as u32, top as u32, w as u32, h as u32)
        }
        None => img,
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
        // 混元仅视觉版支持图片（hunyuan-lite/standard/turbo 是纯文本模型）
        "hunyuan-vision",
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
            "qwen-vl-plus",
            "qwen-vl-max",
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
        assert_eq!(suggest_vision_model("zhipu"), "「glm-4v」");
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
        // 大矩形超出屏幕 90% → 等比缩放保留宽高比
        let png = solid_png(400, 300, [3, 4, 5]);
        let out = crop_and_encode(&b64(&png), Some(200), Some(150), Some(500), Some(300));
        let img = image::load_from_memory(&decode(&out)).unwrap();
        // rw=548 rh=348 → scale=min(1, 360/548, 270/348)=0.6569… → 360×229
        assert_eq!((img.width(), img.height()), (360, 229));
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

    #[test]
    fn crop_and_encode_typed_whole_window_types_return_original() {
        // scroll/nav/screenshot 整窗：返回原 PNG base64，不重编码
        let png = solid_png(100, 80, [1, 2, 3]);
        for at in ["scroll", "nav", "screenshot"] {
            let out = crop_and_encode_typed(&b64(&png), Some(50), Some(40), Some(20), Some(10), at);
            assert_eq!(out, b64(&png));
        }
    }
}
