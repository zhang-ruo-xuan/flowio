//! 精准重放核心逻辑（与平台无关，可单测）。
//!
//! 读取录制源 DB 的 steps（母版锚点：x/y/window_title/description/after_screenshot_path），
//! 生成确定性重放动作序列。不依赖任何 LLM / 视觉模型——这是 Stage 2 壁垒的第一层。

use crate::executor::backend::ReplayBackend;
use rusqlite::params;
use std::time::Duration;

/// 单步重放动作（从源 DB 映射而来，已含坐标锚点）
#[derive(Debug, Clone)]
pub struct ReplayAction {
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
            "SELECT step_number, action_type, x, y, window_title, description, after_screenshot_path, hotkey_combo, uia_anchor \
             FROM steps WHERE recording_id = ?1 ORDER BY order_index",
        )
        .map_err(|e| e.to_string())?;
    let rows = stmt
        .query_map(params![recording_id], |row| {
            Ok(ReplayAction {
                step_number: row.get(0)?,
                action_type: row.get(1)?,
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
            })
        })
        .map_err(|e| e.to_string())?;
    let mut plan = Vec::new();
    for r in rows {
        plan.push(r.map_err(|e| e.to_string())?);
    }
    Ok(plan)
}

/// 执行单步到指定后端（focus / click / drag / type / press_combo）。
///
/// - 有坐标 → 先聚焦窗口再按动作类型派发；
/// - 无坐标（x/y 为 None）→ 仅聚焦窗口（通常是"首点激活窗口"语义）；
/// - click/drag → 鼠标点击（drag 当前降级为 click，终点缺失，Stage 2b 补）；
/// - type/input → 文本注入；
/// - copy/paste → 按结构化组合键下发（如 Ctrl+C / Ctrl+V，不依赖 title 解析）；
/// - hotkey → 按结构化组合键下发（如 Alt+Tab / Win+D）；
/// - 未知 action_type 不再静默丢弃，记录日志便于排查。
pub fn run_action(action: &ReplayAction, backend: &dyn ReplayBackend) {
    // 无坐标：仅聚焦窗口，让后续有坐标的步能落在正确窗口
    let (x, y) = match (action.x, action.y) {
        (Some(x), Some(y)) => (x, y),
        _ => {
            backend.focus(&action.window_title);
            return;
        }
    };
    backend.focus(&action.window_title);
    match action.action_type.as_str() {
        // click/drag：优先按控件锚点（Stage 2b 分辨率无关）；无锚点回退录制坐标。
        "click" | "drag" => {
            if let Some(anchor) = action.uia_anchor.as_deref() {
                if !anchor.trim().is_empty() {
                    backend.click_anchored(anchor, x, y);
                    return;
                }
            }
            backend.click(x, y);
        }
        "type" | "input" => backend.type_text(action.text.as_deref().unwrap_or("")),
        // copy/paste 是语义化的组合键步骤：直接下发对应组合键
        "copy" => backend.press_combo(action.hotkey_combo.as_deref().unwrap_or("Ctrl+C")),
        "paste" => backend.press_combo(action.hotkey_combo.as_deref().unwrap_or("Ctrl+V")),
        "hotkey" => {
            if let Some(combo) = action.hotkey_combo.as_deref() {
                if !combo.is_empty() {
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

    fn mem_db() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute(
            "CREATE TABLE steps (
                id TEXT, recording_id TEXT, order_index INTEGER,
                step_number INTEGER, action_type TEXT, x INTEGER, y INTEGER,
                window_title TEXT, description TEXT, after_screenshot_path TEXT,
                hotkey_combo TEXT, uia_anchor TEXT
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
                "INSERT INTO steps (id, recording_id, order_index, step_number, action_type, x, y, window_title, description, after_screenshot_path, hotkey_combo, uia_anchor) \
                 VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12)",
                params!["s{i}", "rec1", i as i32, *si, at, *x, *y, wt, d, a, hk, ""],
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
            "CREATE TABLE steps (id TEXT, recording_id TEXT, order_index INTEGER, step_number INTEGER, action_type TEXT, x INTEGER, y INTEGER, window_title TEXT, description TEXT, after_screenshot_path TEXT, hotkey_combo TEXT, uia_anchor TEXT)",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO steps VALUES ('s0','rec1',0,1,'click',NULL,NULL,'记事本','',NULL,'','')",
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
            "INSERT INTO steps (id, recording_id, order_index, step_number, action_type, x, y, window_title, description, after_screenshot_path, hotkey_combo, uia_anchor) \
             VALUES ('s5','rec1',4,5,'hotkey',10,20,'记事本','',NULL,'Ctrl+S','')",
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
        conn.execute("INSERT INTO steps (id, recording_id, order_index, step_number, action_type, x, y, window_title, description, after_screenshot_path, hotkey_combo, uia_anchor) VALUES ('s6','rec1',5,6,'copy',0,0,'WPS','',NULL,'Ctrl+C','')", []).unwrap();
        conn.execute("INSERT INTO steps (id, recording_id, order_index, step_number, action_type, x, y, window_title, description, after_screenshot_path, hotkey_combo, uia_anchor) VALUES ('s7','rec1',6,7,'paste',0,0,'WPS','',NULL,'Ctrl+V','')", []).unwrap();
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
            "INSERT INTO steps (id, recording_id, order_index, step_number, action_type, x, y, window_title, description, after_screenshot_path, hotkey_combo, uia_anchor) \
             VALUES ('s9','rec1',7,9,'click',100,200,'WPS','',NULL,'','{\"cls\":\"Button\",\"fx\":0.5,\"fy\":0.5,\"wx\":0.3,\"wy\":0.2}')",
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
            step_number: 1,
            action_type: "click".into(),
            x: Some(1),
            y: Some(1),
            window_title: "W".into(),
            text: None,
            after_screenshot: None,
            hotkey_combo: None,
            uia_anchor: None,
        };
        let plan_only_click = vec![click.clone()];
        assert!(!plan_requires_approval(&plan_only_click));

        let mut mixed = plan_only_click;
        mixed.push(ReplayAction {
            step_number: 2,
            action_type: "copy".into(),
            x: None,
            y: None,
            window_title: "W".into(),
            text: None,
            after_screenshot: None,
            hotkey_combo: Some("Ctrl+C".into()),
            uia_anchor: None,
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
            "CREATE TABLE steps (id TEXT, recording_id TEXT, order_index INTEGER, step_number INTEGER, action_type TEXT, x INTEGER, y INTEGER, window_title TEXT, description TEXT, after_screenshot_path TEXT, hotkey_combo TEXT, uia_anchor TEXT)",
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
                "INSERT INTO steps (id, recording_id, order_index, step_number, action_type, x, y, window_title, description, after_screenshot_path, hotkey_combo, uia_anchor) VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12)",
                rusqlite::params!["s{i}", "rec1", i as i32, *si, at, *x, *y, wt, d, "", hk, ""],
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
