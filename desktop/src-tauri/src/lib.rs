pub mod bridge;
mod page_cancel;

use std::{
    path::PathBuf,
    sync::{Arc, Mutex},
};

use boru_core::{
    desktop_runtime::{DesktopRuntime, DesktopRuntimeConfig},
    snapshot::{
        AppSnapshot, ContactDto, ContactStatusDto, NetworkHealthSnapshot, RoomSnapshot,
        SNAPSHOT_SCHEMA_VERSION,
    },
};
use tauri::{Manager, State};

/// Shared state for read-only desktop bridge commands.
pub struct AppState {
    runtime: Arc<Mutex<Option<DesktopRuntime>>>,
}

fn db_error(error: impl std::fmt::Display) -> bridge::AppError {
    bridge::AppError::Db(error.to_string())
}

#[tauri::command]
async fn get_bootstrap(state: State<'_, AppState>) -> Result<AppSnapshot, bridge::AppError> {
    let runtime = state.runtime.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let guard = runtime.lock().map_err(db_error)?;
        guard
            .as_ref()
            .map(|r| r.get_app_snapshot())
            .ok_or_else(|| bridge::AppError::Internal("runtime unavailable".into()))
    })
    .await
    .map_err(db_error)?
}

/// Lists room conversations. Direct-message threads are included by `get_bootstrap`.
#[tauri::command]
async fn list_conversations(
    state: State<'_, AppState>,
) -> Result<Vec<RoomSnapshot>, bridge::AppError> {
    let snapshot = get_bootstrap(state).await?;
    Ok(snapshot.rooms)
}

#[tauri::command]
async fn get_contacts(state: State<'_, AppState>) -> Result<Vec<ContactDto>, bridge::AppError> {
    let runtime = state.runtime.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let guard = runtime.lock().map_err(db_error)?;
        let runtime = guard
            .as_ref()
            .ok_or_else(|| bridge::AppError::Internal("runtime unavailable".into()))?;
        let contacts = runtime.storage().list_contacts().map_err(db_error)?;
        Ok(contacts
            .into_iter()
            .map(|contact| ContactDto {
                id: contact
                    .user_id
                    .iter()
                    .map(|byte| format!("{byte:02x}"))
                    .collect(),
                display_name: contact
                    .user_id
                    .iter()
                    .map(|byte| format!("{byte:02x}"))
                    .collect(),
                status: ContactStatusDto::Accepted,
            })
            .collect())
    })
    .await
    .map_err(db_error)?
}

#[tauri::command]
async fn get_network_status(
    state: State<'_, AppState>,
) -> Result<NetworkHealthSnapshot, bridge::AppError> {
    let snapshot = get_bootstrap(state).await?;
    snapshot
        .network_health
        .ok_or_else(|| bridge::AppError::Internal("network status unavailable".into()))
}

#[tauri::command]
async fn get_settings(
    state: State<'_, AppState>,
) -> Result<bridge::SettingsSummary, bridge::AppError> {
    let runtime = state.runtime.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let guard = runtime.lock().map_err(db_error)?;
        let runtime = guard
            .as_ref()
            .ok_or_else(|| bridge::AppError::Internal("runtime unavailable".into()))?;
        let theme = runtime
            .storage()
            .kv_get_read_only("theme")
            .map_err(db_error)?;
        Ok(bridge::SettingsSummary {
            schema_version: SNAPSHOT_SCHEMA_VERSION,
            theme,
        })
    })
    .await
    .map_err(db_error)?
}

pub fn run() {
    let app = tauri::Builder::default()
        .manage(AppState {
            runtime: Arc::new(Mutex::new(None)),
        })
        .invoke_handler(tauri::generate_handler![
            get_bootstrap,
            list_conversations,
            get_contacts,
            get_network_status,
            get_settings
        ])
        .setup(|app| {
            // Tauri's application config directory is stable across launches;
            // do not create a new identity when a window is recreated.
            let data_dir: PathBuf = app
                .path()
                .app_data_dir()
                .map_err(|error| anyhow::anyhow!("resolve Boru profile directory: {error}"))?;
            let config = DesktopRuntimeConfig {
                data_dir,
                secret_key: None,
                relay_mode: iroh::RelayMode::Default,
                bind_port: 0,
                enable_mdns: true,
                enable_dht: true,
                publish_direct_addresses: false,
            };
            let (runtime, _handles) =
                tauri::async_runtime::block_on(DesktopRuntime::start_from_config(config))
                    .map_err(|error| anyhow::anyhow!("start Boru runtime: {error:#}"))?;
            *app.state::<AppState>()
                .runtime
                .lock()
                .map_err(|_| anyhow::anyhow!("Boru runtime state lock poisoned"))? = Some(runtime);
            Ok(())
        })
        .build(tauri::generate_context!())
        .expect("failed to initialize Boru desktop");

    // Closing the sole window exits the application (no hidden background
    // mode); this callback performs ordered async shutdown and releases the
    // profile lock only after router/endpoint teardown.
    app.run(|handle, event| {
        if let tauri::RunEvent::Exit = event {
            // Take the runtime out of managed state first: the guard must not be
            // held across the async shutdown below.
            let runtime = {
                let state = handle.state::<AppState>();
                state.runtime.lock().ok().and_then(|mut guard| guard.take())
            };
            if let Some(mut runtime) = runtime {
                let _ = tauri::async_runtime::block_on(runtime.stop());
            }
        }
    });
}
