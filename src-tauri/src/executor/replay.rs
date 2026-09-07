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
            "SELECT step_number, action_type, x, y, window_title, description, after_screenshot_path \
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
            })
        })
        .map_err(|e| e.to_string())?;
    let mut plan = Vec::new();
    for r in rows {
        plan.push(r.map_err(|e| e.to_string())?);
    }
    Ok(plan)
}

/// 执行单步到指定后端（focus / click / drag / type）。
///
/// - 有坐标 → 先聚焦窗口再按动作类型派发；
/// - 无坐标（x/y 为 None）→ 仅聚焦窗口（通常是"首点激活窗口"语义）；
/// - drag 当前降级为 click：源 DB 只存了单点坐标，drag 终点缺失，
///   精确拖拽需 Stage 2b 在录制端补齐（见 brief）。
pub fn run_action(action: &ReplayAction, backend: &dyn ReplayBackend) {
    match (action.x, action.y) {
        (Some(x), Some(y)) => {
            backend.focus(&action.window_title);
            match action.action_type.as_str() {
                "click" => backend.click(x, y),
                // 降级：drag 端点缺失，先以单击兜底，后续 Stage 2b 补终点
                "drag" => backend.click(x, y),
                "type" | "paste" => backend.type_text(action.text.as_deref().unwrap_or("")),
                _ => {}
            }
        }
        _ => {
            // 无坐标：仅聚焦窗口，让后续有坐标的步能落在正确窗口
            backend.focus(&action.window_title);
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

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::Connection;

    fn mem_db() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute(
            "CREATE TABLE steps (
                id TEXT, recording_id TEXT, order_index INTEGER,
                step_number INTEGER, action_type TEXT, x INTEGER, y INTEGER,
                window_title TEXT, description TEXT, after_screenshot_path TEXT
            )",
            [],
        )
        .unwrap();
        let rows = [
            (1i32, "click", 100i32, 200i32, "WPS x.xlsx", "", ""),
            (2, "type", 0, 0, "WPS x.xlsx", "hello 世界", ""),
            (3, "click", 0, 0, "WPS x.xlsx", "", "after.png"),
            (4, "drag", 300, 400, "WPS x.xlsx", "", ""),
        ];
        for (i, (si, at, x, y, wt, d, a)) in rows.iter().enumerate() {
            conn.execute(
                "INSERT INTO steps (id, recording_id, order_index, step_number, action_type, x, y, window_title, description, after_screenshot_path) \
                 VALUES (?1,?2,?3,?4,?5,?6,?7,?8,?9,?10)",
                params!["s{i}", "rec1", i as i32, *si, at, *x, *y, wt, d, a],
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
            "CREATE TABLE steps (id TEXT, recording_id TEXT, order_index INTEGER, step_number INTEGER, action_type TEXT, x INTEGER, y INTEGER, window_title TEXT, description TEXT, after_screenshot_path TEXT)",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO steps VALUES ('s0','rec1',0,1,'click',NULL,NULL,'记事本','',NULL)",
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
}
