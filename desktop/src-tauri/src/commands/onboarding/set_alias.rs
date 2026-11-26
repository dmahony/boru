/// Set the local display alias using camelCase `alias` IPC argument.
#[tauri::command]
pub async fn set_alias(
    _alias: String,
) -> Result<(), crate::ipc::error::CommandError> {
    Err(crate::ipc::error::CommandError::internal())
}
