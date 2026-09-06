//! 实时字幕悬浮窗（屏幕级）。
//!
//! 为什么需要独立窗口：字幕面板原先挂在主窗口内部（position:fixed），
//! 而录制的真实场景是「用户在 WPS/浏览器等其它应用里操作，Flowio 主窗口
//! 在后台」——面板永远看不见（2026-09-06 用户反馈「没实现」，日志证实
//! 后端转写正常、纯显示层问题）。
//!
//! 方案：Tauri 2 运行时建一个小 WebviewWindow —— 透明、无装饰、置顶、
//! 跳过任务栏、点击穿透，贴屏幕底部居中。加载 dist 里的 caption.html
//! （纯 vanilla JS，经 withGlobalTauri 拿到 event.listen），监听全局
//! `voice://caption` 事件逐句显示最近 3 句。可见性由主窗口控制：
//! 开麦 show / 关麦·暂停·停止 hide，生命周期不持有状态。

use tauri::{AppHandle, Emitter, Manager, PhysicalPosition, PhysicalSize, WebviewUrl, WebviewWindow, WebviewWindowBuilder};

const CAPTION_LABEL: &str = "voice-caption";
const WIN_W: i32 = 720;
const WIN_H: i32 = 150;
const BOTTOM_MARGIN: i32 = 90;

/// 拿到字幕窗；不存在就创建（首次开麦时）。
fn ensure_window(app: &AppHandle) -> Result<WebviewWindow, String> {
    if let Some(w) = app.get_webview_window(CAPTION_LABEL) {
        return Ok(w);
    }
    let w = WebviewWindowBuilder::new(app, CAPTION_LABEL, WebviewUrl::App("caption.html".into()))
        .title("flowio-voice-caption")
        .decorations(false)
        .resizable(false)
        .transparent(true)
        .always_on_top(true)
        .skip_taskbar(true)
        .shadow(false)
        .focused(false)
        .visible(false)
        .build()
        .map_err(|e| e.to_string())?;
    // 点击穿透：字幕窗绝不挡用户的操作
    let _ = w.set_ignore_cursor_events(true);
    Ok(w)
}

/// 贴主显示器底部居中（任务栏上方留 90px）。
fn position_bottom_center(w: &WebviewWindow) {
    let Ok(Some(m)) = w.current_monitor() else { return };
    let size = m.size();
    let pos = m.position();
    let x = pos.x + (size.width as i32 - WIN_W) / 2;
    let y = pos.y + size.height as i32 - WIN_H - BOTTOM_MARGIN;
    let _ = w.set_size(PhysicalSize::new(WIN_W as u32, WIN_H as u32));
    let _ = w.set_position(PhysicalPosition::new(x, y));
}

/// 开麦时调用：确保窗口存在并显示（每次显示前先重置字幕内容）。
#[tauri::command]
pub fn voice_caption_show(app: AppHandle) -> Result<(), String> {
    let w = ensure_window(&app)?;
    position_bottom_center(&w);
    let _ = w.emit("voice://caption-reset", ());
    w.show().map_err(|e| e.to_string())
}

/// 关麦 / 暂停 / 停止录制时调用：隐藏（窗口保留复用，销毁留给进程退出）。
#[tauri::command]
pub fn voice_caption_hide(app: AppHandle) -> Result<(), String> {
    if let Some(w) = app.get_webview_window(CAPTION_LABEL) {
        w.hide().map_err(|e| e.to_string())?;
    }
    Ok(())
}

/// 实时字幕是否可用（ASR Key 能否解析出来）。
/// Key 缺失时前端不显示字幕窗，避免「正在聆听…」永远挂着却永远不出字。
#[tauri::command]
pub fn voice_live_supported(db: tauri::State<'_, crate::DbState>) -> bool {
    let Ok(conn) = db.inner().0.lock() else { return false };
    let config = crate::ai::commands::resolve_ai_config(&conn);
    let (key, _) = crate::voice::commands::resolve_asr_key(&config);
    !key.trim().is_empty()
}
