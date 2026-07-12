#![cfg_attr(feature = "net", doc = include_str!("../README.md"))]
//! Broadcast messages to peers subscribed to a topic
//!
//! The crate is designed to be used from the [iroh] crate, which provides a
//! [high level interface](https://docs.rs/iroh/latest/iroh/client/gossip/index.html),
//! but can also be used standalone.
//!
//! [iroh]: https://docs.rs/iroh
#![deny(missing_docs, rustdoc::broken_intra_doc_links)]
#![cfg_attr(iroh_docsrs, feature(doc_cfg))]
#![allow(unexpected_cfgs)]

#[cfg(feature = "net")]
pub use net::Gossip;
#[cfg(feature = "net")]
#[doc(inline)]
pub use net::GOSSIP_ALPN as ALPN;

#[cfg(feature = "net")]
pub mod api;
pub mod metrics;
#[cfg(feature = "net")]
pub mod net;
pub mod proto;

/// Shared chat core — state machine, protocol types, and network event handling.
///
/// Available when the `net` feature is enabled.  Used by the `chat` example
/// and is intended for reuse by other frontends (GUI, headless, etc.).
#[cfg(feature = "net")]
pub mod chat_core;

/// Signed contact and direct-conversation negotiation messages.
#[cfg(feature = "net")]
pub mod contact;

/// Frontend callback trait — decoupled from the core state machine.
///
/// The [`ChatCallbacks`] trait is the interface that frontend state structs
/// implement to receive typed network-event callbacks.  Extracted into its
/// own module so frontends (TUI, iced GUI, headless) can use it without
/// depending on the full `chat_core` implementation.
#[cfg(feature = "net")]
pub mod chat_callbacks;

/// Durable friends list storage for the chat frontends.
#[cfg(feature = "net")]
pub mod friends;

/// Durable room metadata for the chat frontends.
///
/// Persists the room topic so reopening a room reuses the same topic
/// instead of generating a new one each time.
#[cfg(feature = "net")]
pub mod room;

/// Transient multi-room state for the chat frontends.
///
/// Stores the current process's room list for navigation; it is never
/// restored from or written to disk.
#[cfg(feature = "net")]
pub mod room_history;

/// Room-level cleanup helpers for deleting a room's local history and metadata.
#[cfg(feature = "net")]
pub mod room_cleanup;

/// Active-session chat message state. No chat messages are persisted.
#[cfg(feature = "net")]
pub mod chat_history;

/// Durable friend request store — tracks pending/accepted/declined/cancelled\n/// friend requests between peers.\n#[cfg(feature = \"net\")]\npub mod friend_request;\n\n/// Durable encrypted outbox storage for outgoing messages.
///
/// Persists signed (encrypted) outgoing messages before sending so they
/// survive crashes and restarts.  Supports expiry of old entries and
/// duplicate suppression via stable event IDs.
#[cfg(feature = "net")]
pub mod outbox;

/// Encrypted recipient-hosted mailbox for offline direct-message delivery.
#[cfg(feature = "net")]
pub mod mailbox;

/// Whisper protocol — direct QUIC channels for private 1:1 messaging and file transfer.
#[cfg(feature = "net")]
pub mod whisper;

/// `/iroh-chat-inbox/1` direct QUIC protocol for offline-message delivery.
///
/// Uses signed, timestamped messages with authorization checks and replay
/// protection.  The inbox topic is subscribed at startup and kept alive
/// independently of the visible chat room.
#[cfg(feature = "net")]
pub mod inbox;

/// Backfill protocol — lets late-joining peers request message history
/// from existing peers via a dedicated QUIC ALPN.
#[cfg(feature = "net")]
pub mod backfill;

/// Secure, local per-user image storage with content-addressed identifiers.
///
/// Stores images below `<data_dir>/files` with hashed user directories and
/// content-addressed filenames.  File extensions are validated against an
/// allow-list; all others are treated as `.bin`.
#[cfg(feature = "net")]
pub mod image_store;

/// Image preprocessing for chat wire transport.
///
/// Provides resize + quality-retry JPEG compression for sender-side
/// optimization and receiver-side thumbnailing.
#[cfg(feature = "gui")]
pub mod image_optimizer;

/// Opt-in boru-chat debug tracing — append-only event log for diagnosing
/// mesh-forwarding bugs.
///
/// Enable with `BORU_DEBUG=1`.  Auto-initialised by the gossip actor;
/// no manual setup needed.
#[cfg(feature = "net")]
pub mod gossip_debug;

/// Durable friend request data model and persistent store.
///
/// Owns the on-disk `friend_requests.json` file that lives beside
/// `secret_key.txt` and `friends.json`.
#[cfg(feature = "net")]
pub mod friend_request;

pub use proto::TopicId;

/// Room metadata and roster documents synced via the gossip mesh.
///
/// Each room has two logical documents: metadata (name, description, rules)
/// and a roster (member set). Both are broadcast over the gossip topic.
#[cfg(feature = "net")]
pub mod room_docs;
