//! Callbacks trait for the chat frontend — decoupled from the core state machine.
//!
//! The [`ChatCallbacks`] trait defines the interface that a frontend state
//! struct must implement to receive typed callbacks for each kind of network
//! event processed by [`crate::chat_core::handle_net_event`].
//!
//! Separating the trait from `chat_core` allows frontends (TUI, iced GUI,
//! headless) to live in a different module tree without a hard dependency
//! on the core implementation.

use iroh::PublicKey;

use crate::chat_core::MessageHash;
use crate::friends::FriendId;

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

    /// Look up a peer's display name, falling back to a short public key.
    fn resolve_name(&self, peer: &PublicKey) -> String {
        peer.fmt_short().to_string()
    }

    /// Record a peer's announced display name for later resolution.
    fn set_name(&mut self, peer: PublicKey, name: String);

    /// Check whether `peer` is a tracked friend.
    fn is_friend(&self, peer: &PublicKey) -> bool;

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
    fn push_remote(&mut self, label: String, text: String, hash: Option<MessageHash>);

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

    /// A gossip neighbor has connected.
    fn on_neighbor_up(&mut self, peer: PublicKey);

    /// A gossip neighbor has disconnected.
    fn on_neighbor_down(&mut self, peer: PublicKey);

    /// Request the frontend to quit (gossip receiver closed or error).
    fn request_quit(&mut self);
}
