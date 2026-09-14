pub mod actions;
pub mod app_state;
pub mod attachments;
pub mod commands;
pub mod connectivity;
pub mod db;
pub mod demo;
pub mod dto;
pub mod errors;
pub mod labels;
pub mod logging;
pub mod mailto;
pub mod notify;
pub mod outbox;
pub mod outgoing;
pub mod provider;
pub mod reminders;
pub mod render;
pub mod retention;
pub mod rules;
pub mod runtime;
pub mod scheduler;
pub mod search;
pub mod secrets;
pub mod send_later;
pub mod snooze;
pub mod sync;
pub mod unsubscribe;
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

    /// Open a local file with the system's default application. Distinct from
    /// [`open`], which takes a URL; handing `open_url` a filesystem path makes
    /// the plugin reject it, so attachments silently did nothing.
    pub fn open_path(_app: &tauri::AppHandle, path: &str) -> Result<(), String> {
        if path.is_empty() {
            return Err("empty".into());
        }
        #[cfg(not(test))]
        {
            use tauri_plugin_opener::OpenerExt;
            _app.opener()
                .open_path(path, None::<&str>)
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
                // Respect the OS appearance when the setting is "system",
                // otherwise a dark-mode Mac still gets a light flash.
                let dark = match settings.theme.as_str() {
                    "dark" => true,
                    "light" => false,
                    _ => matches!(w.theme(), Ok(tauri::Theme::Dark)),
                };
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
            // Account-level events (auth:expired, connectivity:state) have no
            // runtime host; they need the window handle (P4.6).
            state.set_emitter(app.app_handle().clone());
            app.manage(state);
            // Background engine: per-account poll/drain/backfill + the one
            // scheduler that owns snooze, reminder and send deadlines (P8.2).
            crate::runtime::spawn_supervisor(app.app_handle().clone());
            // Bounded retention for finished work (P6.6). It never touches
            // pending, failed, uncertain or scheduled operations.
            let prune_app = app.app_handle().clone();
            tauri::async_runtime::spawn(async move {
                crate::runtime::prune_finished_work(&prune_app).await;
                crate::retention::run_retention_loop(prune_app).await;
            });
            // P8.4: the badge is computed once at startup from the same query
            // the sidebar uses, so the first number the OS shows is the real
            // one even before any event arrives.
            let badge_app = app.app_handle().clone();
            tauri::async_runtime::spawn(async move {
                crate::runtime::refresh_badge_for_app(&badge_app).await;
            });
            // P9.3: deep links. A `mailto:` that launched the app (or arrived
            // while it was starting) is parsed and held until the composer can
            // take it; nothing is ever sent from a link.
            {
                use tauri_plugin_deep_link::DeepLinkExt;
                let link_app = app.app_handle().clone();
                app.deep_link().on_open_url(move |event| {
                    for url in event.urls() {
                        crate::commands::mailto::handle_url(&link_app, url.as_str());
                    }
                });
                if let Ok(Some(urls)) = app.deep_link().get_current() {
                    for url in urls {
                        crate::commands::mailto::handle_url(app.app_handle(), url.as_str());
                    }
                }
            }
            // Bringing the app forward from a notification opens what it was
            // about (P8.4). The OS reports focus, not the clicked banner.
            if let Some(window) = app.get_webview_window("main") {
                let focus_app = app.app_handle().clone();
                window.on_window_event(move |event| {
                    if let tauri::WindowEvent::Focused(true) = event {
                        crate::runtime::flush_pending_nav(&focus_app);
                    }
                });
            }
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
            // sift-att://<account-id>/<message-id>/<key>: account id arrives as
            // the URI host, message id and key as the path. Ownership is
            // validated by the account-qualified DB lookup (P4.2).
            let uri = req.uri().clone();
            let (aid, mid, part) = {
                let host = uri.host().unwrap_or_default().to_string();
                let path: Vec<&str> = uri.path().trim_start_matches('/').splitn(2, '/').collect();
                match (host.is_empty(), path.as_slice()) {
                    (false, [mid, part]) => (host, mid.to_string(), part.to_string()),
                    _ => (String::new(), String::new(), String::new()),
                }
            };
            if aid.is_empty()
                || mid.is_empty()
                || part.is_empty()
                || aid.contains("..")
                || mid.contains("..")
                || part.contains("..")
            {
                return respond(404, "text/plain", b"bad id".to_vec());
            }
            let app = ctx.app_handle().clone();
            let out: Result<(Vec<u8>, String), String> =
                tauri::async_runtime::block_on(async move {
                    let state = app.state::<AppState>();
                    let message = crate::dto::MessageRef::new(aid, mid);
                    // Ownership: the message must exist in this account.
                    let exists = state
                        .db
                        .message_thread(&message)
                        .await
                        .map_err(|e| e.to_string())?
                        .ok_or_else(|| "no message".to_string())?;
                    let _ = exists;
                    let provider = state
                        .provider_for(&message.account_id)
                        .await
                        .map_err(|e| e.to_string())?;
                    crate::uri_scheme::resolve_attachment(&state.db, &*provider, &message, &part)
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
            commands::storage::storage_usage,
            commands::storage::storage_clear_attachment_cache,
            commands::accounts::accounts_list,
            commands::accounts::accounts_add_google,
            commands::accounts::accounts_probe_email,
            commands::accounts::accounts_add_app_password,
            commands::accounts::accounts_update_app_password,
            commands::accounts::accounts_cancel_add,
            commands::accounts::accounts_remove,
            commands::accounts::accounts_removal_preview,
            commands::accounts::accounts_update,
            commands::system::sync_now,
            commands::system::system_info,
            commands::system::sync_status,
            commands::system::connectivity_state,
            commands::system::app_network_hint,
            commands::system::labels_list,
            commands::system::app_set_badge,
            commands::system::app_open_url,
            commands::system::app_relaunch,
            commands::system::diagnostics_export,
            commands::system::perf_mark,
            commands::threads::threads_query,
            commands::threads::thread_get,
            commands::threads::message_body,
            commands::threads::message_raw_source,
            commands::threads::remote_content_allow,
            commands::threads::remote_content_policy_get,
            commands::threads::remote_content_policy_set,
            commands::threads::remote_content_sender_revoke,
            commands::reminders::reminder_set,
            commands::reminders::reminder_clear,
            commands::reminders::reminder_complete,
            commands::reminders::reminders_list,
            commands::reminders::reminders_open_count,
            commands::rules::rules_list,
            commands::rules::rules_upsert,
            commands::rules::rules_delete,
            commands::rules::rules_preview,
            commands::rules::rules_apply_existing,
            commands::rules::rule_block_sender,
            commands::rules::rules_diagnostics,
            commands::notifications::notifications_state,
            commands::notifications::notifications_enable,
            commands::notifications::notifications_update,
            commands::notifications::vip_list,
            commands::notifications::vip_set,
            commands::notifications::vip_candidates,
            commands::notifications::notification_pending_open,
            commands::mailto::mailto_pending,
            commands::mailto::mailto_take,
            commands::mailto::mailto_parse,
            commands::raw_source::message_view_source,
            commands::raw_source::message_raw_export,
            commands::actions::labels_with_hierarchy,
            commands::actions::label_rename,
            commands::actions::label_delete,
            commands::actions::trash_empty_preview,
            commands::actions::trash_empty,
            commands::storage::storage_trim_attachment_cache,
            commands::storage::storage_pin_attachment,
            commands::compose::send_later_options,
            commands::compose::send_reschedule,
            commands::compose::send_now,
            commands::outbox::threads_action,
            commands::outbox::action_undo,
            commands::outbox::snooze_set,
            commands::outbox::snooze_clear,
            commands::outbox::outbox_list,
            commands::outbox::outbox_get,
            commands::outbox::outbox_retry,
            commands::actions::labels_create,
            commands::compose::drafts_get,
            commands::compose::drafts_list,
            commands::compose::drafts_upsert,
            commands::compose::drafts_delete,
            commands::compose::drafts_send,
            commands::compose::send_cancel,
            commands::compose::compose_limits,
            commands::compose::contacts_suggest,
            commands::compose::attachments_add_from_paths,
            commands::compose::attachments_stage_from_message,
            commands::search::search,
            commands::search::unsubscribe,
            commands::search::saved_search_upsert,
            commands::search::saved_search_delete,
            commands::search::saved_search_list,
            commands::search::saved_search_count,
            commands::search::saved_search_open,
            commands::attachments::attachments_open,
            commands::attachments::attachments_save_as,
            commands::attachments::attachments_save_all,
            commands::attachments::attachments_cancel,
            commands::demo::demo_goto,
        ])
        .run(tauri::generate_context!())
}
