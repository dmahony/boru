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
#![allow(clippy::too_many_arguments, clippy::type_complexity)]
#![allow(clippy::large_enum_variant)]

#[cfg(feature = "net")]
#[doc(inline)]
pub use net::GOSSIP_ALPN as ALPN;
#[cfg(feature = "net")]
pub use net::Gossip;

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
/// Tor-specific address and ticket scaffolding for the custom transport redesign.
#[cfg(feature = "net")]
pub mod tor_transport;

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
/// Active-session chat message state. No chat messages are persisted.
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

/// Encrypted recipient-hosted mailbox for offline direct-message delivery.
#[cfg(feature = "net")]
pub mod mailbox;

/// Whisper protocol — direct QUIC channels for private 1:1 messaging and file transfer.
#[cfg(feature = "net")]
pub mod whisper;

/// Inbox protocol — direct QUIC protocol for offline-message delivery.
///
/// Uses signed, timestamped messages with authorization checks and replay
/// protection.  The inbox topic is subscribed at startup and kept alive
/// independently of the visible chat room.
#[cfg(feature = "net")]
pub mod inbox;

/// Shared folder file indexer and change monitor.
///
/// Scans a local shared folder, builds an in-memory index of file metadata,
/// and watches for filesystem changes via the `notify` crate.
/// File hashing (blake3) is deferred to transfer time (lazy hashing).
#[cfg(feature = "net")]
pub mod file_indexer;

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

/// Streaming blob download from remote peers via iroh-blobs.
///
/// Downloads are streamed directly to a temporary file to avoid loading the
/// entire blob into memory.  After download the content is BLAKE3-hashed and
/// verified against the expected content hash, then atomically renamed.
#[cfg(feature = "net")]
pub mod blob_downloader;
/// Per-peer catalogue builder — constructs a signed catalogue for a
/// specific requesting peer, applying permission and visibility rules.
#[cfg(feature = "net")]
pub mod catalogue_builder;
/// Client for fetching remote file catalogues over the catalogue protocol.
#[cfg(feature = "net")]
pub mod catalogue_client;
/// QUIC protocol handler for the Remote File Catalogue service.
///
/// Accepts connections on `CATALOGUE_ALPN`, authenticates via iroh's
/// connection identity, enforces rate limits, validates protocol versions,
/// evaluates contact/block state, builds per-peer signed catalogues
/// (via [`crate::catalogue_builder`]), and returns versioned responses.
#[cfg(feature = "net")]
pub mod catalogue_handler;
/// Signed file catalogue — a self-contained, verifiable snapshot of a
/// profile's shared files and collections, suitable for gossip-based
/// distribution.
#[cfg(feature = "net")]
pub mod catalogue_model;
/// Lightweight catalogue revision notification — signed hints broadcast over
/// gossip to signal that a peer's catalogue has changed.
#[cfg(feature = "net")]
pub mod catalogue_notify;
/// Catalogue protocol request and response types for the Remote File
/// Catalogue QUIC protocol (`/iroh-chat-catalogue/1`).
///
/// Defines [`CatalogRequest`] (GetCatalogue with known_revision,
/// GetFileDetails) and [`CatalogResponse`] (NotModified, Catalogue,
/// FileDetails, NotFound, PermissionDenied, RateLimited,
/// UnsupportedVersion, InternalError) with stable error codes, payload
/// limits, and pagination support.
#[cfg(feature = "net")]
pub mod catalogue_protocol;
/// Cache refresh policy for remote file catalogues.
///
/// Event-driven state machine that decides when to refresh a cached
/// catalogue based on triggers (profile open, revision notice, staleness,
/// manual refresh, missing file).  No continuous polling.
#[cfg(feature = "net")]
pub mod catalogue_refresh;
/// Core diagnostics — bounded event and probe storage with sequence
/// numbering and thread-safe query methods.
///
/// Always available (no feature gate).  Use [`Diagnostics`] to record
/// [`DiagnosticEvent`]s and [`ReceivedProbe`]s.  Oldest entries are
/// automatically evicted when storage limits are exceeded.
pub mod diagnostics;
/// File access request and response types — protocol for requesting
/// download permission on a specific shared file from its owner.
#[cfg(feature = "net")]
pub mod file_access;
/// QUIC protocol handler for the File Transfer Authorisation service.
///
/// Accepts connections on `TRANSFER_AUTH_ALPN`, authenticates via iroh's
/// connection identity, validates requests, evaluates block/contact state and
/// per-file permissions, verifies availability and content identity, prepares
/// blobs, issues short-lived signed download descriptors, and records
/// diagnostics.  Bounded concurrency via global + per-peer semaphores.
#[cfg(feature = "net")]
pub mod file_access_handler;
/// Prepare offered files for iroh-blobs export.
///
/// Handles registration of imported and referenced files with the iroh-blobs
/// store.  Referenced files are temporarily imported (copied to a managed temp
/// directory, hash-verified, registered, then cleaned up) so the source is
/// never modified.  Imported files are registered from their inline storage.
#[cfg(feature = "net")]
pub mod file_offer;
/// Wire-protocol versioning for Remote File Catalogue QUIC protocols.
///
/// Provides envelope helpers that prefix every frame with a 4-byte protocol
/// version and 4-byte payload length, so unsupported versions can be rejected
/// before deserialization.
#[cfg(feature = "net")]
pub mod protocol_version;

/// Upload concurrency controls — owner-side limits on concurrent blob
/// preparations, verification/registration tasks, per-peer queue depth,
/// request timeouts, and optional per-peer cooldown.
///
/// Used by [`file_access_handler::FileAccessHandler`] to prevent resource
/// exhaustion from multiple simultaneous download requests.
/// See [`upload_concurrency::UploadConcurrencyConfig`] for field-level docs.
#[cfg(feature = "net")]
pub mod upload_concurrency;

/// Abuse-control utilities for the Remote File Catalogue protocol and
/// file-transfer authorisation handlers.
///
/// Provides display-text sanitisation, filesystem-safe filename
/// sanitisation, per-peer sliding-window rate limiting, invalid-signature
/// tracking, MIME type length validation, and total catalogue byte-size
/// validation.
///
/// **All remote metadata is treated as untrusted**, even when
/// cryptographically signed.  A valid signature proves authorship but does
/// not guarantee that the content is safe to render or use as a local
/// filesystem path.
#[cfg(feature = "net")]
pub mod abuse_controls;

/// Download concurrency controls — client-side limits on concurrent background
/// downloads, per-peer in-flight limits, verification transitions, queued →
/// active start rate, and progress-update DB write throttle.
///
/// Used by [`download_manager::DownloadManager`] to prevent resource
/// exhaustion when downloading many files simultaneously.
/// See [`download_concurrency::DownloadConcurrencyConfig`] for field-level docs.
#[cfg(feature = "net")]
pub mod download_concurrency;

/// Durable download state machine — tracks file transfers from remote peers.
///
/// Defines the [`DownloadState`] enum with validated transitions and the
/// [`DownloadRecord`] struct for persisted download tracking.  Records are
/// created **before** any network transfer begins.
pub mod download;
/// Download state machine runtime — transfer worker owning all
/// network-driven state transitions for the durable download lifecycle.
///
/// Provides [`DownloadManager`](download_manager::DownloadManager) (the
/// background actor) and [`DownloadHandle`](download_manager::DownloadHandle)
/// (the safe frontend handle).  Frontends must use [`DownloadHandle`] and
/// **never** call `storage.transition_download()` directly.
#[cfg(feature = "net")]
pub mod download_manager;
/// Retry queue for durable message delivery.
pub mod retry;
/// Safe destination path handling — derive trustworthy local filenames from
/// untrusted remote display names.
pub mod safe_destination;
/// Relational storage layer with managed migrations.
pub mod storage;
/// Durable inbox/outbox storage.
pub mod store;

/// Authoritative delivery-state machine for offline direct-message reliability.
///
/// Defines the single source of truth for outbound message lifecycle:
/// [`DeliveryState`] enum with validated transitions, restart recovery
/// rules, terminal-state semantics, and u8 wire encoding.
pub mod delivery_state;
