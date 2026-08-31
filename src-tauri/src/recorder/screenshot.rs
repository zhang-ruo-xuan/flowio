use image::DynamicImage;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

static COUNTER: AtomicU64 = AtomicU64::new(0);

/// Screenshot storage directory under system temp.
pub fn screenshot_dir() -> PathBuf {
    let dir = std::env::temp_dir().join("flowio_screenshots");
    let _ = std::fs::create_dir_all(&dir);
    dir
}

/// Generate the next JPEG file path: `{prefix}_{timestamp_ms}_{counter}.jpg`
pub fn next_screenshot_path(prefix: &str) -> PathBuf {
    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    let c = COUNTER.fetch_add(1, Ordering::Relaxed);
    screenshot_dir().join(format!("{}_{}_{}.jpg", prefix, ts, c))
}

/// Capture the primary monitor's fullscreen as a DynamicImage.
pub fn capture_fullscreen() -> Result<DynamicImage, String> {
    let monitors = xcap::Monitor::all()
        .map_err(|e| format!("xcap: failed to enumerate monitors: {}", e))?;
    let monitor = monitors
        .into_iter()
        .next()
        .ok_or_else(|| "xcap: no monitor found".to_string())?;
    let img = monitor
        .capture_image()
        .map_err(|e| format!("xcap: failed to capture image: {}", e))?;
    Ok(DynamicImage::ImageRgba8(img))
}

/// Crop the top of an image to remove Flowio's own recording status bar.
///
/// The REC bar has a dark background (#24292e-ish) with red text (R>180, G<80, B<80).
/// Scans the top ~100 px rows. A row is considered watermark only if it has both:
///   - enough red pixels (>5)
///   - a dark background characteristic (majority of row's non-red pixels are dark, L<60)
/// This prevents false positives from red desktop icons or wallpaper elements.
/// If found, crops from the first detected row to just below the last detected row.
pub fn crop_watermark(img: &mut DynamicImage) {
    let w = img.width();
    let h = img.height();
    let scan_h = (h as usize).min(100);
    let buf = img.to_rgba8();
    let bytes = buf.as_raw();

    let mut watermark_start: Option<usize> = None;
    let mut watermark_end: usize = 0;

    for y in 0..scan_h {
        let row_start = y * w as usize * 4;
        let mut red_count = 0usize;
        let mut dark_count = 0usize;
        let mut total_sampled = 0usize;
        let step = 8; // sample every 8th pixel for speed
        for x in (0..w as usize * 4).step_by(step * 4) {
            let idx = row_start + x;
            if idx + 2 < bytes.len() {
                let r = bytes[idx];
                let g = bytes[idx + 1];
                let b = bytes[idx + 2];
                total_sampled += 1;
                if r > 180 && g < 80 && b < 80 {
                    red_count += 1;
                } else {
                    // Check if non-red pixel is dark (lightness < 60)
                    let lum = (r as u32 + g as u32 + b as u32) / 3;
                    if lum < 60 {
                        dark_count += 1;
                    }
                }
            }
        }
        // Only count as watermark row if both: enough red AND mostly dark background
        let non_red = total_sampled.saturating_sub(red_count);
        let dark_ratio = if non_red > 0 { dark_count as f64 / non_red as f64 } else { 0.0 };
        if red_count > 5 && dark_ratio > 0.4 {
            if watermark_start.is_none() {
                watermark_start = Some(y);
            }
            watermark_end = y;
        }
    }

    if let Some(start) = watermark_start {
        let crop_top = (start as u32).min(h.saturating_sub(1));
        if crop_top > 0 && crop_top < h {
            *img = img.crop_imm(0, crop_top, w, h - crop_top);
            eprintln!(
                "[crop_watermark] cropped top {}px (watermark rows {}-{})",
                crop_top, start, watermark_end
            );
        }
    }
}

/// Encode a DynamicImage as JPEG (quality 85) and write to disk.
pub fn capture_frame_jpeg(img: &DynamicImage, path: &Path) -> Result<(), String> {
    let rgb = img.to_rgb8();
    let mut buf = std::io::Cursor::new(Vec::new());
    let mut encoder = image::codecs::jpeg::JpegEncoder::new_with_quality(&mut buf, 85);
    encoder
        .encode(&rgb, img.width(), img.height(), image::ExtendedColorType::Rgb8)
        .map_err(|e| format!("JPEG encode: {}", e))?;
    std::fs::write(path, buf.into_inner()).map_err(|e| format!("write JPEG: {}", e))?;
    Ok(())
}

/// Quick pixel-diff check: has the screen changed significantly between two frames?
pub fn images_differ_significantly(a: &DynamicImage, b: &DynamicImage, threshold: u8) -> bool {
    let (w_a, h_a) = (a.width(), a.height());
    let (w_b, h_b) = (b.width(), b.height());
    if w_a != w_b || h_a != h_b {
        return true;
    }
    let buf_a = a.to_rgb8();
    let buf_b = b.to_rgb8();
    let step = 8;
    let mut diff_count: u64 = 0;
    let mut total: u64 = 0;
    for y in (0..h_a as usize).step_by(step) {
        for x in (0..w_a as usize).step_by(step) {
            total += 1;
            let idx = (y * w_a as usize + x) * 3;
            let pa = &buf_a.as_raw()[idx..idx + 3];
            let pb = &buf_b.as_raw()[idx..idx + 3];
            let dr = (pa[0] as i32 - pb[0] as i32).unsigned_abs() as u8;
            let dg = (pa[1] as i32 - pb[1] as i32).unsigned_abs() as u8;
            let db = (pa[2] as i32 - pb[2] as i32).unsigned_abs() as u8;
            if dr > threshold || dg > threshold || db > threshold {
                diff_count += 1;
            }
        }
    }
    if total == 0 {
        return false;
    }
    let ratio = diff_count as f64 / total as f64;
    ratio > 0.02
}

// ── Click annotation (website-style red circle) ──────────────────────

/// Draw an anti-aliased hollow circle stroke with the given color.
fn draw_circle_stroke(
    img: &mut DynamicImage,
    cx: i32,
    cy: i32,
    radius: f64,
    stroke: f64,
    color: [u8; 3],
) {
    let w = img.width() as i32;
    let h = img.height() as i32;
    let buf = img.as_mut_rgba8().expect("xcap produces RGBA8");
    let half = stroke / 2.0;
    let inner_r = radius - half;
    let outer_r = radius + half;
    let rr = outer_r.ceil() as i32 + 1;
    for dy in -rr..=rr {
        for dx in -rr..=rr {
            let px = cx + dx;
            let py = cy + dy;
            if px < 0 || px >= w || py < 0 || py >= h {
                continue;
            }
            let d = ((dx as f64) * (dx as f64) + (dy as f64) * (dy as f64)).sqrt();
            if d >= inner_r && d <= outer_r {
                // 边缘 1px 抗锯齿渐变
                let alpha = if d < inner_r + 1.0 {
                    (d - inner_r).clamp(0.0, 1.0)
                } else {
                    (outer_r - d).clamp(0.0, 1.0)
                };
                buf.put_pixel(
                    px as u32,
                    py as u32,
                    image::Rgba([color[0], color[1], color[2], (255.0 * alpha) as u8]),
                );
            }
        }
    }
}

/// Draw the click annotation on the image:
///   - a red thin hollow circle centered at the click point (website editor
///     style: #E74C3C, 2.5px stroke, no number badge, no center dot).
/// `step_no` is kept for API compatibility but no longer rendered.
pub fn draw_click_marker(img: &mut DynamicImage, x: i32, y: i32, step_no: i32) {
    draw_click_marker_with_bounds(img, x, y, step_no, None);
}

/// Core drawing routine for `draw_click_marker`. `bounds` is kept for API
/// compatibility but ignored: the website-style annotation is a plain circle
/// centered at the click point, not an element-bounding box.
pub fn draw_click_marker_with_bounds(
    img: &mut DynamicImage,
    x: i32,
    y: i32,
    step_no: i32,
    bounds: Option<(i32, i32, i32, i32)>,
) {
    const RED: [u8; 3] = [0xE7, 0x4C, 0x3C]; // #E74C3C
    let _ = (step_no, bounds);
    draw_circle_stroke(img, x, y, 20.0, 2.5, RED);
}

// ── UIA text retrieval (StepSnap-style, for keyboard steps) ──────────

/// Read the current text value from the focused UIA element.
///
/// Priority: TextPattern2::GetValue → ValuePattern::GetValue.
///
/// Returns `(text, is_password)`. `is_password` 依据 UIA `IsPassword` 属性判断
/// （Windows 原生密码框会设置该属性）；password 为 true 时调用方不得将
/// text 明文入库/外发，应替换为 `[密码已隐藏]`。
pub fn get_focused_element_text() -> Option<(String, bool)> {
    use windows::Win32::System::Com::{
        CoCreateInstance, CoInitializeEx, CLSCTX_ALL, COINIT_MULTITHREADED,
    };
    use windows::Win32::UI::Accessibility::{
        CUIAutomation, IUIAutomation, IUIAutomationTextRange, IUIAutomationTextPattern,
        IUIAutomationValuePattern, UIA_TextPattern2Id, UIA_ValuePatternId,
    };

    unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);

        let automation: IUIAutomation =
            CoCreateInstance(&CUIAutomation, None, CLSCTX_ALL).ok()?;
        let focused = automation.GetFocusedElement().ok()?;

        // 密码框检测：IsPassword=true 时返回的文本可能是明文（少量场景），
        // 必须由调用方脱敏处理。
        let is_password = focused
            .CurrentIsPassword()
            .map(|b| b.0 != 0)
            .unwrap_or(false);

        // 1) TextPattern2 → DocumentRange → GetText (StepSnap priority path)
        if let Ok(tp) = focused.GetCurrentPatternAs::<IUIAutomationTextPattern>(UIA_TextPattern2Id) {
            if let Ok(range) = tp.DocumentRange() {
                let range: IUIAutomationTextRange = range;
                if let Ok(text) = range.GetText(-1) {
                    let s = text.to_string();
                    if !s.is_empty() {
                        return Some((s, is_password));
                    }
                }
            }
        }

        // 2) ValuePattern current value (fallback)
        if let Ok(vp) = focused.GetCurrentPatternAs::<IUIAutomationValuePattern>(UIA_ValuePatternId) {
            if let Ok(s) = vp.CurrentValue() {
                let text = s.to_string();
                if !text.is_empty() {
                    return Some((text, is_password));
                }
            }
        }

        None
    }
}

/// Read the bounding rectangle (screen coordinates) of the currently focused
/// UIA element. Returns `(left, top, width, height)`.
/// Used to crop keyboard steps to the focused input area (Scribe-style)
/// instead of showing the full screen.
pub fn get_focused_element_rect() -> Option<(i32, i32, i32, i32)> {
    use windows::Win32::System::Com::{
        CoCreateInstance, CoInitializeEx, CLSCTX_ALL, COINIT_MULTITHREADED,
    };
    use windows::Win32::UI::Accessibility::{CUIAutomation, IUIAutomation};

    unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);

        let automation: IUIAutomation =
            CoCreateInstance(&CUIAutomation, None, CLSCTX_ALL).ok()?;
        let focused = automation.GetFocusedElement().ok()?;
        let rect = focused.CurrentBoundingRectangle().ok()?;

        let w = rect.right - rect.left;
        let h = rect.bottom - rect.top;
        if w <= 0 || h <= 0 {
            return None;
        }
        Some((rect.left, rect.top, w, h))
    }
}

/// Read the current clipboard text (UTF-16 → String), if available.
/// Returns `None` when the clipboard is empty / non-text / locked.
pub fn get_clipboard_text() -> Option<String> {
    use windows::Win32::Foundation::{HGLOBAL, HWND};
    use windows::Win32::System::DataExchange::{
        CloseClipboard, GetClipboardData, OpenClipboard,
    };
    use windows::Win32::System::Memory::{GlobalLock, GlobalUnlock};

    // CF_UNICODETEXT = 13 (u32 clipboard format). Constant lives behind the
    // Win32_System_Ole feature; we keep the numeric value to avoid pulling in OLE.
    const CF_UNICODETEXT: u32 = 13u32;

    unsafe {
        if !OpenClipboard(HWND(std::ptr::null_mut())).is_ok() {
            return None;
        }
        let handle = match GetClipboardData(CF_UNICODETEXT) {
            Ok(h) => h,
            Err(_) => {
                let _ = CloseClipboard();
                return None;
            }
        };
        if handle.0.is_null() {
            let _ = CloseClipboard();
            return None;
        }
        let ptr = GlobalLock(HGLOBAL(handle.0)) as *const u16;
        if ptr.is_null() {
            let _ = GlobalUnlock(HGLOBAL(handle.0));
            let _ = CloseClipboard();
            return None;
        }
        // null-terminated UTF-16 → String
        let mut len = 0usize;
        while *ptr.add(len) != 0 {
            len += 1;
        }
        let s = String::from_utf16_lossy(std::slice::from_raw_parts(ptr, len));
        let _ = GlobalUnlock(HGLOBAL(handle.0));
        let _ = CloseClipboard();
        Some(s)
    }
}

#[cfg(test)]
mod screenshot_tests {
    use super::*;

    // ── next_screenshot_path ──────────────────────────────

    #[test]
    fn next_screenshot_path_format_and_location() {
        let path = next_screenshot_path("click");
        let file_name = path.file_name().unwrap().to_string_lossy().to_string();

        // 位于 temp/flowio_screenshots 下
        assert_eq!(path.parent().unwrap(), std::env::temp_dir().join("flowio_screenshots"));
        // 格式：click_{ts}_{counter}.jpg
        assert!(file_name.starts_with("click_"), "unexpected name: {}", file_name);
        assert!(file_name.ends_with(".jpg"), "unexpected name: {}", file_name);
        let parts: Vec<&str> = file_name.split('_').collect();
        assert_eq!(parts.len(), 3, "expected 3 parts in {}", file_name);
        // 中段是时间戳，末段是自增计数
        assert!(parts[1].parse::<u128>().is_ok());
        assert!(parts[2].trim_end_matches(".jpg").parse::<u64>().is_ok());
    }

    #[test]
    fn next_screenshot_path_counter_increments() {
        let a = next_screenshot_path("key");
        let b = next_screenshot_path("key");
        let get_counter = |p: &std::path::Path| {
            let n = p.file_name().unwrap().to_string_lossy().to_string();
            n.split('_').last().unwrap().trim_end_matches(".jpg").parse::<u64>().unwrap()
        };
        assert!(get_counter(&b) > get_counter(&a));
    }

    // ── crop_watermark ────────────────────────────────────

    fn solid_rgba(w: u32, h: u32, rgb: (u8, u8, u8)) -> DynamicImage {
        DynamicImage::ImageRgba8(image::RgbaImage::from_pixel(w, h, image::Rgba([rgb.0, rgb.1, rgb.2, 255])))
    }

    /// 构造「顶部红字深底」图像：y=10..30 为深色底 + 每行 10 个可被采样到的红像素
    fn top_red_watermark(w: u32, h: u32) -> DynamicImage {
        let dark = image::Rgba([36u8, 41, 46, 255]); // #24292e
        let red = image::Rgba([0xE7u8, 0x4C, 0x3C, 255]);
        let mut img = image::RgbaImage::from_pixel(w, h, dark);
        for y in 10..30u32 {
            // 采样 step=8 → 红像素放在 x=0,8,...,72
            for (i, x) in (0..80u32).step_by(8).enumerate() {
                let _ = i;
                img.put_pixel(x, y, red);
            }
        }
        DynamicImage::ImageRgba8(img)
    }

    #[test]
    fn crop_watermark_removes_top_red_on_dark() {
        let mut img = top_red_watermark(200, 200);
        crop_watermark(&mut img);
        // 顶部 10 行水印被裁剪，剩余 190 行
        assert_eq!(img.height(), 190);
        assert_eq!(img.width(), 200);
    }

    #[test]
    fn crop_watermark_keeps_clean_image_unchanged() {
        let mut img = solid_rgba(200, 150, (128, 128, 128));
        let before = img.clone();
        crop_watermark(&mut img);
        assert_eq!(img.width(), before.width());
        assert_eq!(img.height(), before.height());
    }

    #[test]
    fn crop_watermark_does_not_crop_pure_red_icon_row() {
        // 纯红桌面图标行：红像素很多但没有深色背景 → 不误裁
        let red = image::Rgba([255u8, 0, 0, 255]);
        let mut img = DynamicImage::ImageRgba8(image::RgbaImage::from_pixel(
            200, 200, red,
        ));
        crop_watermark(&mut img);
        assert_eq!(img.height(), 200, "纯红行不应被裁剪");
    }

    // ── images_differ_significantly ───────────────────────

    #[test]
    fn images_differ_significantly_size_mismatch_true() {
        let a = solid_rgba(100, 100, (0, 0, 0));
        let b = solid_rgba(50, 50, (0, 0, 0));
        assert!(images_differ_significantly(&a, &b, 16));
    }

    #[test]
    fn images_differ_significantly_same_image_false() {
        let a = solid_rgba(100, 100, (0, 0, 0));
        let b = solid_rgba(100, 100, (0, 0, 0));
        assert!(!images_differ_significantly(&a, &b, 16));
    }

    #[test]
    fn images_differ_significantly_big_diff_true() {
        // A 全黑；B 上半部分全白 → 采样点约半数差异
        let a = solid_rgba(100, 100, (0, 0, 0));
        let white = image::Rgba([255u8, 255, 255, 255]);
        let black = image::Rgba([0u8, 0, 0, 255]);
        let mut b = image::RgbaImage::from_pixel(100, 100, black);
        for y in 0..50u32 {
            for x in 0..100u32 {
                b.put_pixel(x, y, white);
            }
        }
        assert!(images_differ_significantly(&a, &DynamicImage::ImageRgba8(b), 16));
    }


    #[test]
    fn crop_watermark_tiny_image_no_panic() {
        let mut img = solid_rgba(1, 1, (0, 0, 0));
        crop_watermark(&mut img);
        assert_eq!(img.width(), 1);
        assert_eq!(img.height(), 1);
    }

    #[test]
    fn crop_watermark_watermark_at_bottom_of_scan_zone() {
        let img = top_red_watermark(200, 200);
        let mut rgba = img.to_rgba8();
        let red = image::Rgba([0xE7u8, 0x4C, 0x3C, 255]);
        for y in 80..100u32 {
            for (_, x) in (0..80u32).step_by(8).enumerate() {
                rgba.put_pixel(x, y, red);
            }
        }
        let mut img2 = DynamicImage::ImageRgba8(rgba);
        crop_watermark(&mut img2);
        // 第一处水印从 y=10 开始 → 裁剪后剩余 190 行
        assert_eq!(img2.height(), 190);
    }

    #[test]
    fn images_differ_significantly_small_diff_false() {
        let a = solid_rgba(100, 100, (128, 128, 128));
        let b = solid_rgba(100, 100, (130, 130, 130));
        assert!(!images_differ_significantly(&a, &b, 32));
    }

    // ── draw_click_marker_with_bounds (website-style red circle) ──

    #[test]
    fn draw_click_marker_draws_red_circle() {
        // Click at (100,100) on a 300x300 canvas; circle radius 20 centered there.
        let mut img = solid_rgba(300, 300, (255, 255, 255));
        draw_click_marker_with_bounds(&mut img, 100, 100, 1, None);
        let buf = img.to_rgba8();

        // Circle stroke: a red pixel on the ring (right point, x=120, y=100)
        let p = buf.get_pixel(120, 100);
        assert!(p.0[0] > 200 && p.0[1] < 100 && p.0[2] < 100, "circle stroke should be red");

        // Center must stay white (no number badge / no center dot)
        let c = buf.get_pixel(100, 100);
        assert!(c.0[0] > 200 && c.0[1] > 200 && c.0[2] > 200, "circle center should be empty");
    }
}
