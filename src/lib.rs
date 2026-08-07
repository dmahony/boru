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
/// Zero-allocation byte-buffer pooling for repeated message construction.
///
/// A [`BufferPool`](buffer_pool::BufferPool) recycles cleared byte buffers
/// through [`PooledBuffer`](buffer_pool::PooledBuffer) /
/// [`PooledBytes`](buffer_pool::PooledBytes) instead of re-allocating on every
/// message.
pub mod buffer_pool;
#[cfg(feature = "net")]
pub mod discovery_backend;
#[cfg(feature = "net")]
pub mod discovery_record;
#[cfg(feature = "net")]
pub mod discovery_validation;
/// Conservative classification for attachment rendering.
pub mod media_classification;
pub mod metrics;
#[cfg(feature = "net")]
pub mod net;
/// Optional network diagnostics over the shared tunnel raw-stream transport.
#[cfg(feature = "net")]
pub mod network_doctor;
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
/// Lightweight HTTP streaming server for progressive video playback.
pub mod streaming_server;
/// Durable video metadata and process-local inline-player coordination.
pub mod video_playback;
/// Content-addressed, bounded poster generation for verified local videos.
pub mod video_poster;
/// Optional GStreamer runtime capability detection for inline video playback.
pub mod video_runtime;

/// Public-room directory — topic derivation, advertisement store, and
/// gossip subscription for discovering public rooms on the same relay.
#[cfg(feature = "net")]
pub mod directory;

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

/// Deflate compression with a preshared dictionary for the gossip wire
/// format.
///
/// Always compiled in (no feature gate) — the `compression` byte on
/// [`SignedMessage`](crate::chat_core::SignedMessage) selects at runtime
/// whether a message uses it.
pub mod wire_compression;

/// Whole-directory (HashSeq collection) transfer — import a folder tree into
/// iroh-blobs as a single collection and export a received collection back
/// to disk as a folder tree.
///
/// Available when the `net` feature is enabled (requires iroh-blobs).
#[cfg(feature = "net")]
pub mod collection_transfer;

/// Semantic event-type mapping for chat system messages.
///
/// Classifies the plain-text system messages produced by
/// [`ChatCallbacks::push_system`](crate::chat_callbacks::ChatCallbacks::push_system)
/// (join/leave, rename, command help, errors, …) into typed
/// [`SystemEventKind`](system_events::SystemEventKind) variants. Pure data
/// mapping — no UI logic, and all original message text is preserved.
#[cfg(feature = "net")]
pub mod system_events;

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
pub mod group_id;

/// Authenticated membership control events.
#[cfg(feature = "net")]
pub mod group_events;

/// Secure member removal and per-epoch credential rotation.
#[cfg(feature = "net")]
pub mod group_epoch;

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

/// Versioned peer invitations used to initiate pairing.
#[cfg(feature = "net")]
pub mod peer_invitation;

/// Pairing flow orchestration and restart recovery.
#[cfg(feature = "net")]
pub mod pairing_service;

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

/// Secure tunnel transport protocol and its dedicated ALPN handler.
#[cfg(feature = "net")]
pub mod tunnel;

/// Local TCP service discovery for the "Share Local Service" dialog.
///
/// Enumerates loopback-reachable listeners, verifies them with connect tests,
/// fingerprints HTTP services, and labels them. Isolated from the GUI so a
/// future non-desktop backend could substitute its own enumeration strategy.
#[cfg(feature = "gui")]
pub mod local_service_scan;

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

/// Opt-in Boru debug tracing — append-only event log for diagnosing
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

/// Relational storage layer with managed migrations.
pub mod storage;
/// Durable inbox/outbox storage.
pub mod store;
/// Durable offline delivery is owned by `outbox_delivery`; no second retry loop
/// is registered here.
/// UI event types emitted by the core layer when persistent state changes.
///
/// Frontends subscribe to these events via a broadcast receiver and reload
/// the affected projection from the repository.
#[cfg(feature = "net")]
pub mod ui_events;

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

/// BlobTicket wormhole sharing — copy a ticket string to share a file outside
/// the friend graph; paste a ticket to receive.  Provides address-info
/// trimming (Id-only vs RelayAndAddresses) and a connect-based preflight.
#[cfg(feature = "net")]
pub mod ticket_share;

/// Text sanitisation for safe display in the UI and logs.
///
/// Strips or replaces control characters, Unicode format characters
/// (bidi overrides, zero-width spaces, etc.), and truncates to a
/// reasonable length.  See the module docs for full details.
pub mod abuse_controls;

/// Human-friendly deterministic peer names derived from [`PublicKey`].
///
/// Provides [`generate_friendly_name`] for stable adjective‑noun names
/// (e.g. "Blue Falcon") and [`fmt_truncated`] for short identifiers
/// ("dfab…961f").  Used by the GUI as the fallback display‑name layer.
pub mod peer_names;

/// Blob transfer — iroh-blobs streaming download from a remote peer to a
/// local temp file.
#[cfg(feature = "net")]
pub mod blob_transfer;

/// Transfer lifecycle telemetry — structured events for download workflows.
#[cfg(feature = "net")]
pub mod transfer_telemetry;

/// Live, deduplicated transfer state for dashboard subscribers.
#[cfg(feature = "net")]
pub mod transfer_state_projection;

/// Data directory resolution with backward compatibility.
///
/// Resolves the application's persistent data directory using the
/// documented priority order (CLI override → BORU_DATA_DIR →
/// BORU_CHAT_DATA_DIR → legacy auto-detection → new XDG default →
/// new CWD fallback).  Always available (no feature gate).
pub mod data_dir;

/// Group encryption — p2panda-based end-to-end encrypted group messaging.
///
/// Provides type bridges between iroh and p2panda cryptographic types,
/// newtype wrappers for peer/operation identities, and the scaffolding
/// for key management, message encryption, and membership tracking.
#[cfg(feature = "net")]
pub mod group_encryption;

/// Bounded blocking file hasher — wraps blake3 hashing in
/// [`tokio::task::spawn_blocking`] with configurable concurrency.
///
/// Always available (no feature gate).  Used by [`file_indexer`] and
/// [`file_access_handler`] to avoid blocking the async runtime with
/// synchronous file I/O and CPU-bound blake3 computation.
#[cfg(feature = "net")]
pub mod file_hasher;

/// Provider-neutral GIF domain model — the [`GifProvider`] trait and its
/// neutral request/response types.
///
/// Always available (no feature gate) because it is pure data plus a
/// trait: no networking, no provider credentials.  Provider-specific
/// wire models live inside the adapter module that implements
/// [`GifProvider`](gif_provider::GifProvider).
pub mod gif_provider;
