// StepSnap-style annotation
mod db;
mod recorder;
mod hotkey;
mod settings;
mod ai;
mod editor;
mod export;
mod file_browser;

use std::collections::HashMap;
use std::sync::Mutex;
use rusqlite::Connection;
use recorder::Recorder;
use tauri::Manager;
use tauri_plugin_store;

pub struct DbState(pub Mutex<Connection>);

pub struct RecordingManager(pub Mutex<HashMap<String, Recorder>>);

pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_store::Builder::default().build())
        .setup(|app| {
            let app_data_dir = app.path().app_data_dir().expect("failed to get app data dir");
            std::fs::create_dir_all(&app_data_dir).ok();
            let db_path = app_data_dir.join("flowio.db");
            let conn = Connection::open(db_path).expect("Failed to open database");
            db::init_db(&conn);
            app.manage(DbState(Mutex::new(conn)));
            app.manage(RecordingManager(Mutex::new(HashMap::new())));

            // Initialize global hotkeys
            hotkey::init_hotkeys(app.handle());

            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            db::commands::list_recordings,
            db::commands::delete_recording,
            db::commands::save_recording,
            db::commands::update_recording_app_name,
            recorder::commands::start_recording,
            recorder::commands::pause_recording,
            recorder::commands::resume_recording,
            recorder::commands::finish_recording,
            recorder::commands::cancel_recording,
            recorder::commands::mark_screenshot_step,
            recorder::commands::get_active_recording,
            settings::commands::get_hotkey_config,
            settings::commands::set_hotkey_config,
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
            editor::commands::delete_step,
            editor::commands::reorder_steps,
            editor::commands::add_step,
            editor::commands::update_recording_title,
            editor::commands::crop_step_screenshot,
            editor::commands::upload_step_screenshot,
            editor::commands::delete_step_screenshot,
            export::commands::export_recording,
            file_browser::list_directory,
            file_browser::get_drives,
            file_browser::get_known_folders,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
