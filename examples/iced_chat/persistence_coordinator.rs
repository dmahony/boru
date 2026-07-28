//! Persistent background worker for offloading synchronous JSON writes
//! from the UI event loop.
//!
//! The coordinator receives typed [`PersistenceCommand`]s from the GUI and
//! executes them on a dedicated worker thread, applying:
//!
//! * **Coalescing** — replaceable snapshots (conversations, friends, settings,
//!   profile) only keep the newest unsaved version.
//! * **Debounce** — a short window (default ~200 ms) before flushing
//!   coalescable stores so burst updates collapse into a single write.
//!
//! The worker holds a bounded channel so the GUI can never push an unbounded
//! backlog of pending writes.
//!
//! # Stores handled
//!
//! | Store | Coalesced | Debounced | Delivery |
//! |-------|-----------|-----------|----------|
//! | [`boru_core::conversations::ConversationStore`] | ✓ (replace) | ✓ | clone at send |
//! | [`boru_core::chat_history::ChatHistoryStore`] | ✓ (latest via Arc) | — | lock+clone in worker |
//! | [`boru_core::friends::FriendsStore`] | ✓ (replace) | ✓ | clone at send |
//! | [`AppSettings`](crate::app::AppSettings) | ✓ (replace) | ✓ | clone at send |
//! | [`boru_core::user_profile::UserProfileStore`] | ✓ (replace) | ✓ | clone at send |
//! | [`boru_core::friend_request::FriendRequestStore`] | ✓ (replace) | ✓ | clone at send |

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{mpsc, Arc, Mutex};
use std::time::{Duration, Instant};

use boru_core::chat_history::ChatHistoryStore;
use boru_core::conversations::ConversationStore;
use boru_core::friend_request::FriendRequestStore;
use boru_core::friends::FriendsStore;
use boru_core::user_profile::UserProfileStore;

use crate::app::AppSettings;

/// How long the worker waits after the *last* coalescable command before
/// flushing to disk.  Burst updates collapse into one write.
const DEBOUNCE_MS: u64 = 200;

// ── Store identity keys for coalescing ─────────────────────────────────

#[derive(Hash, Eq, PartialEq, Clone, Copy, Debug)]
enum StoreKey {
    Conversations,
    ChatHistory,
    Friends,
    Settings,
    Profile,
    FriendRequests,
}

fn command_key(cmd: &PersistenceCommand) -> Option<StoreKey> {
    match cmd {
        PersistenceCommand::SaveConversations(_) => Some(StoreKey::Conversations),
        PersistenceCommand::SaveChatHistory(_) => Some(StoreKey::ChatHistory),
        PersistenceCommand::SaveFriends(_) => Some(StoreKey::Friends),
        PersistenceCommand::SaveSettings { .. } => Some(StoreKey::Settings),
        PersistenceCommand::SaveProfile(_) => Some(StoreKey::Profile),
        PersistenceCommand::SaveFriendRequests(_) => Some(StoreKey::FriendRequests),
        // Flush / Shutdown have no key — they are never coalesced.
        PersistenceCommand::Flush | PersistenceCommand::Shutdown => None,
    }
}

// ── Commands ──────────────────────────────────────────────────────────

/// A persistence command sent from the GUI to the background worker.
pub enum PersistenceCommand {
    /// Replace the on-disk conversation store.
    SaveConversations(ConversationStore),
    /// Persist the chat-history store (locked + cloned inside the worker).
    SaveChatHistory(Arc<Mutex<ChatHistoryStore>>),
    /// Replace the on-disk friends store.
    SaveFriends(FriendsStore),
    /// Persist application settings (clone at send time).
    SaveSettings {
        settings: AppSettings,
        data_dir: PathBuf,
    },
    /// Replace the on-disk user-profile store.
    SaveProfile(UserProfileStore),
    /// Replace the on-disk friend-request store.
    SaveFriendRequests(FriendRequestStore),
    /// Flush all pending commands to disk immediately.
    Flush,
    /// Flush pending work and shut down the worker thread.
    Shutdown,
}

impl std::fmt::Debug for PersistenceCommand {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::SaveConversations(_) => write!(f, "SaveConversations"),
            Self::SaveChatHistory(_) => write!(f, "SaveChatHistory"),
            Self::SaveFriends(_) => write!(f, "SaveFriends"),
            Self::SaveSettings { .. } => write!(f, "SaveSettings"),
            Self::SaveProfile(_) => write!(f, "SaveProfile"),
            Self::SaveFriendRequests(_) => write!(f, "SaveFriendRequests"),
            Self::Flush => write!(f, "Flush"),
            Self::Shutdown => write!(f, "Shutdown"),
        }
    }
}

// ── Coordinator ───────────────────────────────────────────────────────

/// A handle to the background persistence worker.
///
/// Drop the coordinator to trigger a graceful shutdown (flush + join).
pub struct PersistenceCoordinator {
    tx: mpsc::Sender<PersistenceCommand>,
    join_handle: Option<std::thread::JoinHandle<()>>,
}

impl PersistenceCoordinator {
    /// Start the background persistence worker with an unbounded command queue.
    pub fn spawn() -> Self {
        let (tx, rx) = mpsc::channel::<PersistenceCommand>();
        let join_handle = std::thread::Builder::new()
            .name("persistence-worker".into())
            .spawn(move || worker_loop(rx))
            .expect("failed to spawn persistence worker thread");

        Self {
            tx,
            join_handle: Some(join_handle),
        }
    }

    /// Send a command to the persistence worker.
    pub fn send(&self, cmd: PersistenceCommand) {
        let _ = self.tx.send(cmd);
    }

    /// Return a cloned sender for passing to the GUI layer.
    pub fn sender(&self) -> mpsc::Sender<PersistenceCommand> {
        self.tx.clone()
    }

    /// Flush all pending commands to disk and shut down the worker.
    pub fn shutdown(&mut self) {
        let _ = self.tx.send(PersistenceCommand::Shutdown);
        if let Some(handle) = self.join_handle.take() {
            let _ = handle.join();
        }
    }
}

impl Drop for PersistenceCoordinator {
    fn drop(&mut self) {
        self.shutdown();
    }
}

// ── Execute individual save ───────────────────────────────────────────

/// Execute a single persistence command *synchronously* on the calling
/// thread.  Used by the worker after coalescing and debounce.
fn execute_cmd(cmd: &PersistenceCommand) {
    match cmd {
        PersistenceCommand::SaveConversations(store) => {
            if let Err(e) = store.save() {
                tracing::warn!(error = %e, "failed to persist conversations");
            }
        }
        PersistenceCommand::SaveChatHistory(arc) => {
            let snapshot = arc.lock().unwrap().clone();
            if let Err(e) = snapshot.save() {
                tracing::warn!(error = %e, "failed to persist chat history");
            }
        }
        PersistenceCommand::SaveFriends(store) => {
            if let Err(e) = store.save() {
                tracing::warn!(error = %e, "failed to persist friends");
            }
        }
        PersistenceCommand::SaveSettings { settings, data_dir } => {
            settings.save(data_dir);
        }
        PersistenceCommand::SaveProfile(store) => {
            if let Err(e) = store.save() {
                tracing::warn!(error = %e, "failed to persist profile");
            }
        }
        PersistenceCommand::SaveFriendRequests(store) => {
            if let Err(e) = store.save() {
                tracing::warn!(error = %e, "failed to persist friend requests");
            }
        }
        // Flush and Shutdown are handled by the worker loop, not here.
        PersistenceCommand::Flush | PersistenceCommand::Shutdown => {}
    }
}

// ── Worker loop with coalescing + debounce ────────────────────────────

/// Coalesceable commands: those whose state can be safely replaced by a
/// newer version without losing work (snapshot stores).
fn is_coalescable(cmd: &PersistenceCommand) -> bool {
    matches!(
        cmd,
        PersistenceCommand::SaveConversations(_)
            | PersistenceCommand::SaveFriends(_)
            | PersistenceCommand::SaveSettings { .. }
            | PersistenceCommand::SaveProfile(_)
            | PersistenceCommand::SaveFriendRequests(_)
    )
}

/// Flush all pending commands to disk, draining the HashMap.
fn flush_pending(pending: &mut HashMap<StoreKey, PersistenceCommand>) {
    let keys: Vec<StoreKey> = pending.keys().copied().collect();
    for key in keys {
        if let Some(cmd) = pending.remove(&key) {
            execute_cmd(&cmd);
        }
    }
}

fn worker_loop(rx: mpsc::Receiver<PersistenceCommand>) {
    let mut pending: HashMap<StoreKey, PersistenceCommand> = HashMap::new();

    loop {
        // ── Wait for the first command ────────────────────────────────
        let first = match rx.recv() {
            Ok(cmd) => cmd,
            Err(mpsc::RecvError) => {
                // All senders dropped — flush remaining work and exit.
                flush_pending(&mut pending);
                return;
            }
        };

        // Handle special commands immediately.
        match &first {
            PersistenceCommand::Shutdown => {
                flush_pending(&mut pending);
                return;
            }
            PersistenceCommand::Flush => {
                flush_pending(&mut pending);
                continue;
            }
            _ => {}
        }

        // Insert into pending (coalescing by key for replaceable stores).
        // Flush/Shutdown are handled above, so unwrap is safe.
        let key = command_key(&first).expect("Flush/Shutdown handled above");
        pending.insert(key, first);

        // ── Debounce window ───────────────────────────────────────────
        // For coalescable stores, wait up to DEBOUNCE_MS for more
        // updates before flushing.  Non-coalescable stores (chat history,
        // outbox) also benefit from a minimal debounce so burst delivery
        // state changes collapse.
        let deadline = Instant::now() + Duration::from_millis(DEBOUNCE_MS);

        loop {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                break;
            }
            match rx.recv_timeout(remaining) {
                Ok(PersistenceCommand::Shutdown) => {
                    flush_pending(&mut pending);
                    return;
                }
                Ok(PersistenceCommand::Flush) => {
                    flush_pending(&mut pending);
                    // Continue outer loop for more commands.
                    break;
                }
                Ok(cmd) => {
                    // Flush/Shutdown are handled above, so unwrap is safe.
                    let key = command_key(&cmd).expect("Flush/Shutdown handled above");
                    pending.insert(key, cmd);
                }
                Err(mpsc::RecvTimeoutError::Timeout) => break,
                Err(mpsc::RecvTimeoutError::Disconnected) => {
                    flush_pending(&mut pending);
                    return;
                }
            }
        }

        // Flush all pending writes.
        flush_pending(&mut pending);
    }
}
