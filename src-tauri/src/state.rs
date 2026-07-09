//! Tauri-app-level state for the iroh gossip chat.

use tokio::sync::Mutex;

use crate::backend::ChatBackend;

/// Application state managed by Tauri.
pub struct AppState {
    /// The chat backend (iroh node).
    pub backend: Mutex<Option<ChatBackend>>,
}

impl AppState {
    pub fn new() -> Self {
        Self {
            backend: Mutex::new(None),
        }
    }
}
