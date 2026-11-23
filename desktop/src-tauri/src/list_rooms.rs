//! Read-only room-list command projection.

use boru_core::snapshot::AppSnapshot;

use crate::bridge::{dto::RoomSummary, AppError};

/// Project one room summary from the existing snapshot by its conversation ID.
/// This is the single mapping path shared by `list_rooms` and `open_room`.
pub(crate) fn room_summary(snapshot: &AppSnapshot, id: &str) -> Result<RoomSummary, AppError> {
    let room = snapshot
        .rooms
        .iter()
        .find(|room| room.id == id)
        .ok_or_else(|| AppError::NotFound {
            entity: "room".into(),
            id: id.to_owned(),
        })?;
    let unread_count = snapshot
        .unread
        .iter()
        .find(|entry| entry.conversation_id == id)
        .map(|entry| entry.count)
        .unwrap_or(0);

    Ok(RoomSummary {
        id: room.id.clone(),
        name: room.name.clone(),
        // RoomSnapshot currently exposes no message preview, so report null.
        last_message_summary: None,
        unread_count,
    })
}

pub(crate) fn project(snapshot: AppSnapshot) -> Result<Vec<RoomSummary>, AppError> {
    snapshot
        .rooms
        .iter()
        .map(|room| room_summary(&snapshot, &room.id))
        .collect()
}
