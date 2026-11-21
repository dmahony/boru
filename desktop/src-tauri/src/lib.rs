use std::{path::PathBuf, sync::Mutex};

use boru_core::desktop_runtime::{DesktopRuntime, DesktopRuntimeConfig};
use tauri::Manager;

/// The one runtime attached to this application process. The profile lock in
/// DesktopRuntime rejects a second process attempting to open the same profile.
struct RuntimeState(Mutex<Option<DesktopRuntime>>);

pub fn run() {
    let app = tauri::Builder::default()
        .manage(RuntimeState(Mutex::new(None)))
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
            let (runtime, _handles) = tauri::async_runtime::block_on(
                DesktopRuntime::start_from_config(config),
            )
            .map_err(|error| anyhow::anyhow!("start Boru runtime: {error:#}"))?;
            *app.state::<RuntimeState>()
                .0
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
                let state = handle.state::<RuntimeState>();
                state
                    .0
                    .lock()
                    .ok()
                    .and_then(|mut guard| guard.take())
            };
            if let Some(mut runtime) = runtime {
                let _ = tauri::async_runtime::block_on(runtime.stop());
            }
        }
    });
}
