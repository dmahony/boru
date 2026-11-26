/// Read-only onboarding privacy information command.
#[tauri::command]
pub async fn get_onboarding_privacy_info(
) -> Result<(), crate::ipc::error::CommandError> {
    Err(crate::ipc::error::CommandError::internal())
}
