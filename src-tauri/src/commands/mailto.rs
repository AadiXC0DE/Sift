//! `mailto:` deep-link commands (P9.3).
//!
//! A `mailto:` link is a request to *compose*, never to send. The parsed fields
//! are held in one bounded slot until accounts have loaded, so a link that
//! launched the app (or arrived while it was still starting) is not lost and is
//! not applied to the wrong account either. The frontend takes the slot when it
//! is ready; taking it twice returns nothing.

use crate::app_state::AppState;
use crate::dto::PendingMailto;
use crate::errors::SiftError;
use tauri::{AppHandle, Emitter, Manager, State};

/// Route one URL from a deep link.
///
/// The parser refuses anything that is not one of the five mailto fields, so an
/// unknown parameter is a rejection rather than a silent partial compose. The
/// event is emitted as well as the slot being filled: a running frontend reacts
/// immediately, and a starting one pulls the slot.
pub fn handle_url(app: &AppHandle, url: &str) {
    if !crate::mailto::is_mailto(url) {
        return;
    }
    match crate::mailto::parse(url) {
        Ok(pending) => {
            let state = app.state::<AppState>();
            state.remember_mailto(pending.clone());
            let _ = app.emit(
                "deep-link:mailto",
                serde_json::to_value(&pending).unwrap_or(serde_json::Value::Null),
            );
        }
        Err(e) => {
            // A refused link must never be silent: the user clicked something
            // and is owed the reason.
            log::warn!("refused mailto link: {e}");
            let _ = app.emit(
                "deep-link:rejected",
                serde_json::json!({ "code": e.code(), "message": e.to_string() }),
            );
        }
    }
}

/// The pending compose request, if one is waiting.
#[tauri::command]
pub async fn mailto_pending(
    state: State<'_, AppState>,
) -> Result<Option<PendingMailto>, SiftError> {
    Ok(state.pending_mailto())
}

/// Consume the pending compose request. The second caller gets `None`, so a
/// remount cannot open the same draft twice.
#[tauri::command]
pub async fn mailto_take(state: State<'_, AppState>) -> Result<Option<PendingMailto>, SiftError> {
    Ok(state.take_mailto())
}

/// Parse a `mailto:` URL without routing it. Used by tests and by the compose
/// flow when the frontend receives the link itself.
#[tauri::command]
pub async fn mailto_parse(url: String) -> Result<PendingMailto, SiftError> {
    crate::mailto::parse(&url)
}
