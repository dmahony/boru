//! Tauri identity startup detection and explicit identity creation.
use std::{path::{Path, PathBuf}, str::FromStr, sync::{Arc, Mutex}};
use serde::Serialize;
use tauri::State;
use boru_core::desktop_runtime::{DesktopRuntime, DesktopRuntimeConfig};

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum StartupState { NoIdentity, Existing, Selected }
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct IdentityProfile { pub endpoint_id: String, pub display_name: String, pub has_existing_identity: bool }
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StartupResponse { pub state: StartupState, pub endpoint_id: Option<String>, pub display_name: Option<String> }

/// Probe only existing directory entries. Never creates the directory or writes files.
pub fn probe(data_dir: &Path) -> Result<StartupState, String> {
    let key_path = data_dir.join("secret_key.txt");
    if !key_path.exists() { return Ok(StartupState::NoIdentity); }
    let raw = std::fs::read_to_string(key_path).map_err(|_| "identity_load_error".to_owned())?;
    iroh::SecretKey::from_str(raw.trim()).map_err(|_| "invalid_identity".to_owned())?;
    Ok(StartupState::Existing)
}

pub struct IdentityState {
    pub data_dir: PathBuf,
    pub runtime: Arc<Mutex<Option<DesktopRuntime>>>,
    pub tunnel_service: Arc<Mutex<Option<Arc<boru_core::tunnel::service::TunnelService>>>>,
    pub whisper_handle: Arc<Mutex<Option<boru_core::whisper::WhisperHandle>>>,
    pub whisper_events_rx: Arc<Mutex<Option<Arc<tokio::sync::Mutex<tokio::sync::mpsc::Receiver<boru_core::whisper::WhisperEvent>>>>>>,
    pub runtime_handles: Arc<Mutex<Option<boru_core::desktop_runtime::DesktopRuntimeHandles>>>,
}

#[tauri::command]
pub fn get_startup_state(state: State<'_, IdentityState>) -> Result<StartupResponse, String> {
    let startup = probe(&state.data_dir)?;
    if startup == StartupState::NoIdentity {
        return Ok(StartupResponse { state: startup, endpoint_id: None, display_name: None });
    }
    let guard = state.runtime.lock().map_err(|_| "runtime_error".to_owned())?;
    let runtime = guard.as_ref().ok_or_else(|| "runtime_error".to_owned())?;
    let endpoint_id = runtime.endpoint().id().to_string();
    let display_name = runtime.profile_snapshot().map(|profile| profile.display_name);
    Ok(StartupResponse { state: StartupState::Existing, endpoint_id: Some(endpoint_id), display_name })
}

#[tauri::command]
pub fn list_profiles(state: State<'_, IdentityState>) -> Result<Vec<IdentityProfile>, String> {
    match probe(&state.data_dir)? {
        StartupState::NoIdentity => Ok(Vec::new()),
        _ => {
            let raw = std::fs::read_to_string(state.data_dir.join("secret_key.txt")).map_err(|_| "identity_load_error".to_owned())?;
            let key = iroh::SecretKey::from_str(raw.trim()).map_err(|_| "invalid_identity".to_owned())?;
            let endpoint_id = key.public().to_string();
            let display_name = boru_core::user_profile::UserProfileStore::load(&state.data_dir, key.public()).ok().map(|profile| profile.profile().display_name.clone()).unwrap_or_default();
            Ok(vec![IdentityProfile { endpoint_id, display_name, has_existing_identity: true }])
        }
    }
}

fn runtime_config(data_dir: PathBuf, secret_key: Option<iroh::SecretKey>) -> DesktopRuntimeConfig {
    DesktopRuntimeConfig { data_dir, secret_key, relay_mode: iroh::RelayMode::Default, bind_port: 0, enable_mdns: true, enable_dht: true, publish_direct_addresses: false }
}

#[tauri::command]
pub async fn select_identity(endpoint_id: String, state: State<'_, IdentityState>) -> Result<String, String> {
    let raw = std::fs::read_to_string(state.data_dir.join("secret_key.txt")).map_err(|_| "not_found".to_owned())?;
    let key = iroh::SecretKey::from_str(raw.trim()).map_err(|_| "invalid_identity".to_owned())?;
    if key.public().to_string() != endpoint_id { return Err("not_found".to_owned()); }
    start(key, &state).await?;
    Ok(endpoint_id)
}

#[tauri::command]
pub async fn create_identity(display_name: Option<String>, state: State<'_, IdentityState>) -> Result<String, String> {
    if probe(&state.data_dir)? != StartupState::NoIdentity { return Err("identity_exists".to_owned()); }
    let key = iroh::SecretKey::generate();
    std::fs::create_dir_all(&state.data_dir).map_err(|_| "storage_error".to_owned())?;
    let encoded = key.to_bytes().iter().map(|byte| format!("{byte:02x}")).collect::<String>();
    std::fs::write(state.data_dir.join("secret_key.txt"), format!("{encoded}\n")).map_err(|_| "identity_create_error".to_owned())?;
    let endpoint_id = key.public().to_string();
    if let Some(name) = display_name.filter(|name| !name.trim().is_empty()) {
        let name = name.trim().to_owned();
        start(key, &state).await?;
        let guard = state.runtime.lock().map_err(|_| "runtime_error".to_owned())?;
        guard.as_ref().ok_or_else(|| "runtime_error".to_owned())?.storage().kv_set("identity_alias_v1", &name).map_err(|_| "storage_error".to_owned())?;
        return Ok(endpoint_id);
    }
    start(key, &state).await?;
    Ok(endpoint_id)
}

#[tauri::command]
pub fn update_alias(display_name: String, state: State<'_, IdentityState>) -> Result<String, String> {
    let name = display_name.trim();
    if name.is_empty() || name.len() > 80 { return Err("invalid_param".to_owned()); }
    let guard = state.runtime.lock().map_err(|_| "runtime_error".to_owned())?;
    let runtime = guard.as_ref().ok_or_else(|| "runtime_error".to_owned())?;
    runtime.storage().kv_set("identity_alias_v1", &name).map_err(|_| "storage_error".to_owned())?;
    Ok(name.to_owned())
}

async fn start(key: iroh::SecretKey, state: &IdentityState) -> Result<(), String> {
    if state.runtime.lock().map_err(|_| "runtime_error".to_owned())?.is_some() { return Ok(()); }
    let (runtime, handles) = DesktopRuntime::start_from_config(runtime_config(state.data_dir.clone(), Some(key))).await.map_err(|_| "runtime_error".to_owned())?;
    *state.tunnel_service.lock().map_err(|_| "runtime_error".to_owned())? = Some(handles.tunnel_service.clone());
    *state.whisper_handle.lock().map_err(|_| "runtime_error".to_owned())? = Some(handles.whisper_handle.clone());
    *state.whisper_events_rx.lock().map_err(|_| "runtime_error".to_owned())? = Some(handles.whisper_events_rx.clone());
    *state.runtime_handles.lock().map_err(|_| "runtime_error".to_owned())? = Some(handles);
    let mut guard = state.runtime.lock().map_err(|_| "runtime_error".to_owned())?;
    if guard.is_none() { *guard = Some(runtime); }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn fresh_probe_is_non_mutating() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(probe(dir.path()).unwrap(), StartupState::NoIdentity);
        assert!(!dir.path().join("secret_key.txt").exists());
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 0);
    }
    #[test]
    fn valid_and_invalid_existing_identity_probe() {
        let dir = tempfile::tempdir().unwrap();
        let key = iroh::SecretKey::generate();
        std::fs::write(dir.path().join("secret_key.txt"), key.to_bytes().iter().map(|byte| format!("{byte:02x}")).collect::<String>()).unwrap();
        assert_eq!(probe(dir.path()).unwrap(), StartupState::Existing);
    }
}
