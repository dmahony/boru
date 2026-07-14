//! Callbacks trait for the chat frontend — decoupled from the core state machine.
//!
//! The [`ChatCallbacks`] trait defines the interface that a frontend state
//! struct must implement to receive typed callbacks for each kind of network
//! event processed by [`crate::chat_core::handle_net_event`].
//!
//! Separating the trait from `chat_core` allows frontends (TUI, iced GUI,
//! headless) to live in a different module tree without a hard dependency
//! on the core implementation.

use std::time::Duration;

use iroh::PublicKey;

use std::sync::atomic::{AtomicU64, Ordering};

use crate::chat_core::{MessageHash, Ticket};
use crate::chat_history::DeliveryState;
use crate::friends::FriendId;
use crate::user_profile::UserProfile;

/// A stable identifier for a file or image transfer.
///
/// Generated locally when a transfer is initiated. Used to correlate
/// progress events with the specific download they belong to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct TransferId(u64);

impl TransferId {
    /// Create a new transfer ID from a raw u64 value.
    pub fn new(id: u64) -> Self {
        Self(id)
    }

    /// Return the raw u64 value.
    pub fn into_u64(self) -> u64 {
        self.0
    }
}

/// Global counter for allocating fresh [`TransferId`]s.
///
/// Each increment returns a monotonically increasing u64, starting from 0.
/// Using `Relaxed` ordering is sufficient: we only require uniqueness,
/// not synchronization with other memory operations.
static TRANSFER_ID_COUNTER: AtomicU64 = AtomicU64::new(0);

impl TransferId {
    /// Allocate and return a new unique transfer identifier.
    ///
    /// Each call returns a value that is guaranteed to be distinct from
    /// all prior calls in this process.  Thread-safe via atomic increment.
    pub fn next() -> Self {
        Self(TRANSFER_ID_COUNTER.fetch_add(1, Ordering::Relaxed))
    }
}

/// The kind of a download transfer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TransferKind {
    /// A shared file download initiated via `/download`.
    File,
    /// An auto-downloaded image from an ImageShare message.
    Image,
}

/// Lifecycle events for a file/image download.
///
/// Emitted via [`ChatCallbacks::on_transfer_progress`] to let the frontend
/// surface observable progress. Frontends should ignore events for unknown
/// `TransferId`s (e.g. a stale progress callback after cancellation).
///
/// No events are emitted after a transfer reaches a terminal state.
#[derive(Debug, Clone)]
pub enum TransferProgress {
    /// A new download has started.
    Started {
        /// Stable identifier for this transfer.
        id: TransferId,
        /// What is being downloaded.
        kind: TransferKind,
        /// Human-readable name of the file or image.
        name: String,
        /// Total expected bytes when known, otherwise None.
        total: Option<u64>,
    },
    /// Progress update with current byte count.
    Progress {
        /// Stable identifier for this transfer.
        id: TransferId,
        /// What is being downloaded.
        kind: TransferKind,
        /// Human-readable name of the file or image.
        name: String,
        /// Bytes received so far.
        bytes: u64,
        /// Total expected bytes when known, otherwise None.
        total: Option<u64>,
    },
    /// Download completed successfully.
    Completed {
        /// Stable identifier for this transfer.
        id: TransferId,
        /// Kind of the completed transfer.
        kind: TransferKind,
        /// Human-readable name.
        name: String,
    },
    /// Download failed after starting.
    Failed {
        /// Stable identifier for this transfer.
        id: TransferId,
        /// Human-readable name.
        name: String,
        /// Reason the download aborted.
        error: String,
    },
    /// Download was cancelled (the future was dropped before completion).
    Cancelled {
        /// Stable identifier for this transfer.
        id: TransferId,
        /// What was being downloaded.
        kind: TransferKind,
        /// Human-readable name of the file or image.
        name: String,
    },
}

/// Callbacks invoked by [`crate::chat_core::handle_net_event`] to react to
/// network events.
///
/// Implement this trait on your frontend's state struct to receive typed
/// callbacks for each kind of network event. The shared `handle_net_event`
/// function handles all the common logic (friend tracking, name resolution,
/// message modification, etc.) and delegates frontend-specific actions to
/// these methods.
///
/// # Default implementations
///
/// * [`resolve_name`](ChatCallbacks::resolve_name) — format the short public key.
pub trait ChatCallbacks {
    /// Our own [`PublicKey`] — used to filter out self-messages.
    fn local_public(&self) -> PublicKey;

    /// Maximum age allowed for received messages before they are dropped.
    ///
    /// Frontends can override this to make TTL configurable.
    fn message_ttl(&self) -> Duration {
        Duration::from_secs(3600)
    }

    /// Look up a peer's display name, falling back to a short public key.
    fn resolve_name(&self, peer: &PublicKey) -> String {
        peer.fmt_short().to_string()
    }

    /// Return the most recent announced raw display name we already know for this peer.
    ///
    /// Frontends with persistent friend metadata can override this so repeated AboutMe
    /// broadcasts survive reconnects and room refreshes.
    fn last_announced_name(&self, _peer: &PublicKey) -> Option<String> {
        None
    }

    /// Record a peer's announced display name for later resolution.
    fn set_name(&mut self, peer: PublicKey, name: String) -> Option<String>;

    /// Check whether `peer` is a tracked friend.
    fn is_friend(&self, peer: &PublicKey) -> bool;

    /// Check whether `peer` is blocked (their messages are silently dropped).
    fn is_blocked(&self, _peer: &PublicKey) -> bool {
        false
    }

    /// Check whether `peer` is muted (system notifications suppressed).
    fn is_muted(&self, _peer: &PublicKey) -> bool {
        false
    }

    /// Mark a tracked friend as online.
    fn friend_mark_online(&mut self, fid: FriendId);

    /// Mark a tracked friend as offline.
    fn friend_mark_offline(&mut self, fid: FriendId);

    /// Update a friend's last announced display name.
    fn friend_set_name(&mut self, fid: FriendId, name: String);

    /// Notify the frontend that friend state needs persisting.
    fn mark_friends_dirty(&mut self);

    /// Append a system notification to the chat log.
    fn push_system(&mut self, text: String);

    /// Append a remote (incoming) message to the chat log.
    ///
    /// `hash` is the protocol message content hash, if available.
    /// `sent_at` is the protocol's Unix epoch seconds timestamp, if available.
    fn push_remote(
        &mut self,
        peer: PublicKey,
        label: String,
        text: String,
        hash: Option<MessageHash>,
        sent_at: Option<u64>,
    );

    /// Record a pending file download: `(filename, ticket_string)`.
    fn set_pending_file(&mut self, name: String, ticket: String);

    /// Record a pending image download: `(filename, blob_hash, sender_pk)`.
    /// The frontend should automatically download and render the image.
    fn set_pending_image(&mut self, name: String, hash: MessageHash, from: PublicKey);

    /// Check whether any chat entry has the given protocol message hash.
    fn has_message(&self, hash: &MessageHash) -> bool;

    /// Replace the body of the message identified by `hash`.
    fn edit_message(&mut self, hash: &MessageHash, new_text: String);

    /// Mark the message identified by `hash` as deleted.
    fn delete_message(&mut self, hash: &MessageHash);

    /// Add an emoji reaction to the message identified by `hash`.
    fn add_reaction(&mut self, hash: &MessageHash, emoji: String);

    /// Called when a gossip neighbor connects or disconnects.
    ///
    /// The default implementation immediately marks friend status in the
    /// store, pushes a system message, and calls [`on_neighbor_up`](ChatCallbacks::on_neighbor_up)
    /// or [`on_neighbor_down`](ChatCallbacks::on_neighbor_down).
    ///
    /// Override this to debounce rapid transitions (see IcedChat for an
    /// example) — batch pending changes and apply them at a fixed interval.
    fn on_neighbor_status_change(&mut self, peer: PublicKey, online: bool) {
        let fid = FriendId::from_public_key(peer);
        if self.is_friend(&peer) {
            if online {
                self.friend_mark_online(fid);
            } else {
                self.friend_mark_offline(fid);
                self.mark_friends_dirty();
            }
        }
        let name = self.resolve_name(&peer);
        if online {
            self.push_system(format!("{name} joined the chat"));
            self.on_neighbor_up(peer);
        } else {
            self.push_system(format!("{name} left the chat"));
            self.on_neighbor_down(peer);
        }
    }

    /// A gossip neighbor has connected.
    fn on_neighbor_up(&mut self, peer: PublicKey);

    /// A gossip neighbor has disconnected.
    fn on_neighbor_down(&mut self, peer: PublicKey);

    /// Record that we received any kind of gossip activity from a peer
    /// (message, neighbor up/down, presence ping).  Updates the mesh health
    /// timestamp for this peer.
    fn record_activity(&mut self, peer: PublicKey);

    /// Record a presence heartbeat from a peer — updates the last-seen timestamp.
    fn record_presence(&mut self, _peer: PublicKey) {}

    /// Record the ticket a peer advertises for starting a chat with them.
    ///
    /// Parsing, self-ticket filtering, and dirty-state handling are shared
    /// here. Frontends provide [`store_peer_ticket`](Self::store_peer_ticket)
    /// to merge the parsed ticket into their durable friend store.
    fn record_peer_ticket(&mut self, peer: PublicKey, ticket: String) {
        if peer == self.local_public() {
            return;
        }
        let Ok(ticket) = ticket.parse::<Ticket>() else {
            return;
        };
        if self.store_peer_ticket(peer, ticket) {
            self.mark_friends_dirty();
        }
    }

    /// Record a profile image ticket for a peer. (Default no-op).
    fn record_profile_image_ticket(&mut self, _peer: PublicKey, _ticket: String) {}

    /// Clear a peer's previously advertised profile image. (Default no-op).
    ///
    /// Called when a peer broadcasts an `AboutMe` with
    /// `profile_image_ticket: None`, signalling the image was removed.
    fn clear_profile_image(&mut self, _peer: PublicKey) {}

    /// Store profile metadata advertised by a peer. (Default no-op).
    fn on_profile_update(
        &mut self,
        _peer: PublicKey,
        _profile: UserProfile,
    ) {
    }

    /// Merge a parsed peer ticket into frontend-owned durable state.
    /// Returns whether the frontend accepted the ticket.
    fn store_peer_ticket(&mut self, _peer: PublicKey, _ticket: Ticket) -> bool {
        false
    }

    /// Request the frontend to quit (gossip receiver closed or error).
    fn request_quit(&mut self);

    // ── Delivery state callbacks ─────────────────────────────────────

    /// Look up the stable event id for a previously sent message by its content hash.
    ///
    /// Returns `None` if the hash is not tracked (not a self-sent message or
    /// already evicted).
    fn event_id_for_hash(&self, _hash: &MessageHash) -> Option<u64> {
        None
    }

    /// Advance the delivery state of a local message identified by `event_id`.
    ///
    /// The state transition must be valid per [`DeliveryState::can_transition_to`].
    /// This is a no-op by default (frontends that want delivery-state tracking
    /// should override both this and [`event_id_for_hash`](Self::event_id_for_hash)).
    fn update_delivery_state(&mut self, _event_id: u64, _state: DeliveryState) {}

    /// Observable lifecycle event for a file or image download.
    ///
    /// Called when a transfer starts, makes progress, completes, fails, or
    /// is cancelled.  Frontends can use these to show progress bars, update
    /// inline thumbnails, or display completion notifications.
    ///
    /// The default implementation is a no-op.
    fn on_transfer_progress(&mut self, _event: TransferProgress) {}
}
