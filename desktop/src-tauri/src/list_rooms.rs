//! Read-only room-list command projection.

use boru_core::snapshot::AppSnapshot;

use crate::bridge::{dto::RoomSummary, AppError};

pub(crate) fn project(snapshot: AppSnapshot) -> Result<Vec<RoomSummary>, AppError> {
    let unread: std::collections::HashMap<_, _> = snapshot
        .unread
        .into_iter()
        .map(|entry| (entry.conversation_id, entry.count))
        .collect();

    Ok(snapshot
        .rooms
        .into_iter()
        .map(|room| RoomSummary {
            id: room.id.clone(),
            name: room.name,
            // RoomSnapshot currently exposes no message preview, so report null.
            last_message_summary: None,
            unread_count: unread.get(&room.id).copied().unwrap_or(0),
        })
        .collect())
}
