pub mod app_state;
pub mod commands;
pub mod db;
pub mod demo;
pub mod dto;
pub mod errors;
pub mod logging;
pub mod notify;
pub mod outbox;
pub mod provider;
pub mod render;
pub mod runtime;
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

/// Pin rustls to the `ring` crypto backend. Both `ring` (via reqwest) and
/// `aws-lc-rs` (via lettre / platform-verifier defaults) end up in the build,
/// so rustls cannot auto-select one and panics inside the IMAP/SMTP connect
/// future, which silently kills the sign-in command. Safe to call repeatedly;
/// a second call is a no-op.
pub fn install_crypto_provider() {
    let _ = rustls::crypto::ring::default_provider().install_default();
}

pub fn run() {
    install_crypto_provider();
    if let Err(e) = run_inner(true) {
        // A file-logging failure (e.g. locked-down ~/Library/Logs on managed
        // machines or sandboxed test runners) must never brick the app:
        // retry stdout-only so the mailbox still opens.
        if matches!(&e, tauri::Error::PluginInitialization(name, _) if name == "log") {
            eprintln!("file logging unavailable ({e}); continuing stdout-only");
            run_inner(false).expect("tauri run");
        } else {
            panic!("tauri run: {e}");
        }
    }
}

fn run_inner(with_file_log: bool) -> Result<(), tauri::Error> {
    let builder = tauri::Builder::default();
    let builder = if with_file_log {
        builder.plugin(logging::file_logger_plugin())
    } else {
        builder.plugin(logging::stdout_logger_plugin())
    };
    builder
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_notification::init())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .plugin(tauri_plugin_window_state::Builder::default().build())
        .plugin(tauri_plugin_single_instance::init(|_app, _args, _cwd| {}))
        .plugin(tauri_plugin_deep_link::init())
        .plugin(tauri_plugin_dialog::init())
        .setup(|app| {
            // SIFT_DATA_DIR isolates the database for local demo/test runs.
            let data_dir = std::env::var("SIFT_DATA_DIR")
                .map(std::path::PathBuf::from)
                .unwrap_or_else(|_| {
                    app.path()
                        .app_data_dir()
                        .unwrap_or_else(|_| std::path::PathBuf::from("."))
                });
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
            if let Err(e) = tauri::async_runtime::block_on(crate::demo::seed_if_enabled(&db)) {
                eprintln!("demo seed: {e}");
            }
            let state = AppState::new(db, data_dir);
            app.manage(state);
            // Background engine: per-account poll/drain/backfill + snooze watcher.
            crate::runtime::spawn_supervisor(app.app_handle().clone());
            Ok(())
        })
        .register_uri_scheme_protocol("sift-att", |ctx, req| {
            fn respond(status: u16, mime: &str, bytes: Vec<u8>) -> tauri::http::Response<Vec<u8>> {
                tauri::http::Response::builder()
                    .status(status)
                    .header("Content-Type", mime.to_string())
                    .header("Cache-Control", "private, max-age=31536000".to_string())
                    .body(bytes)
                    .unwrap()
            }
            // sift-att://<message_id>/<part_id>: the message id arrives as the
            // URI host with the part as path (be liberal: also accept /m/p).
            let uri = req.uri().clone();
            let (mid, part) = match uri.host() {
                Some(h) if !uri.path().trim_start_matches('/').is_empty() => (
                    h.to_string(),
                    uri.path().trim_start_matches('/').to_string(),
                ),
                _ => match uri.path().trim_start_matches('/').split_once('/') {
                    Some((a, b)) => (a.to_string(), b.to_string()),
                    None => (String::new(), String::new()),
                },
            };
            if mid.is_empty() || part.is_empty() || mid.contains("..") || part.contains("..") {
                return respond(404, "text/plain", b"bad id".to_vec());
            }
            let app = ctx.app_handle().clone();
            let out: Result<(Vec<u8>, String), String> =
                tauri::async_runtime::block_on(async move {
                    let state = app.state::<AppState>();
                    let (aid, _) = state
                        .db
                        .message_thread(&mid)
                        .await
                        .map_err(|e| e.to_string())?
                        .ok_or_else(|| "no message".to_string())?;
                    let provider = state.provider_for(&aid).await.map_err(|e| e.to_string())?;
                    crate::uri_scheme::resolve_attachment(&state.db, &*provider, &mid, &part)
                        .await
                        .map_err(|e| e.to_string())
                });
            match out {
                Ok((bytes, mime)) => respond(200, &mime, bytes),
                Err(_) => respond(404, "text/plain", b"not found".to_vec()),
            }
        })
        .invoke_handler(tauri::generate_handler![
            commands::settings::settings_get,
            commands::settings::settings_set,
            commands::settings::shortcuts_get,
            commands::settings::shortcuts_set,
            commands::accounts::accounts_list,
            commands::accounts::accounts_add_google,
            commands::accounts::accounts_probe_email,
            commands::accounts::accounts_add_app_password,
            commands::accounts::accounts_update_app_password,
            commands::accounts::accounts_cancel_add,
            commands::accounts::accounts_remove,
            commands::accounts::accounts_update,
            commands::system::sync_now,
            commands::system::system_info,
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
            commands::demo::demo_goto,
        ])
        .run(tauri::generate_context!())
}
