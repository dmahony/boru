/// Join a room using a ticket string supplied as camelCase `ticket` from JavaScript.
/// On success the response object property is camelCase `{ topicId }`.
#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct JoinRoomTicketResponse {
    pub topic_id: String,
}

#[tauri::command]
pub async fn join_room_ticket(
    _ticket: String,
) -> Result<JoinRoomTicketResponse, crate::ipc::error::CommandError> {
    Err(crate::ipc::error::CommandError::internal())
}
