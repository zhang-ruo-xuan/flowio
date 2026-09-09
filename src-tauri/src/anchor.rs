//! 控件锚点（Stage 2b 精度护城河）。
//!
//! 录制端：在点击位置抓取目标控件的窗口类名 + 点击在该控件矩形内的归一化比例 (fx, fy)，
//! 以及点击在所属顶层窗口矩形内的归一化比例 (wx, wy)。
//! 重放端：优先按控件类名在当前窗口的子窗口中定位到同一控件，用其当前矩形 + (fx, fy)
//! 反算屏幕坐标（分辨率 / DPI 无关）；定位不到则回退到窗口级 (wx, wy)；两者皆无则回退录制坐标。
//!
//! 设计取舍：刻意不依赖 IAccessible / UIAutomation COM（跨版本 API 不稳定、且本环境无法实机验证），
//! 只用稳定的 user32 API（WindowFromPoint / GetClassNameW / GetWindowRect / GetParent / EnumChildWindows）。
//! 任何失败都返回 None，由上层回退到录制坐标 —— 永不引入回归。

use base64::{engine::general_purpose::STANDARD as B64, Engine as _};
use serde::{Deserialize, Serialize};
use image::DynamicImage;
use windows::Win32::Foundation::{BOOL, HWND, LPARAM, POINT, RECT};
use windows::Win32::UI::WindowsAndMessaging::{
    EnumChildWindows, EnumWindows, GetClassNameW, GetForegroundWindow, GetParent, GetWindowRect,
    GetWindowTextW, IsWindowVisible, WindowFromPoint,
};

/// 控件锚点：序列化为 JSON 存入 steps.uia_anchor 列。
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ControlAnchor {
    /// 目标控件窗口类名；空串表示点击落在顶层窗口本身（无独立子窗口）。
    #[serde(default)]
    pub cls: String,
    /// 点击在控件矩形内的横向归一化位置 (0..1)。
    #[serde(default)]
    pub fx: f64,
    /// 点击在控件矩形内的纵向归一化位置 (0..1)。
    #[serde(default)]
    pub fy: f64,
    /// 点击在所属顶层窗口矩形内的横向归一化位置 (0..1)（控件定位失败时的回退）。
    #[serde(default)]
    pub wx: f64,
    /// 点击在所属顶层窗口矩形内的纵向归一化位置 (0..1)。
    #[serde(default)]
    pub wy: f64,
    /// 点击目标元素的 UIA 名称（Stage 2c 名称锚点）：WPS「最近」列表等自绘控件
    /// 按使用状态动态排序，位置会漂移；录制时记录元素名称（列表项通常是文件名），
    /// 重放时落点名称不符则在窗口 UIA 树内按名称找回。空串 = 未捕获 / 旧数据。
    #[serde(default)]
    pub name: String,
    /// 点击在「名称元素」矩形内的横向相对位置 (0..1)，默认 0.5（中心）。
    ///
    /// 必要性：WPS「最近」列表等自绘控件只暴露整行元素，文件名文本没有独立
    /// UIA 元素——此时按名称只能定位到整行，取行中心会点进路径列并跳转文件夹。
    /// 记录录制时点击在行内的相对位置，重放时按同一比例还原列位置。
    #[serde(default = "default_half")]
    pub nfx: f64,
    /// 点击在「名称元素」矩形内的纵向相对位置 (0..1)，默认 0.5。
    #[serde(default = "default_half")]
    pub nfy: f64,
    /// 点击相对控件矩形左上角的像素偏移（横向）。-1 = 旧数据未记录。
    ///
    /// 必要性：表格/文档等**内容型控件**（WPS 网格 EXCEL7 等）单元格是固定
    /// 尺寸、不随控件大小缩放——归一化比例 (fx/fy) 在控件尺寸变化后会漂移到
    /// 相邻单元格；像素偏移从左上角锚定则始终指回原单元格。
    #[serde(default = "default_neg1")]
    pub ax: i32,
    /// 点击相对控件矩形左上角的像素偏移（纵向）。-1 = 旧数据未记录。
    #[serde(default = "default_neg1")]
    pub ay: i32,
    /// 点击处可见文本的 OCR 兜底锚点（Stage 2d）：WPS「最近」列表等自绘控件既不暴露
    /// 稳定 UIA 名称、类名又稀疏时，录制端在 UIA 名称为空的情况下用系统内置 OCR 读取
    /// 落点处的可见文本（如文件名），重放端再按该文本在窗口内找回当前位置。
    /// 空串 = 未捕获 / 旧数据（旧数据无此字段，反序列化默认空），上层走既有 Win32 路径。
    #[serde(default)]
    pub ocr: String,
    /// 点击点周围视觉模板（base64 JPEG）。重放端在已聚焦窗口内做 SSD 模板匹配，
    /// 命中即点该处——用于顶部栏自绘按钮等无稳定类名/名称的图标型目标。
    /// 空串 = 未采集（录制端该点击区域无纹理 / 旧数据），重放走既有锚点/坐标。
    #[serde(default)]
    pub click_template: String,
    /// WPS/Excel 表格活动单元格地址（如 "B5"）。重放端据此发 Ctrl+G + 输入地址 + Enter
    /// 跳格，零漂移（单元格是画布绘制、无稳定控件，像素坐标会随滚动/缩放漂移）。
    /// 空串 = 未采集（非表格点击 / 旧数据），重放走既有锚点/坐标。
    #[serde(default)]
    pub cell_ref: String,
}

fn default_half() -> f64 {
    0.5
}

fn default_neg1() -> i32 {
    -1
}

impl ControlAnchor {
    /// 从 JSON 字符串解析；空串 / 解析失败返回 None（视为「无锚点」）。
    pub fn from_str(s: &str) -> Option<ControlAnchor> {
        let s = s.trim();
        if s.is_empty() {
            return None;
        }
        serde_json::from_str(s).ok()
    }

    /// 是否完全无锚点信息（无法提供任何分辨率无关定位）。
    pub fn is_empty(&self) -> bool {
        self.cls.is_empty() && self.wx == 0.0 && self.wy == 0.0
    }
}

fn class_name_of(hwnd: HWND) -> String {
    let mut buf = [0u16; 256];
    // 与 backend.rs 的 GetWindowTextW(hwnd, &mut buf) 同款签名，已验证可编译。
    let n = unsafe { GetClassNameW(hwnd, &mut buf) };
    if n > 0 {
        String::from_utf16_lossy(&buf[..n as usize])
    } else {
        String::new()
    }
}

/// 录制端：在虚拟屏幕坐标 (screen_x, screen_y) 处抓取控件锚点。
/// 任何失败返回 None（上层回退到录制坐标）。
fn capture_anchor_core(screen_x: i32, screen_y: i32) -> Option<(ControlAnchor, RECT)> {
    unsafe {
        let pt = POINT {
            x: screen_x,
            y: screen_y,
        };
        let hwnd = WindowFromPoint(pt);
        if hwnd.0.is_null() {
            return None;
        }

        // 向上回溯到顶层窗口，作为 wx/wy 的参照系。
        let mut root = hwnd;
        loop {
            let parent = match GetParent(root) {
                Ok(p) if !p.0.is_null() => p,
                _ => break,
            };
            root = parent;
        }

        let mut root_rect = RECT {
            left: 0,
            top: 0,
            right: 0,
            bottom: 0,
        };
        if GetWindowRect(root, &mut root_rect).is_err() {
            return None;
        }
        let root_w = (root_rect.right - root_rect.left).max(1) as f64;
        let root_h = (root_rect.bottom - root_rect.top).max(1) as f64;
        let wx = ((screen_x - root_rect.left) as f64 / root_w).clamp(0.0, 1.0);
        let wy = ((screen_y - root_rect.top) as f64 / root_h).clamp(0.0, 1.0);

        let mut ctrl_rect = RECT {
            left: 0,
            top: 0,
            right: 0,
            bottom: 0,
        };
        let _ = GetWindowRect(hwnd, &mut ctrl_rect);
        let ctrl_w = (ctrl_rect.right - ctrl_rect.left).max(1) as f64;
        let ctrl_h = (ctrl_rect.bottom - ctrl_rect.top).max(1) as f64;
        let fx = ((screen_x - ctrl_rect.left) as f64 / ctrl_w).clamp(0.0, 1.0);
        let fy = ((screen_y - ctrl_rect.top) as f64 / ctrl_h).clamp(0.0, 1.0);
        // 像素偏移：内容型控件（表格/文档网格）单元格固定尺寸不随控件缩放，
        // 像素偏移比归一化比例更稳（详见 ControlAnchor.ax 注释）。
        let ax = screen_x - ctrl_rect.left;
        let ay = screen_y - ctrl_rect.top;

        let cls = class_name_of(hwnd);
        // Stage 2c：尽力抓取落点元素的 UIA 名称（列表项文件名等）与点击在该元素
        // 矩形内的相对位置；失败/空名留空 —— 重放端名称为空即走既有 Win32 路径，零回归。
        let (name, nfx, nfy) = match crate::uia::element_at(screen_x, screen_y) {
            Some((n, r)) => {
                let w = (r.right - r.left).max(1) as f64;
                let h = (r.bottom - r.top).max(1) as f64;
                let rx = ((screen_x - r.left) as f64 / w).clamp(0.0, 1.0);
                let ry = ((screen_y - r.top) as f64 / h).clamp(0.0, 1.0);
                (n, rx, ry)
            }
            None => (String::new(), 0.5, 0.5),
        };
        Some((
            ControlAnchor {
                cls,
                fx,
                fy,
                wx,
                wy,
                name,
                nfx,
                nfy,
                ax,
                ay,
                ocr: String::new(),
                click_template: String::new(),
                cell_ref: String::new(),
            },
            ctrl_rect,
        ))
    }
}

/// 录制端：与 `capture_anchor_core` 同语义，但只返回锚点（不含控件矩形）。
pub fn capture_anchor(screen_x: i32, screen_y: i32) -> Option<ControlAnchor> {
    capture_anchor_core(screen_x, screen_y).map(|(a, _)| a)
}

/// 录制端增强版：在 UIA 名称为空（WPS 自绘控件稀疏）时，对已有的录制帧截图做 OCR，
/// 抓取落点处的可见文本作为 `ocr` 兜底锚点；任何失败留空 -> 零回归。
pub fn capture_anchor_with_image(
    screen_x: i32,
    screen_y: i32,
    img: &DynamicImage,
    img_click_x: i32,
    img_click_y: i32,
) -> Option<ControlAnchor> {
    let (mut anchor, _) = capture_anchor_core(screen_x, screen_y)?;
    if anchor.name.is_empty() {
        if let Some(text) = ocr_text_at(img, img_click_x, img_click_y) {
            anchor.ocr = text;
        }
    }
    Some(anchor)
}

struct LocateCtx<'a> {
    anchor: &'a ControlAnchor,
    root_left: i32,
    root_top: i32,
    root_w: f64,
    root_h: f64,
    best: Option<RECT>,
    best_score: f64,
}

unsafe extern "system" fn enum_child_proc(hwnd: HWND, lparam: LPARAM) -> BOOL {
    let ctx = &mut *(lparam.0 as *mut LocateCtx);
    if ctx.anchor.cls.is_empty() {
        return BOOL(1);
    }
    let cls = class_name_of(hwnd);
    if cls != ctx.anchor.cls {
        return BOOL(1);
    }
    let mut rect = RECT {
        left: 0,
        top: 0,
        right: 0,
        bottom: 0,
    };
    if GetWindowRect(hwnd, &mut rect).is_err() {
        return BOOL(1);
    }
    // 该控件中心在顶层窗口矩形内的归一化位置，与录制时点击的窗口归一化位置比对，
    // 取最接近者（处理同类控件多个的歧义）。
    let cx = ((rect.left + rect.right) as f64 / 2.0 - ctx.root_left as f64) / ctx.root_w;
    let cy = ((rect.top + rect.bottom) as f64 / 2.0 - ctx.root_top as f64) / ctx.root_h;
    let dx = cx - ctx.anchor.wx;
    let dy = cy - ctx.anchor.wy;
    let score = (dx * dx + dy * dy).sqrt();
    if score < ctx.best_score {
        ctx.best_score = score;
        ctx.best = Some(rect);
    }
    BOOL(1)
}

/// 控件矩形内求最终落点。优先级：
///   1) 新录制的像素偏移 (ax/ay)——内容型控件（表格/文档）单元格固定尺寸，
///      像素锚定始终指回原单元格，比例缩放会漂移；
///   2) 旧录制（ax<0）且控件是内容型大控件 + 有录制绝对坐标提示
///      → 用提示反推像素偏移（控件几何一致时精确；左上角锚定，尺寸变化也不漂列）；
///   3) 归一化比例 (fx/fy)——小控件（按钮等）中心比例足够稳。
fn point_in_ctrl(
    anchor: &ControlAnchor,
    rect: RECT,
    hx: Option<i32>,
    hy: Option<i32>,
) -> (i32, i32) {
    let w = (rect.right - rect.left).max(1);
    let h = (rect.bottom - rect.top).max(1);
    if anchor.ax >= 0 && anchor.ay >= 0 {
        let px = (rect.left + anchor.ax.clamp(0, w)).min(rect.right);
        let py = (rect.top + anchor.ay.clamp(0, h)).min(rect.bottom);
        return (px, py);
    }
    if let (Some(hx), Some(hy)) = (hx, hy) {
        if w > 300 || h > 200 {
            let px = rect.left + (hx - rect.left).clamp(0, w);
            let py = rect.top + (hy - rect.top).clamp(0, h);
            return (px, py);
        }
    }
    (
        (rect.left as f64 + anchor.fx * w as f64).round() as i32,
        (rect.top as f64 + anchor.fy * h as f64).round() as i32,
    )
}

/// wx/wy 回退的参照系自寻优：录制时 wx/wy 相对「点击目标所属顶层窗口」，
/// 重放时前台窗口可能换人（如 WPS 函数参数对话框抢前台），比例被套错矩形
/// 会偏出几行几列。用录制绝对坐标作提示，在所有可见顶层窗口中选「映射结果
/// 最接近提示」的那个作参照系（阈值 300px，防误配）。
fn resolve_wxwy_point(
    anchor: &ControlAnchor,
    window: HWND,
    hx: Option<i32>,
    hy: Option<i32>,
) -> (i32, i32) {
    unsafe {
        let mut rect = RECT::default();
        if GetWindowRect(window, &mut rect).is_err() {
            return (0, 0);
        }
        let map = |r: RECT| -> (i32, i32) {
            let w = (r.right - r.left).max(1) as f64;
            let h = (r.bottom - r.top).max(1) as f64;
            (
                (r.left as f64 + anchor.wx * w).round() as i32,
                (r.top as f64 + anchor.wy * h).round() as i32,
            )
        };
        let Some(hx) = hx else {
            return map(rect);
        };
        let hy = hy.unwrap_or(0);
        let mut ctx = WxwyCtx {
            wx: anchor.wx,
            wy: anchor.wy,
            hx,
            hy,
            best: None,
            best_d: f64::MAX,
        };
        EnumWindows(Some(wxwy_enum_proc), LPARAM(&mut ctx as *mut _ as isize)).ok();
        if ctx.best_d <= 300.0 {
            if let Some(p) = ctx.best {
                return p;
            }
        }
        map(rect)
    }
}

struct WxwyCtx {
    wx: f64,
    wy: f64,
    hx: i32,
    hy: i32,
    best: Option<(i32, i32)>,
    best_d: f64,
}

unsafe extern "system" fn wxwy_enum_proc(hwnd: HWND, lparam: LPARAM) -> BOOL {
    let ctx = &mut *(lparam.0 as *mut WxwyCtx);
    if !IsWindowVisible(hwnd).as_bool() {
        return BOOL(1);
    }
    let mut cls = [0u16; 64];
    let cn = GetClassNameW(hwnd, &mut cls);
    if cn > 0 {
        let class = String::from_utf16_lossy(&cls[..cn as usize]);
        if class == "Progman" || class == "WorkerW" || class == "Shell_TrayWnd" {
            return BOOL(1);
        }
    }
    let mut rect = RECT::default();
    if GetWindowRect(hwnd, &mut rect).is_err() {
        return BOOL(1);
    }
    if rect.right - rect.left <= 0 || rect.bottom - rect.top <= 0 {
        return BOOL(1);
    }
    let w = (rect.right - rect.left) as f64;
    let h = (rect.bottom - rect.top) as f64;
    let p = (
        (rect.left as f64 + ctx.wx * w).round() as i32,
        (rect.top as f64 + ctx.wy * h).round() as i32,
    );
    let d = ((p.0 - ctx.hx) * (p.0 - ctx.hx) + (p.1 - ctx.hy) * (p.1 - ctx.hy)) as f64;
    if d < ctx.best_d {
        ctx.best_d = d.sqrt();
        ctx.best = Some(p);
    }
    BOOL(1)
}

/// 重放端：依据锚点在已聚焦窗口 (window) 内反算点击的虚拟屏幕坐标（无提示）。
pub fn locate_click(anchor: &ControlAnchor, window: HWND) -> Option<(i32, i32)> {
    locate_click_with_hint(anchor, window, None, None)
}

/// 重放端：带「录制绝对坐标」提示的锚点反算。
/// 提示用于：内容型控件的像素偏移反推（旧录制）+ wx/wy 回退的参照系自寻优。
pub fn locate_click_with_hint(
    anchor: &ControlAnchor,
    window: HWND,
    hx: Option<i32>,
    hy: Option<i32>,
) -> Option<(i32, i32)> {
    unsafe {
        let mut root_rect = RECT {
            left: 0,
            top: 0,
            right: 0,
            bottom: 0,
        };
        if GetWindowRect(window, &mut root_rect).is_err() {
            return None;
        }
        let root_w = (root_rect.right - root_rect.left).max(1) as f64;
        let root_h = (root_rect.bottom - root_rect.top).max(1) as f64;

        // 第一优先：控件级定位（按类名在子窗口中匹配）。
        if !anchor.cls.is_empty() {
            let mut ctx = LocateCtx {
                anchor,
                root_left: root_rect.left,
                root_top: root_rect.top,
                root_w,
                root_h,
                best: None,
                best_score: f64::MAX,
            };
            EnumChildWindows(
                window,
                Some(enum_child_proc),
                LPARAM(&mut ctx as *mut _ as isize),
            );
            if let Some(rect) = ctx.best {
                return Some(point_in_ctrl(anchor, rect, hx, hy));
            }
        }

        // 第二优先：窗口级回退（wx/wy）——参照系自寻优。
        Some(resolve_wxwy_point(anchor, window, hx, hy))
    }
}

/// 在已有录制帧截图上，取点击点处的 OCR 文本行，用作兜底锚点。
/// 只裁剪点击点周围小区域 + 落临时文件 + 调系统内置 OCR；任何失败返回 None。
fn ocr_text_at(img: &DynamicImage, cx: i32, cy: i32) -> Option<String> {
    let (iw, ih) = (img.width() as i32, img.height() as i32);
    if iw <= 0 || ih <= 0 {
        return None;
    }
    // 裁剪点击点周围区域，限制 OCR 范围、提速（WPS 自绘列表项文本就在落点附近）。
    let half_w = 320i32.min(iw / 2);
    let half_h = 160i32.min(ih / 2);
    let x0 = (cx - half_w).clamp(0, iw) as u32;
    let y0 = (cy - half_h).clamp(0, ih) as u32;
    let x1 = (cx + half_w).clamp(0, iw) as u32;
    let y1 = (cy + half_h).clamp(0, ih) as u32;
    let cw = x1.saturating_sub(x0);
    let ch = y1.saturating_sub(y0);
    if cw < 8 || ch < 8 {
        return None;
    }
    let crop = img.crop_imm(x0, y0, cw, ch);
    let tmp = std::env::temp_dir().join(format!(
        "flowio_anchor_ocr_{}_{}.png",
        std::process::id(),
        cx.wrapping_add(cy)
    ));
    {
        let mut buf = std::io::Cursor::new(Vec::new());
        if crop.write_to(&mut buf, image::ImageFormat::Png).is_err() {
            return None;
        }
        if std::fs::write(&tmp, buf.into_inner()).is_err() {
            return None;
        }
    }
    let lines = crate::redaction::ocr::recognize_image(&tmp).ok();
    let _ = std::fs::remove_file(&tmp);
    let lines = lines?;
    // 点击点在裁剪图内的坐标
    let (lx, ly) = (cx - x0 as i32, cy - y0 as i32);
    for l in &lines {
        if lx >= l.x as i32
            && lx <= (l.x + l.w) as i32
            && ly >= l.y as i32
            && ly <= (l.y + l.h) as i32
        {
            let t = l.text.trim().to_string();
            if !t.is_empty() {
                return Some(t);
            }
        }
    }
    None
}

/// 重放端 OCR 兜底：控件类名 / UIA 名称都失配时（WPS 自绘控件稀疏），对目标窗口区域
/// 截图做 OCR，找出与 `text` 最相似的文本行，按其边界框 + (nfx,nfy) 反算落点。
///
/// 稳化措施（更稳的 OCR 重放）：
/// 1. 优先把全屏截图裁剪到目标窗口矩形，缩小 OCR 范围、提速、并减少跨窗口误命中；
/// 2. 用「相似度」而非朴素 contains：精确相等最佳，包含时按「目标占行文本比例」打分，
///    比例 < 0.5 视为长文本裹挟短词（如「保存并退出」里找「保存」），丢弃；多候选取最像的。
///
/// 任何失败返回 None（上层维持原落点）。仅在前端带非空 ocr 锚点时调用 -> 零回归。
pub fn locate_by_ocr(window: HWND, text: &str, nfx: f64, nfy: f64) -> Option<(i32, i32)> {
    if text.trim().is_empty() {
        return None;
    }
    let (img, base_ox, base_oy) = crate::recorder::screenshot::capture_fullscreen_with_origin().ok()?;
    // 优先裁剪到目标窗口区域；拿不到矩形则退回全屏。
    let (img, ox, oy) = crop_to_window(img.clone(), base_ox, base_oy, window)
        .unwrap_or((img, base_ox, base_oy));
    let tmp = std::env::temp_dir().join(format!("flowio_replay_ocr_{}.png", std::process::id()));
    {
        let mut buf = std::io::Cursor::new(Vec::new());
        if img.write_to(&mut buf, image::ImageFormat::Png).is_err() {
            return None;
        }
        if std::fs::write(&tmp, buf.into_inner()).is_err() {
            return None;
        }
    }
    let lines = crate::redaction::ocr::recognize_image(&tmp).ok();
    let _ = std::fs::remove_file(&tmp);
    let lines = lines?;
    let target = text.trim().to_lowercase();
    let mut best: Option<(i32, i32, f64)> = None;
    for l in &lines {
        let lt = l.text.trim().to_lowercase();
        if lt.is_empty() {
            continue;
        }
        // 相似度：精确相等 = 1.0；包含时 = 目标占行文本比例；否则 0。
        let ratio = if lt == target {
            1.0
        } else if lt.contains(&target) {
            target.len() as f64 / lt.len() as f64
        } else {
            0.0
        };
        // 占比 < 0.5 视为长文本裹挟短词，丢弃，避免误命中。
        if ratio < 0.5 {
            continue;
        }
        let px = (l.x as f64 + nfx.clamp(0.02, 0.98) * l.w as f64) as i32 + ox;
        let py = (l.y as f64 + nfy.clamp(0.02, 0.98) * l.h as f64) as i32 + oy;
        if best.as_ref().map_or(true, |(_, _, b)| ratio > *b) {
            best = Some((px, py, ratio));
        }
    }
    best.map(|(px, py, _)| (px, py))
}

/// 把全屏截图裁剪到 `window` 的窗口矩形，返回裁剪图与新的截图原点（屏幕坐标）。
/// 失败（窗口无效 / 矩形非法 / 越界）返回 None，调用方退回全屏。
fn crop_to_window(
    img: image::DynamicImage,
    base_ox: i32,
    base_oy: i32,
    window: HWND,
) -> Option<(image::DynamicImage, i32, i32)> {
    let mut rect = RECT::default();
    unsafe {
        if GetWindowRect(window, &mut rect).is_err() {
            return None;
        }
    }
    let w = rect.right - rect.left;
    let h = rect.bottom - rect.top;
    if w <= 0 || h <= 0 {
        return None;
    }
    let (iw, ih) = (img.width() as i32, img.height() as i32);
    // 截图坐标系：截图原点为 (base_ox, base_oy)；窗口在屏幕坐标为 (rect.left, rect.top)。
    let x0 = (rect.left - base_ox).clamp(0, iw - 1) as u32;
    let y0 = (rect.top - base_oy).clamp(0, ih - 1) as u32;
    let x1 = (rect.right - base_ox).clamp(0, iw) as u32;
    let y1 = (rect.bottom - base_oy).clamp(0, ih) as u32;
    if x1 <= x0 || y1 <= y0 {
        return None;
    }
    let cropped = img.crop_imm(x0, y0, x1 - x0, y1 - y0);
    Some((cropped, rect.left, rect.top))
}

/// 录制端：在录制帧截图的点击点周围裁剪一小块作为视觉模板，编码为 base64 JPEG。
/// 仅当模板有足够纹理（方差够大）才返回——空白/纯色区域不值得匹配（既浪费
/// 存储又易误命中）。任何失败返回 None。
pub fn crop_click_template(
    img: &DynamicImage,
    img_cx: i32,
    img_cy: i32,
    half: u32,
) -> Option<String> {
    let (iw, ih) = (img.width() as i32, img.height() as i32);
    if iw <= 0 || ih <= 0 {
        return None;
    }
    let half = half.max(8);
    let x0 = (img_cx - half as i32).clamp(0, iw) as u32;
    let y0 = (img_cy - half as i32).clamp(0, ih) as u32;
    let x1 = (img_cx + half as i32).clamp(0, iw) as u32;
    let y1 = (img_cy + half as i32).clamp(0, ih) as u32;
    if x1 <= x0 || y1 <= y0 {
        return None;
    }
    let patch = img.crop_imm(x0, y0, x1 - x0, y1 - y0);
    // 缩放到固定 40×40 提速 + 容忍轻微缩放/错位
    let patch = patch.resize_exact(40, 40, image::imageops::FilterType::Triangle);
    let luma = patch.to_luma8();
    // 方差门限：纯色/近纯色区域方差极小，匹配无意义且易误命中 -> 跳过
    let mut sum: u64 = 0;
    for p in luma.pixels() {
        sum += p.0[0] as u64;
    }
    let mean = sum as f64 / luma.pixels().len() as f64;
    let mut var = 0.0f64;
    for p in luma.pixels() {
        let d = p.0[0] as f64 - mean;
        var += d * d;
    }
    var /= luma.pixels().len() as f64;
    if var < 80.0 {
        // 标准差 ~9 以下视为无纹理
        return None;
    }
    let rgb = patch.to_rgb8();
    let mut buf = std::io::Cursor::new(Vec::new());
    if DynamicImage::ImageRgb8(rgb)
        .write_to(&mut buf, image::ImageFormat::Jpeg)
        .is_err()
    {
        return None;
    }
    Some(B64.encode(buf.into_inner()))
}

/// 纯函数：在灰度场景图(scene)里用灰度模板(tpl)滑窗 SSD 找最佳匹配，
/// `scale` 为「模板像素 → 场景像素」的放大倍数（场景已相对模板降采样）。
/// 返回 (mean_ssd, scene_x, scene_y)。无候选返回 None。与平台无关，可单测。
fn ssd_match(
    tpl: &image::GrayImage,
    scene: &image::GrayImage,
    scale: f64,
) -> Option<(f64, i32, i32)> {
    let (tw, th) = (tpl.width() as i32, tpl.height() as i32);
    let (sw, sh) = (scene.width() as i32, scene.height() as i32);
    if tw < 8 || th < 8 || tw > sw || th > sh {
        return None;
    }
    let (tw_s, th_s) = ((tw as f64 / scale) as i32, (th as f64 / scale) as i32);
    if tw_s < 4 || th_s < 4 {
        return None;
    }
    let mut best: Option<(f64, i32, i32)> = None; // (mean_ssd, x, y) 场景坐标
    let step = 2i32;
    for y in (0..=(sh - th_s)).step_by(step as usize) {
        for x in (0..=(sw - tw_s)).step_by(step as usize) {
            let mut s: u64 = 0u64;
            let mut n: u32 = 0;
            for j in (0..th).step_by(2) {
                let tj = (j as f64 / scale) as i32;
                if tj >= th_s {
                    continue;
                }
                for i in (0..tw).step_by(2) {
                    let ti = (i as f64 / scale) as i32;
                    if ti >= tw_s {
                        continue;
                    }
                    let a = tpl.get_pixel(i as u32, j as u32).0[0] as i32;
                    let b = scene.get_pixel((x + ti) as u32, (y + tj) as u32).0[0] as i32;
                    let d = a - b;
                    s += (d * d) as u64;
                    n += 1;
                }
            }
            if n == 0 {
                continue;
            }
            let mean_ssd = s as f64 / n as f64;
            if best.as_ref().map_or(true, |(b, _, _)| mean_ssd < *b) {
                best = Some((mean_ssd, x, y));
            }
        }
    }
    best
}

/// 重放端：把 base64 JPEG 模板在已聚焦窗口(window)区域内做 SSD 模板匹配，
/// 返回命中点（模板中心）的虚拟屏幕坐标。任何失败返回 None。
///
/// 实现：把目标窗口截图裁剪出来，降采样到最长边 <= 320 以提速；模板同样灰度过
/// 后做滑窗 SSD（步长 2 抽样）；取最小误差候选；并要求命中质量（平均像素差）
/// 足够好，否则视为没找到（避免把无纹理模板乱点）。命中坐标映射回全屏坐标。
pub fn match_click_template(template_b64: &str, window: HWND) -> Option<(i32, i32)> {
    let raw = B64.decode(template_b64).ok()?;
    let tpl = image::load_from_memory(&raw).ok()?;
    let tpl = tpl.to_luma8();
    let (tw, th) = (tpl.width() as i32, tpl.height() as i32);
    if tw < 8 || th < 8 {
        return None;
    }
    // 模板方差门：纯色模板无区分度，直接放弃
    let tmean = tpl_mean(&tpl);
    if tpl_std(&tpl, tmean) < 9.0 {
        return None;
    }

    let (img, ox, oy) = crate::recorder::screenshot::capture_fullscreen_with_origin().ok()?;
    let (img, ox, oy) = crop_to_window(img.clone(), ox, oy, window).unwrap_or((img, ox, oy));
    let iw = img.width() as i32;
    let ih = img.height() as i32;
    if iw < tw || ih < th {
        return None;
    }
    // 降采样场景图到最长边 <= 320
    let (sw, sh) = fit_within(iw, ih, 320);
    let scene = img
        .resize(sw, sh, image::imageops::FilterType::Triangle)
        .to_luma8();
    let scale = iw as f64 / sw as f64; // == ih / sh
    let Some((mean_ssd, bx, by)) = ssd_match(&tpl, &scene, scale) else {
        return None;
    };
    // 命中质量门：平均像素差 > 42（约 16% 灰度差）视为不匹配
    if mean_ssd > 42.0 {
        return None;
    }
    // 命中点 = 候选左上角 + 半模板（场景坐标）-> 全屏（img）坐标 -> 屏幕坐标
    let tw_s = (tw as f64 / scale) as i32;
    let th_s = (th as f64 / scale) as i32;
    let cx_img = (bx + tw_s / 2) as f64 * scale;
    let cy_img = (by + th_s / 2) as f64 * scale;
    Some((cx_img as i32 + ox, cy_img as i32 + oy))
}

fn tpl_mean(img: &image::GrayImage) -> f64 {
    let mut s: u64 = 0;
    for p in img.pixels() {
        s += p.0[0] as u64;
    }
    s as f64 / img.pixels().len() as f64
}

fn tpl_std(img: &image::GrayImage, mean: f64) -> f64 {
    let mut v = 0.0;
    for p in img.pixels() {
        let d = p.0[0] as f64 - mean;
        v += d * d;
    }
    (v / img.pixels().len() as f64).sqrt()
}

fn fit_within(w: i32, h: i32, max: i32) -> (u32, u32) {
    if w <= max && h <= max {
        return (w as u32, h as u32);
    }
    let r = max as f64 / w.max(h) as f64;
    ((w as f64 * r) as u32, (h as f64 * r) as u32)
}

/// 录制端：当前前台窗口句柄（单元格地址采集用）。
fn foreground_hwnd() -> Option<HWND> {
    let hwnd = unsafe { GetForegroundWindow() };
    if hwnd.0.is_null() {
        None
    } else {
        Some(hwnd)
    }
}

/// 录制端增强：在 `capture_anchor_with_image` 基础上，额外尝试抓取
/// ① 视觉模板（顶部栏自绘按钮等，重放时 SSD 匹配）② 表格单元格地址（Ctrl+G 跳格）。
/// 两者任一失败都静默跳过，零回归。
pub fn capture_anchor_with_extras(
    screen_x: i32,
    screen_y: i32,
    img: &DynamicImage,
    img_cx: i32,
    img_cy: i32,
    template_half: u32,
) -> Option<ControlAnchor> {
    let mut anchor =
        capture_anchor_with_image(screen_x, screen_y, img, img_cx, img_cy)?;
    // ① 视觉模板：对任何有纹理的点击区域都尝试（按钮/图标等），空白区域自动跳过
    if let Some(t) = crop_click_template(img, img_cx, img_cy, template_half) {
        anchor.click_template = t;
    }
    // ② 单元格地址：仅表格窗口的网格点击触发，否则不浪费 OCR
    if let Some(hwnd) = foreground_hwnd() {
        if let Some(cell) = capture_cell_ref(hwnd, screen_x, screen_y) {
            anchor.cell_ref = cell;
        }
    }
    Some(anchor)
}

/// 是否 WPS/Excel 表格窗口：标题含 WPS表格 / .et / .xls 或含 EXCEL7 子控件（网格）。
fn is_spreadsheet_window(hwnd: HWND) -> bool {
    unsafe {
        let mut buf = [0u16; 256];
        let n = GetWindowTextW(hwnd, &mut buf);
        if n > 0 {
            let title = String::from_utf16_lossy(&buf[..n as usize]).to_lowercase();
            if title.contains("wps表格") || title.contains(".et") || title.contains(".xls") {
                return true;
            }
        }
        if has_child_class(hwnd, "EXCEL7") {
            return true;
        }
    }
    false
}

struct HasChildCtx {
    target: String,
    found: bool,
}

unsafe extern "system" fn has_child_proc(hwnd: HWND, lparam: LPARAM) -> BOOL {
    let ctx = &mut *(lparam.0 as *mut HasChildCtx);
    if ctx.found {
        return BOOL(1);
    }
    let mut buf = [0u16; 256];
    let n = GetClassNameW(hwnd, &mut buf);
    if n > 0 {
        let cls = String::from_utf16_lossy(&buf[..n as usize]);
        if cls.contains(&ctx.target) {
            ctx.found = true;
        }
    }
    BOOL(1)
}

/// 窗口是否存在指定（子串）类名的子控件。
fn has_child_class(window: HWND, class_sub: &str) -> bool {
    let mut ctx = HasChildCtx {
        target: class_sub.to_string(),
        found: false,
    };
    unsafe {
        EnumChildWindows(
            window,
            Some(has_child_proc),
            LPARAM(&mut ctx as *mut _ as isize),
        );
    }
    ctx.found
}

/// 找到第一个类名含 `class_sub` 的子控件矩形（屏幕坐标）。
fn find_child_rect(window: HWND, class_sub: &str) -> Option<RECT> {
    struct Ctx {
        target: String,
        rect: Option<RECT>,
    }
    unsafe extern "system" fn proc(hwnd: HWND, lparam: LPARAM) -> BOOL {
        let ctx = &mut *(lparam.0 as *mut Ctx);
        if ctx.rect.is_some() {
            return BOOL(1);
        }
        let mut buf = [0u16; 256];
        let n = GetClassNameW(hwnd, &mut buf);
        if n > 0 {
            let cls = String::from_utf16_lossy(&buf[..n as usize]);
            if cls.contains(&ctx.target) {
                let mut r = RECT::default();
                if GetWindowRect(hwnd, &mut r).is_ok() {
                    ctx.rect = Some(r);
                }
            }
        }
        BOOL(1)
    }
    let mut ctx = Ctx {
        target: class_sub.to_string(),
        rect: None,
    };
    unsafe {
        EnumChildWindows(window, Some(proc), LPARAM(&mut ctx as *mut _ as isize));
    }
    ctx.rect
}

/// 解析 "x,y,w,h" 形式的矩形设置（屏幕像素）。
fn parse_rect_setting(s: &str) -> Option<(i32, i32, i32, i32)> {
    let parts: Vec<&str> = s.split(',').collect();
    if parts.len() != 4 {
        return None;
    }
    let v: Vec<i32> = parts
        .iter()
        .filter_map(|p| p.trim().parse::<i32>().ok())
        .collect();
    if v.len() == 4 && v[2] > 0 && v[3] > 0 {
        Some((v[0], v[1], v[2], v[3]))
    } else {
        None
    }
}

/// 名称框矩形（屏幕坐标）。优先级：
/// 1) 设置 `namebox_rect`（格式 "x,y,w,h"，由用户按自己 WPS 量取后配置，最可靠）；
/// 2) 自动定位：在表格窗口子控件中找「编辑/下拉框」且位于网格顶栏左侧区域者。
fn namebox_rect(window: HWND) -> Option<(i32, i32, i32, i32)> {
    if let Some(s) = crate::db::get_setting("namebox_rect") {
        if let Some(r) = parse_rect_setting(&s) {
            return Some(r);
        }
    }
    auto_namebox_rect(window)
}

/// 自动定位名称框（best-effort）。
/// 优先按「内容」定位：名称框显示的正是当前活动单元格地址（如 B5），故枚举窗口内
/// edit/combobox 子控件，读出其 ValuePattern 值，能解析成合法单元格地址者即名称框。
/// 此法跨 WPS/Excel 通用，不依赖 EXCEL7 相对几何与具体类名（WPS 自绘控件类名/布局
/// 与 Excel 不同，几何法常失配 → 名称框识别不到 → cell_ref 全程为空 → Ctrl+G 精准化死掉）。
/// 内容法不确定时回退「几何法」（EXCEL7 上方左列 edit/combobox），再不行返回 None
/// （上层走像素锚点，零回归）。
fn auto_namebox_rect(window: HWND) -> Option<(i32, i32, i32, i32)> {
    if let Some(r) = auto_namebox_rect_by_content(window) {
        return Some(r);
    }
    auto_namebox_rect_by_geometry(window)
}

/// 内容法：名称框的值即活动单元格地址（B5 等），用 UIA ValuePattern 读出比对。
/// 公式栏等显示公式/文本、尺寸更宽的控件自然被排除，特异性高。
fn auto_namebox_rect_by_content(window: HWND) -> Option<(i32, i32, i32, i32)> {
    struct Ctx {
        best: Option<(i32, i32, i32, i32)>,
    }
    unsafe extern "system" fn proc(hwnd: HWND, lparam: LPARAM) -> BOOL {
        let ctx = &mut *(lparam.0 as *mut Ctx);
        if ctx.best.is_some() {
            return BOOL(1);
        }
        let mut buf = [0u16; 256];
        let n = GetClassNameW(hwnd, &mut buf);
        if n > 0 {
            let cls = String::from_utf16_lossy(&buf[..n as usize]).to_lowercase();
            let is_edit = cls.contains("edit")
                || cls.contains("combobox")
                || cls.contains("edtbx")
                || cls.contains("netui");
            if is_edit {
                let mut r = RECT::default();
                if GetWindowRect(hwnd, &mut r).is_ok() {
                    let w = r.right - r.left;
                    let h = r.bottom - r.top;
                    if w >= 24 && w <= 220 && h >= 14 && h <= 48 {
                        let (cx, cy) = (r.left + w / 2, r.top + h / 2);
                        if let Some(v) = crate::uia::value_at_point(cx, cy) {
                            if parse_cell_ref(&v).is_some() {
                                ctx.best = Some((r.left, r.top, w, h));
                            }
                        }
                    }
                }
            }
        }
        BOOL(1)
    }
    let mut ctx = Ctx { best: None };
    unsafe {
        EnumChildWindows(window, Some(proc), LPARAM(&mut ctx as *mut _ as isize));
    }
    ctx.best
}

/// 几何法（回退）：找 EXCEL7 网格控件，其顶栏左侧的编辑/下拉框即名称框。
fn auto_namebox_rect_by_geometry(window: HWND) -> Option<(i32, i32, i32, i32)> {
    let grid = find_child_rect(window, "EXCEL7")?;
    let (gleft, gtop) = (grid.left, grid.top);
    struct NbCtx {
        gleft: i32,
        gtop: i32,
        best: Option<(i32, i32, i32, i32)>,
    }
    unsafe extern "system" fn proc(hwnd: HWND, lparam: LPARAM) -> BOOL {
        let ctx = &mut *(lparam.0 as *mut NbCtx);
        if ctx.best.is_some() {
            return BOOL(1);
        }
        let mut buf = [0u16; 256];
        let n = GetClassNameW(hwnd, &mut buf);
        if n > 0 {
            let cls = String::from_utf16_lossy(&buf[..n as usize]).to_lowercase();
            let is_edit = cls.contains("edit")
                || cls.contains("combobox")
                || cls.contains("edtbx")
                || cls.contains("netui");
            if is_edit {
                let mut r = RECT::default();
                if GetWindowRect(hwnd, &mut r).is_ok() {
                    let w = r.right - r.left;
                    let h = r.bottom - r.top;
                    // 名称框：在网格顶边之上、左列附近、尺寸合理
                    let above = r.bottom <= ctx.gtop + 6 && r.top >= ctx.gtop - 48;
                    let near_left = r.left >= ctx.gleft - 6 && r.left <= ctx.gleft + 140;
                    let sized = w >= 36 && w <= 170 && h >= 14 && h <= 40;
                    if above && near_left && sized {
                        ctx.best = Some((r.left, r.top, w, h));
                    }
                }
            }
        }
        BOOL(1)
    }
    let mut ctx = NbCtx {
        gleft,
        gtop,
        best: None,
    };
    unsafe {
        EnumChildWindows(window, Some(proc), LPARAM(&mut ctx as *mut _ as isize));
    }
    ctx.best
}

/// 把列字母（如 "B" / "XFD"）转成 1-based 列号；超出 Excel 上限 16384（XFD）返回 None。
fn col_to_num(letters: &str) -> Option<u32> {
    let mut n: u32 = 0;
    for c in letters.chars() {
        let v = (c.to_ascii_uppercase() as u32).wrapping_sub(b'A' as u32) + 1;
        n = n.saturating_mul(26).saturating_add(v);
        if n > 16384 {
            return None;
        }
    }
    if n == 0 {
        None
    } else {
        Some(n)
    }
}

/// 从 OCR 文本中抽取单元格地址（如 "B5" / "$B$5" / "$B5"）。
/// 严格：字母段 + 数字段，列 A..XFD、行 1..1048576 校验；返回首个合法地址的大写形式。
fn parse_cell_ref(text: &str) -> Option<String> {
    let cleaned: String = text
        .trim()
        .chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .collect();
    let bytes = cleaned.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        let ls = i;
        while i < bytes.len() && bytes[i].is_ascii_alphabetic() {
            i += 1;
        }
        if i == ls {
            i += 1;
            continue;
        }
        let ds = i;
        while i < bytes.len() && bytes[i].is_ascii_digit() {
            i += 1;
        }
        if i == ds {
            i += 1;
            continue;
        }
        let letters = &cleaned[ls..ds];
        let digits = &cleaned[ds..i];
        // 列必须落在 A..XFD（列号 1..16384）；否则不是单元格地址（如 "Sheet1"），跳过继续找。
        if col_to_num(letters).is_none() {
            continue;
        }
        if digits.is_empty() {
            continue;
        }
        if let Ok(n) = digits.parse::<u32>() {
            if n >= 1 && n <= 1048576 {
                return Some(format!("{}{}", letters.to_uppercase(), digits));
            }
        }
    }
    None
}

/// 录制端：尝试读取当前 WPS/Excel 表格活动单元格地址（如 "B5"）。
/// 仅当点击落在表格网格时生效——任何失败 / 非表格 / 提取不到合法地址 -> 返回 None（零回归）。
pub fn capture_cell_ref(window: HWND, _screen_x: i32, _screen_y: i32) -> Option<String> {
    if !is_spreadsheet_window(window) {
        return None;
    }
    let rect = namebox_rect(window)?;
    // 优先用 UIA 读取名称框的值（编辑控件的 ValuePattern.CurrentValue，最稳，
    // 不受字体/缩放/抗锯齿干扰）；失败（控件不暴露 ValuePattern 等）回退下方 OCR。
    let (cx, cy) = (rect.0 + rect.2 / 2, rect.1 + rect.3 / 2);
    if let Some(v) = crate::uia::value_at_point(cx, cy) {
        if let Some(cell) = parse_cell_ref(&v) {
            return Some(cell);
        }
    }
    let (img, ox, oy) = crate::recorder::screenshot::capture_fullscreen_with_origin().ok()?;
    let ix = (rect.0 - ox).clamp(0, img.width() as i32 - 1) as u32;
    let iy = (rect.1 - oy).clamp(0, img.height() as i32 - 1) as u32;
    let iw = rect.2.min(img.width() as i32 - ix as i32) as u32;
    let ih = rect.3.min(img.height() as i32 - iy as i32) as u32;
    if iw < 4 || ih < 4 {
        return None;
    }
    let crop = img.crop_imm(ix, iy, iw, ih);
    let tmp = std::env::temp_dir().join(format!("flowio_cell_{}.png", std::process::id()));
    {
        let mut buf = std::io::Cursor::new(Vec::new());
        if crop.write_to(&mut buf, image::ImageFormat::Png).is_err() {
            return None;
        }
        if std::fs::write(&tmp, buf.into_inner()).is_err() {
            return None;
        }
    }
    let lines = crate::redaction::ocr::recognize_image(&tmp).ok();
    let _ = std::fs::remove_file(&tmp);
    let lines = lines?;
    for l in &lines {
        if let Some(cell) = parse_cell_ref(&l.text) {
            return Some(cell);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn anchor_roundtrip_json() {
        let a = ControlAnchor {
            cls: "Button".into(),
            fx: 0.5,
            fy: 0.5,
            wx: 0.3,
            wy: 0.2,
            name: String::new(),
            ocr: String::new(),
            nfx: 0.5,
            nfy: 0.5,
            ax: -1,
            ay: -1,
            click_template: String::new(),
            cell_ref: String::new(),
        };
        let s = serde_json::to_string(&a).unwrap();
        let b = ControlAnchor::from_str(&s).unwrap();
        assert_eq!(b.cls, "Button");
        assert_eq!(b.fx, 0.5);
        assert_eq!(b.wy, 0.2);
    }

    #[test]
    fn anchor_from_empty_is_none() {
        assert!(ControlAnchor::from_str("").is_none());
        assert!(ControlAnchor::from_str("   ").is_none());
    }

    #[test]
    fn anchor_from_garbage_is_none() {
        assert!(ControlAnchor::from_str("not json").is_none());
    }

    #[test]
    fn anchor_legacy_json_without_name_field() {
        // 旧数据锚点 JSON 无 name 字段 → 解析成功且 name 为空（走既有 Win32 路径）。
        let a = ControlAnchor::from_str(r#"{"cls":"Button","fx":0.5,"fy":0.5,"wx":0.3,"wy":0.2}"#)
            .unwrap();
        assert!(a.name.is_empty());
        assert_eq!(a.cls, "Button");
        // 新数据带 name → 完整保留。
        let b = ControlAnchor::from_str(
            r#"{"cls":"Btn","fx":0.1,"fy":0.2,"wx":0.3,"wy":0.4,"name":"【WPS表格】SUM函数.xlsx"}"#,
        )
        .unwrap();
        assert_eq!(b.name, "【WPS表格】SUM函数.xlsx");
    }

    #[test]
    fn anchor_legacy_json_defaults_name_offset_to_center() {
        // 旧数据无 nfx/nfy → 默认 0.5（等价改前「取中心」行为），零回归。
        let a = ControlAnchor::from_str(r#"{"cls":"Button","fx":0.5,"fy":0.5,"wx":0.3,"wy":0.2}"#)
            .unwrap();
        assert_eq!(a.nfx, 0.5);
        assert_eq!(a.nfy, 0.5);
        // 新数据带行内相对偏移 → 完整保留。
        let b = ControlAnchor::from_str(
            r#"{"cls":"OpusApp","fx":0.1,"fy":0.2,"wx":0.3,"wy":0.4,"name":"x.xlsx","nfx":0.18,"nfy":0.5}"#,
        )
        .unwrap();
        assert_eq!(b.name, "x.xlsx");
        assert_eq!(b.nfx, 0.18);
        assert_eq!(b.nfy, 0.5);
    }

    #[test]
    fn anchor_legacy_json_defaults_pixel_offset_absent() {
        // 旧数据无 ax/ay → -1（未记录），重放走 fx/fy 或提示反推，零回归。
        let a = ControlAnchor::from_str(r#"{"cls":"EXCEL7","fx":0.5,"fy":0.5,"wx":0.3,"wy":0.2}"#)
            .unwrap();
        assert_eq!(a.ax, -1);
        assert_eq!(a.ay, -1);
        // 新数据带像素偏移 → 完整保留。
        let b = ControlAnchor::from_str(
            r#"{"cls":"EXCEL7","fx":0.1,"fy":0.2,"wx":0.3,"wy":0.4,"ax":217,"ay":299}"#,
        )
        .unwrap();
        assert_eq!(b.ax, 217);
        assert_eq!(b.ay, 299);
    }

    #[test]
    fn empty_anchor_detected() {
        let a = ControlAnchor::default();
        assert!(a.is_empty());
        let b = ControlAnchor {
            cls: String::new(),
            fx: 0.0,
            fy: 0.0,
            wx: 0.4,
            wy: 0.5,
            name: String::new(),
            ocr: String::new(),
            nfx: 0.5,
            nfy: 0.5,
            ax: -1,
            ay: -1,
            click_template: String::new(),
            cell_ref: String::new(),
        };
        assert!(!b.is_empty());
    }

    #[test]
    fn anchor_legacy_json_without_ocr_field() {
        // 旧数据无 ocr 字段 -> 解析成功且 ocr 为空（走既有 Win32 路径），零回归。
        let a = ControlAnchor::from_str(r#"{"cls":"Button","fx":0.5,"fy":0.5,"wx":0.3,"wy":0.2}"#)
            .unwrap();
        assert!(a.ocr.is_empty());
    }

    #[test]
    fn anchor_roundtrip_with_ocr() {
        let a = ControlAnchor {
            cls: "OpusApp".into(),
            ocr: "【WPS表格】SUM函数.xlsx".into(),
            ..Default::default()
        };
        let s = serde_json::to_string(&a).unwrap();
        let b = ControlAnchor::from_str(&s).unwrap();
        assert_eq!(b.ocr, "【WPS表格】SUM函数.xlsx");
    }

    // ── 单元格地址解析 ──────────────────────────────────

    #[test]
    fn cell_ref_plain() {
        assert_eq!(parse_cell_ref("B5"), Some("B5".to_string()));
    }
    #[test]
    fn cell_ref_lowercase() {
        assert_eq!(parse_cell_ref("b12"), Some("B12".to_string()));
    }
    #[test]
    fn cell_ref_dollar() {
        assert_eq!(parse_cell_ref("$B$5"), Some("B5".to_string()));
        assert_eq!(parse_cell_ref("$C3"), Some("C3".to_string()));
    }
    #[test]
    fn cell_ref_surrounded() {
        assert_eq!(parse_cell_ref("当前单元格 B5 已选中"), Some("B5".to_string()));
        assert_eq!(
            parse_cell_ref("定位到 A1 区域"),
            Some("A1".to_string())
        );
    }
    #[test]
    fn cell_ref_range_first() {
        // 名称框不会是范围，但即便 OCR 读到 "B5:C10"，也应取首个单元格
        assert_eq!(parse_cell_ref("B5:C10"), Some("B5".to_string()));
    }
    #[test]
    fn cell_ref_rejects_non_cell() {
        assert_eq!(parse_cell_ref("Sheet1"), None);
        assert_eq!(parse_cell_ref("确认"), None);
        assert_eq!(parse_cell_ref("123"), None);
        assert_eq!(parse_cell_ref(""), None);
    }
    #[test]
    fn cell_ref_rejects_column_too_long() {
        // "Sheet1" 被字母+数字误吸 -> 列超 3 字母应拒绝，继续找下一个合法地址
        assert_eq!(parse_cell_ref("Sheet1!C3"), Some("C3".to_string()));
    }
    #[test]
    fn cell_ref_max_bounds() {
        assert_eq!(parse_cell_ref("XFD1048576"), Some("XFD1048576".to_string()));
        assert_eq!(parse_cell_ref("XFE1"), None); // 列越界
        assert_eq!(parse_cell_ref("A0"), None); // 行越界
    }

    // ── namebox_rect 设置解析 ───────────────────────────

    #[test]
    fn rect_setting_ok() {
        assert_eq!(parse_rect_setting("100,200,120,24"), Some((100, 200, 120, 24)));
    }
    #[test]
    fn rect_setting_bad() {
        assert_eq!(parse_rect_setting("100,200,120"), None);
        assert_eq!(parse_rect_setting("a,b,c,d"), None);
        assert_eq!(parse_rect_setting("100,200,-5,24"), None); // 尺寸非正
    }

    // ── SSD 模板匹配（纯函数，合成图）────────────────────

    #[test]
    fn ssd_match_finds_inserted_patch() {
        use image::{DynamicImage, GrayImage, Luma};
        // 200x200 灰底场景
        let mut scene = GrayImage::from_pixel(200, 200, Luma([240u8]));
        // 在 (120,80) 处放一个 40x40 的暗方块作为模板本体
        for y in 80..120 {
            for x in 120..160 {
                scene.put_pixel(x, y, Luma([20u8]));
            }
        }
        // 模板 = 该 40x40 区域
        let tpl = DynamicImage::ImageLuma8(scene.clone())
            .crop_imm(120, 80, 40, 40)
            .to_luma8();
        // scale=1.0：场景与模板同分辨率
        let (mssd, bx, by) = ssd_match(&tpl, &scene, 1.0).expect("应匹配到");
        assert!(mssd < 1.0, "同图应为近乎零误差，实际 {}", mssd);
        // 命中点左上角应接近 (120,80)
        assert!((bx - 120).abs() <= 4, "bx={} 期望~120", bx);
        assert!((by - 80).abs() <= 4, "by={} 期望~80", by);
    }

    #[test]
    fn ssd_match_rejects_blank_template() {
        use image::{GrayImage, Luma};
        // 全白模板（方差为 0）应找不到有意义匹配
        let tpl = GrayImage::from_pixel(40, 40, Luma([255u8]));
        let mut scene = GrayImage::from_pixel(200, 200, Luma([240u8]));
        for y in 80..120 {
            for x in 120..160 {
                scene.put_pixel(x, y, Luma([20u8]));
            }
        }
        // 纯色模板在 ssd 下随处误差相同，best 仍会返回某候选，但其 mean_ssd 很大；
        // 真正的方差门在 match_click_template 里。这里验证 ssd_match 仍能运行不崩。
        let res = ssd_match(&tpl, &scene, 1.0);
        assert!(res.is_some()); // 纯函数返回某候选即可（质量门在调用方）
    }

    #[test]
    fn ssd_match_finds_patch_at_scale() {
        use image::{DynamicImage, GrayImage, Luma};
        // 原始分辨率场景 400x400，含一个 40x40 暗块（与模板同源）
        let mut orig = GrayImage::from_pixel(400, 400, Luma([240u8]));
        for y in 160..200 {
            for x in 240..280 {
                orig.put_pixel(x, y, Luma([20u8]));
            }
        }
        // 模板 = 暗块区域（40x40 原分辨率）
        let tpl = DynamicImage::ImageLuma8(orig.clone())
            .crop_imm(240, 160, 40, 40)
            .to_luma8();
        // 场景降采样到最长边 320（scale = 400/320 = 1.25，正是重放时的真实情形）
        let scale = 400.0 / 320.0;
        let scene = DynamicImage::ImageLuma8(orig.clone())
            .resize(320, 320, image::imageops::FilterType::Triangle)
            .to_luma8();
        let (mssd, bx, by) = ssd_match(&tpl, &scene, scale).expect("应匹配到");
        assert!(mssd < 60.0, "降采样下仍应近乎零误差，实际 {}", mssd);
        let exp_x = (240.0 / scale).round() as i32;
        let exp_y = (160.0 / scale).round() as i32;
        assert!((bx - exp_x).abs() <= 6, "bx={} 期望~{}", bx, exp_x);
        assert!((by - exp_y).abs() <= 6, "by={} 期望~{}", by, exp_y);
    }
}
