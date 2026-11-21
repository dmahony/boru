use std::{path::PathBuf, sync::Mutex};

use boru_core::{
    desktop_runtime::{DesktopRuntime, DesktopRuntimeConfig},
    snapshot::AppSnapshot,
    ui_events::{EventCursor, ReplayResult, SequencedUiEvent},
};
use serde::Serialize;
use tauri::{Emitter, Manager, State};

const UI_EVENT: &str = "boru://ui-event";

#[derive(Clone, Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct UiEventEnvelope {
    epoch: String,
    revision: u64,
    event_id: String,
}

#[derive(Serialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
enum ReplayDto {
    Events { events: Vec<UiEventEnvelope> },
    Resnapshot { current_revision: u64 },
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct CursorDto {
    epoch: String,
    revision: u64,
}

fn envelope(event: &SequencedUiEvent) -> UiEventEnvelope {
    let (epoch, _) = event.event_id.rsplit_once(':').unwrap_or_default();
    UiEventEnvelope {
        epoch: epoch.to_owned(),
        revision: event.revision,
        event_id: event.event_id.clone(),
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct SnapshotResponse {
    snapshot: AppSnapshot,
    cursor: UiEventEnvelope,
}

#[tauri::command]
fn get_app_snapshot(state: State<'_, RuntimeState>) -> Result<SnapshotResponse, String> {
    let guard = state
        .0
        .lock()
        .map_err(|_| "runtime state lock poisoned".to_owned())?;
    let runtime = guard
        .as_ref()
        .ok_or_else(|| "runtime unavailable".to_owned())?;
    let (snapshot, cursor) = runtime.get_app_snapshot_with_revision();
    Ok(SnapshotResponse {
        snapshot,
        cursor: UiEventEnvelope {
            epoch: cursor.epoch,
            revision: cursor.revision,
            event_id: String::new(),
        },
    })
}

#[tauri::command]
fn replay_ui_events(
    state: State<'_, RuntimeState>,
    cursor: CursorDto,
) -> Result<ReplayDto, String> {
    let guard = state
        .0
        .lock()
        .map_err(|_| "runtime state lock poisoned".to_owned())?;
    let runtime = guard
        .as_ref()
        .ok_or_else(|| "runtime unavailable".to_owned())?;
    let cursor = EventCursor {
        epoch: cursor.epoch,
        revision: cursor.revision,
    };
    Ok(match runtime.replay_ui_events_from(&cursor) {
        ReplayResult::Events(events) => ReplayDto::Events {
            events: events.iter().map(envelope).collect(),
        },
        ReplayResult::Resnapshot { current_revision } => ReplayDto::Resnapshot { current_revision },
    })
}

/// The one runtime attached to this application process. The profile lock in
/// DesktopRuntime rejects a second process attempting to open the same profile.
struct RuntimeState(Mutex<Option<DesktopRuntime>>);

pub fn run() {
    let app = tauri::Builder::default()
        .manage(RuntimeState(Mutex::new(None)))
        .invoke_handler(tauri::generate_handler![get_app_snapshot, replay_ui_events])
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
            let mut receiver = runtime.subscribe_sequenced_ui_events();
            *app.state::<RuntimeState>()
                .0
                .lock()
                .map_err(|_| anyhow::anyhow!("Boru runtime state lock poisoned"))? = Some(runtime);
            let handle = app.handle().clone();
            tauri::async_runtime::spawn(async move {
                loop {
                    match receiver.recv().await {
                        Ok(event) => {
                            let _ = handle.emit(UI_EVENT, envelope(&event));
                        }
                        Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {
                            let _ = handle.emit("boru://resnapshot-required", ());
                        }
                        Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                    }
                }
            });
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
                state.0.lock().ok().and_then(|mut guard| guard.take())
            };
            if let Some(mut runtime) = runtime {
                let _ = tauri::async_runtime::block_on(runtime.stop());
            }
        }
    });
}
