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

// ── Scribe-style click annotation ────────────────────────────────────

/// Draw a small filled red dot (center marker) at the given point.
pub fn draw_scribe_red_dot(img: &mut DynamicImage, cx: i32, cy: i32, radius: f64) {
    let w = img.width() as i32;
    let h = img.height() as i32;
    let buf = img.as_mut_rgba8().expect("xcap produces RGBA8");
    let rr = radius.ceil() as i32;
    for dy in -rr..=rr {
        for dx in -rr..=rr {
            let px = cx + dx;
            let py = cy + dy;
            if px < 0 || px >= w || py < 0 || py >= h { continue; }
            let d = ((dx as f64) * (dx as f64) + (dy as f64) * (dy as f64)).sqrt();
            if d <= radius {
                let alpha = if d > radius - 1.0 { (radius - d).clamp(0.0, 1.0) } else { 1.0 };
                buf.put_pixel(px as u32, py as u32,
                    image::Rgba([0xE7, 0x4C, 0x3C, (255.0 * alpha) as u8]));
            }
        }
    }
}

/// Draw a Scribe-style red circle marker on the image. #E74C3C, 20px radius, 2.5px stroke.
pub fn draw_scribe_red_circle(img: &mut DynamicImage, cx: i32, cy: i32) {
    let radius: f64 = 20.0;
    let stroke: f64 = 2.5;
    let inner_r = radius - stroke / 2.0;
    let outer_r = radius + stroke / 2.0;

    let w = img.width() as i32;
    let h = img.height() as i32;
    let buf = img.as_mut_rgba8().expect("xcap produces RGBA8");

    let rr = (outer_r.ceil() as i32).max(1);
    for dy in -rr..=rr {
        for dx in -rr..=rr {
            let px = cx + dx;
            let py = cy + dy;
            if px < 0 || px >= w || py < 0 || py >= h { continue; }
            let d = ((dx as f64) * (dx as f64) + (dy as f64) * (dy as f64)).sqrt();

            if d >= inner_r && d <= outer_r {
                let alpha: f32 = if d < inner_r + 1.0 {
                    ((d - inner_r) as f32).clamp(0.0, 1.0)
                } else if d > outer_r - 1.0 {
                    ((outer_r - d) as f32).clamp(0.0, 1.0)
                } else {
                    1.0
                };
                let a = (255.0 * alpha) as u8;
                buf.put_pixel(px as u32, py as u32, image::Rgba([0xE7u8, 0x4Cu8, 0x3Cu8, a]));
            }
        }
    }
    draw_scribe_red_dot(img, cx, cy, 3.0);
}

// ── UIA text retrieval (StepSnap-style, for keyboard steps) ──────────

/// Read the current text value from the focused UIA element.
///
/// Priority: TextPattern2::GetValue → ValuePattern::GetValue.
/// Returns the text string, or None if no accessible focused element.
pub fn get_focused_element_text() -> Option<String> {
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

        // 1) TextPattern2 → DocumentRange → GetText (StepSnap priority path)
        if let Ok(tp) = focused.GetCurrentPatternAs::<IUIAutomationTextPattern>(UIA_TextPattern2Id) {
            if let Ok(range) = tp.DocumentRange() {
                let range: IUIAutomationTextRange = range;
                if let Ok(text) = range.GetText(-1) {
                    let s = text.to_string();
                    if !s.is_empty() {
                        return Some(s);
                    }
                }
            }
        }

        // 2) ValuePattern current value (fallback)
        if let Ok(vp) = focused.GetCurrentPatternAs::<IUIAutomationValuePattern>(UIA_ValuePatternId) {
            if let Ok(s) = vp.CurrentValue() {
                let text = s.to_string();
                if !text.is_empty() {
                    return Some(text);
                }
            }
        }

        None
    }
}
