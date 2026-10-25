//! r740：`flowio://` 唤起与录制入口动作的统一分发点。
//!
//! 为什么抽这一层：原先「解析 URL → 存 pending → emit → 聚焦窗口」在 single-instance
//! 回调和 deep-link 回调里各抄了一遍，只认 `flowio://auth`；加一种动作要改四处。
//! 托盘菜单也要发同一个动作，所以把出口收成 `store_and_emit_action` 一个函数。

use std::sync::Mutex;
use tauri::{AppHandle, Emitter, Manager};

/// 冷启动竞态缓冲：窗口是启动时建的，React 挂上 `listen` 之前到达的动作先存这里，
/// 前端挂载后调 `take_pending_action` 补取一次（与 PendingDeepLink 同一套办法）。
#[derive(Default)]
pub struct PendingAction(pub Mutex<Option<&'static str>>);

/// 认得的唤起动作。加新动作只改这张表。
pub fn try_parse_action(url_str: &str) -> Option<&'static str> {
    let url = url::Url::parse(url_str).ok()?;
    if url.scheme() != "flowio" {
        return None;
    }
    match url.host_str() {
        Some("record") => Some("record"),
        Some("stop") => Some("stop"),
        _ => None,
    }
}

/// 把一个 `flowio://` URL 路由掉，返回「这个 URL 我们认不认」。
///
/// 认证回调（`flowio://auth?token=...`）走原有的 PendingDeepLink + `deep-link-auth`，
/// 动作回调（`record` / `stop`）走 PendingAction + `flowio-action`。
pub fn handle_url(app: &AppHandle, url_str: &str) -> bool {
    if let Some(v) = crate::try_parse_auth_url(url_str) {
        if let Some(pending) = app.try_state::<crate::PendingDeepLink>() {
            *pending.0.lock().unwrap() = Some(v.clone());
        }
        let _ = app.emit("deep-link-auth", v);
        return true;
    }
    if let Some(action) = try_parse_action(url_str) {
        store_and_emit_action(app, action);
        return true;
    }
    false
}

/// 入口动作的唯一出口：托盘菜单、`flowio://record`、第二实例 argv 全都汇到这两行。
pub fn store_and_emit_action(app: &AppHandle, action: &'static str) {
    if let Some(pending) = app.try_state::<PendingAction>() {
        *pending.0.lock().unwrap() = Some(action);
    }
    let _ = app.emit("flowio-action", action);
}

/// 唤起主窗口。关到托盘时窗口是 **隐藏** 不是最小化，只 `set_focus` 对隐藏窗口无效。
pub fn show_main_window(app: &AppHandle) {
    if let Some(w) = app.get_webview_window("main") {
        let _ = w.unminimize();
        let _ = w.show();
        let _ = w.set_focus();
        // 窗口重新露脸就把托盘文案复位，否则「已收起」会一直挂着说假话。
        if let Some(state) = app.try_state::<crate::tray::TrayState>() {
            let _ = state.0.set_tooltip(Some(crate::tray::TOOLTIP_STANDBY));
        }
    }
}

/// 从启动参数 / 环境变量里捞出 `flowio://` URL 并路由。
///
/// Windows 下协议处理器的送达形态不止一种（裸 URL 作 argv、`--flowio-deeplink=` 前缀、
/// 只有 argv[0]、极端情况只剩环境变量），三种都试一遍——少试一种就是点了没反应。
pub fn handle_launch_args(app: &AppHandle, argv: &[String]) -> bool {
    let mut handled = false;
    for arg in argv {
        let url_str = arg.strip_prefix("--flowio-deeplink=").unwrap_or(arg);
        if handle_url(app, url_str) {
            handled = true;
            break;
        }
    }
    if !handled && !argv.is_empty() {
        handled = handle_url(app, &argv[0]);
    }
    if !handled {
        if let Ok(env_url) = std::env::var("FLOWIO_DEEPLINK_URL") {
            handled = handle_url(app, &env_url);
        }
    }
    handled
}

#[tauri::command]
pub fn take_pending_action(state: tauri::State<'_, PendingAction>) -> Option<&'static str> {
    state.0.lock().unwrap().take()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn record_and_stop_hosts_map_to_actions() {
        assert_eq!(try_parse_action("flowio://record"), Some("record"));
        assert_eq!(try_parse_action("flowio://record/"), Some("record"));
        assert_eq!(try_parse_action("flowio://stop"), Some("stop"));
    }

    #[test]
    fn query_fragment_still_resolves() {
        // 站点侧唤起链接可能带来源参数：query 不参与 host 判定
        assert_eq!(try_parse_action("flowio://record?from=site"), Some("record"));
    }

    #[test]
    fn auth_host_is_not_an_action() {
        // 认证回调必须由 handle_url 的 auth 分支接走，不能被当成动作
        assert_eq!(try_parse_action("flowio://auth?token=abc"), None);
    }

    #[test]
    fn unknown_host_and_other_scheme_are_ignored() {
        assert_eq!(try_parse_action("flowio://settings"), None);
        assert_eq!(try_parse_action("https://flowio.cn/record"), None);
        assert_eq!(try_parse_action("not a url"), None);
    }
}
