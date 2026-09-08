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

use serde::{Deserialize, Serialize};
use windows::Win32::Foundation::{BOOL, HWND, LPARAM, POINT, RECT};
use windows::Win32::UI::WindowsAndMessaging::{
    EnumChildWindows, EnumWindows, GetClassNameW, GetParent, GetWindowRect, IsWindowVisible,
    WindowFromPoint,
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
pub fn capture_anchor(screen_x: i32, screen_y: i32) -> Option<ControlAnchor> {
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
        Some(ControlAnchor {
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
        })
    }
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
            nfx: 0.5,
            nfy: 0.5,
            ax: -1,
            ay: -1,
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
            nfx: 0.5,
            nfy: 0.5,
            ax: -1,
            ay: -1,
        };
        assert!(!b.is_empty());
    }
}
