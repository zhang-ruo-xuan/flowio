//! Stage 4：AI 兜底（视觉语义定位）。
//!
//! # 定位
//! 确定性重放（坐标 + 控件锚点）扛住 ~90% 稳定步骤；剩下 ~10% 会因为
//! 「窗口没起来 / 弹窗换了地方 / 分辨率变了 / 控件类名对不上」而漂。
//! 这类漂移靠再加一层规则是补不完的（RPA 的红海就在这里）。
//!
//! 本模块只做**兜底**：确定性执行判定「这一步没打中」时，把当前屏幕 + 该步
//! 意图 + 前后步上下文交给视觉模型，问「现在该点哪里」，拿到坐标后重新下发。
//! 调用频率低（默认每轮上限 [`MAX_CALLS_PER_RUN`] 次），成本可忽略。
//!
//! # 设计原则
//! - **可单测**：解析 / 判定 / 换算全是纯函数，网络与截图只在
//!   [`resolve_via_ai`] 里集中出现。
//! - **默认关闭**：必须显式开启，绝不偷偷联网、绝不改变既有执行行为。
//! - **失败即静默**：AI 不可用 / 超时 / 解析不出坐标 → 返回 Err，调用方按
//!   「没有兜底」继续，绝不让 AI 把执行搞得更糟。

use base64::Engine;
use serde_json::json;

use crate::ai::http::{call_llm, Message};
use crate::ai::types::AiConfig;
use crate::executor::backend::{replay_log, ReplayBackend};

/// 单轮执行允许的 AI 兜底次数上限。
///
/// 兜底是「漂移」而非「常态」：一轮里若超过 3 步都要问 AI，说明整条录制
/// 已经与当前环境失配（软件没装 / 版本差异过大），继续问下去只是在烧钱
/// 且大概率继续错。此时停手，把失败如实报给用户更有价值。
pub const MAX_CALLS_PER_RUN: usize = 3;

/// 喂给视觉模型的截图最大宽度（像素）。
///
/// 4K 屏原图 3840px 宽，base64 后 ~1MB，单次请求既慢又容易触发限流；
/// 视觉模型内部也会缩到 ~1K。这里主动缩到 1280 宽，坐标换算随之按
/// [`compute_unit`] 补偿，精度损失 < 1 逻辑像素。
const MAX_FEED_WIDTH: u32 = 1280;

/// 判定为「AI 已经把点带偏」的最小位移：小于该值认为没救回来，不占用额度。
const MIN_RECOVER_PX: i32 = 8;

// ── 纯函数区（全部可单测，不碰网络 / 不碰屏幕）─────────────────────

/// 从模型回复里解析落点坐标。
///
/// 视觉模型回复格式极不稳定，实测会出现的形态：
/// - `{"x": 512, "y": 340}`
/// - ```` ```json\n{"x":512,"y":340}\n``` ````（带 markdown 代码围栏）
/// - `POINT(512,340)`
/// - `坐标：512,340`
/// - `x=512 y=340`
/// - 一大段分析文字里夹一个 JSON
///
/// 策略：先按 JSON 块找 `{...}` 取 x/y；失败再退化到全文数字对。
/// 只认 **图像像素坐标**（模型看到的是缩放后的截图），调用方再换算到屏幕。
pub fn parse_ai_point(raw: &str) -> Option<(i32, i32)> {
    let s = raw.trim();
    if s.is_empty() {
        return None;
    }

    // 1) 优先：任意 {...} 块里成对出现 x / y（含中文键的极端情况也覆盖）
    if let Some((x, y)) = scan_json_object(s) {
        return Some((x, y));
    }

    // 2) 退化：全文找第一组 "数字 分隔符 数字"（POINT(1,2) / 1,2 / 1 2）
    scan_number_pair(s)
}

/// 在 `s` 里找第一个 `{...}`，从中取数值型 `x` / `y` 字段。
fn scan_json_object(s: &str) -> Option<(i32, i32)> {
    let bytes = s.as_bytes();
    let mut i = 0usize;
    while i < bytes.len() {
        if bytes[i] == b'{' {
            // 从 { 起逐字符累加深度，找到配对的 }
            let mut depth = 0i32;
            let mut j = i;
            while j < bytes.len() {
                match bytes[j] {
                    b'{' => depth += 1,
                    b'}' => {
                        depth -= 1;
                        if depth == 0 {
                            break;
                        }
                    }
                    _ => {}
                }
                j += 1;
            }
            if j < bytes.len() {
                let slice = &s[i..=j];
                if let Some(p) = point_from_json_str(slice) {
                    return Some(p);
                }
                // 该 JSON 块里没有 x/y → 继续找下一个块（模型可能先输出别的）
                i = j + 1;
                continue;
            }
            break;
        }
        i += 1;
    }
    None
}

/// 把单个 `{...}` 字符串按 JSON 解析取 x/y；解析失败再退回正则式键值扫描。
fn point_from_json_str(slice: &str) -> Option<(i32, i32)> {
    if let Ok(v) = serde_json::from_str::<serde_json::Value>(slice) {
        let obj = v.as_object()?;
        let x = num_value(obj.get("x")?)?;
        let y = num_value(obj.get("y")?)?;
        return Some((x.round() as i32, y.round() as i32));
    }
    None
}

fn num_value(v: &serde_json::Value) -> Option<f64> {
    match v {
        serde_json::Value::Number(n) => n.as_f64(),
        serde_json::Value::String(s) => s.trim().parse::<f64>().ok(),
        _ => None,
    }
}

/// 全文扫描 `数字 [非数字]{1,3} 数字`，取第一组。
///
/// 排除纯版本号之类的噪声做不到 100%，但模型被明确要求只输出坐标，
/// 且调用方会额外做「落在图像范围内」的钳制校验。
fn scan_number_pair(s: &str) -> Option<(i32, i32)> {
    let bytes = s.as_bytes();
    let n = bytes.len();
    let mut i = 0usize;
    while i < n {
        if bytes[i].is_ascii_digit() || (bytes[i] == b'-' && i + 1 < n && bytes[i + 1].is_ascii_digit())
        {
            let start = i;
            while i < n && (bytes[i].is_ascii_digit() || bytes[i] == b'-' || bytes[i] == b'.') {
                // 允许一个负号/小数点，遇到第二个符号即停
                if bytes[i] == b'-' && i > start {
                    break;
                }
                i += 1;
            }
            let first = &s[start..i];
            // 跳过 0~3 个分隔符（", " / "," / "，" / " " / "x" 不该出现这里）
            let mut k = i;
            let mut seps = 0usize;
            while k < n && seps < 3 && !bytes[k].is_ascii_digit() {
                // 换行视作分隔终止，避免跨行拼接出错误坐标
                if bytes[k] == b'\n' {
                    break;
                }
                seps += 1;
                k += 1;
            }
            if k < n && bytes[k].is_ascii_digit() && seps > 0 {
                let mut m = k;
                while m < n && bytes[m].is_ascii_digit() {
                    m += 1;
                }
                if let (Ok(a), Ok(b)) = (
                    first.parse::<f64>(),
                    s[k..m].parse::<f64>(),
                ) {
                    let a = a.round();
                    let b = b.round();
                    if a >= 0.0 && b >= 0.0 && a < 100_000.0 && b < 100_000.0 {
                        return Some((a as i32, b as i32));
                    }
                }
            }
            i = k;
            continue;
        }
        i += 1;
    }
    None
}

/// 每张输出图像像素对应多少个**屏幕逻辑像素**。
///
/// 见 [`crate::recorder::screenshot::capture_fullscreen_geometry`] 的推导：
/// 无论位图是物理尺寸还是被 DPI 虚拟化，只要知道显示器逻辑宽与输出图宽，
/// 两者之比就是唯一正确的换算系数（自校正，无需判断 DPI 感知）。
pub fn compute_unit(logical_w: f32, out_w: u32) -> f32 {
    if out_w == 0 || logical_w <= 0.0 {
        return 1.0;
    }
    logical_w / out_w as f32
}

/// 图像像素坐标 → 屏幕虚拟坐标（逻辑像素）。
pub fn image_to_screen(u: i32, v: i32, origin_x: i32, origin_y: i32, unit: f32) -> (i32, i32) {
    let unit = if unit <= 0.0 { 1.0 } else { unit };
    (
        origin_x + (u as f32 * unit).round() as i32,
        origin_y + (v as f32 * unit).round() as i32,
    )
}

/// 把模型给的图像坐标钳制到图像范围内。
///
/// 模型偶尔会吐出负数或超出宽高的值（幻觉 / 把坐标当成了别的坐标系），
/// 直接拿去点击会点到屏幕外或另一台显示器上。
pub fn clamp_to_image(p: (i32, i32), w: u32, h: u32) -> (i32, i32) {
    let w = w.max(1) as i32;
    let h = h.max(1) as i32;
    (p.0.clamp(0, w - 1), p.1.clamp(0, h - 1))
}

/// 本次是否值得向 AI 求助。
///
/// - 无坐标步（纯聚焦 / 全局快捷键）没有「点哪里」的问题 → 不求助；
/// - 只有确认打偏的两种情况才求助：目标窗口没聚焦上（stuck）、截图校验失败。
pub fn should_attempt(action_type: &str, has_point: bool, focus_failed: bool, verify_failed: bool) -> bool {
    if !has_point {
        return false;
    }
    if !(focus_failed || verify_failed) {
        return false;
    }
    matches!(action_type, "click" | "double_click" | "right_click" | "type" | "input")
}

/// AI 给的落点是否值得重发一次（相对原落点的位移足够大）。
pub fn worth_reissue(old: (i32, i32), new: (i32, i32)) -> bool {
    let dx = new.0 - old.0;
    let dy = new.1 - old.1;
    ((dx * dx + dy * dy) as f64).sqrt() >= MIN_RECOVER_PX as f64
}

// ── 提示词 ──────────────────────────────────────────────────

/// 兜底请求的上下文（全部为纯数据，便于单测与复用）。
pub struct FallbackContext<'a> {
    pub step_number: i32,
    pub action_type: &'a str,
    pub window_title: &'a str,
    /// 步骤标题（人类可读意图，AI 生成/用户编辑），如「点击 插入函数」。
    pub step_title: &'a str,
    /// 该步原本要输入的内容（type 步）。
    pub text: Option<&'a str>,
    /// 录制时的落点（屏幕虚拟坐标）。
    pub recorded: Option<(i32, i32)>,
    /// 上一步在做什么（给模型一点时序线索）。
    pub prev_intent: &'a str,
    /// 下一步要做什么（帮助模型判断「点开后应该出现什么」）。
    pub next_intent: &'a str,
    /// 触发兜底的原因（"focus-failed" / "verify-failed"）。
    pub reason: &'a str,
    /// 截图尺寸（输出给模型的图），用于在提示词里声明坐标系范围。
    pub img_w: u32,
    pub img_h: u32,
}

pub const SYSTEM_PROMPT: &str = "你是一个 Windows 桌面自动化执行器的视觉兜底模块。\
确定性重放按录制坐标点了下去，但没有命中预期目标。\
你会收到一张当前屏幕截图和这一步的意图，请判断**现在应该点击哪里**。\
只输出一个 JSON：{\"x\": <整数图像像素横坐标>, \"y\": <整数图像像素纵坐标>, \"why\": \"一句中文说明\"}。\
坐标必须是**截图图像内的像素坐标**（左上角为 0,0）。不要输出除 JSON 以外的任何内容。\
如果截图里确实找不到该目标，输出 {\"x\": -1, \"y\": -1, \"why\": \"原因\"}。";

/// 组装用户提示词（含意图、坐标系声明、上下文）。
pub fn build_prompt(ctx: &FallbackContext) -> String {
    let action_cn = match ctx.action_type {
        "click" | "double_click" => "单击",
        "right_click" => "右键单击",
        "type" | "input" => "先点击输入框/单元格，再输入内容",
        _ => "点击",
    };
    let where_ = ctx
        .recorded
        .map(|(x, y)| format!("录制时的落点大约在全屏坐标 ({}, {})，但现在已经对不上了。", x, y))
        .unwrap_or_else(|| "这一步录制时没有可用坐标。".to_string());

    let mut s = format!(
        "第 {} 步执行失败，需要你判断现在该点哪里。\n\
         触发原因：{}\n\
         目标窗口：{}\n\
         本步意图：{}\n\
         本步动作：{}\n\
         {}\n\
         上一步：{}\n\
         下一步：{}\n\
         截图尺寸：{}x{}（图像像素，左上角 0,0）\n\
         请给出应该点击的图像像素坐标：",
        ctx.step_number,
        reason_cn(ctx.reason),
        if ctx.window_title.trim().is_empty() { "（未记录）" } else { ctx.window_title },
        if ctx.step_title.trim().is_empty() { "（无标题）" } else { ctx.step_title },
        action_cn,
        where_,
        if ctx.prev_intent.trim().is_empty() { "（无）" } else { ctx.prev_intent },
        if ctx.next_intent.trim().is_empty() { "（无）" } else { ctx.next_intent },
        ctx.img_w,
        ctx.img_h,
    );
    if let Some(t) = ctx.text {
        if !t.trim().is_empty() {
            s.push_str(&format!("\n本步要输入的内容：{}", t));
        }
    }
    s
}

fn reason_cn(r: &str) -> &'static str {
    if r.contains("verify") {
        "按录制坐标点击后，画面与该步录制时不一致（没打中）"
    } else {
        "目标窗口没有出现在前台（可能没启动 / 被别的窗口挡住）"
    }
}

// ── 真实调用（唯一涉及截图与网络的部分）──────────────────────

/// AI 兜底成功时给出的落点（已换算到屏幕虚拟坐标）。
#[derive(Debug, Clone)]
pub struct AiPoint {
    pub x: i32,
    pub y: i32,
    /// 模型给的中文理由（写进执行结果面板，用户能看懂「AI 为什么点这里」）。
    pub why: String,
}

/// 抓屏 → 缩图 → 问模型 → 解析 → 换算到屏幕坐标。
///
/// 任何一步失败都返回 Err，调用方按「没有兜底」继续。
pub fn resolve_via_ai(cfg: &AiConfig, ctx: &FallbackContext) -> Result<AiPoint, String> {
    let (img, ox, oy, logical_w) =
        crate::recorder::screenshot::capture_fullscreen_geometry()
            .map_err(|e| format!("抓屏失败：{}", e))?;

    // 缩放到模型友好的宽度（保持宽高比）
    let out = if img.width() > MAX_FEED_WIDTH {
        let h = (img.height() as f32 * (MAX_FEED_WIDTH as f32 / img.width() as f32)).round() as u32;
        img.resize(MAX_FEED_WIDTH, h.max(1), image::imageops::FilterType::Triangle)
    } else {
        img.clone()
    };
    let (ow, oh) = (out.width(), out.height());

    // 图像像素 → 屏幕逻辑像素；纵向与横向同系数（缩放保持宽高比）
    let unit = compute_unit(logical_w, ow);

    // 编码前在图上标出「原本想点的地方」，模型能直观看到偏差方向
    let mut marked = out.clone();
    if let Some((rx, ry)) = ctx.recorded {
        let u = ((rx - ox) as f32 / unit).round() as i32;
        let v = ((ry - oy) as f32 / unit).round() as i32;
        if u >= 0 && v >= 0 && u < ow as i32 && v < oh as i32 {
            crate::recorder::screenshot::draw_click_marker(&mut marked, u, v, ctx.step_number);
        }
    }

    let mut buf = std::io::Cursor::new(Vec::new());
    let rgb = marked.to_rgb8();
    let mut enc = image::codecs::jpeg::JpegEncoder::new_with_quality(&mut buf, 80);
    enc.encode(&rgb, ow, oh, image::ExtendedColorType::Rgb8)
        .map_err(|e| format!("JPEG 编码失败：{}", e))?;
    let b64 = base64::engine::general_purpose::STANDARD.encode(buf.into_inner());

    // 提示词里声明的必须是**喂给模型的这张图**的尺寸，否则模型按错坐标系回答
    let ctx2 = FallbackContext {
        step_number: ctx.step_number,
        action_type: ctx.action_type,
        window_title: ctx.window_title,
        step_title: ctx.step_title,
        text: ctx.text,
        recorded: ctx.recorded,
        prev_intent: ctx.prev_intent,
        next_intent: ctx.next_intent,
        reason: ctx.reason,
        img_w: ow,
        img_h: oh,
    };
    let prompt = build_prompt(&ctx2);

    let messages = vec![
        Message {
            role: "system".to_string(),
            content: serde_json::Value::String(SYSTEM_PROMPT.to_string()),
        },
        Message {
            role: "user".to_string(),
            content: json!([
                { "type": "text", "text": prompt },
                { "type": "image_url", "image_url": { "url": format!("data:image/jpeg;base64,{}", b64) } },
            ]),
        },
    ];

    replay_log(&format!(
        "[ai-fallback] 第 {} 步请求视觉兜底（{}，图 {}x{}）",
        ctx.step_number, ctx.reason, ow, oh
    ));

    let raw = call_llm(cfg, messages).map_err(|e| format!("视觉模型调用失败：{}", e))?;
    replay_log(&format!(
        "[ai-fallback] 第 {} 步模型回复：{}",
        ctx.step_number,
        raw.chars().take(200).collect::<String>()
    ));

    let (u, v) = parse_ai_point(&raw).ok_or_else(|| "模型回复里解析不出坐标".to_string())?;
    if u < 0 || v < 0 {
        return Err("模型判定截图里找不到目标".to_string());
    }
    let (u, v) = clamp_to_image((u, v), ow, oh);
    let (sx, sy) = image_to_screen(u, v, ox, oy, unit);
    let why = extract_why(&raw);
    Ok(AiPoint { x: sx, y: sy, why })
}

/// 从回复里摘 `why` 字段；没有就用空串（结果面板留空，不编造理由）。
fn extract_why(raw: &str) -> String {
    if let Some(s) = raw.find('{') {
        if let Some(e) = raw.rfind('}') {
            if e > s {
                if let Ok(v) = serde_json::from_str::<serde_json::Value>(&raw[s..=e]) {
                    if let Some(w) = v.get("why").and_then(|x| x.as_str()) {
                        return w.chars().take(60).collect();
                    }
                }
            }
        }
    }
    String::new()
}

/// 按动作类型把 AI 给的落点重新下发一次。
///
/// 只对「点得准就有意义」的动作重发：click / 双击 / 右键 / 输入。
/// 拖拽等复合动作交给 AI 风险过高（端点语义难对齐），维持原行为。
pub fn reissue(backend: &dyn ReplayBackend, action_type: &str, p: &AiPoint, text: Option<&str>) {
    match action_type {
        "double_click" => backend.double_click(p.x, p.y),
        "right_click" => backend.right_click(p.x, p.y),
        "type" | "input" => {
            backend.click(p.x, p.y);
            std::thread::sleep(std::time::Duration::from_millis(120));
            if let Some(t) = text {
                if !t.is_empty() {
                    backend.type_text(t);
                }
            }
        }
        _ => backend.click(p.x, p.y),
    }
}

#[cfg(test)]
mod ai_fallback_tests {
    use super::*;

    // ── parse_ai_point ──
    #[test]
    fn parse_plain_json() {
        assert_eq!(parse_ai_point("{\"x\": 512, \"y\": 340}"), Some((512, 340)));
    }

    #[test]
    fn parse_json_in_markdown_fence() {
        let s = "```json\n{\"x\":100,\"y\":200,\"why\":\"按钮在右上\"}\n```";
        assert_eq!(parse_ai_point(s), Some((100, 200)));
    }

    #[test]
    fn parse_json_with_surrounding_prose() {
        let s = "我先分析一下画面……\n结论是 {\"x\": 640, \"y\": 360}。";
        assert_eq!(parse_ai_point(s), Some((640, 360)));
    }

    #[test]
    fn parse_json_string_numbers() {
        assert_eq!(parse_ai_point("{\"x\":\"88\",\"y\":\"99\"}"), Some((88, 99)));
    }

    #[test]
    fn parse_json_float_rounds() {
        assert_eq!(parse_ai_point("{\"x\":10.6,\"y\":20.2}"), Some((11, 20)));
    }

    #[test]
    fn parse_point_function_form() {
        assert_eq!(parse_ai_point("POINT(512,340)"), Some((512, 340)));
    }

    #[test]
    fn parse_bare_pair() {
        assert_eq!(parse_ai_point("512,340"), Some((512, 340)));
        assert_eq!(parse_ai_point("坐标：512, 340"), Some((512, 340)));
    }

    #[test]
    fn parse_x_equals_form() {
        assert_eq!(parse_ai_point("x=512 y=340"), Some((512, 340)));
    }

    #[test]
    fn parse_negative_is_rejected_as_not_found() {
        // 找不到目标时模型输出 -1；解析层仍返回坐标，由调用方判定
        assert_eq!(parse_ai_point("{\"x\": -1, \"y\": -1}"), Some((-1, -1)));
    }

    #[test]
    fn parse_garbage_returns_none() {
        assert_eq!(parse_ai_point("对不起，我无法完成这个任务。"), None);
        assert_eq!(parse_ai_point(""), None);
    }

    #[test]
    fn parse_ignores_first_json_without_xy() {
        // 模型先输出一段无关 JSON，真正的坐标在后面
        let s = "{\"note\":\"nothing\"} then {\"x\":7,\"y\":8}";
        assert_eq!(parse_ai_point(s), Some((7, 8)));
    }

    #[test]
    fn parse_does_not_pair_across_newlines() {
        assert_eq!(parse_ai_point("第一行 123\n第二行 456"), None);
    }

    // ── 坐标换算 ──
    #[test]
    fn unit_is_logical_over_output() {
        // 1920 逻辑宽缩到 1280 输出 → 每输出像素 = 1.5 逻辑像素
        assert!((compute_unit(1920.0, 1280) - 1.5).abs() < 1e-6);
    }

    #[test]
    fn unit_guards_zero() {
        assert_eq!(compute_unit(0.0, 0), 1.0);
    }

    #[test]
    fn image_to_screen_applies_origin_and_unit() {
        // 副屏在主键左侧：原点 (-1920, 0)，2 倍缩放
        assert_eq!(image_to_screen(100, 50, -1920, 0, 2.0), (-1720, 100));
    }

    #[test]
    fn image_to_screen_unit_one_is_identity() {
        assert_eq!(image_to_screen(10, 20, 0, 0, 1.0), (10, 20));
    }

    #[test]
    fn screen_roundtrip_1920_to_1280() {
        // 逻辑宽 1920 的屏，缩到 1280 宽给模型；模型给出图像坐标 (640, 360)
        // 应还原到屏幕正中 (960, 540)
        let unit = compute_unit(1920.0, 1280);
        assert_eq!(image_to_screen(640, 360, 0, 0, unit), (960, 540));
    }

    #[test]
    fn clamp_keeps_point_inside() {
        assert_eq!(clamp_to_image((-5, 99999), 800, 600), (0, 599));
        assert_eq!(clamp_to_image((10, 10), 800, 600), (10, 10));
    }

    // ── 触发判定 ──
    #[test]
    fn should_attempt_requires_failure_signal() {
        assert!(should_attempt("click", true, true, false));
        assert!(should_attempt("click", true, false, true));
        assert!(!should_attempt("click", true, false, false));
    }

    #[test]
    fn should_attempt_rejects_pointless_actions() {
        assert!(!should_attempt("hotkey", true, true, false));
        assert!(!should_attempt("scroll", true, true, false));
        assert!(!should_attempt("drag", true, true, false));
        assert!(!should_attempt("click", false, true, true));
    }

    #[test]
    fn should_attempt_supports_pointable_actions() {
        for a in ["click", "double_click", "right_click", "type", "input"] {
            assert!(should_attempt(a, true, true, false), "{}", a);
        }
    }

    #[test]
    fn worth_reissue_threshold() {
        assert!(worth_reissue((100, 100), (140, 100)));
        assert!(!worth_reissue((100, 100), (103, 100)));
    }

    // ── 提示词 ──
    #[test]
    fn prompt_contains_intent_and_coordinate_space() {
        let p = build_prompt(&FallbackContext {
            step_number: 9,
            action_type: "click",
            window_title: "函数参数",
            step_title: "点击 插入函数",
            text: None,
            recorded: Some((665, 308)),
            prev_intent: "点击 B2 单元格",
            next_intent: "输入 =SUMIF(",
            reason: "focus-failed",
            img_w: 1280,
            img_h: 720,
        });
        assert!(p.contains("第 9 步"), "{}", p);
        assert!(p.contains("函数参数"), "{}", p);
        assert!(p.contains("点击 插入函数"), "{}", p);
        assert!(p.contains("665"), "{}", p);
        assert!(p.contains("1280x720"), "{}", p);
        assert!(p.contains("目标窗口没有出现在前台"), "{}", p);
    }

    #[test]
    fn prompt_states_verify_reason() {
        let p = build_prompt(&FallbackContext {
            step_number: 3,
            action_type: "type",
            window_title: "WPS",
            step_title: "输入公式",
            text: Some("=SUM(A1)"),
            recorded: None,
            prev_intent: "",
            next_intent: "",
            reason: "verify-failed",
            img_w: 100,
            img_h: 100,
        });
        assert!(p.contains("画面与该步录制时不一致"), "{}", p);
        assert!(p.contains("=SUM(A1)"), "{}", p);
        assert!(p.contains("（未记录）") == false, "{}", p);
    }

    #[test]
    fn extract_why_reads_chinese_reason() {
        let raw = "{\"x\":1,\"y\":2,\"why\":\"按钮被弹窗挡住了\"}";
        assert_eq!(extract_why(raw), "按钮被弹窗挡住了");
    }

    #[test]
    fn extract_why_empty_when_missing() {
        assert_eq!(extract_why("{\"x\":1,\"y\":2}"), "");
    }
}
