//! Lightweight UI event types emitted by the core layer when persistent
//! state changes.  The GUI (or any frontend) subscribes to these events
//! and reloads the affected projection from the repository, keeping the
//! UI state as a read-only projection rather than an authority.
//!
//! # Design
//!
//! Each variant carries enough information for the GUI to know *what*
//! changed without carrying the full updated state.  The GUI then calls
//! the repository to reload the affected projection.
//!
//! Events are broadcast through a `tokio::sync::broadcast` channel; the
//! sender is stored on [`Storage`](crate::storage::Storage) so any component with a `&Storage`
//! reference can emit events.

use iroh::PublicKey;
use std::{
    collections::VecDeque,
    sync::{Arc, Mutex},
};

use crate::chat_history::DeliveryState;

/// UI-relevant events emitted when persistent state changes in the core layer.
///
/// Frontends subscribe to these events via a broadcast receiver and reload
/// the affected projection from the repository.
#[derive(Debug, Clone)]
pub enum UiEvent {
    /// An inbound room message was persisted and is ready for display.
    InboundMessage {
        /// Room/topic identifier.
        room_id: String,
        /// Stable database id of the message.
        message_id: i64,
        /// Sender public key bytes.
        sender: [u8; 32],
        /// Unix timestamp in milliseconds.
        timestamp: i64,
        /// Decoded message payload.
        payload: String,
    },
    /// A new message was inserted into a conversation (incoming or outgoing).
    MessageInserted {
        /// Stable database id of the inserted message.
        message_id: i64,
    },
    /// A message's delivery state was updated.
    DeliveryStateChanged {
        /// Stable database id of the affected message.
        message_id: i64,
        /// The new delivery state.
        state: DeliveryState,
    },
    /// A conversation was created, updated, or deleted.
    ConversationChanged {
        /// The 32-byte conversation id that changed.
        conversation_id: [u8; 32],
    },
    /// The persisted unread count for a room changed.
    UnreadChanged {
        /// The room whose persisted unread count changed.
        room_id: [u8; 32],
        /// The count read back from persistent storage.
        unread_count: u32,
    },
    /// A friend request was created, accepted, declined, or cancelled.
    FriendRequestChanged {
        /// Stable database id of the affected request.
        request_id: i64,
    },
    /// A room (chat topic) was created, updated, or deleted.
    RoomChanged {
        /// The room's identifier (topic string).
        room_id: String,
    },
    /// A user profile was updated.
    ProfileChanged {
        /// Public key of the user whose profile changed.
        user_id: PublicKey,
    },
    /// The friends list was modified (friend added/removed/blocked).
    FriendsListChanged,
    /// A production protocol or blob-provider event forwarded by DesktopRuntime.
    ProtocolEvent {
        /// Protocol subsystem that emitted the event.
        source: &'static str,
        /// Debug-formatted event payload.
        detail: String,
    },
}

/// Monotonically numbered event suitable for ordered frontend reconciliation.
#[derive(Debug, Clone)]
pub struct SequencedUiEvent {
    /// Runtime-local strictly increasing revision (starts at 1).
    pub revision: u64,
    /// Stable within this runtime, derived from the runtime epoch and revision.
    pub event_id: String,
    /// UI-safe state-change notification.
    pub event: UiEvent,
}

/// Snapshot/event-log position that remains distinguishable across runtime restarts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EventCursor {
    /// Runtime epoch that generated the revision.
    pub epoch: String,
    /// Event sequence within that epoch.
    pub revision: u64,
}

/// Replay either returns all changes since a cursor or explicitly requires a fresh snapshot.
#[derive(Debug, Clone)]
pub enum ReplayResult {
    /// Retained updates in revision order.
    Events(Vec<SequencedUiEvent>),
    /// Cursor is too old or from a future runtime state; fetch a fresh snapshot.
    Resnapshot {
        /// Latest revision observed when the replay decision was made.
        current_revision: u64,
    },
}

#[derive(Debug)]
struct JournalState {
    revision: u64,
    events: VecDeque<SequencedUiEvent>,
}

/// Bounded event history for deterministic replay/resynchronization.
/// Subscribe to the live channel first, read the snapshot watermark, then call
/// `replay_after(watermark)` to bridge the snapshot/subscription interval.
#[derive(Debug, Clone)]
pub struct UiEventJournal {
    epoch: Arc<str>,
    capacity: usize,
    state: Arc<Mutex<JournalState>>,
}

impl UiEventJournal {
    /// Create a journal with a fixed maximum retained event count.
    pub fn new(epoch: impl Into<String>, capacity: usize) -> Self {
        Self {
            epoch: Arc::from(epoch.into()),
            capacity: capacity.max(1),
            state: Arc::new(Mutex::new(JournalState {
                revision: 0,
                events: VecDeque::new(),
            })),
        }
    }

    /// Append an event and return its assigned envelope.
    pub fn append(&self, event: UiEvent) -> SequencedUiEvent {
        let mut state = self.state.lock().unwrap_or_else(|p| p.into_inner());
        state.revision = state.revision.saturating_add(1);
        let item = SequencedUiEvent {
            revision: state.revision,
            event_id: format!("{}:{}", self.epoch, state.revision),
            event,
        };
        state.events.push_back(item.clone());
        while state.events.len() > self.capacity {
            state.events.pop_front();
        }
        item
    }

    /// Current sequence watermark, suitable for tagging an authoritative snapshot.
    pub fn revision(&self) -> u64 {
        self.state
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .revision
    }

    /// Cursor includes the runtime epoch, so clients cannot replay across a restart.
    pub fn cursor(&self) -> EventCursor {
        EventCursor {
            epoch: self.epoch.to_string(),
            revision: self.revision(),
        }
    }

    /// Replay all retained events after a cursor, or request a fresh snapshot.
    pub fn replay_cursor(&self, cursor: &EventCursor) -> ReplayResult {
        if cursor.epoch != self.epoch.as_ref() {
            return ReplayResult::Resnapshot {
                current_revision: self.revision(),
            };
        }
        self.replay_after(cursor.revision)
    }

    /// Replay all retained events after a numeric cursor, or request a fresh snapshot.
    pub fn replay_after(&self, after_revision: u64) -> ReplayResult {
        let state = self.state.lock().unwrap_or_else(|p| p.into_inner());
        let current_revision = state.revision;
        let oldest = state.events.front().map(|event| event.revision);
        if after_revision > current_revision
            || oldest.is_some_and(|oldest| after_revision.saturating_add(1) < oldest)
        {
            return ReplayResult::Resnapshot { current_revision };
        }
        ReplayResult::Events(
            state
                .events
                .iter()
                .filter(|event| event.revision > after_revision)
                .cloned()
                .collect(),
        )
    }
}

#[cfg(test)]
mod journal_tests {
    use super::*;
    fn event() -> UiEvent {
        UiEvent::FriendsListChanged
    }

    #[test]
    fn rapid_burst_replays_in_order() {
        let journal = UiEventJournal::new("epoch", 512);
        for _ in 0..400 {
            journal.append(event());
        }
        let ReplayResult::Events(events) = journal.replay_after(0) else {
            panic!("unexpected resnapshot")
        };
        assert_eq!(events.len(), 400);
        assert!(events
            .iter()
            .enumerate()
            .all(|(i, event)| event.revision == i as u64 + 1));
        assert_eq!(events[0].event_id, "epoch:1");
    }

    #[test]
    fn cursor_from_another_runtime_requires_snapshot() {
        let journal = UiEventJournal::new("new-runtime", 8);
        journal.append(event());
        let old_cursor = EventCursor {
            epoch: "old-runtime".into(),
            revision: 0,
        };
        assert!(matches!(
            journal.replay_cursor(&old_cursor),
            ReplayResult::Resnapshot {
                current_revision: 1
            }
        ));
    }

    #[test]
    fn overflow_and_future_cursor_require_snapshot() {
        let journal = UiEventJournal::new("epoch", 2);
        for _ in 0..3 {
            journal.append(event());
        }
        assert!(matches!(
            journal.replay_after(0),
            ReplayResult::Resnapshot {
                current_revision: 3
            }
        ));
        assert!(matches!(
            journal.replay_after(4),
            ReplayResult::Resnapshot {
                current_revision: 3
            }
        ));
        match journal.replay_after(1) {
            ReplayResult::Events(events) => assert_eq!(
                events.iter().map(|e| e.revision).collect::<Vec<_>>(),
                [2, 3]
            ),
            ReplayResult::Resnapshot { .. } => panic!("cursor is still retained"),
        }
    }
}
