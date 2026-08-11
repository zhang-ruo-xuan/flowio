use tauri::Emitter;
use tauri_plugin_global_shortcut::{
    GlobalShortcutExt, Shortcut, ShortcutState, Modifiers, Code,
};

/// Register global hotkeys.
/// Ctrl+Shift+R – start/stop recording
/// Ctrl+Shift+P – pause/resume recording
/// Ctrl+Shift+S – stop recording
pub fn init_hotkeys(app: &tauri::AppHandle) {
    // Register the plugin
    let handle = app.clone();
    app.plugin(
        tauri_plugin_global_shortcut::Builder::new()
            .with_handler(move |_app, shortcut, event| {
                if event.state != ShortcutState::Pressed {
                    return;
                }

                let mod_ctrl_shift = Modifiers::CONTROL | Modifiers::SHIFT;

                if shortcut.matches(mod_ctrl_shift, Code::KeyR) {
                    let _ = handle.emit("shortcut-record", ());
                } else if shortcut.matches(mod_ctrl_shift, Code::KeyP) {
                    let _ = handle.emit("shortcut-pause", ());
                } else if shortcut.matches(mod_ctrl_shift, Code::KeyS) {
                    let _ = handle.emit("shortcut-stop", ());
                }
            })
            .build(),
    );

    let mod_ctrl_shift = Modifiers::CONTROL | Modifiers::SHIFT;

    // Register the shortcuts
    if let Err(e) = app.global_shortcut().register(
        Shortcut::new(Some(mod_ctrl_shift), Code::KeyR),
    ) {
        eprintln!("Failed to register Ctrl+Shift+R: {}", e);
    }
    if let Err(e) = app.global_shortcut().register(
        Shortcut::new(Some(mod_ctrl_shift), Code::KeyP),
    ) {
        eprintln!("Failed to register Ctrl+Shift+P: {}", e);
    }
    if let Err(e) = app.global_shortcut().register(
        Shortcut::new(Some(mod_ctrl_shift), Code::KeyS),
    ) {
        eprintln!("Failed to register Ctrl+Shift+S: {}", e);
    }
}
