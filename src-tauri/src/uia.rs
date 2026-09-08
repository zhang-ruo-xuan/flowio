//! UI Automation 轻量封装（Stage 2c 名称锚点）。
//!
//! 动机：WPS「最近」列表等自绘控件按使用状态动态排序，Win32 类名锚点只能定位到
//! 列表控件本身，列表项位置漂移后会点错文档。录制时额外记录落点元素的 UIA 名称
//! （列表项的 Name 通常是文件名），重放时在窗口 UIA 子树内按名称找回该元素，
//! 点其当前位置中心 —— 与位置无关，列表重排也不怕。
//!
//! 🔑 最小矩形规则（2026-09-08 实测教训）：WPS 最近列表「整行容器」的 Name 也是
//! 文件名，先序遍历/FindFirst 会先命中行容器，其 BoundingRectangle 中心在窗口
//! 最大化时恰好落在路径列（点击即跳转文件夹）。因此必须 FindAll 取全部同名元素，
//! 选 **矩形面积最小** 的那个 —— 那才是真正的文件名文本元素。
//!
//! 设计取舍：尽力而为。任何 UIA 失败（元素不可达、名称为空、无匹配）都返回 None，
//! 上层回退到既有 Win32 路径 —— 永不引入回归。
//! COM 初始化：线程内首次调用 CoInitializeEx(APARTMENTTHREADED)，之后复用；
//! 若线程已按其他模式初始化（RPC_E_CHANGED_MODE），UIA 在 MTA 下同样可用，继续。

use windows::Win32::Foundation::{HWND, POINT, RECT};
use windows::Win32::System::Com::{
    CoCreateInstance, CoInitializeEx, CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED,
};
use windows::Win32::UI::Accessibility::{
    CUIAutomation, IUIAutomation, IUIAutomationElement, TreeScope_Subtree, UIA_NamePropertyId,
};

/// 获取（并按需初始化 COM 后创建）UIA 自动化对象。
fn automation() -> Option<IUIAutomation> {
    unsafe {
        // 已初始化（任何模式）或初始化失败都继续尝试：UIA 在 STA/MTA 下均可用。
        let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
        CoCreateInstance(&CUIAutomation, None, CLSCTX_INPROC_SERVER)
            .ok()
    }
}

/// 取屏幕坐标处 UIA 元素的名称（列表项文件名等）。空名 / 失败返回 None。
pub fn element_name_at(x: i32, y: i32) -> Option<String> {
    element_at(x, y).map(|(n, _)| n)
}

/// 取屏幕坐标处 UIA 元素的 (名称, 矩形)。失败 / 空名返回 None。
///
/// 矩形用于记录「点击点在元素内的相对位置」：WPS「最近」列表等自绘控件
/// 只暴露整行元素（文件名文本无独立 UIA 元素），此时只能按行内相对偏移
/// 还原列位置，取行中心会点进路径列并跳转文件夹。
pub fn element_at(x: i32, y: i32) -> Option<(String, RECT)> {
    let auto = automation()?;
    let el = unsafe { auto.ElementFromPoint(POINT { x, y }) }.ok()?;
    let name = unsafe { el.CurrentName() }.ok()?;
    let name = to_name(&name)?;
    let r = unsafe { el.CurrentBoundingRectangle() }.ok()?;
    if r.right <= r.left || r.bottom <= r.top {
        return None;
    }
    Some((name, r))
}

/// 名称锚点定位：在窗口 hwnd 的 UIA 子树内按名称找出全部同名元素，选矩形最小者
/// （真正的文本元素，而非整行容器），返回其当前矩形中心。
/// 找不到时回退校验落点 (cx, cy) 处元素名称，一致则维持落点。
/// 任何失败返回 None（上层维持原落点）。全过程留痕 replay_log。
pub fn locate_by_name(
    hwnd: HWND,
    name: &str,
    nfx: f64,
    nfy: f64,
    hint_x: Option<i32>,
    hint_y: Option<i32>,
    cx: i32,
    cy: i32,
) -> Option<(i32, i32)> {
    if name.trim().is_empty() {
        return None;
    }
    let log = |msg: &str| crate::executor::backend::replay_log(&format!("[anchor-name] {}", msg));

    let Some(auto) = automation() else {
        log("UIA 不可用 -> 维持原落点");
        return None;
    };
    let Ok(root) = (unsafe { auto.ElementFromHandle(hwnd) }) else {
        log("目标窗口 UIA 根元素不可达 -> 维持原落点");
        return None;
    };

    // 1) 原生 FindAll 按名称找全部匹配，选最小矩形（文本元素）。
    if let Ok(cond) = (unsafe {
        auto.CreatePropertyCondition(UIA_NamePropertyId, &windows::core::VARIANT::from(name))
    }) {
        if let Ok(arr) = (unsafe { root.FindAll(TreeScope_Subtree, &cond) }) {
            if let Ok(len) = (unsafe { arr.Length() }) {
                let mut best: Option<RECT> = None;
                let mut best_area = i64::MAX;
                for i in 0..len {
                    let Ok(el) = (unsafe { arr.GetElement(i) }) else {
                        continue;
                    };
                    let Ok(r) = (unsafe { el.CurrentBoundingRectangle() }) else {
                        continue;
                    };
                    let (w, h) = (r.right - r.left, r.bottom - r.top);
                    if w <= 0 || h <= 0 {
                        continue;
                    }
                    let area = (w as i64) * (h as i64);
                    if area < best_area {
                        best = Some(r);
                        best_area = area;
                    }
                }
                if let Some(r) = best {
                    // 行内相对位置：自绘列表只暴露整行时，取行中心会点进路径列，
                    // 必须用「点击在行内的相对位置」还原列位置。优先级：
                    //   1) 新录制带 nfx/nfy → 直接用；
                    //   2) 旧录制（nfx/nfy 为默认 0.5）且命中元素是宽容器
                    //      → 用录制绝对点击坐标反推相对位置（窗口几何一致时准确）；
                    //   3) 否则取中心（与改前一致，零回归）。
                    let w = r.right - r.left;
                    let h = r.bottom - r.top;
                    let is_container = w > 300 || h > 200;
                    let (nfx, nfy) = if (nfx - 0.5).abs() > 0.0001 || (nfy - 0.5).abs() > 0.0001 {
                        (nfx, nfy)
                    } else if let (Some(hx), Some(hy)) = (hint_x, hint_y) {
                        if is_container {
                            (
                                ((hx - r.left) as f64 / w.max(1) as f64).clamp(0.02, 0.98),
                                ((hy - r.top) as f64 / h.max(1) as f64).clamp(0.02, 0.98),
                            )
                        } else {
                            (0.5, 0.5)
                        }
                    } else {
                        (0.5, 0.5)
                    };
                    let p = (
                        (r.left as f64 + nfx.clamp(0.02, 0.98) * w as f64).round() as i32,
                        (r.top as f64 + nfy.clamp(0.02, 0.98) * h as f64).round() as i32,
                    );
                    log(&format!(
                        "name='{}' 树内命中 {} 个同名元素 -> 最小矩形 {}x{} at({},{}) nf=({:.3},{:.3}) 落点 ({},{})",
                        name,
                        len,
                        w,
                        h,
                        r.left,
                        r.top,
                        nfx,
                        nfy,
                        p.0,
                        p.1
                    ));
                    return Some(p);
                }
            }
        }
    }

    // 2) 树内无匹配：校验落点处元素名称，一致则维持原落点（无漂移场景）。
    if let Ok(el) = (unsafe { auto.ElementFromPoint(POINT { x: cx, y: cy }) }) {
        if let Ok(n) = (unsafe { el.CurrentName() }) {
            if let Some(s) = to_name(&n) {
                if s == name {
                    log(&format!(
                        "name='{}' 树内未命中但落点名称一致 -> 维持 ({},{})",
                        name, cx, cy
                    ));
                    return Some((cx, cy));
                }
            }
        }
    }

    log(&format!(
        "name='{}' 未命中且落点 ({},{}) 名称不符 -> 维持原落点",
        name, cx, cy
    ));
    None
}

fn to_name(b: &windows::core::BSTR) -> Option<String> {
    let s = b.to_string();
    let t = s.trim();
    if t.is_empty() {
        None
    } else {
        Some(t.to_string())
    }
}
