//! Tauri IPC command handlers for the iroh gossip chat.

use tauri::{AppHandle, Manager as _};

use crate::backend::{ChatBackend, FrontendEvent, IpcResult, OnlineUserInfo, StatusSnapshot};
use crate::state::AppState as TauriState;

use crate::chat_entry::ChatEntryJson;

/// Initialize the chat backend (must be called once on startup).
#[tauri::command]
pub async fn init_backend(
    app_handle: AppHandle,
    state: tauri::State<'_, TauriState>,
) -> IpcResult<String> {
    let data_dir = app_handle
        .path()
        .app_data_dir()
        .map_err(|e| format!("can't get app data dir: {e}"))?
        .join("iroh-gossip-chat");

    let mut backend_lock = state.backend.lock().await;
    if backend_lock.is_some() {
        return Err("backend already initialized".to_string());
    }

    // Create the event channel for backend → frontend events
    let (event_tx, mut event_rx) = tokio::sync::mpsc::unbounded_channel();

    let backend = ChatBackend::new(data_dir, event_tx)
        .await
        .map_err(|e| format!("failed to init backend: {e}"))?;

    // Spawn event bridge: backend events → Tauri window events
    let handle = app_handle.clone();
    tokio::spawn(async move {
        use tauri::Emitter as _;
        while let Some(event) = event_rx.recv().await {
            match &event {
                FrontendEvent::NewEntry { .. } => {
                    let _ = handle.emit("chat-new-entry", &event);
                }
                FrontendEvent::StatusUpdate { .. } => {
                    let _ = handle.emit("chat-status", &event);
                }
                FrontendEvent::Ticket { .. } => {
                    let _ = handle.emit("chat-ticket", &event);
                }
                FrontendEvent::Topic { .. } => {
                    let _ = handle.emit("chat-topic", &event);
                }
                FrontendEvent::Nickname { .. } => {
                    let _ = handle.emit("chat-nickname", &event);
                }
                FrontendEvent::Disconnected => {
                    let _ = handle.emit("chat-disconnected", &event);
                }
                FrontendEvent::Error { .. } => {
                    let _ = handle.emit("chat-error", &event);
                }
                FrontendEvent::OnlineUserList { .. } => {
                    let _ = handle.emit("chat-online-users", &event);
                }
            }
        }
    });

    *backend_lock = Some(backend);
    Ok("backend initialized".to_string())
}

/// Create a new chat room.
#[tauri::command]
pub async fn create_room(
    state: tauri::State<'_, TauriState>,
) -> IpcResult<String> {
    let mut backend_lock = state.backend.lock().await;
    let backend = backend_lock.as_mut()
        .ok_or_else(|| "backend not initialized".to_string())?;
    backend.create_room(None).await
}

/// Join a chat room from a ticket.
#[tauri::command]
pub async fn join_room(
    state: tauri::State<'_, TauriState>,
    ticket: String,
) -> IpcResult<String> {
    let mut backend_lock = state.backend.lock().await;
    let backend = backend_lock.as_mut()
        .ok_or_else(|| "backend not initialized".to_string())?;
    backend.join_room(&ticket).await
}

/// Send a text message.
#[tauri::command]
pub async fn send_message(
    state: tauri::State<'_, TauriState>,
    text: String,
) -> IpcResult<()> {
    let mut backend_lock = state.backend.lock().await;
    let backend = backend_lock.as_mut()
        .ok_or_else(|| "backend not initialized".to_string())?;
    backend.send_message(&text).await
}

/// Set the display nickname.
#[tauri::command]
pub async fn set_nickname(
    state: tauri::State<'_, TauriState>,
    name: String,
) -> IpcResult<()> {
    let mut backend_lock = state.backend.lock().await;
    let backend = backend_lock.as_mut()
        .ok_or_else(|| "backend not initialized".to_string())?;
    backend.set_nickname(&name).await
}

/// Get the current room ticket.
#[tauri::command]
pub async fn get_ticket(
    state: tauri::State<'_, TauriState>,
) -> IpcResult<String> {
    let backend_lock = state.backend.lock().await;
    let backend = backend_lock.as_ref()
        .ok_or_else(|| "backend not initialized".to_string())?;
    backend.get_ticket_string().ok_or_else(|| "no active room".to_string())
}

/// Get all chat log entries.
#[tauri::command]
pub async fn get_entries(
    state: tauri::State<'_, TauriState>,
) -> IpcResult<Vec<ChatEntryJson>> {
    let backend_lock = state.backend.lock().await;
    let backend = backend_lock.as_ref()
        .ok_or_else(|| "backend not initialized".to_string())?;
    let entries = backend.get_entries().await;
    Ok(entries.into_iter().map(ChatEntryJson::from).collect())
}

/// Get connection status.
#[tauri::command]
pub async fn get_status(
    state: tauri::State<'_, TauriState>,
) -> IpcResult<StatusSnapshot> {
    let backend_lock = state.backend.lock().await;
    let backend = backend_lock.as_ref()
        .ok_or_else(|| "backend not initialized".to_string())?;
    Ok(backend.get_status().await)
}

/// Get the list of online peers with their display names.
#[tauri::command]
pub async fn get_online_peers(
    state: tauri::State<'_, TauriState>,
) -> IpcResult<Vec<OnlineUserInfo>> {
    let backend_lock = state.backend.lock().await;
    let backend = backend_lock.as_ref()
        .ok_or_else(|| "backend not initialized".to_string())?;
    Ok(backend.get_online_peers().await)
}
