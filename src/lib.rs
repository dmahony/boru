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

#[cfg(feature = "net")]
pub use net::Gossip;
#[cfg(feature = "net")]
#[doc(inline)]
pub use net::GOSSIP_ALPN as ALPN;

#[cfg(any(feature = "net", feature = "rpc"))]
pub mod api;
pub mod metrics;
#[cfg(feature = "net")]
pub mod net;
pub mod proto;
/// Tor-specific address and ticket scaffolding for the custom transport redesign.
pub mod tor_transport;

/// Shared chat core — state machine, protocol types, and network event handling.
///
/// Available when the `net` feature is enabled.  Used by the `chat` example
/// and is intended for reuse by other frontends (GUI, headless, etc.).
#[cfg(feature = "net")]
pub mod chat_core;

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

/// Durable multi-room history for the chat frontends.
///
/// Stores a list of known rooms so the GUI can show a recent-chat list
/// on startup — like Telegram/Signal's conversation list.
#[cfg(feature = "net")]
pub mod room_history;

/// Durable chat message history for the chat frontends.
///
/// Stores every signed gossip message so late-joiners can catch up
/// and messages survive restarts.
#[cfg(feature = "net")]
pub mod chat_history;

/// Minimal small-room messaging using direct QUIC connections.
///
/// Replaces the gossip broadcast tree for rooms with ≤10 members.
/// Broadcasts messages over direct connections and measures latency.
#[cfg(feature = "net")]
pub mod small_room;

/// Whisper protocol — direct QUIC channels for private 1:1 messaging and file transfer.
#[cfg(feature = "net")]
pub mod whisper;

/// History backfill protocol — lets late-joining peers request message history
/// from existing peers via a dedicated QUIC ALPN.
#[cfg(feature = "net")]
pub mod backfill;

pub use proto::TopicId;

/// Room metadata and roster documents synced via the gossip mesh.
///
/// Each room has two logical documents: metadata (name, description, rules)
/// and a roster (member set). Both are broadcast over the gossip topic.
#[cfg(feature = "net")]
pub mod room_docs;
