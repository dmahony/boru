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
#[cfg(feature = "net")]
pub mod discovery_backend;
#[cfg(feature = "net")]
pub mod discovery_record;
#[cfg(feature = "net")]
pub mod discovery_validation;
pub mod metrics;
#[cfg(feature = "net")]
pub mod net;
/// Versioned, privacy-aware peer invitations for out-of-band pairing.
pub mod peer_invitation;
pub mod proto;
pub mod public_room;
#[cfg(feature = "net")]
/// Public-room configuration defaults and limits.
///
/// All tuning parameters for DHT discovery timing, record validation
/// strictness, peer-count bounds, message size, nickname length, rate
/// limits, blob announcement limits, download limits, and backfill caps
/// are centralised here.  See [`PublicRoomConfig`] for field-level docs.
pub mod public_room_config;
/// Continuous DHT publication and discovery for public rooms.
///
/// Spawns background tasks that periodically re-publish local presence and
/// discover new peers on the DHT.  Discovered peers are forwarded through
/// an mpsc channel for the caller to join.
#[cfg(feature = "net")]
pub mod public_room_continuous;
/// QR encoding and decoding for peer invitations.
pub mod qr;

/// Bounded dynamic peer joiner — joins discovered peers into the gossip mesh
/// with dedup, backoff, retries, and concurrency limits.
#[cfg(feature = "net")]
pub mod dynamic_joiner;
/// Safety and rate-limit enforcement for untrusted public-room message flows.
///
/// Wraps [`PublicRoomConfig`] with per-peer state for message size, nickname
/// length, message rate, blob announcements, and download-queue bounds.
/// Pass `None` for private rooms to skip every check.
#[cfg(feature = "net")]
pub mod public_room_safety;
/// Boru-specific public-room topic tracker that wraps a [`TopicDiscoveryBackend`]
/// with boru's identity model for publish-once / discover-once operations.
#[cfg(feature = "net")]
pub mod public_room_tracker;
pub mod topic_derivation;

/// Per-room discovery secrets — cryptographically random 32-byte keys
/// that isolate private rooms on the DHT.
///
/// Always available (no feature gate) so that [`RoomStore`] can
/// (de)serialize secrets without the `net` feature.
pub mod discovery_secret;

/// Private-room topic tracker — thin wrapper over [`TopicDiscoveryBackend`]
/// with domain-separated namespace derivation and peer isolation.
#[cfg(feature = "net")]
pub mod private_room_tracker;

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

/// Bounded startup burst scheduler for queued download admissions.
#[cfg(feature = "net")]
pub mod bounded_startup_scheduler;

/// Bounded admission and resource controls for file downloads.
#[cfg(feature = "net")]
pub mod download_limits;

/// Durable friends list storage for the chat frontends.
#[cfg(feature = "net")]
pub mod friends;

/// Durable conversation records for the chat frontends.
///
/// Persists conversation metadata keyed by gossip topic, surviving
/// application restarts.  Separate from the transient room-history list.
#[cfg(feature = "net")]
pub mod conversations;

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

/// Secure legacy room-secret migration: owner-signed, topic-bound,
/// epoch-versioned upgrades with deterministic conflict resolution.
#[cfg(feature = "net")]
// pub mod room_secret_migration;
#[cfg(feature = "net")]
pub mod chat_history;

/// Durable friend request store — tracks pending/accepted/declined/cancelled
/// friend requests between peers.
#[cfg(feature = "net")]
pub mod friend_request;

/// Durable encrypted outbox storage for outgoing messages.
///
/// Persists signed (encrypted) outgoing messages before sending so they
/// survive crashes and restarts.  Supports expiry of old entries and
/// duplicate suppression via stable event IDs.
#[cfg(feature = "net")]
pub mod outbox;
/// Single-owner durable offline delivery worker.
pub mod outbox_delivery;

/// Encrypted recipient-hosted mailbox for offline direct-message delivery.
#[cfg(feature = "net")]
pub mod mailbox;

/// Whisper protocol — direct QUIC channels for private 1:1 messaging and file transfer.
#[cfg(feature = "net")]
pub mod whisper;

/// Shared folder file indexer and change monitor.
///
/// Scans a local shared folder, builds an in-memory index of file metadata,
/// and watches for filesystem changes via the `notify` crate.
/// File hashing (blake3) is deferred to transfer time (lazy hashing).
#[cfg(feature = "net")]
pub mod file_indexer;

/// `/iroh-chat-inbox/1` direct QUIC protocol for offline-message delivery.
///
/// Uses signed, timestamped messages with authorization checks and replay
/// protection.  Delivery is direct QUIC via the inbox ALPN; it is independent
/// of room gossip topics and the visible chat room.
#[cfg(feature = "net")]
pub mod inbox;

/// Backfill protocol — lets late-joining peers request message history
/// from existing peers via a dedicated QUIC ALPN.
#[cfg(feature = "net")]
pub mod backfill;

/// Per-user profile settings and sharing controls.
///
/// Owns the on-disk `user_profile.json` that lives beside `secret_key.txt`.
/// Controls file sharing, download permissions, and path security.
#[cfg(feature = "net")]
pub mod user_profile;

/// Remote-safe representation of shared file entries for wire transfer.
#[cfg(feature = "net")]
pub mod catalogue_model;

/// Durable download states and post-transfer verification helpers.
pub mod download;

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

/// Pure-Rust image compression — resize and JPEG-encode with caller-specified
/// parameters.
///
/// Always available (no feature gate). Uses the `image` crate's pure-Rust JPEG
/// encoder with no C FFI dependencies.
pub mod compression;

/// Opt-in boru-chat debug tracing — append-only event log for diagnosing
/// mesh-forwarding bugs.
///
/// Enable with `BORU_DEBUG=1`.  Auto-initialised by the gossip actor;
/// no manual setup needed.
#[cfg(feature = "net")]
pub mod gossip_debug;

pub use proto::TopicId;

/// Room metadata and roster documents synced via the gossip mesh.
///
/// Each room has two logical documents: metadata (name, description, rules)
/// and a roster (member set). Both are broadcast over the gossip topic.
#[cfg(feature = "net")]
pub mod room_docs;

/// Performance instrumentation — timing samples, RAII timers, and
/// slow-operation detection.
///
/// Enable at runtime with `BORU_PERF=1`.  Provides a global singleton
/// that accumulates samples and prints a summary report.
pub mod perf;

/// Core diagnostics — bounded event and probe storage with sequence
/// numbering and thread-safe query methods.
///
/// Always available (no feature gate).  Use [`Diagnostics`] to record
/// [`DiagnosticEvent`]s and [`ReceivedProbe`]s.  Oldest entries are
/// automatically evicted when storage limits are exceeded.
pub mod diagnostics;

// Durable offline delivery is owned by `outbox_delivery`; no second retry loop
// is registered here.
/// Relational storage layer with managed migrations.
pub mod storage;
/// Durable inbox/outbox storage.
pub mod store;

/// Catalogue retrieval protocol — versioned request/response wire wrappers.
///
/// Always available (no feature gate).  Defines [`CatalogWireRequest`],
/// [`CatalogWireResponse`], inner [`CatalogRequest`]/[`CatalogResponse`]
/// enums, and wire-safe [`CatalogErrorCode`].
pub mod catalogue_protocol;

/// File access protocol — versioned request/response wire wrappers.
///
/// Always available (no feature gate).  Defines [`FileAccessWireRequest`],
/// [`FileAccessWireResponse`], inner [`FileAccessRequest`]/[`FileAccessResponse`]
/// types, and wire-safe [`FileAccessErrorCode`].
pub mod file_access_protocol;

// ── New modules (catalogue + file access) ────────────────────────────────────

/// Versioned wire-frame protocol helpers — `read_frame` / `write_frame`.
pub mod protocol_version;

/// Central size and count limits for catalogue protocol traffic.
pub mod catalogue_limits;

/// Per-peer and global rate limiting for catalogue protocol connections.
pub mod catalogue_rate_limits;

/// Catalogue retrieval protocol handler — server side.
pub mod catalogue_handler;

/// Catalogue retrieval client — fetches and verifies a signed catalogue
/// from a remote peer.
pub mod catalogue_client;

/// File access (download-authorisation) protocol handler — server side.
#[cfg(feature = "net")]
pub mod file_access_handler;

/// Download state-machine manager — tick-driven worker that processes
/// queued downloads through the full lifecycle.
#[cfg(feature = "net")]
pub mod download_manager;

/// Download initiation — validates preconditions (catalogue verified,
/// file metadata valid, no conflicting download) before queuing a new
/// durable download.
#[cfg(feature = "net")]
pub mod download_initiation;

/// File access transfer client — requests fresh download descriptors from
/// a remote peer and verifies the signed response.
#[cfg(feature = "net")]
pub mod file_access_client;

/// Safe destination selection — sanitises remote display names to prevent
/// path traversal and filename injection.
pub mod safe_destination;

/// Text sanitisation for safe display in the UI and logs.
///
/// Strips or replaces control characters, Unicode format characters
/// (bidi overrides, zero-width spaces, etc.), and truncates to a
/// reasonable length.  See the module docs for full details.
pub mod abuse_controls;

/// Blob transfer — iroh-blobs streaming download from a remote peer to a
/// local temp file.
#[cfg(feature = "net")]
pub mod blob_transfer;

/// Transfer lifecycle telemetry — structured events for download workflows.
#[cfg(feature = "net")]
pub mod transfer_telemetry;
// dummy
