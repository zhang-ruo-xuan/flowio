//! Stage 2a/2b 精准执行器 Tauri 命令 + Stage 3 审批层。
//!
//! 审批层（可选闸门）：当 `approval_mode="check"` 且计划含敏感步骤（hotkey/copy/
//! paste/drag）且用户尚未批准时，直接返回 `needs-approval` 且**不下发任何桌面动作**，
//! 由前端先调 `preview_replay` 展示计划、用户点「批准」后再带 `approved=true` 重放。

use crate::DbState;
use crate::executor::backend::{ReplayBackend, WindowsBackend};
use crate::executor::replay::{
    load_replay_plan, run_action, classify_sensitivity, plan_requires_approval, approval_engaged,
    ReplayAction,
};
use crate::executor::verify::verify_step;
use serde::Serialize;
use std::time::Duration;
use tauri::Manager;

/// 单步执行结果（审计/校验产物）
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StepOutcome {
    pub step_number: i32,
    pub action_type: String,
    /// ok | verify-fail | verify-skip | step-panic
    pub status: String,
    pub note: String,
    /// 执行后截图与预期截图的差异率（0~1），无校验时为 null
    pub diff_ratio: Option<f32>,
}

/// 整份录制的执行报告
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ExecReport {
    pub recording_id: String,
    pub total: usize,
    pub outcomes: Vec<StepOutcome>,
    /// 整体状态：success | partial | failed | needs-approval | no-steps
    pub overall_status: String,
}

/// 单步重放预览（前端审批 UI 用，不执行）
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PreviewStep {
    pub step_number: i32,
    pub action_type: String,
    /// 人类可读的该步目标描述（坐标 / 控件 / 组合键 / 文本输入等）
    pub target: String,
    /// 是否敏感步骤（需审批）
    pub sensitive: bool,
}

/// 重放预览（不执行、不碰桌面）
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ReplayPreview {
    pub recording_id: String,
    pub total: usize,
    pub requires_approval: bool,
    pub steps: Vec<PreviewStep>,
}

/// 落点偏差采集的「录制基准坐标」。
/// 🔴 drag 必须用真实起点 `start_x/start_y`：x/y 存的是裁图框中心（编辑器裁切
/// 用），拿它对比会虚报几十像素假偏差（2026-09-08 实测：拖拽定位只差 2px 却
/// 报成 Δy-56，把排查方向带偏两轮）。旧录制 drag 无起点（降级 click）沿用 x/y。
fn delta_reference(a: &ReplayAction) -> (Option<i32>, Option<i32>) {
    if a.action_type == "drag" {
        if let (Some(sx), Some(sy)) = (a.start_x, a.start_y) {
            return (Some(sx), Some(sy));
        }
    }
    (a.x, a.y)
}

/// 该步骤窗口是否为「桌面 / 资源管理器」上下文（新建文件重命名发生的场景）。
fn is_desktop_rename_context(title: &str) -> bool {
    let t = title.trim();
    t.is_empty()
        || t == "Program Manager"
        || t.contains("资源管理器")
        || t.contains("Explorer")
        || t.eq_ignore_ascii_case("Windows Explorer")
}

/// 文本是否形如文件名（name.ext，ext 为 1-5 位字母数字）。
/// 用于识别「重命名输入」候选；识别错了也无副作用——修正仅在
/// 「name.ext.ext 存在且 name.ext 不存在」时才会真正动手。
fn looks_like_file_name(t: &str) -> bool {
    let t = t.trim();
    if t.is_empty() || t.contains('\\') || t.contains('/') || t.contains('\n') {
        return false;
    }
    match t.rsplit_once('.') {
        Some((base, ext)) => {
            !base.is_empty()
                && !ext.is_empty()
                && ext.len() <= 5
                && ext.chars().all(|c| c.is_ascii_alphanumeric())
        }
        None => false,
    }
}

/// 生成单步的人类可读目标描述（供审批 UI 展示）。
fn describe_target(action: &ReplayAction) -> String {
    match action.action_type.as_str() {
        "click" | "drag" => {
            if let Some(anchor) = action.uia_anchor.as_deref() {
                if !anchor.trim().is_empty() {
                    if let Ok(v) = serde_json::from_str::<serde_json::Value>(anchor) {
                        let cls = v.get("cls").and_then(|c| c.as_str()).unwrap_or("控件");
                        return format!("控件 {} @ 归一化锚点", cls);
                    }
                }
            }
            if let (Some(x), Some(y)) = (action.x, action.y) {
                format!("坐标 ({}, {})", x, y)
            } else {
                "窗口内点击".into()
            }
        }
        "type" | "input" => "输入文本".into(),
        "copy" => "复制（剪贴板）".into(),
        "paste" => "粘贴（剪贴板）".into(),
        "hotkey" => format!("组合键 {}", action.hotkey_combo.clone().unwrap_or_default()),
        _ => {
            if action.window_title.trim().is_empty() {
                "（未知动作）".into()
            } else {
                format!("聚焦窗口 {}", action.window_title)
            }
        }
    }
}

/// 执行（重放）一份录制：用 Win32 确定性重放真实点选 WPS 等桌面应用；
/// 每步后抓屏与录制预期截图比对，差异超阈值标记为 `verify-fail`。
///
/// Stage 3 扩展（均向后兼容，默认不启用闸门）：
/// - `approval_mode`："off"（默认）永不上闸；"check" 时若计划含敏感步骤且未批准则
///   返回 `overall_status="needs-approval"` 且**不下发任何桌面动作**。
/// - `approved`：用户在预览 UI 点「批准」后传 true，解除闸门。
///
/// 已知边界（MVP）：同机同分辨率；drag 降级为 click（端点未存）；
/// 无坐标步仅聚焦窗口。详见 executor-stage2a-trae-brief.md。
#[tauri::command]
pub fn execute_recording(
    app: tauri::AppHandle,
    db: tauri::State<'_, DbState>,
    recording_id: String,
    step_delay_ms: Option<u64>,
    approval_mode: Option<String>,
    approved: Option<bool>,
) -> Result<ExecReport, String> {
    let delay = step_delay_ms.unwrap_or(300);

    let plan = {
        let conn = db.0.lock().map_err(|e| e.to_string())?;
        load_replay_plan(&conn, &recording_id)?
    };

    // ── 诊断：确认本次执行所用 exe 是否含窗口最小化逻辑，以及主窗口命中情况 ──
    // 放在最前，便于一眼判断「用户是否启动了带最小化逻辑的新包」：
    // 若日志首行出现本行且 main_window_exists=true，说明新包在跑、能拿到主窗口。
    let _diag_main = app.get_webview_window("main");
    let _diag_total = app.webview_windows().len();
    crate::executor::backend::replay_log(&format!(
        "[window] enter execute_recording: main_window_exists={} total_windows={}",
        _diag_main.is_some(), _diag_total
    ));

    // ── Stage 3 审批闸门：上闸则不做任何桌面动作，仅留痕并返回 needs-approval ──
    let requires = plan_requires_approval(&plan);
    let mode = approval_mode.unwrap_or_else(|| "off".to_string());
    let approved = approved.unwrap_or(false);
    if approval_engaged(requires, &mode, approved) {
        return Ok(ExecReport {
            recording_id: recording_id.clone(),
            total: plan.len(),
            outcomes: vec![],
            overall_status: "needs-approval".into(),
        });
    }

    // ── 正常重放（与 Stage 2a/2b 逻辑一致）──
    let total = plan.len();
    let mut outcomes = Vec::with_capacity(total);
    /// 落点被锚点修正的步骤（执行摘要用，见文件末尾 [summary] 日志）。
    struct DeltaRow {
        step: i32,
        rx: i32,
        ry: i32,
        ax: i32,
        ay: i32,
        d: i32,
    }
    let mut deltas: Vec<DeltaRow> = Vec::new();

    // 重放前清空焦点缓存，避免上一轮残留窗口句柄误导本轮首步聚焦。
    crate::executor::backend::reset_replay_focus_cache();

    // ── 执行期间最小化录步自身窗口 ──
    // 真实执行会向桌面绝对坐标下发鼠标/键盘；录步窗口若遮挡目标位置
    //（桌面图标、右键菜单落点等）会吃掉点击，导致整条链路空转。
    // Drop guard 保证无论中途成功/失败/提前返回，结束后都还原窗口。
    struct RestoreWindowsOnDrop(Vec<tauri::WebviewWindow>);
    impl Drop for RestoreWindowsOnDrop {
        fn drop(&mut self) {
            for w in &self.0 {
                let _ = w.unminimize();
                let _ = w.set_focus();
            }
        }
    }
    // 取需要最小化的窗口：优先按默认主窗口 label "main"；若该 label 在运行时
    // 缺失（极端情况），枚举所有窗口、排除语音字幕窗兜底。Tauri 默认主窗口 label="main"。
    let windows_to_minimize: Vec<tauri::WebviewWindow> = match app.get_webview_window("main") {
        Some(w) => vec![w],
        None => app
            .webview_windows()
            .into_values()
            .filter(|w| w.label() != "voice-caption")
            .collect(),
    };
    let _restore_guard = RestoreWindowsOnDrop(windows_to_minimize.clone());
    for w in &windows_to_minimize {
        let _ = w.minimize();
    }
    // Win32 硬兜底：若 Tauri 层因 label 解析异常未命中窗口（此前用户侧实测日志
    // 全程无 [window] 行），这里按可见标题再最小化一次，双重保证「执行期间让位」。
    let win32_minimized = crate::executor::backend::minimize_windows_by_title("录步");
    crate::executor::backend::replay_log(&format!(
        "[window] 执行开始，已最小化录步窗口（Tauri {} 个 / Win32 {} 个，避免遮挡目标坐标）",
        windows_to_minimize.len(),
        win32_minimized
    ));
    // 等最小化动画结束、桌面就绪，避免首步点击落在还在收缩的窗口上
    std::thread::sleep(Duration::from_millis(400));

    // ── 桌面类流程：显示桌面，避免第三方窗口吃掉桌面坐标 ──
    // 取证（2026-09-08 13:03 重放）：末步 at=(1574,192) hit='Chrome Legacy Window'，
    // 桌面坐标被 Chrome 占据 → 点击落进 Chrome，整条「右键新建文件」链路空转。
    // 仅当计划含桌面/空标题步骤时才显示桌面，不打扰纯应用内流程的现有窗口。
    if plan.iter().any(|a| {
        let t = a.window_title.trim();
        t.is_empty()
            || t == "Program Manager"
            || t.contains("文件资源管理器")
            || t.eq_ignore_ascii_case("Windows Explorer")
            || t.contains("Windows-Explorer")
    }) {
        if crate::executor::backend::show_desktop() {
            crate::executor::backend::replay_log(
                "[window] 计划含桌面步骤 → 已显示桌面(MIN_ALL)，确保桌面坐标不被第三方窗口吃掉",
            );
            std::thread::sleep(Duration::from_millis(300));
        } else {
            crate::executor::backend::replay_log(
                "[window] 计划含桌面步骤，但显示桌面失败（未找到 Shell_TrayWnd）",
            );
        }
    }

    let backend = WindowsBackend;
    // 降频校验：长录制每 2~3 步抽验一次（末步必验），显著减少每步全屏截图次数，
    // 缓解"卡顿"；短录制仍逐步骤验，校验门功能不变。
    let verify_every: usize = if total > 30 { 3 } else if total > 15 { 2 } else { 1 };
    let n = plan.len();
    // 「新建文件重命名」双后缀修正的待确认名（13:59 实测 qqqq.txt -> qqqq.txt.txt）
    let mut pending_rename: Option<String> = None;
    // 冷启动等待总预算：全程累计最多等 30 秒，避免多步各自等待导致执行过慢
    let mut wait_budget_ms: u64 = 30_000;
    for (i, action) in plan.iter().enumerate() {
        let mut status = "ok".to_string();
        let mut note = String::new();
        let mut diff: Option<f32> = None;

        let do_verify = verify_every == 1 || (i % verify_every == 0) || i + 1 == n;

        // ── 冷启动等待：目标窗口出现且可响应后再操作 ──
        // WPS/Office 等重应用启动需数秒，窗口标题出现后主线程可能仍在加载
        // （“未响应”），此时下发的点击/输入会被系统丢弃 → 整链空转。
        // 仅在窗口不存在或挂起时等待，命中即返回（正常流程零额外延迟）；
        // 单次上限 8s、全程预算 30s，超时按旧行为继续，绝不拖死执行。
        if wait_budget_ms > 0
            && !action.window_title.trim().is_empty()
            && !is_desktop_rename_context(&action.window_title)
        {
            let timeout = wait_budget_ms.min(8_000);
            let t0 = std::time::Instant::now();
            match crate::executor::backend::wait_window_ready(&action.window_title, timeout) {
                Some(waited) if waited > 0 => {
                    crate::executor::backend::replay_log(&format!(
                        "[wait] 目标窗口 '{}' 等待 {}ms 后就绪（应用启动/加载完成）",
                        action.window_title, waited
                    ));
                }
                _ => {}
            }
            wait_budget_ms = wait_budget_ms.saturating_sub(t0.elapsed().as_millis() as u64);
        }

        // ── 根因修复：桌面重命名输入前 Ctrl+A 全选 ──
        // Win11 资源管理器「新建文件」自动重命名只全选主名（新建文本文档），
        // 不含扩展名 → 输入 name.ext 只替换主名，残留 .txt 得 name.ext.txt
        // （2026-09-08 14:22 实测 aaaa.txt -> aaaa.txt.txt，手动操作同样中招）。
        // 先 Ctrl+A 全选整个名字（含扩展名）再输入，整名替换得 name.ext。
        if action.action_type == "type"
            && is_desktop_rename_context(&action.window_title)
            && action.text.as_deref().map_or(false, |t| looks_like_file_name(t))
        {
            backend.press_combo("Ctrl+A");
            std::thread::sleep(Duration::from_millis(80));
            crate::executor::backend::replay_log(
                "[rename] 桌面重命名输入前已 Ctrl+A 全选（整名替换，防主名残留扩展名）",
            );
        }

        // 单步隔离：任何一步的 unsafe Windows 调用或截图校验若 panic，
        // 只标记该步失败并继续，绝不拖垮整个进程（直接缓解"闪退"）。
        let step_result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            run_action(action, &backend);
            if do_verify {
                verify_step(&action.after_screenshot)
            } else {
                None
            }
        }));
        match step_result {
            Ok(d) => diff = d,
            Err(_) => {
                status = "step-panic".into();
                note = "该步执行时发生异常，已隔离，不影响后续步骤".into();
            }
        }

        // 聚焦结果必须在判定前取出（take 语义），否则会被下一步的聚焦覆盖。
        let focus_failed = crate::executor::backend::take_focus_ok() == Some(false)
            && !action.window_title.trim().is_empty();

        if status != "step-panic" {
            if action.action_type == "drag" && action.end_x.is_none() {
                note = "drag 降级为 click：该步无拖拽终点（旧录制）；重新录制后为真拖拽".into();
            }

            // 目标窗口没出现：这是级联失败的头号起点（后续步骤会在错误的
            // 前台空转）。写进每步记录 + 醒目日志，用户不看日志也能在结果
            // 面板里看到「卡在第几步、卡在哪个窗口」。
            if focus_failed
            {
                let msg = format!(
                    "目标窗口『{}』未出现/未聚焦 → 已回退录制坐标执行",
                    action.window_title
                );
                crate::executor::backend::replay_log(&format!(
                    "[stuck] 第 {} 步 title='{}'：{}（后续依赖该窗口的步骤预计连锁失败）",
                    action.step_number, action.window_title, msg
                ));
                if note.is_empty() {
                    note = msg;
                } else {
                    note = format!("{}；{}", note, msg);
                }
            }

            // 校验门：执行后截图 vs 预期
            if let Some(d) = diff {
                if d > 0.08 {
                    status = "verify-fail".into();
                    note = format!("截图差异 {:.2} 超阈值 0.08", d);
                }
            } else if do_verify
                && action
                    .after_screenshot
                    .as_deref()
                    .map_or(false, |p| !p.is_empty())
            {
                note = "verify-skip：抓屏或预期图不可用".into();
            } else if !do_verify {
                note = format!("verify-sampled（每 {} 步抽验，本步跳过）", verify_every);
            }
        }

        // ── 落点偏差采集：录制坐标 vs 锚点反算后的真实落点 ──
        // 锚点每一代演进都在「改这里」，但用户看到的现象只会是「点偏了」。
        // 攒一份清单，让执行末尾一行日志说清「哪几步被挪了、挪了多少」。
        // 🔴 drag 的 x/y 存的是「裁图框中心」（编辑器裁切用），真实起点在
        // start_x/start_y —— 拿 x/y 对比会虚报几十像素的假偏差（实测
        // 定位只差 2px 却报了 Δy-56，误导两轮排查方向）。
        let (rx, ry) = delta_reference(&action);
        if let (Some(rx), Some(ry)) = (rx, ry) {
            if let Some((ax, ay)) = crate::executor::backend::take_point() {
                let d = (((ax - rx).pow(2) + (ay - ry).pow(2)) as f64).sqrt();
                if d >= 24.0 {
                    deltas.push(DeltaRow {
                        step: action.step_number,
                        rx,
                        ry,
                        ax,
                        ay,
                        d: d.round() as i32,
                    });
                }
            }
        }

        outcomes.push(StepOutcome {
            step_number: action.step_number,
            action_type: action.action_type.clone(),
            status,
            note,
            diff_ratio: diff,
        });

        // ── 「新建文件重命名」双后缀修正 ──
        // 桌面/资源管理器重命名场景输入 name.ext 后，Enter 确认可能落盘为
        // name.ext.ext。结果导向保守修正：紧随 Enter 后若双后缀文件存在且
        // 期望名不存在，改回期望名。仅在桌面/资源管理器上下文触发。
        if action.action_type == "type" {
            if is_desktop_rename_context(&action.window_title) {
                if let Some(t) = action.text.as_deref() {
                    if looks_like_file_name(t) {
                        pending_rename = Some(t.trim().to_string());
                    }
                }
            } else {
                pending_rename = None;
            }
        } else if action.action_type == "hotkey"
            && action.hotkey_combo.as_deref() == Some("Enter")
        {
            // 不 take：实测（14:22）第二个 Enter 确认会让资源管理器再次产生
            // name.ext.txt，每个桌面 Enter 后都需检查。修正条件极保守
            // （仅 name.ext.ext 存在且 name.ext 不存在才动手），重复检查无副作用。
            if let Some(expected) = pending_rename.clone() {
                // 等资源管理器完成重命名落盘后再检查
                std::thread::sleep(Duration::from_millis(700));
                crate::executor::backend::fix_double_extension(&expected);
            }
        }

        if delay > 0 {
            std::thread::sleep(Duration::from_millis(delay));
        }
    }

    // 终检：所有步骤结束后（含最后一轮 Explorer 确认事务）再兜底修正一次，
    // 防止中途修正被资源管理器后续确认覆盖。
    if let Some(expected) = pending_rename.as_deref() {
        crate::executor::backend::fix_double_extension(expected);
    }

    // ── 执行摘要：一次看清锚点把哪些步挪了多少 ──
    // 目的是把「点偏了」这类靠肉眼/翻长日志才能定位的问题，压缩成一行结论。
    if deltas.is_empty() {
        crate::executor::backend::replay_log(
            "[summary] 落点与录制坐标全部一致（锚点未做修正）",
        );
    } else {
        let mut rows = deltas;
        rows.sort_by(|a, b| b.d.cmp(&a.d));
        let worst = &rows[0];
        let list: Vec<String> = rows
            .iter()
            .take(8)
            .map(|r| format!("#{} Δx{:+} Δy{:+}", r.step, r.ax - r.rx, r.ay - r.ry))
            .collect();
        crate::executor::backend::replay_log(&format!(
            "[summary] 锚点修正 {} 步；最大 {}px（第 {} 步 录制({},{}) → 落点({},{})）；明细 {}",
            rows.len(),
            worst.d,
            worst.step,
            worst.rx,
            worst.ry,
            worst.ax,
            worst.ay,
            list.join(" ")
        ));
    }

    let ok = outcomes
        .iter()
        .filter(|o| o.status == "ok")
        .count();
    let failed = total.saturating_sub(ok);
    let overall_status = if total == 0 {
        "no-steps".into()
    } else if failed == 0 {
        "success".into()
    } else if ok == 0 {
        "failed".into()
    } else {
        "partial".into()
    };

    Ok(ExecReport {
        recording_id,
        total,
        outcomes,
        overall_status,
    })
}

/// 解析重放计划用于审批 UI（不执行、不碰桌面）。
#[tauri::command]
pub fn preview_replay(
    db: tauri::State<'_, DbState>,
    recording_id: String,
) -> Result<ReplayPreview, String> {
    let plan = {
        let conn = db.0.lock().map_err(|e| e.to_string())?;
        load_replay_plan(&conn, &recording_id)?
    };
    let requires = plan_requires_approval(&plan);
    let steps = plan
        .iter()
        .map(|a| PreviewStep {
            step_number: a.step_number,
            action_type: a.action_type.clone(),
            target: describe_target(a),
            sensitive: classify_sensitivity(&a.action_type),
        })
        .collect();
    Ok(ReplayPreview {
        recording_id,
        total: plan.len(),
        requires_approval: requires,
        steps,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 偏差基准：drag 用真实起点（start_x/start_y），其余用 x/y；
    /// 旧录制 drag 无起点时降级用 x/y。锁定 2026-09-08 修复：
    /// 拿裁图框中心 x/y 对比曾把「定位只差 2px」虚报成 Δy-56。
    #[test]
    fn delta_reference_drag_uses_start_point() {
        use crate::executor::replay::ReplayAction;
        let mk = |at: &str| ReplayAction {
            step_number: 1,
            action_type: at.into(),
            x: Some(666),
            y: Some(364),
            window_title: "WPS".into(),
            text: None,
            after_screenshot: None,
            hotkey_combo: None,
            uia_anchor: None,
            mouse_button: String::new(),
            start_x: Some(665),
            start_y: Some(308),
            end_x: Some(667),
            end_y: Some(420),
            scroll_delta: None,
        };
        let (rx, ry) = delta_reference(&mk("drag"));
        assert_eq!((rx, ry), (Some(665), Some(308)));
        let (rx, ry) = delta_reference(&mk("click"));
        assert_eq!((rx, ry), (Some(666), Some(364)));
        // 旧录制：无起点 → 沿用 x/y
        let mut old = mk("drag");
        old.start_x = None;
        old.start_y = None;
        let (rx, ry) = delta_reference(&old);
        assert_eq!((rx, ry), (Some(666), Some(364)));
    }

    #[test]
    fn describe_target_variants() {
        let click = ReplayAction {
            scroll_delta: None,
            start_x: None, start_y: None, end_x: None, end_y: None, step_number: 1, action_type: "click".into(), x: Some(10), y: Some(20), window_title: "WPS".into(), text: None, after_screenshot: None, hotkey_combo: None, uia_anchor: None, mouse_button: String::new() };
        assert_eq!(describe_target(&click), "坐标 (10, 20)");

        let anchor = ReplayAction {
            scroll_delta: None,
            start_x: None, start_y: None, end_x: None, end_y: None, step_number: 2, action_type: "click".into(), x: Some(1), y: Some(2), window_title: "WPS".into(), text: None, after_screenshot: None, hotkey_combo: None, uia_anchor: Some("{\"cls\":\"Button\",\"fx\":0.5,\"fy\":0.5,\"wx\":0.3,\"wy\":0.2}".into()), mouse_button: String::new() };
        assert!(describe_target(&anchor).contains("Button"), "got: {}", describe_target(&anchor));

        let copy = ReplayAction {
            scroll_delta: None,
            start_x: None, start_y: None, end_x: None, end_y: None, step_number: 3, action_type: "copy".into(), x: None, y: None, window_title: "WPS".into(), text: None, after_screenshot: None, hotkey_combo: Some("Ctrl+C".into()), uia_anchor: None, mouse_button: String::new() };
        assert_eq!(describe_target(&copy), "复制（剪贴板）");

        let hk = ReplayAction {
            scroll_delta: None,
            start_x: None, start_y: None, end_x: None, end_y: None, step_number: 4, action_type: "hotkey".into(), x: None, y: None, window_title: "WPS".into(), text: None, after_screenshot: None, hotkey_combo: Some("Ctrl+S".into()), uia_anchor: None, mouse_button: String::new() };
        assert_eq!(describe_target(&hk), "组合键 Ctrl+S");

        let ty = ReplayAction {
            scroll_delta: None,
            start_x: None, start_y: None, end_x: None, end_y: None, step_number: 5, action_type: "type".into(), x: None, y: None, window_title: "WPS".into(), text: Some("hi".into()), after_screenshot: None, hotkey_combo: None, uia_anchor: None, mouse_button: String::new() };
        assert_eq!(describe_target(&ty), "输入文本");
    }

    #[test]
    #[test]
    fn desktop_rename_context_detection() {
        assert!(is_desktop_rename_context("Program Manager"));
        assert!(is_desktop_rename_context(""));
        assert!(is_desktop_rename_context("文件资源管理器"));
        assert!(!is_desktop_rename_context("*qqqq.txt - Notepad"));
        assert!(!is_desktop_rename_context("WPS x.xlsx"));
    }

    #[test]
    fn file_name_like_detection() {
        assert!(looks_like_file_name("qqqq.txt"));
        assert!(looks_like_file_name("report.docx"));
        assert!(looks_like_file_name("a.b"));
        assert!(!looks_like_file_name("no-ext"));
        assert!(!looks_like_file_name("multi\nline.txt"));
        assert!(!looks_like_file_name("path/to.txt"));
        assert!(!looks_like_file_name(".hidden"));
        assert!(!looks_like_file_name("name.toolongext"));
        assert!(!looks_like_file_name("是地方还是看"));
    }
}
