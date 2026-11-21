use std::{path::PathBuf, sync::{Arc, Mutex}, time::Duration};

use boru_core::{desktop_runtime::{DesktopRuntime, DesktopRuntimeConfig}, media_handles::{MediaHandleError, MediaHandleRegistry, MAX_CHUNK_BYTES}};
use serde::Serialize;
use tauri::Manager;

struct RuntimeState(Mutex<Option<DesktopRuntime>>);
struct MediaState(Arc<MediaHandleRegistry>);

#[derive(Debug, Serialize)]
#[serde(rename_all = "snake_case")]
enum MediaIpcError { InvalidHandle, Expired, Unauthorized, WrongType, InvalidContentType, TooLarge, InvalidChunk, Cancelled, Io }
impl From<MediaHandleError> for MediaIpcError {
    fn from(error: MediaHandleError) -> Self {
        match error { MediaHandleError::InvalidHandle => Self::InvalidHandle, MediaHandleError::Expired => Self::Expired, MediaHandleError::Unauthorized => Self::Unauthorized, MediaHandleError::WrongType => Self::WrongType, MediaHandleError::InvalidContentType => Self::InvalidContentType, MediaHandleError::TooLarge => Self::TooLarge, MediaHandleError::InvalidChunk => Self::InvalidChunk, MediaHandleError::Cancelled => Self::Cancelled, MediaHandleError::Io => Self::Io }
    }
}
#[derive(Serialize)]
struct MediaHandleDto { handle: String, max_chunk_bytes: usize }
#[derive(Serialize)]
struct MediaChunkDto { offset: u64, bytes: Vec<u8>, complete: bool }

// Authorization identity is derived from Tauri, never from caller-supplied IPC data.
fn owner(window: &tauri::WebviewWindow) -> String { window.label().to_owned() }

#[tauri::command]
async fn media_begin_upload(window: tauri::WebviewWindow, state: tauri::State<'_, MediaState>, content_type: String) -> Result<MediaHandleDto, MediaIpcError> {
    let registry = state.0.clone(); let owner = owner(&window);
    let handle = tauri::async_runtime::spawn_blocking(move || registry.begin_upload(&owner, &content_type)).await.map_err(|_| MediaIpcError::Io)??;
    Ok(MediaHandleDto { handle, max_chunk_bytes: MAX_CHUNK_BYTES })
}
#[tauri::command]
async fn media_write_chunk(window: tauri::WebviewWindow, state: tauri::State<'_, MediaState>, handle: String, offset: u64, bytes: Vec<u8>) -> Result<u64, MediaIpcError> {
    if bytes.len() > MAX_CHUNK_BYTES { return Err(MediaIpcError::InvalidChunk); }
    let registry = state.0.clone(); let owner = owner(&window);
    tauri::async_runtime::spawn_blocking(move || registry.write_chunk(&owner, &handle, offset, &bytes)).await.map_err(|_| MediaIpcError::Io)?.map_err(Into::into)
}
#[tauri::command]
async fn media_finish_upload(window: tauri::WebviewWindow, state: tauri::State<'_, MediaState>, handle: String) -> Result<u64, MediaIpcError> {
    let registry = state.0.clone(); let owner = owner(&window);
    tauri::async_runtime::spawn_blocking(move || registry.size(&owner, &handle, true)).await.map_err(|_| MediaIpcError::Io)?.map_err(Into::into)
}
#[tauri::command]
async fn media_read_chunk(window: tauri::WebviewWindow, state: tauri::State<'_, MediaState>, handle: String, offset: u64, limit: usize) -> Result<MediaChunkDto, MediaIpcError> {
    if limit == 0 || limit > MAX_CHUNK_BYTES { return Err(MediaIpcError::InvalidChunk); }
    let registry = state.0.clone(); let owner = owner(&window);
    tauri::async_runtime::spawn_blocking(move || {
        let size = registry.size(&owner, &handle, false)?;
        let bytes = registry.read_chunk(&owner, &handle, offset, limit)?;
        Ok::<_, MediaHandleError>(MediaChunkDto { offset, complete: offset.saturating_add(bytes.len() as u64) >= size, bytes })
    }).await.map_err(|_| MediaIpcError::Io)?.map_err(Into::into)
}
#[tauri::command]
async fn media_cancel(window: tauri::WebviewWindow, state: tauri::State<'_, MediaState>, handle: String) -> Result<(), MediaIpcError> {
    let registry = state.0.clone(); let owner = owner(&window);
    tauri::async_runtime::spawn_blocking(move || registry.cancel(&owner, &handle)).await.map_err(|_| MediaIpcError::Io)?.map_err(Into::into)
}

pub fn run() {
    let media_registry = Arc::new(MediaHandleRegistry::new(Duration::from_secs(15 * 60), boru_core::media_handles::MAX_MEDIA_BYTES).expect("create private media directory"));
    let app = tauri::Builder::default()
        .manage(RuntimeState(Mutex::new(None)))
        .manage(MediaState(media_registry))
        .invoke_handler(tauri::generate_handler![media_begin_upload, media_write_chunk, media_finish_upload, media_read_chunk, media_cancel])
        .setup(|app| {
            let data_dir: PathBuf = app.path().app_data_dir().map_err(|error| anyhow::anyhow!("resolve Boru profile directory: {error}"))?;
            let config = DesktopRuntimeConfig { data_dir, secret_key: None, relay_mode: iroh::RelayMode::Default, bind_port: 0, enable_mdns: true, enable_dht: true, publish_direct_addresses: false };
            let (runtime, _handles) = tauri::async_runtime::block_on(DesktopRuntime::start_from_config(config)).map_err(|error| anyhow::anyhow!("start Boru runtime: {error:#}"))?;
            *app.state::<RuntimeState>().0.lock().map_err(|_| anyhow::anyhow!("Boru runtime state lock poisoned"))? = Some(runtime);
            Ok(())
        })
        .build(tauri::generate_context!()).expect("failed to initialize Boru desktop");
    app.run(|handle, event| {
        if let tauri::RunEvent::Exit = event {
            let runtime = { let state = handle.state::<RuntimeState>(); state.0.lock().ok().and_then(|mut guard| guard.take()) };
            if let Some(mut runtime) = runtime { let _ = tauri::async_runtime::block_on(runtime.stop()); }
        }
    });
}
