//! 精准重放核心逻辑（与平台无关，可单测）。
//!
//! 读取录制源 DB 的 steps（母版锚点：x/y/window_title/description/after_screenshot_path），
//! 生成确定性重放动作序列。不依赖任何 LLM / 视觉模型——这是 Stage 2 壁垒的第一层。

use crate::anchor::ControlAnchor;
use crate::executor::backend::ReplayBackend;
use rusqlite::params;
use std::time::Duration;

/// 单步重放动作（从源 DB 映射而来，已含坐标锚点）
#[derive(Debug, Clone)]
pub struct ReplayAction {
    /// 滚轮：delta 正=向上，负=向下（单位为 WHEEL_DELTA 倍数，一格通常 120）。
    pub scroll_delta: Option<i32>,
    pub step_number: i32,
    pub action_type: String,
    /// 屏幕坐标锚点；None 表示该步录制时未落坐标（如窗口聚焦前的首点）
    pub x: Option<i32>,
    pub y: Option<i32>,
    pub window_title: String,
    /// type/paste 步的输入文本
    pub text: Option<String>,
    /// 该步执行后截图（用于校验门）
    pub after_screenshot: Option<String>,
    /// 机器可读组合键串（hotkey/copy/paste 步有值；如 "Ctrl+C" / "Alt+Tab" / "Win+D"）。
    /// 重放端据此下发组合键；为空表示非组合键步骤。
    pub hotkey_combo: Option<String>,
    /// 控件锚点 JSON（Stage 2b）：点击处控件类名 + 归一化点击比例，分辨率无关重放用。
    /// 为空表示无锚点，回退到录制坐标。
    pub uia_anchor: Option<String>,
    /// 鼠标按键（"left"/"right"，空=left）。click 步为 "right" 时重放下发右键。
    /// 旧录制该列为空 → 按左键（旧行为）。
    pub mouse_button: String,
    /// 拖拽按下点 / 松开点（仅 drag 步有值；None = 旧录制，降级为 click）。
    pub start_x: Option<i32>,
    pub start_y: Option<i32>,
    pub end_x: Option<i32>,
    pub end_y: Option<i32>,
}

/// 从源 DB 读取某录制的重放计划。
///
/// 只取重放必需字段，不做任何智能推断——坐标即坐标，文本即文本。
pub fn load_replay_plan(
    conn: &rusqlite::Connection,
    recording_id: &str,
) -> Result<Vec<ReplayAction>, String> {
    let mut stmt = conn
        .prepare(
            "SELECT step_number, action_type, x, y, window_title, description, after_screenshot_path, hotkey_combo, uia_anchor, mouse_button, title, \
             start_x, start_y, end_x, end_y, scroll_delta \
             FROM steps WHERE recording_id = ?1 ORDER BY order_index",
        )
        .map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map(params![recording_id], |row| {
            // 旧数据兜底：mouse_button 列加入前的「双击」步骤（标题含「双击」）
            // 没有按键信息，按左键双击还原，避免老教程双击退化为单击。
            let action_type: String = row.get(1)?;
            let mut mouse_button = row.get::<_, Option<String>>(9)?.unwrap_or_default();
            if action_type == "click" && mouse_button.is_empty() {
                if let Ok(Some(t)) = row.get::<_, Option<String>>(10) {
                    if t.contains("双击") {
                        mouse_button = "left-double".to_string();
                    }
                }
            }
            Ok(ReplayAction {
                step_number: row.get(0)?,
                action_type,
                x: row.get(2)?,
                y: row.get(3)?,
                window_title: row.get(4)?,
                text: match row.get::<_, Option<String>>(5)? {
                    Some(t) if !t.trim().is_empty() => Some(t),
                    _ => None,
                },
                after_screenshot: row.get::<_, Option<String>>(6)?,
                hotkey_combo: match row.get::<_, Option<String>>(7)? {
                    Some(c) if !c.trim().is_empty() => Some(c),
                    _ => None,
                },
                uia_anchor: match row.get::<_, Option<String>>(8)? {
                    Some(a) if !a.trim().is_empty() => Some(a),
                    _ => None,
                },
                mouse_button,
                start_x: row.get(11).unwrap_or(None).or_else(|| row.get(2).ok()),
                start_y: row.get(12).unwrap_or(None).or_else(|| row.get(3).ok()),
                end_x: row.get(13).unwrap_or(None),
                end_y: row.get(14).unwrap_or(None),
                scroll_delta: row.get(15).unwrap_or(None),
            })
        })
        .map_err(|e| e.to_string())?;
    let mut plan = Vec::new();
    for r in rows {
        plan.push(r.map_err(|e| e.to_string())?);
    }
    Ok(plan)
}

/// 该步是否录制到表格单元格地址（用于零漂移跳格）。无锚点 / 无地址返回 None。
fn cell_ref_of(action: &ReplayAction) -> Option<String> {
    let a = action.uia_anchor.as_deref()?;
    let anchor = ControlAnchor::from_str(a)?;
    let c = anchor.cell_ref.trim();
    if c.is_empty() {
        None
    } else {
        Some(c.to_string())
    }
}

/// 该步是否录制到视觉模板（用于图标 SSD 匹配）。无锚点 / 无模板返回 None。
fn template_of(action: &ReplayAction) -> Option<String> {
    let a = action.uia_anchor.as_deref()?;
    let anchor = ControlAnchor::from_str(a)?;
    let t = anchor.click_template.trim();
    if t.is_empty() {
        None
    } else {
        Some(t.to_string())
    }
}

/// 执行单步到指定后端（focus / click / drag / type / press_combo）。
///
/// - 有坐标 → 先聚焦窗口再按动作类型派发；
/// - 无坐标（x/y 为 None）→ 仅聚焦窗口（通常是"首点激活窗口"语义）；
/// - click → 鼠标点击（右键/双击按 mouse_button 派发）；
/// - drag → 真拖拽（按下点→松开点，起点锚点校正、终点同步平移补偿）；
///   旧录制无终点时降级为 click（兼容存量数据，不引入回归）；
/// - type/input → 文本注入；
/// - copy/paste → 按结构化组合键下发（如 Ctrl+C / Ctrl+V，不依赖 title 解析）；
/// - hotkey → 按结构化组合键下发（如 Alt+Tab / Win+D）；
/// - 未知 action_type 不再静默丢弃，记录日志便于排查。
pub fn run_action(action: &ReplayAction, backend: &dyn ReplayBackend) {
    crate::executor::backend::replay_log(&format!(
        "[run_action] type={} title='{}' text={:?} combo={:?} xy=({:?},{:?})",
        action.action_type,
        action.window_title,
        action.text,
        action.hotkey_combo,
        action.x,
        action.y
    ));
    // 无坐标：仅聚焦窗口，让后续有坐标的步能落在正确窗口
    let (x, y) = match (action.x, action.y) {
        (Some(x), Some(y)) => (x, y),
        _ => {
            backend.focus(&action.window_title);
            return;
        }
    };
    // focus 结果决定锚点是否可用：wx/wy 锚点是相对「录制时根窗口」的比例，
    // focus 失败说明前台不是目标窗口（如目标是桌面/标题已变），此时用前台窗口
    // 反算必然系统性错位（曾把桌面右键点进 Flowio 自己的窗口）。禁用锚点，
    // 直接用录制时的绝对屏幕坐标——与 Stage 2b 之前的行为一致，永不引入回归。
    let focused = backend.focus(&action.window_title);
    let anchor_json = if focused {
        action.uia_anchor.as_deref().filter(|s| !s.trim().is_empty())
    } else {
        if !action.window_title.trim().is_empty() {
            crate::executor::backend::replay_log(&format!(
                "[anchor] focus 失败 title='{}' -> 禁用锚点，回退录制绝对坐标 ({},{})",
                action.window_title, x, y
            ));
        }
        None
    };
    match action.action_type.as_str() {
        // click：优先按控件锚点（Stage 2b 分辨率无关）；无锚点回退录制坐标。
        "click" => {
            // 右键/双击：桌面右键菜单、双击打开文件等链路依赖。
            let is_right = action.mouse_button == "right";
            let is_double = action.mouse_button.ends_with("-double");
            // 菜单项点击（锚点 cls 指向右键/下拉菜单窗口）：菜单窗口全局可查，与 focus
            // 结果无关，也不能走 wx/wy 窗口级回退（录制时菜单窗口的 wx/wy 常为 1.0/1.0
            // 无效值）—— 必须等菜单弹出后按「菜单矩形 × fx/fy」反算，否则会在菜单出现
            // 前落空并顺手把菜单关掉（13:45 实测：step2 打进桌面 FolderView）。
            if !is_right && !is_double {
                if let Some(a) = action.uia_anchor.as_deref() {
                    if crate::executor::backend::is_menu_anchor(a) {
                        backend.click_menu_anchored(a, x, y);
                        return;
                    }
                }
            }
            // 单元格确定性地址寻址（WPS/Excel）：Ctrl+G 跳格，零漂移。
            // 单元格是画布绘制、无稳定控件，像素坐标随滚动/缩放漂移；录制时已抓到
            // 单元格地址（如 B5），重放用 Ctrl+G + 输入地址 + Enter 直接跳过去。
            if !is_right && !is_double {
                if let Some(cell) = cell_ref_of(action) {
                    crate::executor::backend::replay_log(&format!(
                        "[cell] 地址寻址 '{}' -> Ctrl+G + 输入 + Enter (回退 {},{})",
                        cell, x, y
                    ));
                    backend.press_combo("Ctrl+g");
                    backend.type_text(&cell);
                    backend.press_combo("Enter");
                    return;
                }
            }
            // 图标模板匹配（顶部栏自绘按钮等无稳定名称的目标）：在已聚焦窗口内 SSD 匹配，
            // 命中即点该处，天然纠正滚动/缩放/UI 重排带来的漂移。未命中由后端回退录制坐标。
            if !is_right && !is_double {
                if let Some(tpl) = template_of(action) {
                    backend.click_template(&tpl, x, y);
                    return;
                }
            }
            if let Some(anchor) = anchor_json {
                if is_right {
                    backend.right_click_anchored(anchor, x, y);
                } else if is_double {
                    backend.double_click_anchored(anchor, x, y);
                } else {
                    backend.click_anchored(anchor, x, y);
                }
                return;
            }
            if is_right {
                backend.right_click(x, y);
            } else if is_double {
                backend.double_click(x, y);
            } else {
                backend.click(x, y);
            }
        }
        // drag：录制端已存下「按下点 → 松开点」（start_*/end_*），重放真拖拽；
        // 旧录制没有终点（列加之前的存量数据）→ 维持旧的降级 click，不引入回归。
        "drag" => {
            match (action.end_x, action.end_y) {
                (Some(tx), Some(ty)) => {
                    let (sx, sy) = (
                        action.start_x.unwrap_or(x),
                        action.start_y.unwrap_or(y),
                    );
                    match anchor_json {
                        Some(anchor) => backend.drag_anchored(anchor, sx, sy, tx, ty),
                        None => backend.drag(sx, sy, tx, ty),
                    }
                }
                _ => {
                    crate::executor::backend::replay_log(&format!(
                        "[drag] 该步无终点（旧录制）→ 降级为 click ({},{})",
                        x, y
                    ));
                    match anchor_json {
                        Some(anchor) => backend.click_anchored(anchor, x, y),
                        None => backend.click(x, y),
                    }
                }
            }
        }
        // scroll：滚轮步骤（正=向上，负=向下）。录制端此前完全不产生这类步骤
        // （hook 把滚轮事件丢了）→ 教程里的滚动重放不出来，后续点击全错位。
        "scroll" => {
            let d = action.scroll_delta.unwrap_or(0);
            if d != 0 {
                backend.scroll(x, y, d);
            }
        }
        // type/input：字符只能进「当前键盘焦点」所在的控件。focus 失败说明前台
        // 不是录制时的目标窗口（典型：对话框没打开/被别的窗口抢了焦点），此时
        // 补聚焦一次再输入——否则文本会打进上一个控件，表现为「输入没生效」，
        // 后续步骤（如 Enter 触发确定）必然级联失败且极难排查。
        "type" | "input" => {
            // 合并步骤（scroll→type）：录制端已把滚动段并入输入步骤，滚动量在
            // scroll_delta、滚轮位置在 start_x/start_y（type 步骤原本恒为 None）。
            // 先按录制时的滚轮位置滚动，再走聚焦+输入链路，滚动语义不丢失。
            if let Some(d) = action.scroll_delta {
                if d != 0 {
                    let (sx, sy) = (
                        action.start_x.unwrap_or(x),
                        action.start_y.unwrap_or(y),
                    );
                    backend.scroll(sx, sy, d);
                    std::thread::sleep(Duration::from_millis(80));
                }
            }
            if !focused && !action.window_title.trim().is_empty() {
                crate::executor::backend::replay_log(&format!(
                    "[type-warn] 焦点未切到 '{}' → 补聚焦一次再输入（原窗口可能未打开）",
                    action.window_title
                ));
                let retry = backend.focus(&action.window_title);
                std::thread::sleep(Duration::from_millis(120));
                crate::executor::backend::replay_log(&format!(
                    "[type-warn] 补聚焦结果={}（仍失败则文本会进错控件）",
                    retry
                ));
            }
            // 重放输入模式：replace = 先 Ctrl+A 全选清空再输入（防旧内容串味）；
            // 缺省 append（保持原行为，零回归）。由 settings.replay_type_mode 控制。
            if crate::db::get_setting("replay_type_mode").as_deref() == Some("replace") {
                backend.press_combo("Ctrl+A");
            }
            backend.type_text(action.text.as_deref().unwrap_or(""));
        }
        // copy/paste 是语义化的组合键步骤：直接下发对应组合键
        "copy" => backend.press_combo(action.hotkey_combo.as_deref().unwrap_or("Ctrl+C")),
        "paste" => backend.press_combo(action.hotkey_combo.as_deref().unwrap_or("Ctrl+V")),
        "hotkey" => {
            if let Some(combo) = action.hotkey_combo.as_deref() {
                if !combo.is_empty() {
                    // 与 type 同理：Enter/Tab 等按键交给「键盘焦点」所在的窗口。
                    // 焦点不在目标窗口时补聚焦一次，否则 Enter 打在旧窗口上，
                    // 表现为「按了没反应，对话框一直不弹」（2026-09-08 WPS 实测：
                    // 插入函数里 Enter 后『函数参数』8 秒不出现 → 后续 8 步全废）。
                    if !focused && !action.window_title.trim().is_empty() {
                        crate::executor::backend::replay_log(&format!(
                            "[hotkey-warn] 焦点未切到 '{}' → 补聚焦一次再发 '{}'",
                            action.window_title, combo
                        ));
                        let retry = backend.focus(&action.window_title);
                        std::thread::sleep(Duration::from_millis(120));
                        crate::executor::backend::replay_log(&format!(
                            "[hotkey-warn] 补聚焦结果={}（仍失败则按键会发给旧窗口）",
                            retry
                        ));
                    }
                    backend.press_combo(combo);
                }
            }
        }
        other => {
            eprintln!("[replay] 未处理的 action_type: {}", other);
        }
    }
}

/// 顺序重放整份计划。
pub fn run_replay(plan: &[ReplayAction], backend: &dyn ReplayBackend, step_delay_ms: u64) {
    for action in plan {
        run_action(action, backend);
        if step_delay_ms > 0 {
            std::thread::sleep(Duration::from_millis(step_delay_ms));
        }
    }
}

/// ── Stage 3 审批闸门（纯函数，可单测）──

/// 敏感性分类：
/// - 敏感（需审批）：hotkey（任意组合键，可能触发系统级动作）、
///   copy/paste（读写剪贴板，数据面）、drag（可能移动文件/触发破坏性操作）。
/// - 非敏感：click/type/input 等确定性 UI 操作。
pub fn classify_sensitivity(action_type: &str) -> bool {
    matches!(action_type, "hotkey" | "copy" | "paste" | "drag")
}

/// 计划是否含敏感步骤（任一敏感即需审批）。
pub fn plan_requires_approval(plan: &[ReplayAction]) -> bool {
    plan.iter().any(|a| classify_sensitivity(&a.action_type))
}

/// 审批闸门是否生效。
/// - mode="off"（默认）：永不上闸，保持向后兼容（MVP 原行为）。
/// - mode="check" 且 计划需审批 且 未批准 → 上闸（不下发任何动作，仅返回 needs-approval）。
pub fn approval_engaged(requires_approval: bool, mode: &str, approved: bool) -> bool {
    mode == "check" && requires_approval && !approved
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::executor::backend::DryRunBackend;
    use rusqlite::Connection;

    /// focus 永远失败的后端：模拟目标窗口找不到（如桌面 'Program Manager'）。
    struct FailingFocusBackend {
        inner: DryRunBackend,
    }
    impl ReplayBackend for FailingFocusBackend {
        fn focus(&self, title: &str) -> bool {
            let _ = self.inner.focus(title);
            false
        }
        fn click(&self, x: i32, y: i32) {
            self.inner.click(x, y);
        }
        fn drag(&self, x1: i32, y1: i32, x2: i32, y2: i32) {
            self.inner.drag(x1, y1, x2, y2);
        }
        fn type_text(&self, text: &str) {
            self.inner.type_text(text);
        }
        fn press_combo(&self, combo: &str) {
            self.inner.press_combo(combo);
        }
        fn click_anchored(&self, a: &str, x: i32, y: i32) {
            self.inner.click_anchored(a, x, y);
        }
        fn click_template(&self, tpl: &str, x: i32, y: i32) {
            self.inner.click_template(tpl, x, y);
        }
        fn right_click(&self, x: i32, y: i32) {
            self.inner.right_click(x, y);
        }
        fn right_click_anchored(&self, a: &str, x: i32, y: i32) {
            self.inner.right_click_anchored(a, x, y);
        }
        fn double_click(&self, x: i32, y: i32) {
            self.inner.double_click(x, y);
        }
        fn double_click_anchored(&self, a: &str, x: i32, y: i32) {
            self.inner.double_click_anchored(a, x, y);
        }
        fn click_menu_anchored(&self, a: &str, x: i32, y: i32) {
            self.inner.click_menu_anchored(a, x, y);
        }
        fn drag_anchored(&self, a: &str, fx: i32, fy: i32, tx: i32, ty: i32) {
            self.inner.drag_anchored(a, fx, fy, tx, ty);
        }
        fn scroll(&self, x: i32, y: i32, delta: i32) {
            self.inner.scroll(x, y, delta);
        }
    }

    #[test]
    fn drag_with_end_point_issues_real_drag() {
        // 回归：drag 步骤此前只有「裁图框中心」一个坐标，重放被降级成 click，
        // 永远选不中区域（用户报「不能拖拽」）。有 start_*/end_* 时必须真拖。
        let backend = DryRunBackend::new();
        let action = ReplayAction {
            scroll_delta: None,
            step_number: 1,
            action_type: "drag".into(),
            x: Some(500),
            y: Some(400), // 裁图框中心（旧语义）
            window_title: "WPS".into(),
            text: None,
            after_screenshot: None,
            hotkey_combo: None,
            uia_anchor: None,
            mouse_button: String::new(),
            start_x: Some(300),
            start_y: Some(300),
            end_x: Some(900),
            end_y: Some(500),
        };
        run_action(&action, &backend);
        let acts = backend.actions.lock().unwrap();
        assert!(
            acts.iter().any(|a| a.starts_with("drag(300,300->900,500")),
            "应按 drag 起止点下发真拖拽，实际: {:?}",
            acts
        );
    }

    #[test]
    fn drag_without_end_point_falls_back_to_click() {
        // 旧录制（列加之前）没有终点：维持降级 click，不引入回归。
        let backend = DryRunBackend::new();
        let action = ReplayAction {
            scroll_delta: None,
            step_number: 1,
            action_type: "drag".into(),
            x: Some(500),
            y: Some(400),
            window_title: "WPS".into(),
            text: None,
            after_screenshot: None,
            hotkey_combo: None,
            uia_anchor: None,
            mouse_button: String::new(),
            start_x: None,
            start_y: None,
            end_x: None,
            end_y: None,
        };
        run_action(&action, &backend);
        let acts = backend.actions.lock().unwrap();
        assert!(
            acts.iter().any(|a| a.starts_with("click(500,400")),
            "无终点应降级为 click，实际: {:?}",
            acts
        );
    }

    #[test]
    fn scroll_step_dispatches_wheel() {
        // 回归：滚轮此前被 hook 丢弃（"e.g. wheel. Skip"），教程录不到滚动，
        // 重放也无从下发 → 网页/长文档类教程后续点击全错位。
        let backend = DryRunBackend::new();
        let action = ReplayAction {
            step_number: 1,
            action_type: "scroll".into(),
            x: Some(800),
            y: Some(500),
            window_title: "Browser".into(),
            text: None,
            after_screenshot: None,
            hotkey_combo: None,
            uia_anchor: None,
            mouse_button: String::new(),
            start_x: None,
            start_y: None,
            end_x: None,
            end_y: None,
            scroll_delta: Some(-360),
        };
        run_action(&action, &backend);
        let acts = backend.actions.lock().unwrap();
        assert!(
            acts.iter().any(|a| a.starts_with("scroll(800,500,-360)")),
            "应下发滚轮，实际: {:?}",
            acts
        );
    }

    #[test]
    fn test_menu_anchor_uses_menu_channel_not_absolute() {
        // 回归：菜单项步骤的锚点 cls 指向右键菜单窗口；即使 focus 失败（桌面场景
        // 必失败），也必须走 click_menu_anchored（等菜单弹出后按菜单矩形反算），
        // 不能被「focus 失败禁用锚点」规则降级成绝对坐标——那会在菜单出现前落空。
        let backend = FailingFocusBackend {
            inner: DryRunBackend::new(),
        };
        let anchor = r#"{"cls":"Microsoft.UI.Content.PopupWindowSiteBridge","fx":0.342,"fy":0.847,"wx":1.0,"wy":1.0}"#;
        let action = ReplayAction {
            scroll_delta: None,
            start_x: None, start_y: None, end_x: None, end_y: None,
            step_number: 2,
            action_type: "click".into(),
            x: Some(996),
            y: Some(635),
            window_title: String::new(),
            text: None,
            after_screenshot: None,
            hotkey_combo: None,
            uia_anchor: Some(anchor.into()),
            mouse_button: "left".into(),
        };
        run_action(&action, &backend);
        let acts = backend.inner.actions.lock().unwrap().clone();
        assert!(
            acts.iter().any(|a| a.starts_with("click_menu_anchored(")),
            "菜单锚点步骤必须走菜单通道, acts={:?}",
            acts
        );
    }

    #[test]
    fn test_focus_fail_disables_anchor_falls_back_to_absolute() {
        // 回归：桌面右键流程 focus('Program Manager') 必然失败，
        // 锚点 wx/wy 若仍相对前台窗口（Flowio 自己）反算会系统性错位。
        // focus 失败时必须禁用锚点，直接用录制绝对坐标下发。
        let backend = FailingFocusBackend {
            inner: DryRunBackend::new(),
        };
        let anchor = r#"{"cls":"SysListView32","fx":0.38,"fy":0.16,"wx":0.38,"wy":0.16}"#;
        let action = ReplayAction {
            scroll_delta: None,
            start_x: None, start_y: None, end_x: None, end_y: None,
            step_number: 1,
            action_type: "click".into(),
            x: Some(746),
            y: Some(181),
            window_title: "Program Manager".into(),
            text: None,
            after_screenshot: None,
            hotkey_combo: None,
            uia_anchor: Some(anchor.into()),
            mouse_button: "right".into(),
        };
        run_action(&action, &backend);
        let acts = backend.inner.actions.lock().unwrap().clone();
        // focus 记录 + 绝对坐标右键（非 right_click_anchored）
        assert_eq!(acts.len(), 2, "acts={:?}", acts);
        assert!(acts[0].starts_with("focus(Program Manager)"));
        assert!(
            acts.iter().any(|a| a.starts_with("right_click(746,181)")),
            "focus 失败应回退绝对坐标右键，acts={:?}",
            acts
        );
        assert!(
            !acts.iter().any(|a| a.contains("anchored")),
            "focus 失败不得走锚点路径，acts={:?}", acts
        );

        // 对照组：focus 成功（DryRunBackend）且有锚点 → 仍走锚点路径
        let ok_backend = DryRunBackend::new();
        run_action(&action, &ok_backend);
        let ok_acts = ok_backend.actions.lock().unwrap().clone();
        assert!(
            ok_acts.iter().any(|a| a.contains("right_click_anchored")),
            "focus 成功应保留锚点路径，acts={:?}",
            ok_acts
        );
    }


    fn mem_db() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute(
            "CREATE TABLE steps (
                id TEXT, recording_id TEXT, order_index INTEGER,
                step_number INTEGER, action_type TEXT, x INTEGER, y INTEGER,
                window_title TEXT, description TEXT, after_screenshot_path TEXT,
                hotkey_combo TEXT, uia_anchor TEXT, mouse_button TEXT, title TEXT,
                start_x INTEGER, start_y INTEGER, end_x INTEGER, end_y INTEGER,
                scroll_delta INTEGER
            )",
            [],
        )
        .unwrap();
        let rows = [
            (1i32, "click", 100i32, 200i32, "WPS x.xlsx", "", "", ""),
            (2, "type", 0, 0, "WPS x.xlsx", "hello 世界", "", ""),
            (3, "click", 0, 0, "WPS x.xlsx", "", "after.png", ""),
            (4, "drag", 300, 400, "WPS x.xlsx", "", "", ""),
        ];
        for (i, (si, at, x, y, wt, d, a, hk)) in rows.iter().enumerate() {
            conn.execute(
                "INSERT INTO steps (id, recording_id, order_index, step_number, action_type, x, y, window_title, description, after_screenshot_path, hotkey_combo, uia_anchor, mouse_button) \
                 VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13)",
                params!["s{i}", "rec1", i as i32, *si, at, *x, *y, wt, d, a, hk, "", ""],
            )
            .unwrap();
        }
        conn
    }

    #[test]
    fn test_load_replay_plan_maps_coords_and_text() {
        let conn = mem_db();
        let plan = load_replay_plan(&conn, "rec1").unwrap();
        assert_eq!(plan.len(), 4);
        assert_eq!(plan[0].action_type, "click");
        assert_eq!((plan[0].x, plan[0].y), (Some(100), Some(200)));
        assert_eq!(plan[1].action_type, "type");
        assert_eq!(plan[1].text.as_deref(), Some("hello 世界"));
        assert_eq!((plan[2].x, plan[2].y), (Some(0), Some(0)));
        assert_eq!(plan[3].action_type, "drag");
        assert_eq!((plan[3].x, plan[3].y), (Some(300), Some(400)));
    }

    #[test]
    fn test_dry_run_backend_records_action_sequence() {
        let conn = mem_db();
        let plan = load_replay_plan(&conn, "rec1").unwrap();
        let backend = crate::executor::backend::DryRunBackend::new();
        run_replay(&plan, &backend, 0);
        let acts = backend.actions.lock().unwrap().clone();
        // 每步 = focus + 实际动作，4 步共 8 条记录
        assert_eq!(acts.len(), 8);
        assert!(acts[0].starts_with("focus(WPS x.xlsx)"));
        assert!(acts[1].contains("click(100"));
        // "hello 世界" = 5 ascii + 1 空格 + 2 汉字 = 8 字符
        assert!(acts[3].contains("type(8 chars)"), "got: {}", acts[3]);
        assert!(acts[4].starts_with("focus") && acts[5].contains("click(0"));
        assert!(acts[7].contains("click(300"));
    }

    #[test]
    fn test_no_coord_step_only_focuses() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute(
            "CREATE TABLE steps (id TEXT, recording_id TEXT, order_index INTEGER, step_number INTEGER, action_type TEXT, x INTEGER, y INTEGER, window_title TEXT, description TEXT, after_screenshot_path TEXT, hotkey_combo TEXT, uia_anchor TEXT, mouse_button TEXT, title TEXT, start_x INTEGER, start_y INTEGER, end_x INTEGER, end_y INTEGER, scroll_delta INTEGER)",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO steps VALUES ('s0','rec1',0,1,'click',NULL,NULL,'记事本','',NULL,'','','','',NULL,NULL,NULL,NULL,NULL)",
            [],
        )
        .unwrap();
        let plan = load_replay_plan(&conn, "rec1").unwrap();
        assert_eq!(plan[0].x, None);
        let backend = crate::executor::backend::DryRunBackend::new();
        run_action(&plan[0], &backend);
        let acts = backend.actions.lock().unwrap().clone();
        assert_eq!(acts.len(), 1);
        assert!(acts[0].starts_with("focus(记事本)") && !acts[0].contains("click"));
    }

    #[test]
    fn test_hotkey_press_combo_dispatched() {
        let conn = mem_db();
        conn.execute(
            "INSERT INTO steps (id, recording_id, order_index, step_number, action_type, x, y, window_title, description, after_screenshot_path, hotkey_combo, uia_anchor, mouse_button) \
             VALUES ('s5','rec1',4,5,'hotkey',10,20,'记事本','',NULL,'Ctrl+S','','')",
            [],
        )
        .unwrap();
        let plan = load_replay_plan(&conn, "rec1").unwrap();
        assert_eq!(plan.len(), 5);
        let hk = &plan[4];
        assert_eq!(hk.action_type, "hotkey");
        assert_eq!(hk.hotkey_combo.as_deref(), Some("Ctrl+S"));
        let backend = crate::executor::backend::DryRunBackend::new();
        run_action(hk, &backend);
        let acts = backend.actions.lock().unwrap().clone();
        assert_eq!(acts.len(), 2);
        assert!(acts[0].starts_with("focus(记事本)"));
        assert!(acts[1].contains("hotkey(Ctrl+S)"), "got: {}", acts[1]);
    }

    #[test]
    fn test_copy_paste_press_combo() {
        let conn = mem_db();
        conn.execute("INSERT INTO steps (id, recording_id, order_index, step_number, action_type, x, y, window_title, description, after_screenshot_path, hotkey_combo, uia_anchor, mouse_button) VALUES ('s6','rec1',5,6,'copy',0,0,'WPS','',NULL,'Ctrl+C','','')", []).unwrap();
        conn.execute("INSERT INTO steps (id, recording_id, order_index, step_number, action_type, x, y, window_title, description, after_screenshot_path, hotkey_combo, uia_anchor, mouse_button) VALUES ('s7','rec1',6,7,'paste',0,0,'WPS','',NULL,'Ctrl+V','','')", []).unwrap();
        let plan = load_replay_plan(&conn, "rec1").unwrap();
        let backend = crate::executor::backend::DryRunBackend::new();
        run_replay(&plan, &backend, 0);
        let acts = backend.actions.lock().unwrap().clone();
        assert!(acts.iter().any(|a| a.contains("hotkey(Ctrl+C)")), "acts: {:?}", acts);
        assert!(acts.iter().any(|a| a.contains("hotkey(Ctrl+V)")), "acts: {:?}", acts);
    }

    #[test]
    fn test_click_uses_anchor_when_present() {
        let conn = mem_db();
        conn.execute(
            "INSERT INTO steps (id, recording_id, order_index, step_number, action_type, x, y, window_title, description, after_screenshot_path, hotkey_combo, uia_anchor, mouse_button) \
             VALUES ('s9','rec1',7,9,'click',100,200,'WPS','',NULL,'','{\"cls\":\"Button\",\"fx\":0.5,\"fy\":0.5,\"wx\":0.3,\"wy\":0.2}','')",
            [],
        )
        .unwrap();
        let plan = load_replay_plan(&conn, "rec1").unwrap();
        let step = plan.iter().find(|s| s.step_number == 9).unwrap();
        assert!(step.uia_anchor.is_some());
        let backend = crate::executor::backend::DryRunBackend::new();
        run_action(step, &backend);
        let acts = backend.actions.lock().unwrap().clone();
        assert!(acts.iter().any(|a| a.starts_with("focus(WPS)")));
        // 有锚点 → 走 click_anchored（带回退坐标），而非裸 click(100,200)
        assert!(
            acts.iter().any(|a| a.contains("click_anchored(100,200,")),
            "acts: {:?}",
            acts
        );
        assert!(
            !acts.iter().any(|a| a.starts_with("click(100")),
            "不应走裸 click，got: {:?}",
            acts
        );
    }

    #[test]
    fn test_click_without_anchor_falls_back_to_coords() {
        let conn = mem_db();
        // mem_db 的 click 行 uia_anchor 为空
        let plan = load_replay_plan(&conn, "rec1").unwrap();
        let step = plan.iter().find(|s| s.action_type == "click").unwrap();
        assert!(step.uia_anchor.is_none());
        let backend = crate::executor::backend::DryRunBackend::new();
        run_action(step, &backend);
        let acts = backend.actions.lock().unwrap().clone();
        assert!(
            acts.iter().any(|a| a.starts_with("click(100")),
            "无锚点应回退到裸 click，got: {:?}",
            acts
        );
    }

    /// 锁定右键重放契约：click 步 mouse_button="right" → 必须下发 right_click（锚点路径同理）。
    /// 桌面右键菜单（新建文件等）链路依赖此行为；若派发被回退成左键，该测试立即失败。
    #[test]
    fn test_right_click_step_dispatches_right_button() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute(
            "CREATE TABLE steps (id TEXT, recording_id TEXT, order_index INTEGER, step_number INTEGER, action_type TEXT, x INTEGER, y INTEGER, window_title TEXT, description TEXT, after_screenshot_path TEXT, hotkey_combo TEXT, uia_anchor TEXT, mouse_button TEXT, title TEXT, start_x INTEGER, start_y INTEGER, end_x INTEGER, end_y INTEGER, scroll_delta INTEGER)",
            [],
        )
        .unwrap();
        // 无锚点右键
        conn.execute(
            "INSERT INTO steps (id, recording_id, order_index, step_number, action_type, x, y, window_title, description, after_screenshot_path, hotkey_combo, uia_anchor, mouse_button) VALUES ('r1','rec1',0,1,'click',655,269,'Program Manager','','','','','right')",
            [],
        )
        .unwrap();
        // 带锚点右键
        conn.execute(
            "INSERT INTO steps (id, recording_id, order_index, step_number, action_type, x, y, window_title, description, after_screenshot_path, hotkey_combo, uia_anchor, mouse_button) VALUES ('r2','rec1',1,2,'click',701,460,'Program Manager','','','','{\"cls\":\"Button\",\"fx\":0.5,\"fy\":0.5,\"wx\":0.5,\"wy\":0.5}','right')",
            [],
        )
        .unwrap();
        // 对照组：左键 click 不受影响
        conn.execute(
            "INSERT INTO steps (id, recording_id, order_index, step_number, action_type, x, y, window_title, description, after_screenshot_path, hotkey_combo, uia_anchor, mouse_button) VALUES ('r3','rec1',2,3,'click',100,100,'Program Manager','','','','','left')",
            [],
        )
        .unwrap();
        let plan = load_replay_plan(&conn, "rec1").unwrap();
        assert_eq!(plan[0].mouse_button, "right");
        let backend = DryRunBackend::new();
        for action in &plan {
            run_action(action, &backend);
        }
        let acts = backend.actions.lock().unwrap().clone();
        assert!(
            acts.iter().any(|a| a.contains("right_click(655")),
            "右键步应下发 right_click，acts={:?}",
            acts
        );
        assert!(
            acts.iter().any(|a| a.contains("right_click_anchored(701")),
            "带锚点右键步应走 right_click_anchored，acts={:?}",
            acts
        );
        assert!(
            acts.iter().any(|a| a.contains("click(100")),
            "左键步行为不变，acts={:?}",
            acts
        );
        // 右键不得再走左键路径
        assert!(
            !acts.iter().any(|a| a.starts_with("click(655")),
            "右键步不应退化成左键，acts={:?}",
            acts
        );
    }

    /// 锁定双击重放契约：click 步 mouse_button="left-double" → 必须下发 double_click；
    /// 旧数据兜底：mouse_button 为空但标题含「双击」→ 同样下发 double_click。
    /// 双击打开文件等链路依赖此行为；若退化为单击，该测试立即失败。
    #[test]
    fn test_double_click_step_dispatches_double_click() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute(
            "CREATE TABLE steps (id TEXT, recording_id TEXT, order_index INTEGER, step_number INTEGER, action_type TEXT, x INTEGER, y INTEGER, window_title TEXT, description TEXT, after_screenshot_path TEXT, hotkey_combo TEXT, uia_anchor TEXT, mouse_button TEXT, title TEXT, start_x INTEGER, start_y INTEGER, end_x INTEGER, end_y INTEGER, scroll_delta INTEGER)",
            [],
        )
        .unwrap();
        // 新数据：mouse_button 显式 left-double
        conn.execute(
            "INSERT INTO steps (id, recording_id, order_index, step_number, action_type, x, y, window_title, description, after_screenshot_path, hotkey_combo, uia_anchor, mouse_button, title) VALUES ('d1','rec1',0,1,'click',300,200,'Program Manager','','','','','left-double','双击 文件')",
            [],
        )
        .unwrap();
        // 旧数据兜底：mouse_button 为空 + 标题含「双击」
        conn.execute(
            "INSERT INTO steps (id, recording_id, order_index, step_number, action_type, x, y, window_title, description, after_screenshot_path, hotkey_combo, uia_anchor, mouse_button, title) VALUES ('d2','rec1',1,2,'click',300,200,'Program Manager','','','','','','双击 新建文档')",
            [],
        )
        .unwrap();
        // 带锚点双击
        conn.execute(
            "INSERT INTO steps (id, recording_id, order_index, step_number, action_type, x, y, window_title, description, after_screenshot_path, hotkey_combo, uia_anchor, mouse_button, title) VALUES ('d3','rec1',2,3,'click',120,80,'Program Manager','','','','{\"cls\":\"Button\",\"fx\":0.5,\"fy\":0.5,\"wx\":0.5,\"wy\":0.5}','left-double','双击 图标')",
            [],
        )
        .unwrap();
        // 对照组：普通左键不受兜底影响
        conn.execute(
            "INSERT INTO steps (id, recording_id, order_index, step_number, action_type, x, y, window_title, description, after_screenshot_path, hotkey_combo, uia_anchor, mouse_button, title) VALUES ('d4','rec1',3,4,'click',50,50,'Program Manager','','','','','left','点击 桌面')",
            [],
        )
        .unwrap();
        let plan = load_replay_plan(&conn, "rec1").unwrap();
        assert_eq!(plan[0].mouse_button, "left-double");
        assert_eq!(plan[1].mouse_button, "left-double", "旧数据应按标题兜底为 left-double");
        let backend = DryRunBackend::new();
        for action in &plan {
            run_action(action, &backend);
        }
        let acts = backend.actions.lock().unwrap().clone();
        assert!(
            acts.iter().any(|a| a.contains("double_click(300")),
            "双击步应下发 double_click，acts={:?}",
            acts
        );
        assert!(
            acts.iter().filter(|a| a.contains("double_click(300")).count() == 2,
            "新旧两条双击数据都应下发 double_click，acts={:?}",
            acts
        );
        assert!(
            acts.iter().any(|a| a.contains("double_click_anchored(120")),
            "带锚点双击步应走 double_click_anchored，acts={:?}",
            acts
        );
        assert!(
            acts.iter().any(|a| a.contains("click(50")),
            "普通左键行为不变，acts={:?}",
            acts
        );
        assert!(
            !acts.iter().any(|a| a.starts_with("click(300")),
            "双击步不应退化成单击，acts={:?}",
            acts
        );
    }

    #[test]
    fn test_classify_sensitivity() {
        assert!(!classify_sensitivity("click"));
        assert!(!classify_sensitivity("type"));
        assert!(!classify_sensitivity("input"));
        assert!(classify_sensitivity("hotkey"));
        assert!(classify_sensitivity("copy"));
        assert!(classify_sensitivity("paste"));
        assert!(classify_sensitivity("drag"));
    }

    #[test]
    fn test_plan_requires_approval() {
        let click = ReplayAction {
            scroll_delta: None,
            start_x: None, start_y: None, end_x: None, end_y: None,
            step_number: 1,
            action_type: "click".into(),
            x: Some(1),
            y: Some(1),
            window_title: "W".into(),
            text: None,
            after_screenshot: None,
            hotkey_combo: None,
            uia_anchor: None,
            mouse_button: String::new(),
        };
        let plan_only_click = vec![click.clone()];
        assert!(!plan_requires_approval(&plan_only_click));

        let mut mixed = plan_only_click;
        mixed.push(ReplayAction {
            scroll_delta: None,
            start_x: None, start_y: None, end_x: None, end_y: None,
            step_number: 2,
            action_type: "copy".into(),
            x: None,
            y: None,
            window_title: "W".into(),
            text: None,
            after_screenshot: None,
            hotkey_combo: Some("Ctrl+C".into()),
            uia_anchor: None,
            mouse_button: String::new(),
        });
        assert!(plan_requires_approval(&mixed));
    }

    #[test]
    fn test_approval_engaged() {
        // 无可敏感步骤 → 不上闸
        assert!(!approval_engaged(false, "check", false));
        // 模式 off → 永不上闸（向后兼容默认行为）
        assert!(!approval_engaged(true, "off", false));
        // 已批准 → 上闸解除
        assert!(!approval_engaged(true, "check", true));
        // 含敏感 + check + 未批准 → 上闸
        assert!(approval_engaged(true, "check", false));
    }

    /// 锁定「创建文件」重放编排：Ctrl+S（hotkey，聚焦主窗口）→ 输文件名（type，聚焦"另存为"）
    /// → 回车（hotkey）。这是「重放没建出目标文件」的核心复现路径；若导航键/输入步的聚焦或
    /// 组合键下发被回退，该测试立即失败，防止静默回归。
    #[test]
    fn test_save_as_replay_sequence_locks_create_file_flow() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute(
            "CREATE TABLE steps (id TEXT, recording_id TEXT, order_index INTEGER, step_number INTEGER, action_type TEXT, x INTEGER, y INTEGER, window_title TEXT, description TEXT, after_screenshot_path TEXT, hotkey_combo TEXT, uia_anchor TEXT, mouse_button TEXT, title TEXT, start_x INTEGER, start_y INTEGER, end_x INTEGER, end_y INTEGER, scroll_delta INTEGER)",
            [],
        )
        .unwrap();
        let rows = [
            (1i32, "hotkey", 0i32, 0i32, "无标题 - 记事本", "Ctrl+S 保存", "Ctrl+S"),
            (2i32, "type", 0i32, 0i32, "另存为", "test.txt", ""),
            (3i32, "hotkey", 0i32, 0i32, "另存为", "Enter 确认", "Enter"),
        ];
        for (i, (si, at, x, y, wt, d, hk)) in rows.iter().enumerate() {
            conn.execute(
                "INSERT INTO steps (id, recording_id, order_index, step_number, action_type, x, y, window_title, description, after_screenshot_path, hotkey_combo, uia_anchor, mouse_button) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13)",
                rusqlite::params!["s{i}", "rec1", i as i32, *si, at, *x, *y, wt, d, "", hk, "", ""],
            )
            .unwrap();
        }
        let plan = load_replay_plan(&conn, "rec1").unwrap();
        let backend = DryRunBackend::new();
        for action in &plan {
            run_action(action, &backend);
        }
        let acts = backend.actions.lock().unwrap().clone();
        assert!(
            acts.iter().any(|a| a == "focus(无标题 - 记事本)"),
            "Ctrl+S 步应先聚焦记事本主窗口，acts={:?}",
            acts
        );
        assert!(
            acts.iter().any(|a| a == "hotkey(Ctrl+S)"),
            "Ctrl+S 应以组合键下发，acts={:?}",
            acts
        );
        assert!(
            acts.iter().any(|a| a == "focus(另存为)"),
            "输文件名步应聚焦另存为对话框，acts={:?}",
            acts
        );
        assert!(
            acts.iter().any(|a| a.starts_with("type(")),
            "应注入文件名文本，acts={:?}",
            acts
        );
        assert!(
            acts.iter().any(|a| a == "hotkey(Enter)"),
            "回车确认应以组合键下发（导航键已落步），acts={:?}",
            acts
        );
    }
}
