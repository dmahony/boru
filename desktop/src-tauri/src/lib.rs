pub mod bridge;
pub mod settings;
pub mod rooms;
mod list_rooms;
pub mod commands;
#[cfg(feature = "diagnostics-test")]
mod diagnostics_test;
mod page_cancel;
pub mod persistence;

use std::{
    collections::HashMap,
    path::PathBuf,
    str::FromStr,
    sync::{Arc, Mutex},
};
use boru_core::ui_events::{EventCursor, ReplayResult, SequencedUiEvent};
use serde::Serialize;
use tauri::{Emitter, Manager, State};

const UI_EVENT: &str = "boru://ui-event";

#[derive(Clone, Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct UiEventEnvelope {
    epoch: String,
    revision: u64,
    event_id: String,
}

#[derive(Serialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
enum ReplayDto {
    Events { events: Vec<UiEventEnvelope> },
    Resnapshot { current_revision: u64 },
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct CursorDto {
    epoch: String,
    revision: u64,
}

fn envelope(event: &SequencedUiEvent) -> UiEventEnvelope {
    let (epoch, _) = event.event_id.rsplit_once(':').unwrap_or_default();
    UiEventEnvelope {
        epoch: epoch.to_owned(),
        revision: event.revision,
        event_id: event.event_id.clone(),
    }
}

use boru_core::{
    desktop_runtime::{DesktopRuntime, DesktopRuntimeConfig},
    snapshot::{
        AppSnapshot, ContactDto, ContactStatusDto, NetworkHealthSnapshot, RoomSnapshot,
        SNAPSHOT_SCHEMA_VERSION,
    },
};


#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct SnapshotResponse {
    snapshot: AppSnapshot,
    cursor: UiEventEnvelope,
}

#[tauri::command]
async fn get_app_snapshot(state: State<'_, AppState>) -> Result<SnapshotResponse, String> {
    let runtime = state.runtime.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let guard = runtime.lock().map_err(|_| "runtime state lock poisoned".to_owned())?;
        let runtime = guard.as_ref().ok_or_else(|| "runtime unavailable".to_owned())?;
        let (snapshot, cursor) = runtime.get_app_snapshot_with_revision();
        Ok(SnapshotResponse { snapshot, cursor: UiEventEnvelope { epoch: cursor.epoch, revision: cursor.revision, event_id: String::new() } })
    }).await.map_err(|e| e.to_string())?
}

#[tauri::command]
async fn replay_ui_events(state: State<'_, AppState>, cursor: CursorDto) -> Result<ReplayDto, String> {
    let runtime = state.runtime.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let guard = runtime.lock().map_err(|_| "runtime state lock poisoned".to_owned())?;
        let runtime = guard.as_ref().ok_or_else(|| "runtime unavailable".to_owned())?;
        let cursor = EventCursor { epoch: cursor.epoch, revision: cursor.revision };
        Ok(match runtime.replay_ui_events_from(&cursor) {
            ReplayResult::Events(events) => ReplayDto::Events { events: events.iter().map(envelope).collect() },
            ReplayResult::Resnapshot { current_revision } => ReplayDto::Resnapshot { current_revision },
        })
    }).await.map_err(|e| e.to_string())?
}

/// Shared state for read-only desktop bridge commands.
pub struct AppState {
    runtime: Arc<Mutex<Option<DesktopRuntime>>>,
    page_requests: Arc<Mutex<HashMap<String, page_cancel::RequestCancel>>>,
    sent_command_ids: Arc<Mutex<std::collections::HashSet<String>>>,
}

#[cfg(test)]
mod test_fixture;
#[cfg(test)]
mod tests;

#[cfg_attr(not(test), allow(dead_code))]
fn command_handler<R: tauri::Runtime>() -> impl Fn(tauri::ipc::Invoke<R>) -> bool {
    tauri::generate_handler![
        get_bootstrap,
        list_conversations,
        get_messages_page,
        cancel_messages_page,
        get_contacts,
        get_network_status,
        get_settings,
        update_settings,
        get_app_snapshot,
        replay_ui_events,
        list_rooms,
        send_message,
        mark_read,
        commands::open_room::open_room
    ]
}

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

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct SendMessageRequest { idempotency_key: String, conversation_id: Vec<u8>, text: String, thread_target: Option<boru_core::threads::ThreadTarget> }
#[derive(serde::Deserialize)]
struct SendMessageArgs { request: SendMessageRequest }
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct SendMessageResponse { event_id: u64, message_hash: String }
#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct MarkReadRequest { conversation_id: Vec<u8> }

#[tauri::command]
async fn send_message(state: State<'_, AppState>, args: SendMessageArgs) -> Result<SendMessageResponse, bridge::AppError> {
    let request = args.request;
    if request.idempotency_key.trim().is_empty() { return Err(bridge::AppError::InvalidParam { field: "idempotency_key".into(), reason: "must not be empty".into() }); }
    if request.text.trim().is_empty() { return Err(bridge::AppError::InvalidParam { field: "text".into(), reason: "must not be empty".into() }); }
    let conversation_id: [u8; 32] = request.conversation_id.try_into().map_err(|_| bridge::AppError::InvalidParam { field: "conversation_id".into(), reason: "must contain exactly 32 bytes".into() })?;
    let key = request.idempotency_key;
    // Reserve the key before any durable or transport work so a replayed key can
    // never reach the network. The reservation is released on failure below.
    {
        let mut sent = state.sent_command_ids.lock().map_err(db_error)?;
        if !sent.insert(key.clone()) { return Err(bridge::AppError::Conflict); }
    }
    let release_key = || {
        if let Ok(mut sent) = state.sent_command_ids.lock() { sent.remove(&key); }
    };
    let runtime = state.runtime.clone();
    let admit_key = key.clone();
    let admitted = tauri::async_runtime::spawn_blocking(move || {
        let guard = runtime.lock().map_err(db_error)?;
        let runtime = guard.as_ref().ok_or_else(|| bridge::AppError::Internal("runtime unavailable".into()))?;
        let prepared = match runtime.application_commands().send_message(boru_core::application_commands::SendMessage { idempotency_key: admit_key, conversation_id, text: request.text, thread_target: request.thread_target }) {
            boru_core::application_commands::CommandOutcome::Applied(value) => value,
            boru_core::application_commands::CommandOutcome::Rejected(error) => return Err(match error.code {
                boru_core::application_commands::CommandErrorCode::InvalidInput => bridge::AppError::InvalidParam { field: "request".into(), reason: error.message.into() },
                boru_core::application_commands::CommandErrorCode::Conflict => bridge::AppError::Conflict,
                boru_core::application_commands::CommandErrorCode::Unavailable => bridge::AppError::Internal(error.message.into()),
            }),
        };
        Ok((prepared, runtime.room_sender_session()))
    }).await;
    let (prepared, session) = match admitted {
        Ok(Ok(value)) => value,
        Ok(Err(error)) => { release_key(); return Err(error); }
        Err(error) => { release_key(); return Err(db_error(error)); }
    };
    let hash = prepared.message_hash;
    if session.broadcast(conversation_id, prepared.encoded.to_vec()).await.is_err() {
        release_key();
        return Err(bridge::AppError::Internal("room broadcast failed".into()));
    }
    Ok(SendMessageResponse { event_id: prepared.event_id, message_hash: hash.iter().map(|byte| format!("{byte:02x}")).collect() })
}

#[tauri::command]
async fn mark_read(state: State<'_, AppState>, request: MarkReadRequest) -> Result<u32, bridge::AppError> {
    let id: [u8; 32] = request.conversation_id.try_into().map_err(|_| bridge::AppError::InvalidParam { field: "conversation_id".into(), reason: "must contain exactly 32 bytes".into() })?;
    with_runtime(state, move |runtime| {
        runtime.mark_read(&id).map(|previous| previous.unwrap_or_default()).map_err(db_error)
    }).await
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
pub(crate) fn contacts_inner(
    runtime: &DesktopRuntime,
) -> Result<Vec<ContactDto>, bridge::AppError> {
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
    let raw_preferences = runtime
        .storage()
        .kv_get_read_only(settings::SETTINGS_KEY)
        .map_err(db_error)?;
    let preferences = settings::DesktopPreferences::decode(raw_preferences.as_deref())
        .map_err(bridge::AppError::Internal)?;
    Ok(bridge::SettingsSummary {
        schema_version: SNAPSHOT_SCHEMA_VERSION,
        theme,
        preferences,
    })
}

#[tauri::command]
async fn get_bootstrap(state: State<'_, AppState>) -> Result<AppSnapshot, bridge::AppError> {
    with_runtime(state, bootstrap_inner).await
}

/// Lists room summaries from the current app snapshot projection.
#[tauri::command]
async fn list_rooms(state: State<'_, AppState>) -> Result<Vec<bridge::dto::RoomSummary>, bridge::AppError> {
    with_runtime(state, |runtime| {
        list_rooms::project(runtime.get_app_snapshot(), &runtime.message_store())
    })
    .await
}

/// Lists room conversations. Direct-message threads are included by `get_bootstrap`.
#[tauri::command]
async fn list_conversations(
    state: State<'_, AppState>,
) -> Result<Vec<RoomSnapshot>, bridge::AppError> {
    with_runtime(state, conversations_inner).await
}

/// Load one newest-first history page for a conversation.
///
/// Shared by `get_messages_page` and `open_room` so that cursor handling,
/// pagination and row mapping exist exactly once.
fn load_messages_page(
    runtime: &DesktopRuntime,
    conversation_id: &str,
    cursor: Option<String>,
    limit: Option<u32>,
    _request_id: Option<String>,
) -> Result<bridge::dto::MessageHistoryPage, bridge::AppError> {
    let params = bridge::PageParams::new(limit)?;
    let cursor = cursor
        .map(|value| bridge::decode_cursor(&value))
        .transpose()?;
    let topic = boru_core::proto::TopicId::from_str(conversation_id).map_err(|_| {
        bridge::AppError::InvalidParam {
            field: "conversation_id".into(),
            reason: "must be a topic identifier".into(),
        }
    })?;
    if !runtime
        .get_app_snapshot()
        .rooms
        .iter()
        .any(|room| room.id == conversation_id)
    {
        return Err(bridge::AppError::NotFound {
            entity: "conversation".into(),
            id: conversation_id.to_owned(),
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
    // Read from MessageStore, not the legacy Storage chat_messages table: the
    // send and receive paths persist into MessageStore, so reading Storage here
    // hid every message the app itself produced.
    let mut rows = runtime
        .message_store()
        .get_messages_for_topic_before(topic.as_bytes(), before, params.limit as usize + 1)
        .map_err(db_error)?;
    let has_more = rows.len() > params.limit as usize;
    rows.truncate(params.limit as usize);
    let mut items = Vec::with_capacity(rows.len());
    for row in rows {
        let timestamp = row.timestamp_ms;
        let id = row.id;
        let delivery_state = row.delivery_state;
        let Some(bytes) = row.signed_bytes else {
            return Err(bridge::AppError::Internal(
                "stored message has no signed envelope".into(),
            ));
        };
        let (sender, message, _sent_at) =
            boru_core::chat_core::SignedMessage::verify_and_decode(&bytes).map_err(db_error)?;
        let (kind, body) = match message {
            boru_core::chat_core::Message::Message { text } => ("text".to_owned(), text),
            other => ("event".to_owned(), format!("{other:?}")),
        };
        items.push(bridge::dto::MessageHistoryItem {
            id: id.to_string(),
            conversation_id: conversation_id.to_owned(),
            sender_id: sender.to_string(),
            timestamp_ms: timestamp.max(0) as u64,
            kind,
            body,
            delivery_status: match delivery_state.to_ascii_lowercase().as_str() {
                "queued" => Some(bridge::dto::DeliveryStatusDto::Queued),
                "sent" => Some(bridge::dto::DeliveryStatusDto::Sent),
                "delivered" => Some(bridge::dto::DeliveryStatusDto::Delivered),
                "seen" => Some(bridge::dto::DeliveryStatusDto::Seen),
                "failed" => Some(bridge::dto::DeliveryStatusDto::Failed),
                _ => None,
            },
        });
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
}

#[tauri::command]
async fn get_messages_page(
    state: State<'_, AppState>,
    conversation_id: String,
    cursor: Option<String>,
    limit: Option<u32>,
    request_id: String,
) -> Result<bridge::dto::MessageHistoryPage, bridge::AppError> {
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
        load_messages_page(runtime, &conversation_id, cursor, limit, Some(request_id))
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

#[tauri::command]
async fn update_settings(
    state: State<'_, AppState>,
    preferences: settings::DesktopPreferences,
) -> Result<bridge::SettingsSummary, bridge::AppError> {
    preferences.validate().map_err(|reason| bridge::AppError::InvalidParam {
        field: "preferences".into(),
        reason,
    })?;
    with_runtime(state, move |runtime| {
        let value = preferences.encode().map_err(bridge::AppError::Internal)?;
        runtime.storage().kv_set(settings::SETTINGS_KEY, &value).map_err(db_error)?;
        settings_inner(runtime)
    }).await
}

use std::time::Duration;
use boru_core::media_handles::{MediaHandleError, MediaHandleRegistry, MAX_CHUNK_BYTES};

struct MediaState(Arc<MediaHandleRegistry>);

#[derive(Debug, Serialize)]
#[serde(rename_all = "snake_case")]
enum MediaIpcError { InvalidHandle, Expired, Unauthorized, WrongType, InvalidContentType, TooLarge, InvalidChunk, Cancelled, Io }
impl From<MediaHandleError> for MediaIpcError {
    fn from(error: MediaHandleError) -> Self {
        match error { MediaHandleError::InvalidHandle => Self::InvalidHandle, MediaHandleError::Expired => Self::Expired, MediaHandleError::Unauthorized => Self::Unauthorized, MediaHandleError::WrongType => Self::WrongType, MediaHandleError::InvalidContentType => Self::InvalidContentType, MediaHandleError::TooLarge => Self::TooLarge, MediaHandleError::InvalidChunk => Self::InvalidChunk, MediaHandleError::Cancelled => Self::Cancelled, MediaHandleError::Io => Self::Io }
    }
}
#[derive(Serialize)]
struct MediaHandleDto { handle: String, max_chunk_bytes: usize }
// Authorization identity is derived from Tauri, never from caller-supplied IPC data.
fn owner(window: &tauri::WebviewWindow) -> String { window.label().to_owned() }

fn request_header<'a>(request: &'a tauri::ipc::Request<'_>, name: &str) -> Result<&'a str, MediaIpcError> {
    request.headers().get(name).and_then(|value| value.to_str().ok()).ok_or(MediaIpcError::InvalidChunk)
}

#[tauri::command]
async fn media_begin_upload(window: tauri::WebviewWindow, state: tauri::State<'_, MediaState>, content_type: String) -> Result<MediaHandleDto, MediaIpcError> {
    let registry = state.0.clone(); let owner = owner(&window);
    let handle = tauri::async_runtime::spawn_blocking(move || registry.begin_upload(&owner, &content_type)).await.map_err(|_| MediaIpcError::Io)??;
    Ok(MediaHandleDto { handle, max_chunk_bytes: MAX_CHUNK_BYTES })
}
#[tauri::command]
async fn media_write_chunk(window: tauri::WebviewWindow, state: tauri::State<'_, MediaState>, request: tauri::ipc::Request<'_>) -> Result<u64, MediaIpcError> {
    let handle = request_header(&request, "handle")?.to_owned();
    let offset = request_header(&request, "offset")?.parse().map_err(|_| MediaIpcError::InvalidChunk)?;
    let bytes = match request.body() { tauri::ipc::InvokeBody::Raw(bytes) => bytes.clone(), tauri::ipc::InvokeBody::Json(_) => return Err(MediaIpcError::InvalidChunk) };
    if bytes.len() > MAX_CHUNK_BYTES { return Err(MediaIpcError::InvalidChunk); }
    let registry = state.0.clone(); let owner = owner(&window);
    tauri::async_runtime::spawn_blocking(move || registry.write_chunk(&owner, &handle, offset, &bytes)).await.map_err(|_| MediaIpcError::Io)?.map_err(Into::into)
}
#[tauri::command]
async fn media_finish_upload(window: tauri::WebviewWindow, state: tauri::State<'_, MediaState>, handle: String) -> Result<u64, MediaIpcError> {
    let registry = state.0.clone(); let owner = owner(&window);
    tauri::async_runtime::spawn_blocking(move || registry.finish_upload(&owner, &handle)).await.map_err(|_| MediaIpcError::Io)?.map_err(Into::into)
}
#[tauri::command]
async fn media_read_chunk(window: tauri::WebviewWindow, state: tauri::State<'_, MediaState>, request: tauri::ipc::Request<'_>) -> Result<tauri::ipc::Response, MediaIpcError> {
    let handle = request_header(&request, "handle")?.to_owned();
    let offset = request_header(&request, "offset")?.parse().map_err(|_| MediaIpcError::InvalidChunk)?;
    let limit: usize = request_header(&request, "limit")?.parse().map_err(|_| MediaIpcError::InvalidChunk)?;
    if limit == 0 || limit > MAX_CHUNK_BYTES { return Err(MediaIpcError::InvalidChunk); }
    let registry = state.0.clone(); let owner = owner(&window);
    let bytes = tauri::async_runtime::spawn_blocking(move || registry.read_chunk(&owner, &handle, offset, limit)).await.map_err(|_| MediaIpcError::Io)?.map_err(MediaIpcError::from)?;
    Ok(tauri::ipc::Response::new(bytes))
}
#[tauri::command]
async fn media_cancel(window: tauri::WebviewWindow, state: tauri::State<'_, MediaState>, handle: String) -> Result<(), MediaIpcError> {
    let registry = state.0.clone(); let owner = owner(&window);
    tauri::async_runtime::spawn_blocking(move || registry.cancel(&owner, &handle)).await.map_err(|_| MediaIpcError::Io)?.map_err(Into::into)
}

pub fn run() {
    let media_registry = Arc::new(MediaHandleRegistry::new(Duration::from_secs(15 * 60), boru_core::media_handles::MAX_MEDIA_BYTES).expect("create private media directory"));
    let app = tauri::Builder::default()
        .manage(AppState {
            runtime: Arc::new(Mutex::new(None)),
            page_requests: Arc::new(Mutex::new(HashMap::new())),
            sent_command_ids: Arc::new(Mutex::new(std::collections::HashSet::new())),
        })
        .manage(MediaState(media_registry))
        .invoke_handler(tauri::generate_handler![get_bootstrap, list_conversations, get_messages_page, cancel_messages_page, get_contacts, get_network_status, get_settings, get_app_snapshot, replay_ui_events, list_rooms, send_message, mark_read, commands::open_room::open_room, media_begin_upload, media_write_chunk, media_finish_upload, media_read_chunk, media_cancel,
            #[cfg(feature = "diagnostics-test")]
            diagnostics_test::test_diagnostics_ping
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
            let mut receiver = runtime.subscribe_sequenced_ui_events();
            *app.state::<AppState>().runtime
                .lock()
                .map_err(|_| anyhow::anyhow!("Boru runtime state lock poisoned"))? = Some(runtime);
            let handle = app.handle().clone();
            tauri::async_runtime::spawn(async move {
                loop {
                    match receiver.recv().await {
                        Ok(event) => {
                            let _ = handle.emit(UI_EVENT, envelope(&event));
                        }
                        Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {
                            let _ = handle.emit("boru://resnapshot-required", ());
                        }
                        Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                    }
                }
            });

            Ok(())
        })
        .build(tauri::generate_context!()).expect("failed to initialize Boru desktop");
    app.run(|handle, event| {
        if let tauri::RunEvent::Exit = event {
            let runtime = { let state = handle.state::<AppState>(); state.runtime.lock().ok().and_then(|mut guard| guard.take()) };
            if let Some(mut runtime) = runtime { let _ = tauri::async_runtime::block_on(runtime.stop()); }
        }
    });
}
