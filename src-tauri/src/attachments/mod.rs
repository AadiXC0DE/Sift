//! Attachment lifecycle (P2.1–P2.4, P2.6).
//!
//! * [`service`] — identity, cache resolution, streaming, progress, saving,
//!   opening. The only path from an attachment row to bytes.
//! * [`cache`] — on-disk layout and atomic publication.
//! * [`naming`] — safe basenames and the original display name.
//! * [`quarantine`] — macOS download marking for cache files and user copies.
//! * [`in_use`] — leases that keep a file being read from being evicted
//!   (P10.4).
//!
//! `attachments_open`, `attachments_save_as`, `attachments_save_all`,
//! `attachments_cancel`, URI resolution and inline rendering all go through
//! here, so there is one locator rule and one cache state machine.

pub mod cache;
pub mod in_use;
pub mod naming;
pub mod quarantine;
pub mod service;

pub use service::{
    cancel, copy_to, ensure_local, open, owned_record, resolve_bytes, save_all_into,
    unique_destination, AttachmentRuntime, AttachmentTransport, ProviderTransport, TransportSource,
};

use crate::app_state::AppState;
use tauri::Emitter;

/// Emit attachment progress to the frontend. One event per attachment id at
/// most 10 times/second; bytes never travel over IPC.
pub fn progress_sink(app: &tauri::AppHandle) -> service::ProgressSink {
    let app = app.clone();
    std::sync::Arc::new(move |p: &crate::dto::AttachmentProgress| {
        let _ = app.emit("attachment-progress", p);
    })
}

/// Runtime bound to the app: the configured data directory plus a Tauri
/// progress emitter.
pub fn runtime_for(state: &AppState, app: &tauri::AppHandle) -> AttachmentRuntime {
    AttachmentRuntime::with_progress(state.db.clone(), state.data_dir.clone(), progress_sink(app))
}
