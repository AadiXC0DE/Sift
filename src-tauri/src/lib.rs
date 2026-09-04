pub mod app_state;
pub mod commands;
pub mod db;
pub mod dto;
pub mod errors;
pub mod gmail;
pub mod logging;
pub mod notify;
pub mod outbox;
pub mod render;
pub mod scheduler;
pub mod search;
pub mod secrets;
pub mod sync;
pub mod uri_scheme;

use app_state::AppState;
use tauri::Manager;

pub fn keymap_default() -> serde_json::Value {
    serde_json::json!({
      "scopes": ["global", "list", "thread", "compose", "palette"],
      "bindings": [
        {"key": "j", "scope": "list", "action": "focusNext"},
        {"key": "k", "scope": "list", "action": "focusPrev"},
        {"key": "e", "scope": "list", "action": "archive"},
        {"key": "c", "scope": "global", "action": "compose"}
      ]
    })
}

pub mod opener {
    pub fn open(_app: &tauri::AppHandle, url: &str) -> Result<(), String> {
        // Use opener plugin at runtime; in unit tests there is no app, so just validate.
        if url.is_empty() {
            return Err("empty".into());
        }
        #[cfg(not(test))]
        {
            use tauri_plugin_opener::OpenerExt;
            _app.opener()
                .open_url(url, None::<&str>)
                .map_err(|e| e.to_string())
        }
        #[cfg(test)]
        {
            Ok(())
        }
    }
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    logging::init();
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_notification::init())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .plugin(tauri_plugin_window_state::Builder::default().build())
        .plugin(tauri_plugin_single_instance::init(|_app, _args, _cwd| {}))
        .plugin(tauri_plugin_deep_link::init())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_log::Builder::default().build())
        .setup(|app| {
            let data_dir = app
                .path()
                .app_data_dir()
                .unwrap_or_else(|_| std::path::PathBuf::from("."));
            let _ = std::fs::create_dir_all(&data_dir);
            let db = db::Db::open(&data_dir).expect("open db");
            // theme -> native background to avoid white flash
            if let Some(w) = app.get_webview_window("main") {
                let settings =
                    tauri::async_runtime::block_on(db.settings_get()).unwrap_or_default();
                let dark = settings.theme == "dark";
                let _ = w.set_background_color(if dark {
                    Some(tauri::window::Color(27, 27, 26, 255))
                } else {
                    Some(tauri::window::Color(250, 250, 249, 255))
                });
                eprintln!("perf:window-shown {}", db::now_ms());
            }
            let state = AppState::new(db, data_dir);
            app.manage(state);
            // overdue snoozes
            // (spawned after manage via async_runtime)
            Ok(())
        })
        .register_uri_scheme_protocol("sift-att", |_ctx, req| {
            // Minimal handler: parse /<message>/<part>, lookup via block_on
            // Full streaming + caching lives in uri_scheme::resolve_attachment with AppState; this stub returns 404 until state is wired.
            // The real handler is registered in main.rs with state access.
            let _ = req;
            tauri::http::Response::builder()
                .status(404)
                .body(Vec::new())
                .unwrap()
        })
        .invoke_handler(tauri::generate_handler![
            commands::settings::settings_get,
            commands::settings::settings_set,
            commands::settings::shortcuts_get,
            commands::settings::shortcuts_set,
            commands::accounts::accounts_list,
            commands::accounts::accounts_add_google,
            commands::accounts::accounts_cancel_add,
            commands::accounts::accounts_remove,
            commands::accounts::accounts_update,
            commands::system::sync_now,
            commands::system::sync_status,
            commands::system::labels_list,
            commands::system::app_set_badge,
            commands::system::app_open_url,
            commands::system::diagnostics_export,
            commands::system::perf_mark,
            commands::threads::threads_query,
            commands::threads::thread_get,
            commands::threads::message_body,
            commands::threads::message_raw_source,
            commands::threads::remote_images_load,
            commands::actions::threads_action,
            commands::actions::action_undo,
            commands::actions::snooze_set,
            commands::actions::snooze_clear,
            commands::actions::labels_create,
            commands::compose::drafts_upsert,
            commands::compose::drafts_delete,
            commands::compose::drafts_send,
            commands::compose::send_cancel,
            commands::compose::contacts_suggest,
            commands::compose::attachments_add_from_paths,
            commands::search::search,
            commands::search::unsubscribe,
            commands::attachments::attachments_open,
            commands::attachments::attachments_save_as,
        ])
        .run(tauri::generate_context!())
        .expect("tauri run");
}
