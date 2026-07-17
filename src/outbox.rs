//! Legacy JSON outbox migration reader.
//!
//! Writing to `outbox.json` has been removed. The SQLite-backed
//! [`MessageStore`](crate::store::MessageStore) is the durable source.
//! This module only retains the [`OutboxEntry`] type and [`OutboxStore`]
//! load methods for reading existing on-disk data during migration.

use std::{
    collections::HashMap,
    fs,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

use n0_error::{Result, StdResultExt};
use serde::{Deserialize, Serialize};

use crate::chat_history::DeliveryState;
use crate::proto::TopicId;

/// Current schema version — bump on breaking format changes.
const OUTBOX_SCHEMA_VERSION: u32 = 1;

/// Name of the on-disk outbox file (lives beside `secret_key.txt`).
pub const OUTBOX_FILE_NAME: &str = "outbox.json";

/// Default for serde.
fn default_outbox_schema_version() -> u32 {
    OUTBOX_SCHEMA_VERSION
}

/// Compute the on-disk path for the outbox file.
fn outbox_file_path(data_dir: &Path) -> PathBuf {
    data_dir.join(OUTBOX_FILE_NAME)
}

/// A single entry in the legacy JSON outbox.
///
/// This type is retained for migration reading. In the new flow, outbox
/// entries are stored in the SQLite `outbox` table.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct OutboxEntry {
    /// Stable, monotonically-increasing event identifier.
    pub event_id: u64,
    /// blake3 hex hash of the raw signed message bytes.
    pub hash: String,
    /// The gossip topic this message belongs to.
    pub topic: TopicId,
    /// The raw signed message bytes, ready to pass to `GossipSender::broadcast`.
    pub signed_bytes: Vec<u8>,
    /// Current delivery state of this message.
    #[serde(default)]
    pub delivery_state: DeliveryState,
    /// Unix-epoch milliseconds when this entry was created.
    pub created_at: u64,
    /// Number of send attempts.
    #[serde(default)]
    pub retry_count: u32,
}

impl OutboxEntry {
    /// Create a new outbox entry with [`DeliveryState::Queued`].
    pub fn new(event_id: u64, topic: TopicId, signed_bytes: Vec<u8>) -> Self {
        let hash = crate::chat_history::blake3_hex(&signed_bytes);
        Self {
            event_id,
            hash,
            topic,
            signed_bytes,
            delivery_state: DeliveryState::Queued,
            created_at: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis() as u64,
            retry_count: 0,
        }
    }
}

/// Legacy outbox store — **read-only migration reader**.
///
/// Persistence is now handled by SQLite. This type only exists to load
/// any existing `outbox.json` for migration.
///
/// `save()`, delivery-state transitions, expiry, and retry tracking
/// have been removed. One component (RetryWorker) owns retry; one
/// path (SQLite) handles incoming and outgoing persistence.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct OutboxStore {
    #[serde(default = "default_outbox_schema_version")]
    schema_version: u32,
    #[serde(default)]
    entries: HashMap<u64, OutboxEntry>,
    #[serde(default)]
    ordered_ids: Vec<u64>,
    #[serde(skip)]
    data_dir: PathBuf,
}

impl OutboxStore {
    /// Create an empty outbox store bound to a data directory.
    pub fn empty_at(data_dir: impl Into<PathBuf>) -> Self {
        Self {
            schema_version: OUTBOX_SCHEMA_VERSION,
            entries: HashMap::new(),
            ordered_ids: Vec::new(),
            data_dir: data_dir.into(),
        }
    }

    /// Return the on-disk outbox file path.
    pub fn file_path(&self) -> PathBuf {
        outbox_file_path(&self.data_dir)
    }

    /// Load the outbox store from disk, if it exists.
    pub fn load(data_dir: impl AsRef<Path>) -> Result<Option<Self>> {
        let data_dir = data_dir.as_ref();
        let path = outbox_file_path(data_dir);
        if !path.exists() {
            return Ok(None);
        }
        let raw = fs::read_to_string(&path)
            .with_std_context(|_| format!("failed to read outbox file {}", path.display()))?;
        let mut store: Self = serde_json::from_str(&raw)
            .with_std_context(|_| format!("failed to parse outbox file {}", path.display()))?;
        if store.schema_version != OUTBOX_SCHEMA_VERSION {
            return Err(n0_error::anyerr!(
                "unsupported outbox schema version {} in {}",
                store.schema_version,
                path.display()
            ));
        }
        store.data_dir = data_dir.to_path_buf();
        store
            .ordered_ids
            .retain(|id| store.entries.contains_key(id));
        Ok(Some(store))
    }

    /// Load the outbox store, falling back to an empty store on failure.
    pub fn load_or_default(data_dir: impl AsRef<Path>) -> Self {
        let data_dir = data_dir.as_ref();
        match Self::load(data_dir) {
            Ok(Some(store)) => store,
            Ok(None) => Self::empty_at(data_dir),
            Err(err) => {
                eprintln!(
                    "warning: failed to load outbox from {}: {err}",
                    outbox_file_path(data_dir).display()
                );
                Self::empty_at(data_dir)
            }
        }
    }

    /// Push a new entry into the outbox. Rejects duplicate event IDs.
    pub fn push(&mut self, entry: OutboxEntry) -> std::result::Result<(), String> {
        let event_id = entry.event_id;
        if self.entries.contains_key(&event_id) {
            return Err(format!("duplicate outbox entry for event_id {}", event_id));
        }
        self.entries.insert(event_id, entry);
        self.ordered_ids.push(event_id);
        Ok(())
    }

    /// Get an entry by event ID.
    pub fn get(&self, event_id: u64) -> Option<&OutboxEntry> {
        self.entries.get(&event_id)
    }

    /// Get a mutable entry by event ID.
    pub fn get_mut(&mut self, event_id: u64) -> Option<&mut OutboxEntry> {
        self.entries.get_mut(&event_id)
    }

    /// Remove an entry by event ID. Returns whether it was found.
    pub fn remove(&mut self, event_id: u64) -> bool {
        let removed = self.entries.remove(&event_id).is_some();
        if removed {
            self.ordered_ids.retain(|id| *id != event_id);
        }
        removed
    }

    /// Return all entries in insertion order.
    pub fn entries(&self) -> Vec<&OutboxEntry> {
        self.ordered_ids
            .iter()
            .filter_map(|id| self.entries.get(id))
            .collect()
    }

    /// Return all entries with `Queued` delivery state.
    pub fn pending(&self) -> Vec<&OutboxEntry> {
        self.ordered_ids
            .iter()
            .filter_map(|id| self.entries.get(id))
            .filter(|e| e.delivery_state == DeliveryState::Queued)
            .collect()
    }

    /// Return all entries with a given delivery state.
    pub fn with_state(&self, state: DeliveryState) -> Vec<&OutboxEntry> {
        self.ordered_ids
            .iter()
            .filter_map(|id| self.entries.get(id))
            .filter(|e| e.delivery_state == state)
            .collect()
    }

    /// Check whether an entry with the given event ID exists.
    pub fn contains(&self, event_id: u64) -> bool {
        self.entries.contains_key(&event_id)
    }

    /// Number of entries currently in the outbox.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether the outbox is empty.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Remove all entries.
    pub fn clear(&mut self) {
        self.entries.clear();
        self.ordered_ids.clear();
    }

    /// Remove all entries for a specific topic.
    pub fn remove_topic(&mut self, topic: &TopicId) -> usize {
        let to_remove: Vec<u64> = self
            .ordered_ids
            .iter()
            .filter_map(|id| {
                self.entries
                    .get(id)
                    .filter(|e| e.topic == *topic)
                    .map(|_| *id)
            })
            .collect();
        for id in &to_remove {
            self.entries.remove(id);
        }
        self.ordered_ids.retain(|id| !to_remove.contains(id));
        to_remove.len()
    }

    /// Path to the outbox file, for display.
    pub fn display_path(&self) -> String {
        self.file_path().display().to_string()
    }

    /// Update the delivery state of an entry (in-memory only).
    pub fn update_delivery_state(
        &mut self,
        event_id: u64,
        state: DeliveryState,
    ) -> std::result::Result<(), String> {
        match self.entries.get_mut(&event_id) {
            Some(entry) => {
                entry.delivery_state = state;
                Ok(())
            }
            None => Err(format!("entry {} not found", event_id)),
        }
    }

    /// Increment the retry count of an entry.
    pub fn increment_retry(&mut self, event_id: u64) {
        if let Some(entry) = self.entries.get_mut(&event_id) {
            entry.retry_count = entry.retry_count.saturating_add(1);
        }
    }
}
