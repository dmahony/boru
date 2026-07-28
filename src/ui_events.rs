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
//! sender is stored on [`Storage`] so any component with a `&Storage`
//! reference can emit events.

use iroh::PublicKey;

use crate::chat_history::DeliveryState;

/// UI-relevant events emitted when persistent state changes in the core layer.
///
/// Frontends subscribe to these events via a broadcast receiver and reload
/// the affected projection from the repository.
#[derive(Debug, Clone)]
pub enum UiEvent {
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
}
