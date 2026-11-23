/// Open a room and return its summary and first history page.
///
/// Request: `{ "conversationId": "<topic-id>" }` (camelCase). The response
/// is `OpenRoomResponse`; history defaults to 50 messages. The first-page
/// request ID is deterministic: `open_room:{conversation_id}:0`.
#[tauri::command]
pub async fn open_room(
    state: tauri::State<'_, crate::AppState>,
    conversation_id: String,
) -> Result<crate::bridge::dto::OpenRoomResponse, crate::bridge::AppError> {
    use std::str::FromStr;
    let topic = boru_core::proto::TopicId::from_str(&conversation_id).map_err(|_| crate::bridge::AppError::InvalidParam {
        field: "conversation_id".into(), reason: "must be a topic identifier".into(),
    })?;
    let runtime_cell = state.runtime.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let guard = runtime_cell.lock().map_err(crate::db_error)?;
        let runtime = guard.as_ref().ok_or_else(|| crate::bridge::AppError::Internal("runtime unavailable".into()))?;
        let snapshot = runtime.get_app_snapshot();
        let room = crate::list_rooms::room_summary(&snapshot, &conversation_id)?;
        let (rows, has_more) = runtime.storage().list_chat_messages_page(&topic, None, 50).map_err(crate::db_error)?;
        let mut messages = Vec::with_capacity(rows.len());
        for (timestamp, id, bytes, status) in rows {
            let (sender, message, _) = boru_core::chat_core::SignedMessage::verify_and_decode(&bytes).map_err(crate::db_error)?;
            let (kind, body) = match message {
                boru_core::chat_core::Message::Message { text } => ("text".to_owned(), text),
                other => ("event".to_owned(), format!("{other:?}")),
            };
            let delivery_status = status.and_then(|value| match value.as_str() {
                "queued" => Some(boru_core::snapshot::DeliveryStatusDto::Queued),
                "sent" => Some(boru_core::snapshot::DeliveryStatusDto::Sent),
                "delivered" => Some(boru_core::snapshot::DeliveryStatusDto::Delivered),
                "seen" => Some(boru_core::snapshot::DeliveryStatusDto::Seen),
                "failed" => Some(boru_core::snapshot::DeliveryStatusDto::Failed),
                _ => None,
            });
            messages.push(crate::bridge::dto::MessageDto { id: id.to_string(), conversation_id: conversation_id.clone(), sender_id: sender.to_string(), timestamp_ms: timestamp.max(0) as u64, kind, body, delivery_status, attachment_handle: None });
        }
        tokio::runtime::Handle::current().block_on(runtime.join_room(*topic.as_bytes())).map_err(crate::db_error)?;
        let next_cursor = if has_more { messages.last().map(|message| crate::bridge::encode_cursor(&crate::bridge::MessageCursor { timestamp: message.timestamp_ms as i64, id: message.id.clone() })) } else { None };
        Ok(crate::bridge::dto::OpenRoomResponse { room, messages, next_cursor })
    }).await.map_err(crate::db_error)?
}
