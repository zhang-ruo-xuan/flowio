use tauri::Emitter;
use tauri_plugin_global_shortcut::{
    GlobalShortcutExt, Shortcut, ShortcutState, Modifiers, Code,
};

/// Map a pressed shortcut to the Flowio event name it triggers.
///
/// Only Alt+Shift+S / Alt+Shift+P / Alt+Shift+E are recognized;
/// everything else (wrong modifier combo, wrong key, extra modifiers)
/// maps to `None` and is ignored by the handler.
fn resolve_shortcut_event(shortcut: &Shortcut) -> Option<&'static str> {
    let mod_alt_shift = Modifiers::ALT | Modifiers::SHIFT;

    if shortcut.matches(mod_alt_shift, Code::KeyS) {
        Some("shortcut-record")
    } else if shortcut.matches(mod_alt_shift, Code::KeyP) {
        Some("shortcut-pause")
    } else if shortcut.matches(mod_alt_shift, Code::KeyE) {
        Some("shortcut-stop")
    } else {
        None
    }
}

/// Register global hotkeys.
/// Alt+Shift+S – 开始 / 恢复 / 停止（三态切换）
/// Alt+Shift+P – 暂停
/// Alt+Shift+E – 停止
pub fn init_hotkeys(app: &tauri::AppHandle) {
    // Register the plugin
    let handle = app.clone();
    let _ = app.plugin(
        tauri_plugin_global_shortcut::Builder::new()
            .with_handler(move |_app, shortcut, event| {
                if event.state != ShortcutState::Pressed {
                    return;
                }

                if let Some(event_name) = resolve_shortcut_event(shortcut) {
                    let _ = handle.emit(event_name, ());
                }
            })
            .build(),
    );

    let mod_alt_shift = Modifiers::ALT | Modifiers::SHIFT;

    // Register the shortcuts
    if let Err(e) = app.global_shortcut().register(
        Shortcut::new(Some(mod_alt_shift), Code::KeyS),
    ) {
        eprintln!("Failed to register Alt+Shift+S: {}", e);
    }
    if let Err(e) = app.global_shortcut().register(
        Shortcut::new(Some(mod_alt_shift), Code::KeyP),
    ) {
        eprintln!("Failed to register Alt+Shift+P: {}", e);
    }
    if let Err(e) = app.global_shortcut().register(
        Shortcut::new(Some(mod_alt_shift), Code::KeyE),
    ) {
        eprintln!("Failed to register Alt+Shift+E: {}", e);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sc(mods: Modifiers, code: Code) -> Shortcut {
        Shortcut::new(Some(mods), code)
    }

    #[test]
    fn alt_shift_s_triggers_record() {
        let s = sc(Modifiers::ALT | Modifiers::SHIFT, Code::KeyS);
        assert_eq!(resolve_shortcut_event(&s), Some("shortcut-record"));
    }

    #[test]
    fn alt_shift_p_triggers_pause() {
        let s = sc(Modifiers::ALT | Modifiers::SHIFT, Code::KeyP);
        assert_eq!(resolve_shortcut_event(&s), Some("shortcut-pause"));
    }

    #[test]
    fn alt_shift_e_triggers_stop() {
        let s = sc(Modifiers::ALT | Modifiers::SHIFT, Code::KeyE);
        assert_eq!(resolve_shortcut_event(&s), Some("shortcut-stop"));
    }

    #[test]
    fn old_ctrl_shift_is_ignored() {
        // 旧版 Ctrl+Shift+S/P/E 已停用：不再触发任何事件
        let mod_ctrl_shift = Modifiers::CONTROL | Modifiers::SHIFT;
        assert_eq!(resolve_shortcut_event(&sc(mod_ctrl_shift, Code::KeyS)), None);
        assert_eq!(resolve_shortcut_event(&sc(mod_ctrl_shift, Code::KeyP)), None);
        assert_eq!(resolve_shortcut_event(&sc(mod_ctrl_shift, Code::KeyE)), None);
    }

    #[test]
    fn missing_shift_is_ignored() {
        // 只有 Alt 没有 Shift：不触发
        assert_eq!(resolve_shortcut_event(&sc(Modifiers::ALT, Code::KeyS)), None);
        assert_eq!(resolve_shortcut_event(&sc(Modifiers::ALT, Code::KeyP)), None);
        assert_eq!(resolve_shortcut_event(&sc(Modifiers::ALT, Code::KeyE)), None);
    }

    #[test]
    fn missing_alt_is_ignored() {
        assert_eq!(resolve_shortcut_event(&sc(Modifiers::SHIFT, Code::KeyS)), None);
    }

    #[test]
    fn no_modifiers_is_ignored() {
        assert_eq!(resolve_shortcut_event(&sc(Modifiers::empty(), Code::KeyS)), None);
    }

    #[test]
    fn alt_shift_other_key_is_ignored() {
        assert_eq!(resolve_shortcut_event(&sc(Modifiers::ALT | Modifiers::SHIFT, Code::KeyA)), None);
        assert_eq!(resolve_shortcut_event(&sc(Modifiers::ALT | Modifiers::SHIFT, Code::KeyQ)), None);
        assert_eq!(resolve_shortcut_event(&sc(Modifiers::ALT | Modifiers::SHIFT, Code::F1)), None);
    }

    #[test]
    fn extra_ctrl_modifier_is_ignored() {
        // Alt+Shift+Ctrl+S 带多余修饰键：不触发（matches 精确匹配）
        let s = sc(Modifiers::ALT | Modifiers::SHIFT | Modifiers::CONTROL, Code::KeyS);
        assert_eq!(resolve_shortcut_event(&s), None);
    }
}
