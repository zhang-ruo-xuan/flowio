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

/// Current mouse position in virtual-screen coordinates.
pub fn get_cursor_pos() -> Option<(i32, i32)> {
    use windows::Win32::Foundation::POINT;
    use windows::Win32::UI::WindowsAndMessaging::GetCursorPos;
    unsafe {
        let mut p = POINT::default();
        if GetCursorPos(&mut p).is_ok() {
            Some((p.x, p.y))
        } else {
            None
        }
    }
}

/// Capture the monitor the user is actually working on.
///
/// Returns `(image, monitor_x, monitor_y)` where the origin is the monitor's
/// top-left corner in virtual-screen coordinates, so callers can translate
/// global input coordinates (mouse clicks, UIA rectangles) into image space
/// with a simple subtraction.
///
/// Picking the cursor's monitor matters: the old code always took the first
/// monitor, so on a multi-monitor setup every step was captured from the
/// wrong screen.
pub fn capture_fullscreen_with_origin() -> Result<(DynamicImage, i32, i32), String> {
    let monitors = xcap::Monitor::all()
        .map_err(|e| format!("xcap: failed to enumerate monitors: {}", e))?;
    if monitors.is_empty() {
        return Err("xcap: no monitor found".to_string());
    }
    let (cx, cy) = get_cursor_pos().unwrap_or((0, 0));
    let monitor = monitors
        .iter()
        .find(|m| {
            let (mx, my) = (m.x(), m.y());
            let (mw, mh) = (m.width() as i32, m.height() as i32);
            cx >= mx && cx < mx + mw && cy >= my && cy < my + mh
        })
        .or_else(|| monitors.iter().find(|m| m.is_primary()))
        .unwrap_or(&monitors[0]);
    let (ox, oy) = (monitor.x(), monitor.y());
    let img = monitor
        .capture_image()
        .map_err(|e| format!("xcap: failed to capture image: {}", e))?;
    Ok((DynamicImage::ImageRgba8(img), ox, oy))
}

/// Capture the monitor under the cursor as a DynamicImage.
///
/// The image is **never cropped to the active window**: the editor's 全图 view
/// must show the whole screen, and window-cropping produced the broken
/// narrow strips users reported.
pub fn capture_fullscreen() -> Result<DynamicImage, String> {
    capture_fullscreen_with_origin().map(|(img, _, _)| img)
}

/// Crop the top of an image to remove Flowio's own recording status bar.
///
/// The REC bar has a dark background (#24292e-ish) with red text (R>180, G<80, B<80).
/// Scans the top ~100 px rows. A row is considered watermark only if it has both:
///   - enough red pixels (>5)
///   - a dark background characteristic (majority of row's non-red pixels are dark, L<60)
/// This prevents false positives from red desktop icons or wallpaper elements.
/// If found, crops from the first detected row to just below the last detected row.
/// Returns the number of pixels cropped from the top (0 when no watermark was found),
/// so callers can adjust crop coordinates that refer to the pre-crop image space.
pub fn crop_watermark(img: &mut DynamicImage) -> usize {
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
            return crop_top as usize;
        }
    }
    0
}

// ── Crop geometry ────────────────────────────────────────────

/// Build a crop box centred on `(cx, cy)` that is guaranteed to sit fully
/// inside the image.
///
/// Solves the two ways the old code produced a broken thumbnail:
///   - a tight element box near a screen edge was clamped by the caller, which
///     slid the whole box sideways and pushed the element out of the centre
///     (or cut it in half);
///   - a 20 px-tall text box yielded an unreadable sliver.
///
/// The box is grown to at least `min_w * min_h` around the anchor and then
/// **shifted** — never resized — back inside the image. The centre returned is
/// the final one, so re-deriving the rectangle from `(centre, w, h)` on the
/// frontend reproduces exactly this box.
///
/// Returns `(centre_x, centre_y, width, height)` in image coordinates.
pub fn centered_crop_box(
    cx: i32,
    cy: i32,
    w: i32,
    h: i32,
    img_w: i32,
    img_h: i32,
    min_w: i32,
    min_h: i32,
) -> (i32, i32, i32, i32) {
    if img_w <= 0 || img_h <= 0 {
        return (cx, cy, w.max(1), h.max(1));
    }
    let bw = w.max(1).max(min_w).min(img_w);
    let bh = h.max(1).max(min_h).min(img_h);
    let x0 = (cx - bw / 2).clamp(0, img_w - bw);
    let y0 = (cy - bh / 2).clamp(0, img_h - bh);
    (x0 + bw / 2, y0 + bh / 2, bw, bh)
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

/// Read the currently **selected** text in the focused UIA element via
/// `IUIAutomationTextPattern::GetSelection`. Used to populate the description
/// for Ctrl+A / Select-All steps (the clipboard does NOT change on select-all,
/// so `get_clipboard_text` is wrong for this case).
///
/// Falls back to the entire document range when no selection exists, so the
/// description still carries the text the user "selected".
pub fn get_focused_selected_text() -> Option<String> {
    use windows::Win32::System::Com::{
        CoCreateInstance, CoInitializeEx, CLSCTX_ALL, COINIT_MULTITHREADED,
    };
    use windows::Win32::UI::Accessibility::{
        CUIAutomation, IUIAutomation, IUIAutomationTextPattern, UIA_TextPattern2Id,
    };

    unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);

        let automation: IUIAutomation =
            CoCreateInstance(&CUIAutomation, None, CLSCTX_ALL).ok()?;
        let focused = automation.GetFocusedElement().ok()?;
        let tp: IUIAutomationTextPattern =
            focused.GetCurrentPatternAs(UIA_TextPattern2Id).ok()?;

        // 优先：GetSelection 拿到的是用户实际选中的 range 数组（Chromium/Edge/Edit 控件）。
        let selections = tp.GetSelection().ok()?;
        let mut picked: Option<String> = None;
        if let Ok(len) = selections.Length() {
            if len > 0 {
                let mut parts: Vec<String> = Vec::new();
                for i in 0..len {
                    if let Ok(range) = selections.GetElement(i) {
                        if let Ok(text) = range.GetText(-1) {
                            let s = text.to_string();
                            if !s.is_empty() {
                                parts.push(s);
                            }
                        }
                    }
                }
                if !parts.is_empty() {
                    picked = Some(parts.join(""));
                }
            }
        }

        // 兜底：控件不支持多 selection 时，读整个文档范围作为"被全选的文本"。
        if picked.is_none() {
            if let Ok(range) = tp.DocumentRange() {
                if let Ok(text) = range.GetText(-1) {
                    let s = text.to_string();
                    if !s.is_empty() {
                        picked = Some(s);
                    }
                }
            }
        }

        picked
    }
}

/// Bounding rectangle (screen coordinates) of the text the user has currently
/// selected, via UIA `TextPattern::GetSelection` →
/// `TextRange::GetBoundingRectangles`.
///
/// This is the precise answer to "where did the copy happen". The focused
/// element is useless for that: in a browser it is the whole document, and a
/// few hundred milliseconds later — once the user has pasted somewhere — it
/// reports the paste target instead. Both failure modes are what made copy
/// steps crop to the find-in-page bar.
///
/// Returns `(left, top, width, height)`; `None` when there is no selection or
/// the control does not expose text geometry.
pub fn get_selected_text_rect() -> Option<(i32, i32, i32, i32)> {
    use windows::Win32::System::Com::{
        CoCreateInstance, CoInitializeEx, CLSCTX_ALL, COINIT_MULTITHREADED,
    };
    use windows::Win32::System::Ole::{
        SafeArrayAccessData, SafeArrayDestroy, SafeArrayUnaccessData,
    };
    use windows::Win32::UI::Accessibility::{
        CUIAutomation, IUIAutomation, IUIAutomationTextPattern, UIA_TextPattern2Id,
    };

    unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);

        let automation: IUIAutomation =
            CoCreateInstance(&CUIAutomation, None, CLSCTX_ALL).ok()?;
        let focused = automation.GetFocusedElement().ok()?;
        let tp: IUIAutomationTextPattern =
            focused.GetCurrentPatternAs(UIA_TextPattern2Id).ok()?;
        let selections = tp.GetSelection().ok()?;
        let len = selections.Length().ok()?;
        if len <= 0 {
            return None;
        }

        let (mut min_x, mut min_y) = (f64::MAX, f64::MAX);
        let (mut max_x, mut max_y) = (f64::MIN, f64::MIN);
        let mut found = false;

        for i in 0..len {
            let Ok(range) = selections.GetElement(i) else { continue };
            let Ok(sa) = range.GetBoundingRectangles() else { continue };
            if sa.is_null() {
                continue;
            }
            let mut data: *mut std::ffi::c_void = std::ptr::null_mut();
            if SafeArrayAccessData(sa, &mut data).is_err() || data.is_null() {
                let _ = SafeArrayDestroy(sa);
                continue;
            }
            // 每 4 个 double 描述一个矩形：x, y, width, height
            let quads = ((*sa).rgsabound[0].cElements as usize) / 4;
            let vals = std::slice::from_raw_parts(data as *const f64, quads * 4);
            for q in vals.chunks_exact(4) {
                let (x, y, w, h) = (q[0], q[1], q[2], q[3]);
                if w <= 0.0 || h <= 0.0 {
                    continue;
                }
                found = true;
                min_x = min_x.min(x);
                min_y = min_y.min(y);
                max_x = max_x.max(x + w);
                max_y = max_y.max(y + h);
            }
            let _ = SafeArrayUnaccessData(sa);
            let _ = SafeArrayDestroy(sa);
        }

        if !found {
            return None;
        }
        let left = min_x.round() as i32;
        let top = min_y.round() as i32;
        let w = (max_x - min_x).round() as i32;
        let h = (max_y - min_y).round() as i32;
        if w < 4 || h < 4 {
            return None;
        }
        Some((left, top, w, h))
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

/// Same as `get_focused_element_rect`, but additionally validates the rectangle
/// against the captured image bounds (`img_w` x `img_h`).
///
/// Some apps (notably Chromium-based browsers) report *document/virtual*
/// coordinates for the focused element, which can be far outside the visible
/// screen (e.g. y=8429 for a link deep in a scrolled page). Cropping on those
/// coordinates yields garbage thumbnails, so we require:
///   - the rect is inside the image bounds (at least 50% overlap),
///   - both dimensions are within a sane range [8, image size].
/// Anything else returns `None`, letting callers fall back to the window-area
/// crop instead of an invalid region.
/// Focused-element rectangle expressed in **window-image coordinates**.
///
/// The previous heuristic (`get_focused_element_rect_checked`) compared UIA
/// *screen* coordinates against the *window image* size, which mixed two
/// coordinate systems: for any window that is not at (0,0) a perfectly valid
/// rectangle could be rejected (→ fallback to whole window → "empty" crop),
/// while a bogus one could slip through (→ blank crop).
///
/// This variant takes the window's screen rect and returns `(x, y, w, h)`
/// already translated into the window-cropped image space, so callers only
/// need to subtract the watermark cut from `y`.
///
/// Returns `None` when the element is unusable: degenerate size, larger than
/// the window, or less than 50% visible inside it (Chromium reports
/// document/virtual coords such as y=8429 for off-screen content).
pub fn get_focused_element_rect_in_window(
    win_x: i32,
    win_y: i32,
    win_w: i32,
    win_h: i32,
) -> Option<(i32, i32, i32, i32)> {
    let (lx, ty, w, h) = get_focused_element_rect()?;
    if w < 8 || h < 8 || w > win_w || h > win_h {
        return None;
    }
    let vis_w = (lx + w).min(win_x + win_w) - lx.max(win_x);
    let vis_h = (ty + h).min(win_y + win_h) - ty.max(win_y);
    if vis_w <= 0 || vis_h <= 0 {
        return None;
    }
    let overlap = (vis_w as f64 / w as f64).min(vis_h as f64 / h as f64);
    if overlap < 0.5 {
        return None;
    }
    Some((lx - win_x, ty - win_y, w, h))
}

/// Translate a UIA **screen-space** rectangle into image space, clipping it to
/// the visible part of the captured monitor.
///
/// Chromium reports document/virtual coordinates for in-page content (y=8429
/// for a link far down a scrolled page), so a raw rectangle is unusable as a
/// crop anchor. Clipping keeps whatever is actually on screen — for a
/// select-all + copy that is the visible top of the selection, which is
/// exactly the part the reader needs to see.
///
/// Returns `None` when nothing meaningful remains visible.
pub fn screen_rect_in_image(
    rect: (i32, i32, i32, i32),
    origin_x: i32,
    origin_y: i32,
    img_w: i32,
    img_h: i32,
) -> Option<(i32, i32, i32, i32)> {
    let (lx, ty, w, h) = rect;
    if w < 4 || h < 4 {
        return None;
    }
    let x0 = lx.max(origin_x);
    let y0 = ty.max(origin_y);
    let x1 = (lx + w).min(origin_x + img_w);
    let y1 = (ty + h).min(origin_y + img_h);
    if x1 - x0 < 4 || y1 - y0 < 4 {
        return None;
    }
    Some((x0 - origin_x, y0 - origin_y, x1 - x0, y1 - y0))
}

/// Bounding box of the pixels that differ between two same-sized images.
///
/// This is the key to "crop exactly what the shortcut changed": comparing the
/// frame captured at keystroke time (`a`) with a live capture taken after the
/// UI settled (`b`) yields the region where the app actually reacted — the
/// find-in-page bar for Ctrl+F, the selection highlight for Ctrl+A, the
/// inserted text for Ctrl+V.
///
/// `ignore_top` skips the first N rows (residual recording-watermark band).
///
/// Rejects two failure modes and returns `None`:
///   - change ratio < 0.08% → just noise (blinking caret, clock ticks);
///   - change ratio > 55%   → the whole view changed (window switch, page
///     navigation) so a bounding box carries no information.
/// Also rejects boxes too small to be a meaningful crop target.
pub fn changed_region(
    a: &DynamicImage,
    b: &DynamicImage,
    threshold: u8,
    ignore_top: u32,
) -> Option<(i32, i32, i32, i32)> {
    if a.width() != b.width() || a.height() != b.height() {
        return None;
    }
    let (w, h) = (a.width() as i32, a.height() as i32);
    if w <= 0 || h <= 0 {
        return None;
    }
    let ra = a.to_rgb8();
    let rb = b.to_rgb8();
    let (sa, sb) = (ra.as_raw(), rb.as_raw());
    let step = 4usize;
    let start_y = (ignore_top as usize).min(h as usize);

    let (mut min_x, mut min_y) = (i32::MAX, i32::MAX);
    let (mut max_x, mut max_y) = (-1i32, -1i32);
    let (mut hits, mut total) = (0u32, 0u32);
    // i32::unsigned_abs() yields u32, so the threshold must be u32 too.
    let t = threshold as u32;

    for y in (start_y..h as usize).step_by(step) {
        let row = y * w as usize;
        for x in (0..w as usize).step_by(step) {
            let i = (row + x) * 3;
            let dr = (sa[i] as i32 - sb[i] as i32).unsigned_abs();
            let dg = (sa[i + 1] as i32 - sb[i + 1] as i32).unsigned_abs();
            let db = (sa[i + 2] as i32 - sb[i + 2] as i32).unsigned_abs();
            total += 1;
            if dr > t || dg > t || db > t {
                hits += 1;
                let (xi, yi) = (x as i32, y as i32);
                if xi < min_x {
                    min_x = xi;
                }
                if yi < min_y {
                    min_y = yi;
                }
                if xi > max_x {
                    max_x = xi;
                }
                if yi > max_y {
                    max_y = yi;
                }
            }
        }
    }

    if max_x < 0 || total == 0 {
        return None;
    }
    let ratio = hits as f64 / total as f64;
    // Lower bound: a real UI change (find bar, selection highlight) covers far
    // more than a blinking caret, which the 24x16 size guard below already
    // rejects — so the bar can stay low without letting noise through.
    //
    // Upper bound is deliberately generous (0.85). A very large changed area
    // is *not* a failure: "select all" legitimately repaints most of the
    // document. Rejecting it makes the caller fall back to the full-window
    // image, which is never tighter than a big bounding box — so accepting is
    // always at least as good as giving up. Only a genuinely total repaint
    // (ratio ~1.0, i.e. the whole viewport scrolled or the app was replaced)
    // is rejected, because there the "changed region" carries no information.
    if ratio < 0.0004 || ratio > 0.85 {
        return None;
    }

    let pad = 12i32;
    let x0 = (min_x - pad).max(0);
    let y0 = (min_y - pad).max(0);
    let x1 = (max_x + step as i32 + pad).min(w);
    let y1 = (max_y + step as i32 + pad).min(h);
    let (bw, bh) = (x1 - x0, y1 - y0);
    if bw < 24 || bh < 16 {
        return None;
    }
    Some((x0, y0, bw, bh))
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
    use image::{GenericImage, ImageBuffer, Rgb};

    /// Build an all-black RGB image of the requested size.
    fn black(w: u32, h: u32) -> DynamicImage {
        DynamicImage::ImageRgb8(ImageBuffer::<Rgb<u8>, Vec<u8>>::from_pixel(w, h, Rgb([0, 0, 0])))
    }

    /// Paint a white rectangle, simulating "UI appeared here".
    fn paint(img: &mut DynamicImage, x: u32, y: u32, w: u32, h: u32) {
        let buf = img.as_mut_rgb8().expect("test images are Rgb8");
        for yy in y..(y + h).min(buf.height()) {
            for xx in x..(x + w).min(buf.width()) {
                buf.put_pixel(xx, yy, Rgb([255, 255, 255]));
            }
        }
    }

    // ── changed_region ────────────────────────────────────

    #[test]
    fn changed_region_locates_the_appearing_ui() {
        let mut before = black(400, 300);
        let mut after = before.clone();
        // 一个 120x40 的"查找框"出现在右上角
        paint(&mut after, 240, 20, 120, 40);

        let (x, y, w, h) = changed_region(&before, &after, 24, 0).expect("should detect change");
        // 外扩 12px padding，且采样步长为 4
        assert!(x <= 240 && y <= 20, "box must start at/inside the change: {x},{y}");
        assert!(x + w >= 360 && y + h >= 60, "box must cover the change: {x},{y},{w},{h}");
        assert!(w < 200 && h < 120, "box must stay local: {w}x{h}");
        let _ = &mut before;
    }

    #[test]
    fn changed_region_rejects_identical_images() {
        let a = black(200, 200);
        let b = a.clone();
        assert!(changed_region(&a, &b, 24, 0).is_none());
    }

    #[test]
    fn changed_region_rejects_full_view_change() {
        let before = black(200, 200);
        let mut after = before.clone();
        paint(&mut after, 0, 0, 200, 200);
        // 整屏都变了 → 边界框没有信息量
        assert!(changed_region(&before, &after, 24, 0).is_none());
    }

    #[test]
    fn changed_region_rejects_mismatched_sizes() {
        let a = black(200, 200);
        let b = black(200, 199);
        assert!(changed_region(&a, &b, 24, 0).is_none());
    }

    #[test]
    fn changed_region_ignores_top_band() {
        let before = black(400, 300);
        let mut after = before.clone();
        // 顶部 10px 是残留水印带，必须被忽略
        paint(&mut after, 0, 0, 400, 10);
        assert!(changed_region(&before, &after, 24, 12).is_none());
    }

    /// "全选" selects most of the document: the changed area is large but it is
    /// still the region the reader cares about. Rejecting it made the caller
    /// fall back to the full-window image, which is never tighter — this is the
    /// regression the 0.85 upper bound exists for.
    #[test]
    fn changed_region_accepts_large_selection_change() {
        let before = black(400, 400);
        let mut after = before.clone();
        // 覆盖 ~70% 面积（400x400 中 y=120..400）
        paint(&mut after, 20, 120, 360, 280);
        let r = changed_region(&before, &after, 24, 6).expect("大面积选区应被接受");
        assert!(r.2 > 300, "宽度应覆盖选区: {:?}", r);
        assert!(r.3 > 250, "高度应覆盖选区: {:?}", r);
        // 仍然比整窗紧：上方未变化的 120px 不该被包进来
        assert!(r.1 >= 100, "不应把未变化的顶部区域包进来: {:?}", r);
    }

    // ── centered_crop_box ─────────────────────────────────

    #[test]
    fn centered_crop_box_keeps_anchor_centred() {
        let (cx, cy, w, h) = centered_crop_box(500, 400, 200, 100, 1920, 1080, 420, 280);
        assert_eq!((w, h), (420, 280), "放大到最小尺寸");
        assert_eq!((cx, cy), (500, 400), "锚点仍在正中");
    }

    #[test]
    fn centered_crop_box_shifts_instead_of_shrinking_at_edge() {
        // 锚点贴左边界：裁剪框整体右移，不能被缩小（缩小=内容被切一半）
        let (cx, cy, w, h) = centered_crop_box(10, 400, 200, 100, 1920, 1080, 420, 280);
        assert_eq!((w, h), (420, 280), "尺寸不变");
        assert_eq!(cx - w / 2, 0, "整体平移贴到左边界");
        assert_eq!(cy, 400);
    }

    #[test]
    fn centered_crop_box_clamps_bottom_right() {
        let (cx, cy, w, h) = centered_crop_box(1910, 1070, 200, 100, 1920, 1080, 420, 280);
        assert_eq!(cx + w / 2, 1920, "右边界内");
        assert_eq!(cy + h / 2, 1080, "下边界内");
    }

    #[test]
    fn centered_crop_box_never_bigger_than_image() {
        let (_, _, w, h) = centered_crop_box(5, 5, 5000, 5000, 300, 200, 420, 280);
        assert_eq!((w, h), (300, 200));
    }

    // ── screen_rect_in_image ──────────────────────────────

    #[test]
    fn screen_rect_in_image_translates_origin() {
        // 副屏在左侧：原点 (-1920, 0)
        assert_eq!(
            screen_rect_in_image((-1820, 100, 200, 50), -1920, 0, 1920, 1080),
            Some((100, 100, 200, 50))
        );
    }

    #[test]
    fn screen_rect_in_image_clips_offscreen_document_rect() {
        // Chromium 虚拟坐标：一个滚到页面深处的元素（y 远超屏幕）
        assert_eq!(
            screen_rect_in_image((100, 8000, 300, 4000), 0, 0, 1920, 1080),
            None,
            "完全在屏幕外"
        );
        // 部分可见：裁出可见的那一段
        assert_eq!(
            screen_rect_in_image((100, 900, 300, 4000), 0, 0, 1920, 1080),
            Some((100, 900, 300, 180))
        );
    }

    #[test]
    fn screen_rect_in_image_rejects_degenerate() {
        assert_eq!(screen_rect_in_image((0, 0, 2, 2), 0, 0, 1920, 1080), None);
    }

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
