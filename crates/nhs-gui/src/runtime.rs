//! The Tauri shell: one window, three commands, no policy.
//!
//! `state` is a read-only snapshot, `dispatch` is the only way to change anything,
//! `cancel` stops the job that is running now. Background results arrive as a
//! `nhs:state` event, so the window never polls.

use std::sync::Arc;

use nhs_adapter::Session;
use serde_json::Value;
use tauri::{Emitter, Manager, State};

use crate::bridge::{event_from_json, BridgeError, CancelAck, UiState};
use crate::service::Service;

#[tauri::command]
#[allow(clippy::needless_pass_by_value)] // Tauri passes the managed state by value.
fn state(service: State<'_, Service>) -> Result<UiState, BridgeError> {
    service.snapshot()
}

#[tauri::command]
#[allow(clippy::needless_pass_by_value)] // Tauri passes the managed state by value.
fn dispatch(service: State<'_, Service>, event: Value) -> Result<UiState, BridgeError> {
    let parsed = event_from_json(&event)?;
    service.dispatch(parsed)
}

#[tauri::command]
#[allow(clippy::needless_pass_by_value)] // Tauri passes the managed state by value.
fn cancel(service: State<'_, Service>) -> Result<CancelAck, BridgeError> {
    service.cancel()
}

/// Start the window. Returns when the event loop exits.
///
/// # Errors
/// Propagates Tauri's startup failure (no display, bad configuration, webview
/// missing) so `main` can print it and exit non-zero.
pub fn run() -> Result<(), Box<dyn std::error::Error>> {
    tauri::Builder::default()
        .setup(|app| {
            let handle = app.handle().clone();
            let service = Service::new(Session::new(None, None));
            service.set_notifier(Arc::new(move |state| {
                // A closed window is not an error worth surfacing.
                let _ = handle.emit("nhs:state", state);
            }))?;
            app.manage(service);
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![state, dispatch, cancel])
        .run(tauri::generate_context!())?;
    Ok(())
}
