//! JSON-serializable chat entry wrapper.

use serde::Serialize;
use iroh_gossip::chat_core::ChatEntry;

/// A message entry that's safe to serialize over the Tauri IPC bridge.
#[derive(Debug, Clone, Serialize)]
pub struct ChatEntryJson {
    pub kind: String,
    pub label: String,
    pub body: String,
    pub edited: bool,
    pub reactions: Vec<String>,
}

impl From<ChatEntry> for ChatEntryJson {
    fn from(e: ChatEntry) -> Self {
        let kind = match e.kind {
            iroh_gossip::chat_core::ChatKind::System => "system",
            iroh_gossip::chat_core::ChatKind::Local => "local",
            iroh_gossip::chat_core::ChatKind::Remote => "remote",
        };
        Self {
            kind: kind.to_string(),
            label: e.label,
            body: e.body,
            edited: e.edited,
            reactions: e.reactions,
        }
    }
}
