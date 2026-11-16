//! Tauri-bound transport glue. The local relay uses normal Tauri IPC, with its own
//! invoke key, ACL and resource table. It shares this process's existing AppState.

mod policy;
mod resources;

use serde_json::{Value, json};
use std::{
    collections::HashMap,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
};
use tauri::{Emitter, Listener, Manager};
use tokio::sync::{Notify, OwnedSemaphorePermit, broadcast, oneshot};
use tt_adapter_browser::{Config, HostRequest};

pub(crate) const RELAY_LABEL: &str = "browser-relay";

pub(crate) struct BrowserAccess {
    pub(crate) ready: AtomicBool,
    pub(crate) ready_notify: Notify,
    pub(crate) replies: Mutex<HashMap<String, PendingInvoke>>,
    pub(crate) events: broadcast::Sender<Value>,
}

pub(crate) struct PendingInvoke {
    pub(crate) reply: oneshot::Sender<Value>,
    // HTTP timeout/disconnect does not cancel native work. Hold the slot until
    // the native command settles; reconnecting cannot bypass this work limit.
    pub(crate) _permit: OwnedSemaphorePermit,
}

pub(super) fn install(app: &mut tauri::App) -> Result<(), Box<dyn std::error::Error>> {
    let config_path = app.path().app_config_dir()?.join("browser-access.json");
    let Some(config) = Config::load(&config_path)? else {
        return Ok(());
    };
    // Enter Tauri's existing runtime while constructing the Tokio listener.
    let server = tauri::async_runtime::block_on(async { tt_adapter_browser::bind(config) })?;
    let (mut requests, events, serve) = server.into_parts();
    let state = Arc::new(BrowserAccess {
        ready: AtomicBool::new(false),
        ready_notify: Notify::new(),
        replies: Mutex::new(HashMap::new()),
        events,
    });
    app.manage(state.clone());
    let dev_origin = app.config().build.dev_url.as_ref().map(|url| url.origin());
    let builder = tauri::WebviewWindowBuilder::new(
        app.handle(),
        RELAY_LABEL,
        tauri::WebviewUrl::App("browser-relay.html".into()),
    )
    .visible(false)
    .focused(false)
    .skip_taskbar(true)
    .title("TauriTavern browser transport")
    .on_navigation(move |url| {
        url.path() == "/browser-relay.html"
            && ((url.scheme() == "tauri" && url.host_str() == Some("localhost"))
                || (matches!(url.scheme(), "http" | "https")
                    && url.host_str() == Some("tauri.localhost"))
                || (cfg!(dev) && dev_origin.as_ref() == Some(&url.origin())))
    });
    let builder =
        if let Some(root) = crate::infrastructure::paths::isolated_preview_root(app.handle())? {
            builder.data_directory(root.join("cache").join("relay-webview"))
        } else {
            builder
        };
    let relay = builder.build()?;
    // Keep this local page navigation-free: it contains no user HTML or eval.
    let _ = relay;
    for name in ["app-ready", "app-error", "tauritavern-backend-error"] {
        let state = state.clone();
        app.listen(name, move |event| {
            let payload = serde_json::from_str::<Value>(event.payload()).unwrap_or(Value::Null);
            let _ = state
                .events
                .send(json!({"kind": "event", "event": name, "payload": payload}));
        });
    }
    let handle = app.handle().clone();
    tauri::async_runtime::spawn(async move {
        while let Some(request) = requests.recv().await {
            match request {
                HostRequest::Invoke {
                    mut payload,
                    reply,
                    permit,
                } => {
                    if !state.ready.load(Ordering::Acquire) {
                        let _ = tokio::time::timeout(
                            std::time::Duration::from_secs(10),
                            state.ready_notify.notified(),
                        )
                        .await;
                    }
                    if !state.ready.load(Ordering::Acquire) {
                        let _ = reply.send(
                            json!({"ok": false, "error": "Desktop browser relay is not ready"}),
                        );
                        continue;
                    }
                    if let Err(error) = policy::validate(&payload) {
                        let _ = reply.send(json!({"ok": false, "error": error}));
                        continue;
                    }
                    let id = uuid::Uuid::new_v4().to_string();
                    payload["id"] = Value::String(id.clone());
                    {
                        let mut replies = state.replies.lock().expect("browser replies");
                        replies.insert(
                            id.clone(),
                            PendingInvoke {
                                reply,
                                _permit: permit,
                            },
                        );
                    }
                    if handle
                        .emit_to(
                            tauri::EventTarget::webview_window(RELAY_LABEL),
                            "tt-browser-invoke",
                            payload,
                        )
                        .is_err()
                        && let Some(reply) =
                            state.replies.lock().expect("browser replies").remove(&id)
                    {
                        let _ = reply.reply.send(
                            json!({"ok": false, "error": "Desktop browser relay unavailable"}),
                        );
                    }
                }
                HostRequest::Resource {
                    request,
                    reply,
                    permit,
                } => {
                    let handle = handle.clone();
                    tauri::async_runtime::spawn_blocking(move || {
                        let _permit = permit;
                        let _ = reply.send(resources::serve(&handle, *request));
                    });
                }
            }
        }
    });
    tauri::async_runtime::spawn(async move {
        if let Err(error) = serve.await {
            tracing::error!("Browser access listener stopped: {error}");
        }
    });
    Ok(())
}
