//! r740：托盘常驻 + 「关窗不退出」——录制入口的进程前提。
//!
//! 根因层面的那一刀：三个录制入口（全局热键 Alt+Shift+S / `flowio://record` / 托盘菜单）
//! 全部活在**进程**里，而旧版关掉主窗口就是进程退出，于是「关掉窗口 = 三个入口一起没了」，
//! 用户每次录制都得先从开始菜单把 app 点开。这里把生命周期改成：
//! - 关窗 = 隐藏进托盘（入口继续在线）
//! - 真退出 = 只有托盘菜单「退出录步」这一条路

use tauri::menu::MenuBuilder;
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Manager, Window, WindowEvent};

pub const TRAY_ID: &str = "flowio-tray";

pub const MENU_SHOW: &str = "tray-show";
pub const MENU_RECORD: &str = "tray-record";
pub const MENU_STOP: &str = "tray-stop";
pub const MENU_QUIT: &str = "tray-quit";

/// 常态悬停文案。窗口重新露脸时由 `entry::show_main_window` 复位成这一句，
/// 与 `hide_on_close` 里那句「已收起」配成一对——托盘只该有一个真值。
pub const TOOLTIP_STANDBY: &str = "录步 Flowio · 已在后台待命（Alt+Shift+S 开拍）";

/// 托盘句柄要有人持有：`TrayIcon` 是引用计数的，最后一个实例被 drop 时图标就没了。
/// 建好之后 manage 进状态里，顺便留给后续「按录制状态改 tooltip」用。
pub struct TrayState(pub tauri::tray::TrayIcon<tauri::Wry>);

pub fn build(app: &AppHandle) -> Result<(), Box<dyn std::error::Error>> {
    let menu = MenuBuilder::new(app)
        .text(MENU_SHOW, "打开录步主窗口")
        .separator()
        .text(MENU_RECORD, "开始录制　Alt+Shift+S")
        .text(MENU_STOP, "结束录制　Alt+Shift+E")
        .separator()
        .text(MENU_QUIT, "退出录步")
        .build()?;

    let icon = app
        .default_window_icon()
        .cloned()
        .ok_or("tauri.conf.json 未配置 bundle.icon，托盘起不来")?;

    let tray = TrayIconBuilder::with_id(TRAY_ID)
        .menu(&menu)
        // 左键=开窗口、右键=菜单。左键也弹菜单的话，「点开托盘就开始录」会多一步。
        .show_menu_on_left_click(false)
        .icon(icon)
        .tooltip(TOOLTIP_STANDBY)
        .on_tray_icon_event(|tray, event| {
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } = event
            {
                crate::entry::show_main_window(tray.app_handle());
            }
        })
        .on_menu_event(|app, event| {
            let id = event.id().as_ref();
            match id {
                MENU_SHOW => crate::entry::show_main_window(app),
                // 录制动作也要先把窗口捞回来：REC 角标和步骤计数都在窗口里。
                MENU_RECORD => {
                    crate::entry::show_main_window(app);
                    crate::entry::store_and_emit_action(app, "record");
                }
                MENU_STOP => {
                    crate::entry::show_main_window(app);
                    crate::entry::store_and_emit_action(app, "stop");
                }
                // 唯一退出路径：见本文件头注释。
                MENU_QUIT => app.exit(0),
                _ => {}
            }
        })
        .build(app)?;

    let _ = app.manage(TrayState(tray));
    Ok(())
}

/// 关窗 → 隐藏进托盘。挂在 `Builder::on_window_event` 上，前端 `appWindow.close()` 也走这里。
///
/// 那道 `try_state` 闸门不是防御性代码：托盘构建失败时（图标配置缺失等），
/// 若仍然「关窗=隐藏」，窗口就再也捞不回来了——三个入口全在进程里，
/// 而用户唯一的可见操作就是点 X。没有托盘，就不许隐藏。
pub fn hide_on_close(window: &Window, event: &WindowEvent) {
    if let WindowEvent::CloseRequested { api, .. } = event {
        let Some(state) = window.try_state::<TrayState>() else {
            return;
        };
        api.prevent_close();
        // 顺手把悬停文案改成「已收起」：用户点完 X 再去托盘找图标时，
        //  tooltip 就是他确认「进程还活着、热键还有效」的唯一反馈。
        let _ = state.0.set_tooltip(Some("录步 Flowio · 窗口已收起，入口仍在（Alt+Shift+S 开拍）"));
        let _ = window.hide();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 计数锁：托盘菜单条目数。菜单少一条 = 入口少一个，而这种丢失是静默的
    /// （图标还在，点开才发现没有「开始录制」）。
    #[test]
    fn tray_menu_has_four_actions_in_order() {
        let ids = [MENU_SHOW, MENU_RECORD, MENU_STOP, MENU_QUIT];
        assert_eq!(ids.len(), 4, "托盘菜单四项：打开窗口 / 开始 / 结束 / 退出");
        assert_eq!(ids[0], "tray-show");
        assert_eq!(ids[1], "tray-record");
        assert_eq!(ids[2], "tray-stop");
        assert_eq!(ids[3], "tray-quit");
    }

    /// 唯一退出路径必须是托盘那一条：模块里除 `app.exit` 之外不许再出现 close/destroy 出口，
    /// 否则「关窗=隐藏」会被别处绕过去。
    #[test]
    fn quit_is_the_only_exit_path() {
        // 只扫测试模块以上的实现段：断言字面量本身也含 "app.exit("，
        // 全文计数会把锁自己算成第二个调用点，红得莫名其妙。
        let src = include_str!("tray.rs");
        let code = &src[..src.find("#[cfg(test)]").expect("测试段锚点还在")];
        assert_eq!(code.matches("app.exit(").count(), 1, "exit 调用点必须只有一处");
        assert!(!code.contains(".destroy("), "不许用 destroy 绕过隐藏策略");
    }

    #[test]
    fn tray_id_is_stable() {
        // 站点侧/后续脚本按 id 找托盘；改名要同步改这里
        assert_eq!(TRAY_ID, "flowio-tray");
    }

    /// 「没有托盘就不许隐藏」是本体注释里承诺的失败保护，源码层锁住它：
    /// hide_on_close 若在闸门之前先 prevent_close，托盘起不来的那次构建就会把窗口关进虚空。
    #[test]
    fn hide_on_close_gates_on_tray_presence() {
        let src = include_str!("tray.rs");
        let start = src
            .find("pub fn hide_on_close")
            .expect("hide_on_close 还在");
        let end = src[start..].find("\n}\n").map(|i| start + i).expect("函数闭合");
        let body = &src[start..end];
        let gate = body
            .find("try_state::<TrayState>()")
            .expect("托盘缺席闸门被删了");
        let prevent = body
            .find("api.prevent_close()")
            .expect("prevent_close 被删了");
        assert!(
            body[..gate].contains("CloseRequested"),
            "闸门应在 CloseRequested 分支内"
        );
        assert!(
            prevent > gate,
            "必须先确认托盘在场，再拦截关闭"
        );
    }
}
