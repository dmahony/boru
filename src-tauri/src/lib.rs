//! Iroh Gossip Chat — Tauri 2 Desktop App library.

mod backend;
mod chat_entry;
mod ipc;
mod state;

use std::path::PathBuf;

use state::AppState as TauriState;
use tauri::Manager as _;
use tracing_subscriber::EnvFilter;

/// Application directories for the iroh gossip chat backend.
pub fn get_data_dir(app_handle: &tauri::AppHandle) -> Result<PathBuf, String> {
    app_handle
        .path()
        .app_data_dir()
        .map(|p| p.join("iroh-gossip-chat"))
        .map_err(|e| format!("can't get app data dir: {e}"))
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    // Initialize tracing once
    let _ = tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| EnvFilter::new("info"))
        )
        .try_init();

    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .setup(|app| {
            // Manage the state container (backend is lazy-initialized via IPC)
            app.manage(TauriState::new());

            #[cfg(debug_assertions)]
            if let Some(window) = app.get_webview_window("main") {
                window.open_devtools();
            }

            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            ipc::init_backend,
            ipc::create_room,
            ipc::join_room,
            ipc::send_message,
            ipc::set_nickname,
            ipc::get_ticket,
            ipc::get_entries,
            ipc::get_status,
            ipc::get_online_peers,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
