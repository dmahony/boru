//! `open_room` Tauri command.
//!
//! Request: `{ "conversationId": "<topic-id>" }` (camelCase, Tauri's default
//! command-argument casing). Response: [`crate::bridge::dto::OpenRoomResponse`]
//! with the room summary, the first history page (`MessageDto`s) and an opaque
//! `nextCursor`, which is null when the history fits in one page.
//!
//! The page size is the bridge's default limit (50 messages) and the first-page
//! cursor request id is deterministic: `open_room:{conversation_id}:0`.
//!
//! Both projections are reused rather than re-implemented:
//! * `list_rooms::room_summary` is the single room mapper (it also produces the
//!   `AppError::NotFound { entity: "room" }` used for an unknown room);
//! * `crate::load_messages_page` is the single pagination/cursor/mapping path
//!   shared with `get_messages_page`.
//!
//! The topic subscription uses the runtime's persistent room-sender registry
//! (`DesktopRuntime::room_sender_session`), so opening the same room twice
//! reuses the existing sender instead of subscribing again — a later
//! `send_message` for the conversation therefore finds a live sender.

use std::str::FromStr;

use crate::bridge::AppError;

/// Open a room, returning its summary and first history page.
#[tauri::command]
pub async fn open_room(
    state: tauri::State<'_, crate::AppState>,
    conversation_id: String,
) -> Result<crate::bridge::dto::OpenRoomResponse, AppError> {
    let topic = boru_core::proto::TopicId::from_str(&conversation_id).map_err(|_| {
        AppError::InvalidParam {
            field: "conversation_id".into(),
            reason: "must be a topic identifier".into(),
        }
    })?;

    let runtime_cell = state.runtime.clone();
    let request_id = format!("open_room:{conversation_id}:0");
    let conversation = conversation_id.clone();
    let (room, page, session) = tauri::async_runtime::spawn_blocking(move || {
        let guard = runtime_cell.lock().map_err(crate::db_error)?;
        let runtime = guard
            .as_ref()
            .ok_or_else(|| AppError::Internal("runtime unavailable".into()))?;
        let snapshot = runtime.get_app_snapshot();
        let room = crate::list_rooms::room_summary(
            &snapshot,
            &runtime.message_store(),
            &conversation,
        )?;
        let page = crate::load_messages_page(runtime, &conversation, None, None, Some(request_id))?;
        // Owned, `Send` handle: the std runtime guard is dropped before the
        // subscription is awaited.
        let session = runtime.room_sender_session();
        Ok::<_, AppError>((room, page, session))
    })
    .await
    .map_err(crate::db_error)??;

    session
        .join(*topic.as_bytes())
        .await
        .map_err(crate::db_error)?;

    let messages = page
        .items
        .into_iter()
        .map(|item| crate::bridge::dto::MessageDto {
            id: item.id,
            conversation_id: item.conversation_id,
            sender_id: item.sender_id,
            timestamp_ms: item.timestamp_ms,
            kind: item.kind,
            body: item.body,
            delivery_status: item.delivery_status,
            attachment_handle: None,
        })
        .collect();

    Ok(crate::bridge::dto::OpenRoomResponse {
        room,
        messages,
        next_cursor: page.next_cursor.map(|cursor| cursor.as_str().to_owned()),
    })
}
