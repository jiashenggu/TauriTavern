use std::sync::{Arc, atomic::Ordering};
use serde_json::Value;
use tauri::{State, Webview};
use crate::app::host::browser_access::{BrowserAccess, RELAY_LABEL};
use crate::presentation::errors::CommandError;

fn authorize(webview: &Webview) -> Result<(), CommandError> {
    if webview.label() != RELAY_LABEL {
        return Err(CommandError::BadRequest("Browser transport requires the local relay".into()));
    }
    Ok(())
}

#[tauri::command]
pub fn browser_access_ready(
    webview: Webview,
    state: State<'_, Arc<BrowserAccess>>,
) -> Result<(), CommandError> {
    authorize(&webview)?;
    state.ready.store(true, Ordering::Release);
    state.ready_notify.notify_one();
    Ok(())
}

#[tauri::command]
pub fn browser_access_reply(
    webview: Webview,
    state: State<'_, Arc<BrowserAccess>>,
    id: String,
    value: Value,
) -> Result<(), CommandError> {
    authorize(&webview)?;
    if let Some(reply) = state.replies.lock().expect("browser replies").remove(&id) {
        let _ = reply.reply.send(value);
    }
    Ok(())
}

#[tauri::command]
pub fn browser_access_channel(
    webview: Webview,
    state: State<'_, Arc<BrowserAccess>>,
    id: String,
    value: Value,
) -> Result<(), CommandError> {
    authorize(&webview)?;
    let _ = state.events.send(serde_json::json!({"kind": "channel", "id": id, "value": value}));
    Ok(())
}
