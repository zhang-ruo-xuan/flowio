use base64::Engine;
use crate::ai::domain::AppDomain;
use crate::ai::http::{self, Message};
use crate::ai::parse;
use crate::ai::prompt;
use crate::ai::sanitize;
use crate::ai::types::{AiConfig, AiJob, AiResult, StepInput, StepOutput};

/// JPEG quality for cropped screenshots (85 = good balance of size and fidelity).
const CROP_JPEG_QUALITY: u8 = 85;

/// Choose which screenshot to send to the vision model for a given step.
///
/// Spreadsheet apps (WPS 表格 / Excel / LibreOffice Calc / Numbers …) place the
/// relevant UI — the ribbon, formula bar and function dialog — across the whole
/// window, so a focused 400×400 crop often hides it. For those steps we send the
/// full (already redacted) screenshot instead, trading more tokens for accuracy.
/// Non-spreadsheet steps keep the cheaper focused crop. `force_domain`
/// additionally triggers the forced domain's behavior when the user manually
/// flagged the recording (even if the window title doesn't contain a keyword).
fn ai_image_for_step<'a>(step: &'a StepInput, full_marked: &'a str, force_domain: Option<AppDomain>) -> &'a str {
    let domain = force_domain.unwrap_or_else(|| crate::ai::domain::detect_domain(&step.window_title));
    if domain.full_image() && !full_marked.is_empty() {
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
    // 领域模式：window_title 命中关键词自动判定领域，或用户手动标记（domain_mode）。
    // forced 模式（domain_mode 非 None）对全部分步强制使用指定领域。
    let mut force_domain: Option<AppDomain> = job
        .domain_mode
        .as_deref()
        .map(crate::ai::domain::AppDomain::parse);
    // 录制指定软件：无论窗口标题如何，强制「整软件全图」喂图策略。
    // 用专属 TargetApp 域（full_image/skip_redaction/before_after 全开）而非
    // Spreadsheet——避免把任意软件（VS Code/Chrome…）误当成表格、硬塞「表格软件」
    // 专项提示；step hint 改由各步真实窗口标题检测出的领域给出。
    if job.target_app.is_some() {
        force_domain = Some(AppDomain::TargetApp);
    }
    for s in &mut job.steps {
        // 整软件全图模式（target_app）与表格软件：跳过 OCR 脱敏模糊，让 AI 看清
        // 完整画面文字（表格的单元格/函数名/数据表取值，或指定软件的整体界面）。
        // ⚠️ 隐私权衡：整图原样送往视觉模型——仅建议用于非敏感演示数据。
        // ⚠️ 隐私权衡：表格内容会原样送往视觉模型——仅建议用于非敏感演示数据；
        // 若含手机号/身份证等真实 PII，仍应由用户自行脱敏后再录制。
        let dom = force_domain.unwrap_or_else(|| crate::ai::domain::detect_domain(&s.window_title));
        if dom.skip_redaction() {
            log::info!("[ai] 跳过脱敏（领域={}）：{}", dom, s.window_title);
            continue;
        }
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
        let mut step_prompt = prompt::build_step_prompt(step, step_number, force_domain, job.target_app.as_deref());

        // Detect dramatic window change between before/after screenshots
        // and prepend a hard override instruction when detected.
        if detect_window_change(&original_before[i], &original_marked[i]) {
            step_prompt = format!(
                "【强制指令】此步骤的 before 和 after 截图完全不同——用户执行的是窗口管理操作（关闭窗口、切换窗口、打开新应用等），而不是在 before 截图的界面上操作。你必须以录制的原始动作类型和原始标题为准来撰写描述，严禁根据 before 截图中的网页内容猜测操作（如投稿、点赞等）。\n\n{}",
                step_prompt
            );
        }

        // 整软件全图 / 表格软件：若同时有 before/after 截图，提示模型对比前后状态。
        let dom = force_domain.unwrap_or_else(|| crate::ai::domain::detect_domain(&step.window_title));
        if dom.before_after() && !original_before[i].is_empty() {
            let ba_hint = if matches!(dom, AppDomain::TargetApp) {
                "【整软件】你将收到两张截图：第一张为操作前、第二张为操作后，请对比前后状态判断操作。"
            } else {
                "【表格软件】你将收到两张截图：第一张为操作前、第二张为操作后，请对比前后状态判断操作（如某区域被填充/复制）。"
            };
            step_prompt = format!("{}\n{}", step_prompt, ba_hint);
        }

        let mut messages = vec![Message {
            role: "system".to_string(),
            content: serde_json::Value::String(system_prompt.clone()),
        }];

        // Build user message with text + optional screenshot(s).
        // 默认投喂 after 裁剪图；表格软件改为投喂 after 全图（original_marked），
        // 并可附带 before 全图（original_before），让模型对比前后状态识别填充/复制。
        let ai_image = ai_image_for_step(step, &original_marked[i], force_domain);
        let has_marked = !ai_image.is_empty();
        let before_image: Option<&String> = if dom.before_after() && !original_before[i].is_empty() {
            Some(&original_before[i])
        } else {
            None
        };

        if !has_marked && before_image.is_none() {
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
            if let Some(b) = before_image {
                content_parts.push(serde_json::json!({
                    "type": "image_url",
                    "image_url": {
                        "url": format!("data:image/jpeg;base64,{}", b),
                    },
                }));
            }
            if has_marked {
                content_parts.push(serde_json::json!({
                    "type": "image_url",
                    "image_url": {
                        "url": format!("data:image/jpeg;base64,{}", ai_image),
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
            Ok(mut output) => {
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
                // ★ 标题防撑爆：LLM 是软约束（prompt 要求短标题但可能超），
                //   落库前程序级兜底截到 10 字 +「…」，与录制标题同口径。
                output.title = crate::recorder::activity::truncate_label(
                    &output.title,
                    crate::recorder::activity::TITLE_LABEL_MAX_CHARS,
                );
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
/// 5b 对齐插件 `computeCropRect`（中心 + 恒 50% 窗口 + 贴边平移夹回），
/// 与导出端 / 前端同一套几何（见 `ai_crop_region` 与 `crop_region_50`）。
/// 优先级：
/// 1. 元素矩形或点击点：恒 50% 窗口；
/// 2. 无可用锚点：整图。
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
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum AiCropRegion {
    Rect(i32, i32, i32, i32),
    Full,
    Outside,
}

/// AI 裁图区域的纯几何（不碰图片字节）。
///
/// 5b 对齐插件 `flowio-extension/src/shared/cropRegion.ts::computeCropRect`：
/// **中心 + 恒 50% 窗口（FOCUS_VISIBLE_FRAC=0.5，倍率 2×）+ 贴边平移夹回**。
/// 实现委托给 `export/commands.rs::crop_region_50` —— 三端唯一纯几何：
///   1. `src/lib/cropAnchor.ts::cropBoxFromAnchor`（前端）
///   2. `src-tauri/src/export/commands.rs::export_crop_box`（导出 docx/html/pdf）
///   3. 本函数（AI 管线）
/// 旧的 PAD=24 / MIN=280 / 缩到 90% / type 左扩 1/3 / drag extraH 体系已随 5b 废弃，
/// 与前端、导出端一样不再因元素尺寸改变窗口；元素矩形仅作「有元素」标志。
/// 一致性由 `npm run check:crop`（第 6 段黄金值）与 cargo 侧 `both_consumers_*` 测试兜底。
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
    match crate::export::commands::crop_region_50(
        img_w, img_h, click_x, click_y, crop_w, crop_h, action_type,
    ) {
        Some((l, t, w, h)) => AiCropRegion::Rect(l, t, w, h),
        None => AiCropRegion::Full,
    }
}

/// Stage C: 按 action_type 差异化裁剪策略。
/// - scroll / nav / screenshot：整窗截图（不裁，原样返回，保留上下文）
/// - 其他（type/click/copy/paste/drag/hotkey/key/shortcut 等）：恒 50% 窗口
///   （5b 对齐插件 computeCropRect；type/drag 不再做左扩 / extraH 差异化，
///   与前端 cropBoxFromAnchor、导出 export_crop_box 同几何）
pub(crate) fn crop_and_encode_typed(
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
            let c = img.crop_imm(left as u32, top as u32, w as u32, h as u32);
            // 空白裁剪守卫（与前端 lib/cropBlank.ts / export 同步）：区域几乎纯色时
            // 回退整图 —— 纯色小图喂给视觉模型毫无信息量，整图才拿得到上下文。
            if crate::export::commands::is_blank_crop(&c) {
                img
            } else {
                c
            }
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
pub(crate) fn model_supports_vision(model: &str) -> bool {
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
    use crate::ai::domain::AppDomain;
    use super::*;

    fn b64(bytes: &[u8]) -> String {
        base64::engine::general_purpose::STANDARD.encode(bytes)
    }

    /// 构造**带纹理**的 PNG。
    /// ⚠️ 必须带纹理：空白裁剪守卫（is_blank_crop，2026-09-05 加）会把纯色区域的
    /// 裁剪回退成整图，纯色测试图会让所有几何断言意外走整图分支。
    fn solid_png(w: u32, h: u32, rgb: [u8; 3]) -> Vec<u8> {
        let mut img = image::RgbImage::new(w, h);
        for (x, y, p) in img.enumerate_pixels_mut() {
            // 逐像素交替的确定性纹理：任何步长采样都能采到不同值 → 永远判非纯色
            let v = ((x * 7 + y * 13) % 32) as i32;
            *p = image::Rgb([
                (rgb[0] as i32 + v).clamp(0, 255) as u8,
                rgb[1],
                rgb[2],
            ]);
        }
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
            ai_image_for_step(&step, &full, None),
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
            ai_image_for_step(&step, &full, None),
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
            ai_image_for_step(&step, &full, None),
            "CROPPED_BASE64",
            "全图为空时即使表格软件也回退到裁剪图"
        );
    }

    #[test]
    fn ai_image_for_step_force_spreadsheet_overrides_unrelated_title() {
        let full = "FULL_REDACTED_BASE64".to_string();
        let cropped = "CROPPED_BASE64".to_string();
        // 窗口标题无任何表格关键词，但用户手动标记了表格模式 → 仍送全图
        let step = step_with_window("销售表", &cropped);
        assert_eq!(
            ai_image_for_step(&step, &full, Some(AppDomain::Spreadsheet)),
            "FULL_REDACTED_BASE64",
            "手动表格模式应覆盖窗口标题关键词缺失"
        );
    }

    #[test]
    fn ai_image_for_step_force_target_app_full_image() {
        let full = "FULL_REDACTED_BASE64".to_string();
        let cropped = "CROPPED_BASE64".to_string();
        // 录 VS Code，标题无任何表格关键词，但指定软件模式（TargetApp）→ 仍送整软件全图
        let step = step_with_window("main.rs - Visual Studio Code", &cropped);
        assert_eq!(
            ai_image_for_step(&step, &full, Some(AppDomain::TargetApp)),
            "FULL_REDACTED_BASE64",
            "录制指定软件应强制整软件全图，不依赖标题关键词"
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
    fn crop_and_encode_click_only_fallback_50pct() {
        // 无元素矩形：以点击点为中心裁屏幕约 50%（1000*0.5=500，插件 FOCUS_VISIBLE_FRAC）
        let png = solid_png(1000, 1000, [50, 60, 70]);
        let out = crop_and_encode(&b64(&png), Some(500), Some(500), None, None);
        let img = image::load_from_memory(&decode(&out)).unwrap();
        assert_eq!((img.width(), img.height()), (500, 500));
    }

    #[test]
    fn crop_and_encode_rect_fixed_50pct_window() {
        // 元素矩形不再影响几何：恒 50% 窗口（5b 对齐插件 computeCropRect）。
        // 旧的 MIN=280 放大 / 缩到 90% 等比缩放体系已废弃。
        let png = solid_png(1000, 1000, [50, 60, 70]);
        let out = crop_and_encode(&b64(&png), Some(500), Some(500), Some(200), Some(100));
        let img = image::load_from_memory(&decode(&out)).unwrap();
        assert_eq!((img.width(), img.height()), (500, 500));
        // 大元素同样只裁 50% 窗口（不再尝试缩到 90% 屏幕）
        let out2 = crop_and_encode(&b64(&png), Some(500), Some(500), Some(870), Some(400));
        let img2 = image::load_from_memory(&decode(&out2)).unwrap();
        assert_eq!((img2.width(), img2.height()), (500, 500));
    }

    #[test]
    fn crop_and_encode_rect_larger_than_screen_fixed_window() {
        // 元素比屏幕还大（500x300 落在 400x300 的图里）：5b 后几何与元素无关，
        // 仍取恒 50% 窗口（200x150），不再尝试缩到 90% 或整图保元素。
        let png = solid_png(400, 300, [3, 4, 5]);
        let out = crop_and_encode(&b64(&png), Some(200), Some(150), Some(500), Some(300));
        let img = image::load_from_memory(&decode(&out)).unwrap();
        assert_eq!((img.width(), img.height()), (200, 150));
    }

    #[test]
    fn crop_and_encode_coords_clamped_at_edge() {
        // 点击 (0,0) 无矩形：fallback 300*0.5=150 → 150×150，贴边夹回以 (0,0) 为左上
        let png = solid_png(300, 300, [1, 2, 3]);
        let out = crop_and_encode(&b64(&png), Some(0), Some(0), None, None);
        let img = image::load_from_memory(&decode(&out)).unwrap();
        assert_eq!((img.width(), img.height()), (150, 150));
    }

    #[test]
    fn crop_and_encode_partial_negative_coord_falls_back_full() {
        // x 有效但 y 为负 → 无合法锚点，走全图分支（与插件 computeCropRect 一致：
        // 坐标非负才可定位）
        let png = solid_png(100, 100, [7, 8, 9]);
        let out = crop_and_encode(&b64(&png), Some(50), Some(-1), None, None);
        let img = image::load_from_memory(&decode(&out)).unwrap();
        assert_eq!((img.width(), img.height()), (100, 100));
    }

    #[test]
    fn crop_and_encode_out_of_bounds_coords_no_panic() {
        // 修复前：x=5000 超出 100x100 图片 → left=4800 > img_w → w 为负 → crop_imm panic
        // 5b 后：crop_region_50 贴边平移夹回 → 裁图内 50×50（不再原样返回）
        let png = solid_png(100, 100, [9, 9, 9]);
        let out = crop_and_encode(&b64(&png), Some(5000), Some(5000), None, None);
        let img = image::load_from_memory(&decode(&out)).unwrap();
        assert_eq!((img.width(), img.height()), (50, 50));
    }

    #[test]
    fn crop_and_encode_one_axis_out_of_bounds_no_panic() {
        let png = solid_png(100, 100, [11, 12, 13]);
        let out = crop_and_encode(&b64(&png), Some(5000), Some(50), None, None);
        // x 方向出界 → clamp 到贴边，仍裁 50×50
        let img = image::load_from_memory(&decode(&out)).unwrap();
        assert_eq!((img.width(), img.height()), (50, 50));
    }

    #[test]
    fn crop_and_encode_typed_type_click_same_window() {
        // 5b：type 左扩 1/3 已废弃，type 与 click 输出相同 50% 窗口
        let png = solid_png(800, 600, [9, 8, 7]);
        let base = crop_and_encode_typed(&b64(&png), Some(400), Some(300), Some(100), Some(40), "click");
        let typed = crop_and_encode_typed(&b64(&png), Some(400), Some(300), Some(100), Some(40), "type");
        let base_img = image::load_from_memory(&decode(&base)).unwrap();
        let typed_img = image::load_from_memory(&decode(&typed)).unwrap();
        assert_eq!(
            (typed_img.width(), typed_img.height()),
            (base_img.width(), base_img.height())
        );
        assert_eq!((typed_img.width(), typed_img.height()), (400, 300));
    }

    /// 空白裁剪守卫（2026-09-05）：裁剪区域几乎纯色时回退整图 ——
    /// 导出端（maybe_crop_screenshot_bytes）与 AI 端（crop_and_encode_typed）
    /// 两个消费端对同一份数据必须同为「回退整图」，防止编辑器看整图而
    /// 导出/送 AI 的还是空白小图（行为分叉）。
    /// 场景取自真实案例：WPS 加载中录到「Program Manager」宿主伪元素矩形
    /// (237,790) 420×280，裁出纯灰空白，加载动画实际在画面中心。
    #[test]
    fn blank_crop_fallback_full_both_consumers() {
        let raw = {
            let img = image::RgbImage::from_pixel(800, 600, image::Rgb([214, 214, 214]));
            let mut buf = Vec::new();
            let enc = image::codecs::jpeg::JpegEncoder::new_with_quality(&mut buf, 90);
            image::DynamicImage::ImageRgb8(img)
                .write_with_encoder(enc)
                .unwrap();
            buf
        };
        let b64s = b64(&raw);

        // 消费端 A：导出 → 尺寸必须是整图、字节原样（不重编码）
        let (bytes, dims) = crate::export::commands::maybe_crop_screenshot_bytes(
            raw,
            Some(237),
            Some(790),
            Some(420),
            Some(280),
            "type",
        );
        assert_eq!(dims, Some((800, 600)), "导出端应回退整图");

        // 消费端 B：AI → 输出也必须是整图尺寸
        let out = crop_and_encode_typed(&b64s, Some(237), Some(790), Some(420), Some(280), "type");
        let img = image::load_from_memory(&decode(&out)).unwrap();
        assert_eq!((img.width(), img.height()), (800, 600), "AI 端应回退整图");

        // 导出端回退时字节必须原样
        assert_eq!(bytes, decode(&b64s), "回退整图时应原样返回原始字节");
    }

    /* ── 裁图几何（ai_crop_region）───────────────────────────────
     * 5b 对齐插件 computeCropRect：中心 + 恒 50% 窗口 + 贴边平移夹回。
     * 旧的 PAD=24 / MIN=280 / 缩到 90% / type 左扩 1/3 / drag extraH 已废弃，
     * 本端与前端 cropBoxFromAnchor、导出 export_crop_box 共用 crop_region_50。
     *
     * 场景：800x600 截图，锚点中心 (400,300)
     *   rw = 800*0.5 = 400, rh = 600*0.5 = 300
     *   left = 400-200 = 200, top = 300-150 = 150 → (200,150,400,300)
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
    fn ai_crop_region_golden_type_click_drag_same() {
        // 元素矩形只作「有元素」标志：type/click/drag 一律恒 50% 窗口
        for at in ["type", "click", "drag"] {
            assert_eq!(
                rect(ai_crop_region(
                    800, 600, Some(400), Some(300), Some(300), Some(40), at
                )),
                (200, 150, 400, 300),
                "{at} 应为恒 50% 窗口"
            );
        }
    }

    #[test]
    fn ai_crop_region_edge_corner_clamped() {
        // 贴边 / 负中心：窗口被 clamp 回图内（与前端 cropBoxFromAnchor 同口径）
        assert_eq!(
            rect(ai_crop_region(
                800, 600, Some(0), Some(0), Some(300), Some(40), "click"
            )),
            (0, 0, 400, 300)
        );
        assert_eq!(
            rect(ai_crop_region(
                800, 600, Some(-50), Some(-50), Some(300), Some(40), "click"
            )),
            (0, 0, 400, 300)
        );
        assert_eq!(
            rect(ai_crop_region(
                800, 600, Some(800), Some(600), Some(300), Some(40), "click"
            )),
            (400, 300, 400, 300)
        );
    }

    #[test]
    fn ai_crop_region_fallback_and_full_cases() {
        // 只有点击点：屏幕 50%（插件 FOCUS_VISIBLE_FRAC=0.5）
        assert_eq!(
            rect(ai_crop_region(
                800, 600, Some(400), Some(300), None, None, "click"
            )),
            (200, 150, 400, 300)
        );
        // 无坐标 → Full（用全图，但会重编码）
        assert!(
            matches!(
                ai_crop_region(800, 600, None, None, None, None, "click"),
                AiCropRegion::Full
            ),
            "无锚点应为 Full"
        );
        // 退化图尺寸 → Outside（原样返回，不重编码）
        assert!(
            matches!(
                ai_crop_region(0, 0, Some(1), Some(1), Some(2), Some(2), "click"),
                AiCropRegion::Outside
            ),
            "退化图尺寸应为 Outside"
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
    fn ai_crop_region_tiny_elements_still_50pct_window() {
        // 极小元素：5b 后不再放大到 MIN=280，恒 50% 窗口（800x600 → 400x300）
        let (l, t, w, h) = rect(ai_crop_region(
            800, 600, Some(400), Some(300), Some(10), Some(10), "click",
        ));
        assert_eq!((l, t, w, h), (200, 150, 400, 300));
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
     * 存库锚点由录制端 recorder/commands.rs 生成（bake CROP_PAD=24 / MIN 420x280），
     * 5b 后导出端与 AI 端都委托 `export/commands.rs::crop_region_50`——
     * 同一份存库锚点得到**完全相同**的裁图矩形（中心 + 恒 50% 窗口 + 贴边夹回）。
     * 旧的「导出 PAD=8 / AI PAD=24 刻意不同」体系已废弃。
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
    fn both_consumers_must_agree_on_crop_region() {
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
            (1366, 768, 300, 40, "drag", "低分辨率输入框(drag)"),
        ];

        println!(
            "\n{:<16} {:<6} {:>9} {:>10} | {:>11} {:>11} | {:>8}",
            "场景", "type", "元素", "落库值", "导出框", "AI 框", "一致性"
        );
        let mut violations: Vec<String> = Vec::new();

        for (iw, ih, ew, eh, at, name) in cases {
            let (cx, cy) = (iw / 2, ih / 2);
            let (sx, sy, sw, sh) = stored_anchor(iw, ih, ew, eh, cx, cy);

            let export = match export_crop_box(iw, ih, Some(sx), Some(sy), Some(sw), Some(sh), at) {
                Some(v) => v,
                None => {
                    println!("{name}: 导出端不裁剪（整窗类型）");
                    continue;
                }
            };
            let ai = match ai_crop_region(iw, ih, Some(sx), Some(sy), Some(sw), Some(sh), at) {
                AiCropRegion::Rect(l, t, w, h) => (l, t, w, h),
                other => {
                    violations.push(format!(
                        "{name}（{iw}x{ih} {at}，元素 {ew}x{eh}）：AI 端应为 Rect，实际 {:?}",
                        other
                    ));
                    continue;
                }
            };

            // 5b 口径：两端同一纯几何，输出必须逐字段一致
            if export != ai {
                violations.push(format!(
                    "{name}（{iw}x{ih} {at}，元素 {ew}x{eh}，落库 {sw}x{sh}）：导出 {export:?} ≠ AI {ai:?}"
                ));
            }

            // 且必须是「中心 + 恒 50% 窗口 + 贴边夹回」
            let (l, t, w, h) = ai;
            let want_w = (iw as f32 * 0.5).round() as i32;
            let want_h = (ih as f32 * 0.5).round() as i32;
            assert_eq!(
                (w, h),
                (want_w, want_h),
                "{name}（{iw}x{ih} {at}）：窗口应为恒 50% ({want_w}x{want_h})，实际 {w}x{h}"
            );
            assert!(l >= 0 && t >= 0 && l + w <= iw && t + h <= ih);
        }

        assert!(
            violations.is_empty(),
            "导出端与 AI 端裁图不一致：\n  - {}",
            violations.join("\n  - ")
        );
    }
}
