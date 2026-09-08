//! UI Automation 轻量封装（Stage 2c 名称锚点）。
//!
//! 动机：WPS「最近」列表等自绘控件按使用状态动态排序，Win32 类名锚点只能定位到
//! 列表控件本身，列表项位置漂移后会点错文档。录制时额外记录落点元素的 UIA 名称
//! （列表项的 Name 通常是文件名），重放时若落点名称不符则在窗口 UIA 树内按名称
//! 找回该元素，点其当前位置中心 —— 与位置无关，列表重排也不怕。
//!
//! 设计取舍：尽力而为。任何 UIA 失败（COM 未初始化异常、元素不可达、名称为空、
//! 超出遍历预算）都返回 None，上层回退到既有 Win32 路径 —— 永不引入回归。
//! COM 初始化：线程内首次调用 CoInitializeEx(APARTMENTTHREADED)，之后复用；
//! 若线程已按其他模式初始化（RPC_E_CHANGED_MODE），UIA 在 MTA 下同样可用，继续。

use windows::Win32::Foundation::{HWND, POINT};
use windows::Win32::System::Com::{
    CoCreateInstance, CoInitializeEx, CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED,
};
use windows::Win32::UI::Accessibility::{
    CUIAutomation, IUIAutomation, IUIAutomationElement, IUIAutomationTreeWalker,
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
    let auto = automation()?;
    let el = unsafe { auto.ElementFromPoint(POINT { x, y }) }.ok()?;
    let name = unsafe { el.CurrentName() }.ok()?;
    to_name(&name)
}

/// 名称锚点定位：校验落点 (cx, cy) 处元素名称是否仍与录制一致；一致则维持落点，
/// 不一致（列表重排/内容变化）则在窗口 hwnd 的 UIA 子树内按名称找回元素，
/// 返回其当前矩形中心。任何失败返回 None（上层维持原落点）。
pub fn locate_by_name(hwnd: HWND, name: &str, cx: i32, cy: i32) -> Option<(i32, i32)> {
    if name.trim().is_empty() {
        return None;
    }
    let auto = automation()?;

    // 1) 落点校验：名称仍一致 → 无漂移，直接用原落点。
    if let Ok(el) = unsafe { auto.ElementFromPoint(POINT { x: cx, y: cy }) } {
        if let Ok(n) = unsafe { el.CurrentName() } {
            if let Some(s) = to_name(&n) {
                if s == name {
                    return Some((cx, cy));
                }
            }
        }
    }

    // 2) 落点漂移 → 在目标窗口子树内按名称找回。
    let root = unsafe { auto.ElementFromHandle(hwnd) }.ok()?;
    let walker = unsafe { auto.ControlViewWalker() }.ok()?;
    let mut budget: usize = 1200; // 节点预算：防超大 UIA 树拖慢执行
    dfs_by_name(&root, &walker, name, &mut budget)
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

/// 深度优先按名称精确匹配元素，返回其 BoundingRectangle 中心（屏幕坐标）。
fn dfs_by_name(
    el: &IUIAutomationElement,
    walker: &IUIAutomationTreeWalker,
    name: &str,
    budget: &mut usize,
) -> Option<(i32, i32)> {
    if *budget == 0 {
        return None;
    }
    *budget -= 1;
    if let Ok(n) = unsafe { el.CurrentName() } {
        if let Some(s) = to_name(&n) {
            if s == name {
                if let Ok(r) = unsafe { el.CurrentBoundingRectangle() } {
                    return Some(((r.left + r.right) / 2, (r.top + r.bottom) / 2));
                }
            }
        }
    }
    // 遍历子树（ControlView）：先子后弟。
    let mut child = unsafe { walker.GetFirstChildElement(el) }.ok();
    while let Some(c) = child {
        if let Some(p) = dfs_by_name(&c, walker, name, budget) {
            return Some(p);
        }
        if *budget == 0 {
            return None;
        }
        child = unsafe { walker.GetNextSiblingElement(&c) }.ok();
    }
    None
}
