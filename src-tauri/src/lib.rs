// StepSnap-style annotation
mod db;
mod recorder;
mod settings;
mod ai;
mod editor;
mod export;
mod file_browser;
mod redaction;
mod hotkey;
mod voice;
mod job_notify;
mod executor;
mod anchor;
mod uia;

use std::collections::HashMap;
use std::sync::Mutex;
use rusqlite::Connection;
use recorder::Recorder;
use tauri::{Manager, Emitter, AppHandle};
use tauri_plugin_deep_link::DeepLinkExt;
use tauri_plugin_store;
use tauri_plugin_notification;

/// 用系统默认浏览器打开 URL
#[tauri::command]
async fn open_external_url(url: String) -> Result<(), String> {
    // tauri_plugin_opener 提供了 shell::open
    tauri_plugin_opener::open_url(&url, None::<&str>)
        .map_err(|e| format!("打开链接失败: {}", e))
}

/// 解析 flowio://auth?token=...&id=...&email=...&display_name=...&bio=...&avatar=... 返回 JSON value
fn try_parse_auth_url(url_str: &str) -> Option<serde_json::Value> {
    let url = url::Url::parse(url_str).ok()?;
    if url.scheme() != "flowio" || url.host_str() != Some("auth") {
        return None;
    }
    let token = url.query_pairs().find(|(k, _)| k == "token").map(|(_, v)| v.to_string())?;
    let id = url.query_pairs().find(|(k, _)| k == "id").map(|(_, v)| v.to_string()).unwrap_or_default();
    let email = url.query_pairs().find(|(k, _)| k == "email").map(|(_, v)| v.to_string()).unwrap_or_default();
    let display_name = url.query_pairs().find(|(k, _)| k == "display_name").map(|(_, v)| v.to_string());
    let bio = url.query_pairs().find(|(k, _)| k == "bio").map(|(_, v)| v.to_string());
    let avatar = url.query_pairs().find(|(k, _)| k == "avatar").map(|(_, v)| v.to_string());
    Some(serde_json::json!({
        "token": token,
        "id": id,
        "email": email,
        "display_name": display_name,
        "bio": bio,
        "avatar": avatar,
    }))
}

pub struct DbState(pub Mutex<Connection>);

pub struct RecordingManager(pub Mutex<HashMap<String, Recorder>>);

/// 安装 panic 落盘钩子：任何线程的 panic 都把时间/位置/堆栈追加写入
/// `D:\录步\flowio_panic.log`，便于远程诊断"闪退"（打包后的 exe 默认吞掉 stderr）。
fn install_panic_hook() {
    use std::io::Write;
    let default = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let ts = chrono::Local::now().format("%Y-%m-%d %H:%M:%S");
        let mut msg = String::new();
        if let Some(s) = info.payload().downcast_ref::<&str>() {
            msg.push_str(s);
        } else if let Some(s) = info.payload().downcast_ref::<String>() {
            msg.push_str(s);
        }
        let loc = info
            .location()
            .map(|l| format!("{}:{}:{}", l.file(), l.line(), l.column()))
            .unwrap_or_else(|| "unknown".into());
        let bt = std::backtrace::Backtrace::force_capture();
        let entry = format!("[{}] PANIC at {}: {}\n{:?}\n\n", ts, loc, msg, bt);
        if let Ok(mut f) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(r"D:\录步\flowio_panic.log")
        {
            let _ = f.write_all(entry.as_bytes());
        }
        default(info);
    }));
}

/// 启动时收到的 deep-link 认证数据，防止前端监听器尚未注册时事件丢失
pub struct PendingDeepLink(pub Mutex<Option<serde_json::Value>>);

/// 前端启动后调用，检查是否有未处理的 deep-link 认证数据
#[tauri::command]
fn get_pending_deep_link(state: tauri::State<'_, PendingDeepLink>) -> Option<serde_json::Value> {
    state.0.lock().unwrap().take()
}

pub fn run() {
    install_panic_hook();
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_store::Builder::default().build())
        .plugin(tauri_plugin_single_instance::init(|app, argv, _cwd| {
            // 第二实例被唤醒时（如 deep-link URL），deep-link 插件不会触发 on_open_url，
            // URL 通过 argv 传入，需在此手动解析并转发事件
            let mut handled = false;
            for arg in &argv {
                // 兼容三种格式：裸 URL（flowio://...）、--flowio-deeplink=flowio://...、
                // Windows 协议处理器可能传 URL 作为唯一 argv
                let url_str = arg.strip_prefix("--flowio-deeplink=").unwrap_or(arg);
                if let Some(u) = try_parse_auth_url(url_str) {
                    handled = true;
                    // 持久化到 PendingDeepLink，防止前端监听器尚未就绪时事件丢失
                    if let Some(pending) = app.try_state::<PendingDeepLink>() {
                        *pending.0.lock().unwrap() = Some(u.clone());
                    }
                    let _ = app.emit("deep-link-auth", u);
                    break;
                }
            }
            // argv 为空时尝试从 argv[0] 裸 URL 解析（Windows 可能将 URL 作为程序路径传入）
            if !handled && !argv.is_empty() {
                if let Some(u) = try_parse_auth_url(&argv[0]) {
                    handled = true;
                    if let Some(pending) = app.try_state::<PendingDeepLink>() {
                        *pending.0.lock().unwrap() = Some(u.clone());
                    }
                    let _ = app.emit("deep-link-auth", u);
                }
            }
            // Windows 某些情况下 argv 是空或只包含程序路径，尝试从环境变量获取
            if !handled {
                if let Ok(env_url) = std::env::var("FLOWIO_DEEPLINK_URL") {
                    if let Some(u) = try_parse_auth_url(&env_url) {
                        handled = true;
                        if let Some(pending) = app.try_state::<PendingDeepLink>() {
                            *pending.0.lock().unwrap() = Some(u.clone());
                        }
                        let _ = app.emit("deep-link-auth", u);
                    }
                }
            }
            // 聚焦窗口
            if let Some(w) = app.get_webview_window("main") {
                let _ = w.set_focus();
            }
        }))
        .plugin(tauri_plugin_deep_link::init())
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_notification::init())
        .setup(|app| {
            // 全局快捷键：Alt+Shift+S 录制 / Alt+Shift+P 暂停 / Alt+Shift+E 停止
            hotkey::init_hotkeys(app.handle());

            let app_data_dir = app.path().app_data_dir().expect("failed to get app data dir");
            std::fs::create_dir_all(&app_data_dir).ok();
            let db_path = app_data_dir.join("flowio.db");
            // 启动即自动备份 DB（保留 5 份）：防数据目录被外部清空/损坏后全灭。
            // 失败静默，绝不阻塞启动。
            if let Some(bak) = db::auto_backup_database(&db_path, 5) {
                println!("[startup] db auto-backup -> {}", bak.display());
            }
            // 后台线程（语音转写/翻译）也必须用同一个绝对路径，
            // 否则相对路径会在 cwd 另开空库（历史 bug：语音「没有步骤」）
            db::set_db_path(db_path.clone());
            let conn = Connection::open(db_path).expect("Failed to open database");
            db::init_db(&conn);
            settings::commands::refresh_auto_redact_policy(&conn);
            app.manage(DbState(Mutex::new(conn)));
            app.manage(RecordingManager(Mutex::new(HashMap::new())));
            app.manage(PendingDeepLink(Mutex::new(None)));
            app.manage(voice::commands::VoiceCaptureState(Default::default()));
            app.manage(voice::capture::MicCaptureState(Default::default()));

            // 扩展 → 桌面端 选区上报通道：浏览器扩展在页面内读到精确 selection
            // 矩形后 POST 到本机回环端口，COPY 步骤裁图优先用它（L0）。
            // 仅监听 127.0.0.1；扩展未安装时静默无影响，自动回落到 UIA。
            recorder::selection_bridge::start_server();

            // Deep-link handler: flowio://auth?token=...&id=...&email=...
            let handle = app.handle().clone();
            // Register the flowio:// scheme (Windows writes HKCU\Software\Classes\flowio)
            let _ = app.deep_link().register("flowio");
            app.deep_link().on_open_url(move |event| {
                for url in event.urls() {
                    println!("[deep-link] 收到 URL: {}", url); // 调试日志
                    if let Some(u) = try_parse_auth_url(url.as_str()) {
                        // 先持久化，防止前端尚未就绪
                        if let Some(pending) = handle.try_state::<PendingDeepLink>() {
                            *pending.0.lock().unwrap() = Some(u.clone());
                        }
                        let _ = handle.emit("deep-link-auth", u);
                        break;
                    }
                }
                // 聚焦窗口
                if let Some(w) = handle.get_webview_window("main") {
                    let _ = w.set_focus();
                }
            });

            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            db::commands::list_recordings,
            db::commands::set_current_user,
            db::commands::claim_recording,
            db::commands::delete_recording,
            db::commands::delete_recordings,
            db::commands::save_recording,
            db::commands::duplicate_recording,
            db::commands::update_recording_app_name,
            recorder::commands::start_recording,
            recorder::commands::list_target_windows,
            recorder::commands::pause_recording,
            recorder::commands::resume_recording,
            recorder::commands::finish_recording,
            recorder::commands::cancel_recording,
            recorder::commands::mark_screenshot_step,
            recorder::commands::get_active_recording,
            settings::commands::get_appearance_config,
            settings::commands::set_appearance_config,
            ai::commands::generate_ai_steps,
            ai::commands::get_ai_config,
            ai::commands::get_first_ai_config,
            ai::commands::set_ai_config,
            ai::commands::remove_ai_config,
            ai::commands::set_default_provider,
            ai::commands::get_default_provider,
            ai::commands::test_api_key,
            ai::commands::validate_custom_api,
            ai::commands::list_custom_providers,
            ai::commands::add_custom_provider,
            ai::commands::remove_custom_provider,
            editor::commands::load_recording,
            editor::commands::update_step,
            editor::commands::update_step_crop,
            editor::commands::delete_step,
            editor::commands::replace_in_steps,
            editor::commands::reorder_steps,
            editor::commands::merge_steps,
            editor::commands::auto_merge_steps,
            editor::commands::simplify_steps_ai,
            editor::commands::undo_last_merge,
            editor::commands::can_undo_merge,
            editor::commands::push_edit_snapshot,
            editor::commands::edit_undo,
            editor::commands::edit_redo,
            editor::commands::edit_history_sizes,
            editor::commands::editor_view_get,
            editor::commands::editor_view_set,
            editor::commands::add_step,
            editor::commands::update_recording_title,
            editor::commands::crop_step_screenshot,
            editor::commands::save_step_screenshot_full,
            editor::commands::upload_step_screenshot,
            editor::commands::delete_step_screenshot,
            export::commands::export_recording,
            export::commands::load_skill_package,
            settings::commands::get_redaction_config,
            settings::commands::set_redaction_config,
            settings::commands::get_setting,
            settings::commands::set_setting,
            redaction::commands::redact_recording_text,
            redaction::commands::redact_step_screenshot,
            redaction::commands::scan_redact_screenshots,
            redaction::commands::cancel_scan_redact,
            voice::commands::voice_dbg_log,
            voice::commands::voice_begin,
            voice::commands::voice_append,
            voice::capture::voice_mic_start,
            voice::capture::voice_mic_set_paused,
            voice::capture::voice_mic_samples,
            voice::capture::voice_mic_stop,
            voice::commands::voice_finish,
            voice::commands::voice_cancel,
            voice::commands::voice_get_hotwords,
            voice::commands::voice_set_hotwords,
            voice::commands::voice_get_correction_enabled,
            voice::commands::voice_set_correction_enabled,
            ai::translate::translate_recording,
            ai::translate::get_recording_translations,
            ai::translate::save_recording_translations,
            file_browser::list_directory,
            file_browser::get_drives,
            file_browser::get_known_folders,
            open_external_url,
            get_pending_deep_link,
            executor::commands::execute_recording,
            executor::commands::preview_replay,
            executor::commands::get_replay_audit,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}

#[cfg(test)]
mod tests {
    use super::*;

    fn assert_send<T: Send>() {}
    fn assert_sync<T: Sync>() {}

    #[test]
    fn db_state_holds_connection() {
        let conn = Connection::open_in_memory().unwrap();
        db::init_db(&conn);
        let state = DbState(Mutex::new(conn));
        let guard = state.0.lock().unwrap();
        let n: i64 = guard
            .query_row("SELECT COUNT(*) FROM recordings", [], |r| r.get(0))
            .unwrap();
        assert_eq!(n, 0);
    }

    #[test]
    fn recording_manager_starts_empty_and_inserts() {
        let mgr = RecordingManager(Mutex::new(HashMap::new()));
        {
            let mut map = mgr.0.lock().unwrap();
            map.insert("r1".into(), Recorder::new("r1".into(), "T".into()));
        }
        let map = mgr.0.lock().unwrap();
        assert_eq!(map.len(), 1);
        assert_eq!(map["r1"].title, "T");
    }

    #[test]
    fn state_types_are_send_sync() {
        // 编译期断言：Tauri State 要求 Send + Sync
        assert_send::<DbState>();
        assert_sync::<DbState>();
        assert_send::<RecordingManager>();
        assert_sync::<RecordingManager>();
    }
}
