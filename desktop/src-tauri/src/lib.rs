pub mod bridge;
mod page_cancel;

use std::{
    collections::HashMap,
    path::PathBuf,
    str::FromStr,
    sync::{Arc, Mutex},
};

use boru_core::{
    desktop_runtime::{DesktopRuntime, DesktopRuntimeConfig},
    snapshot::{
        AppSnapshot, ContactDto, ContactStatusDto, NetworkHealthSnapshot, RoomSnapshot,
        SNAPSHOT_SCHEMA_VERSION,
    },
};
use tauri::{Manager, State};

/// Shared state for read-only desktop bridge commands.
pub struct AppState {
    runtime: Arc<Mutex<Option<DesktopRuntime>>>,
    page_requests: Arc<Mutex<HashMap<String, page_cancel::RequestCancel>>>,
}

#[cfg(test)]
mod test_fixture;

fn db_error(error: impl std::fmt::Display) -> bridge::AppError {
    bridge::AppError::Db(error.to_string())
}

struct PageRequestGuard {
    requests: Arc<Mutex<HashMap<String, page_cancel::RequestCancel>>>,
    id: String,
}

impl Drop for PageRequestGuard {
    fn drop(&mut self) {
        if let Ok(mut requests) = self.requests.lock() {
            requests.remove(&self.id);
        }
    }
}

/// Run `read` against the managed runtime on the blocking pool.
///
/// Kept separate from the command functions so the command bodies can be
/// exercised from unit tests, where `tauri::State` cannot be constructed.
async fn with_runtime<T, F>(state: State<'_, AppState>, read: F) -> Result<T, bridge::AppError>
where
    F: FnOnce(&DesktopRuntime) -> Result<T, bridge::AppError> + Send + 'static,
    T: Send + 'static,
{
    let runtime = state.runtime.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let guard = runtime.lock().map_err(db_error)?;
        let runtime = guard
            .as_ref()
            .ok_or_else(|| bridge::AppError::Internal("runtime unavailable".into()))?;
        read(runtime)
    })
    .await
    .map_err(db_error)?
}

/// Bootstrap snapshot projection used by the `get_bootstrap` command.
pub(crate) fn bootstrap_inner(runtime: &DesktopRuntime) -> Result<AppSnapshot, bridge::AppError> {
    Ok(runtime.get_app_snapshot())
}

/// Room list projection used by `list_conversations`.
pub(crate) fn conversations_inner(
    runtime: &DesktopRuntime,
) -> Result<Vec<RoomSnapshot>, bridge::AppError> {
    Ok(bootstrap_inner(runtime)?.rooms)
}

/// Contact list projection used by `get_contacts`.
pub(crate) fn contacts_inner(runtime: &DesktopRuntime) -> Result<Vec<ContactDto>, bridge::AppError> {
    let contacts = runtime.storage().list_contacts().map_err(db_error)?;
    Ok(contacts
        .into_iter()
        .map(|contact| ContactDto {
            id: contact
                .user_id
                .iter()
                .map(|byte| format!("{byte:02x}"))
                .collect(),
            display_name: contact
                .user_id
                .iter()
                .map(|byte| format!("{byte:02x}"))
                .collect(),
            status: ContactStatusDto::Accepted,
        })
        .collect())
}

/// Network status projection used by `get_network_status`.
pub(crate) fn network_status_inner(
    runtime: &DesktopRuntime,
) -> Result<NetworkHealthSnapshot, bridge::AppError> {
    bootstrap_inner(runtime)?
        .network_health
        .ok_or_else(|| bridge::AppError::Internal("network status unavailable".into()))
}

/// Settings projection used by `get_settings`.
pub(crate) fn settings_inner(
    runtime: &DesktopRuntime,
) -> Result<bridge::SettingsSummary, bridge::AppError> {
    let theme = runtime
        .storage()
        .kv_get_read_only("theme")
        .map_err(db_error)?;
    Ok(bridge::SettingsSummary {
        schema_version: SNAPSHOT_SCHEMA_VERSION,
        theme,
    })
}

#[tauri::command]
async fn get_bootstrap(state: State<'_, AppState>) -> Result<AppSnapshot, bridge::AppError> {
    with_runtime(state, bootstrap_inner).await
}

/// Lists room conversations. Direct-message threads are included by `get_bootstrap`.
#[tauri::command]
async fn list_conversations(
    state: State<'_, AppState>,
) -> Result<Vec<RoomSnapshot>, bridge::AppError> {
    with_runtime(state, conversations_inner).await
}

#[tauri::command]
async fn get_messages_page(
    state: State<'_, AppState>,
    conversation_id: String,
    cursor: Option<String>,
    limit: Option<u32>,
    request_id: String,
) -> Result<bridge::dto::MessageHistoryPage, bridge::AppError> {
    let params = bridge::PageParams::new(limit)?;
    let cursor = cursor
        .map(|value| bridge::decode_cursor(&value))
        .transpose()?;
    let topic = boru_core::proto::TopicId::from_str(&conversation_id).map_err(|_| {
        bridge::AppError::InvalidParam {
            field: "conversation_id".into(),
            reason: "must be a topic identifier".into(),
        }
    })?;
    let request_cancel = page_cancel::RequestCancel::new();
    state
        .page_requests
        .lock()
        .map_err(db_error)?
        .insert(request_id.clone(), request_cancel.clone());
    let page_requests = state.page_requests.clone();
    let _request_guard = PageRequestGuard {
        requests: page_requests.clone(),
        id: request_id.clone(),
    };
    let runtime = state.runtime.clone();
    let result = page_cancel::run_cancellable_blocking(request_cancel, move || {
        let guard = runtime.lock().map_err(db_error)?;
        let runtime = guard
            .as_ref()
            .ok_or_else(|| bridge::AppError::Internal("runtime unavailable".into()))?;
        if !runtime
            .get_app_snapshot()
            .rooms
            .iter()
            .any(|room| room.id == conversation_id)
        {
            return Err(bridge::AppError::NotFound {
                entity: "conversation".into(),
                id: conversation_id,
            });
        }
        let before = cursor
            .map(|cursor| {
                Ok((
                    cursor.timestamp,
                    cursor
                        .id
                        .parse::<i64>()
                        .map_err(|_| bridge::AppError::InvalidParam {
                            field: "cursor".into(),
                            reason: "invalid message id".into(),
                        })?,
                ))
            })
            .transpose()?;
        let (rows, has_more) = runtime
            .storage()
            .list_chat_messages_page(&topic, before, params.limit)
            .map_err(db_error)?;
        let mut items = Vec::with_capacity(rows.len());
        for (timestamp, id, bytes) in rows {
            let (sender, message, sent_at) =
                boru_core::chat_core::SignedMessage::verify_and_decode(&bytes).map_err(db_error)?;
            let (kind, body) = match message {
                boru_core::chat_core::Message::Message { text } => ("text".to_owned(), text),
                other => ("event".to_owned(), format!("{other:?}")),
            };
            items.push(bridge::dto::MessageHistoryItem {
                id: id.to_string(),
                conversation_id: conversation_id.clone(),
                sender_id: sender.to_string(),
                timestamp_ms: timestamp.max(0) as u64,
                kind,
                body,
            });
            let _ = sent_at;
        }
        let next_cursor = if has_more {
            items.last().map(|item| {
                bridge::dto::MessageHistoryCursor::new(bridge::encode_cursor(
                    &bridge::MessageCursor {
                        timestamp: item.timestamp_ms as i64,
                        id: item.id.clone(),
                    },
                ))
            })
        } else {
            None
        };
        Ok(bridge::dto::MessageHistoryPage {
            schema_version: SNAPSHOT_SCHEMA_VERSION,
            items,
            next_cursor,
        })
    })
    .await;
    result
}

#[tauri::command]
fn cancel_messages_page(
    state: State<'_, AppState>,
    request_id: String,
) -> Result<(), bridge::AppError> {
    let requests = state.page_requests.lock().map_err(db_error)?;
    if let Some(request) = requests.get(&request_id) {
        request.cancel();
    }
    Ok(())
}

#[tauri::command]
async fn get_contacts(state: State<'_, AppState>) -> Result<Vec<ContactDto>, bridge::AppError> {
    with_runtime(state, contacts_inner).await
}

#[tauri::command]
async fn get_network_status(
    state: State<'_, AppState>,
) -> Result<NetworkHealthSnapshot, bridge::AppError> {
    with_runtime(state, network_status_inner).await
}

#[tauri::command]
async fn get_settings(
    state: State<'_, AppState>,
) -> Result<bridge::SettingsSummary, bridge::AppError> {
    with_runtime(state, settings_inner).await
}

pub fn run() {
    let app = tauri::Builder::default()
        .manage(AppState {
            runtime: Arc::new(Mutex::new(None)),
            page_requests: Arc::new(Mutex::new(HashMap::new())),
        })
        .invoke_handler(tauri::generate_handler![
            get_bootstrap,
            list_conversations,
            get_messages_page,
            cancel_messages_page,
            get_contacts,
            get_network_status,
            get_settings
        ])
        .setup(|app| {
            // Tauri's application config directory is stable across launches;
            // do not create a new identity when a window is recreated.
            let data_dir: PathBuf = app
                .path()
                .app_data_dir()
                .map_err(|error| anyhow::anyhow!("resolve Boru profile directory: {error}"))?;
            let config = DesktopRuntimeConfig {
                data_dir,
                secret_key: None,
                relay_mode: iroh::RelayMode::Default,
                bind_port: 0,
                enable_mdns: true,
                enable_dht: true,
                publish_direct_addresses: false,
            };
            let (runtime, _handles) =
                tauri::async_runtime::block_on(DesktopRuntime::start_from_config(config))
                    .map_err(|error| anyhow::anyhow!("start Boru runtime: {error:#}"))?;
            *app.state::<AppState>()
                .runtime
                .lock()
                .map_err(|_| anyhow::anyhow!("Boru runtime state lock poisoned"))? = Some(runtime);
            Ok(())
        })
        .build(tauri::generate_context!())
        .expect("failed to initialize Boru desktop");

    // Closing the sole window exits the application (no hidden background
    // mode); this callback performs ordered async shutdown and releases the
    // profile lock only after router/endpoint teardown.
    app.run(|handle, event| {
        if let tauri::RunEvent::Exit = event {
            // Take the runtime out of managed state first: the guard must not be
            // held across the async shutdown below.
            let runtime = {
                let state = handle.state::<AppState>();
                state.runtime.lock().ok().and_then(|mut guard| guard.take())
            };
            if let Some(mut runtime) = runtime {
                let _ = tauri::async_runtime::block_on(runtime.stop());
            }
        }
    });
}
