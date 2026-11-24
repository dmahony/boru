//! Read-only room-list command projection.

use std::str::FromStr;

use boru_core::{snapshot::AppSnapshot, store::MessageStore};

use crate::bridge::{dto::RoomSummary, AppError};

/// Project one room summary from the snapshot and canonical message store.
/// This is shared by `list_rooms` and `open_room`.
pub(crate) fn room_summary(
    snapshot: &AppSnapshot,
    store: &MessageStore,
    id: &str,
) -> Result<RoomSummary, AppError> {
    let room = snapshot
        .rooms
        .iter()
        .find(|room| room.id == id)
        .ok_or_else(|| AppError::NotFound {
            entity: "room".into(),
            id: id.to_owned(),
        })?;
    let topic_bytes = boru_core::proto::TopicId::from_str(id)
        .ok()
        .map(|topic| *topic.as_bytes());
    let meta = topic_bytes
        .as_ref()
        .map(|topic| store.get_conversation_meta(topic))
        .transpose()
        .map_err(|error| AppError::Db(error.to_string()))?
        .flatten();

    Ok(RoomSummary {
        id: room.id.clone(),
        name: room.name.clone(),
        last_message_summary: meta
            .as_ref()
            .and_then(|meta| (!meta.last_message_preview.is_empty()).then(|| meta.last_message_preview.clone())),
        unread_count: meta.map(|meta| meta.unread_count).unwrap_or(0),
    })
}

pub(crate) fn project(
    snapshot: AppSnapshot,
    store: &MessageStore,
) -> Result<Vec<RoomSummary>, AppError> {
    snapshot
        .rooms
        .iter()
        .map(|room| room_summary(&snapshot, store, &room.id))
        .collect()
}
