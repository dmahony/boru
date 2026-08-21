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
/// Authoritative room roles, capabilities, signed moderation events, and
/// versioned authorization-state persistence.
pub mod authorization;
/// Zero-allocation byte-buffer pooling for repeated message construction.
///
/// A [`BufferPool`](buffer_pool::BufferPool) recycles cleared byte buffers
/// through [`PooledBuffer`](buffer_pool::PooledBuffer) /
/// [`PooledBytes`](buffer_pool::PooledBytes) instead of re-allocating on every
/// message.
pub mod buffer_pool;
/// Call identities and call-control state shared by frontends.
pub mod call;
/// Small-group voice-room metadata, membership, input policy, and fan-out.
pub mod voice_room;
#[cfg(feature = "net")]
pub mod discovery_backend;
/// Global DHT bootstrap tracker (BORU-DHT-01) — lets a fresh internet-only node
/// find bootstrap peers over the Mainline DHT and feed them into the discovery
/// mesh. Net-gated: it reuses the `TopicDiscoveryBackend` abstraction (DHT),
/// mirroring `discovery_backend`.
#[cfg(feature = "net")]
pub mod discovery_bootstrap;
/// Adaptive DHT discovery cadence policy (BORU-DHT-05) — a pure, unit-testable,
/// UI-independent state machine that decides the base delay before each next
/// DHT discovery lookup from mesh-health signals. Net-gated: consumed by the
/// net-gated discovery loops (public/private room trackers, bootstrap).
#[cfg(feature = "net")]
pub mod discovery_cadence;
#[cfg(feature = "net")]
pub mod discovery_record;
#[cfg(feature = "net")]
pub mod discovery_validation;
/// Conservative classification for attachment rendering.
pub mod media_classification;
/// Peer-ID-backed message mentions and member autocomplete.
pub mod mentions;
pub mod metrics;
#[cfg(feature = "net")]
pub mod net;
/// Optional network diagnostics over the shared tunnel raw-stream transport.
#[cfg(feature = "net")]
pub mod network_doctor;
pub mod proto;
/// Address-only reply references and unresolved-parent resolution.
pub mod replies;
/// Deterministic actor-scoped reaction state and event projection.
pub mod reactions;
/// Authenticated, deterministic pinned-message state and operations.
#[cfg(feature = "net")]
pub mod pinned_messages;
pub mod public_room;
#[cfg(feature = "net")]
/// Public-room configuration defaults and limits.
///
/// All tuning parameters for DHT discovery timing, record validation
/// strictness, peer-count bounds, message size, nickname length, rate
/// limits, blob announcement limits, download limits, and backfill caps
/// are centralised here.  See [`PublicRoomConfig`](crate::public_room_config::PublicRoomConfig) for field-level docs.
pub mod public_room_config;
/// Continuous DHT publication and discovery for public rooms.
///
/// Spawns background tasks that periodically re-publish local presence and
/// discover new peers on the DHT.  Discovered peers are forwarded through
/// an mpsc channel for the caller to join.
#[cfg(feature = "net")]
pub mod public_room_continuous;
/// Global DHT public-room registry — a relay-independent browseable index of
/// discoverable room metadata (name, topic, ticket, owner).
#[cfg(feature = "net")]
pub mod room_registry;
/// Lightweight HTTP streaming server for progressive video playback.
pub mod streaming_server;
/// Durable video metadata and process-local inline-player coordination.
pub mod video_playback;
/// Content-addressed, bounded poster generation for verified local videos.
pub mod video_poster;
/// Optional GStreamer runtime capability detection for inline video playback.
pub mod video_runtime;

/// Feature-gated screen sharing subsystem boundary.
#[cfg(feature = "screen-sharing")]
pub mod screen_share;

/// Localhost-only configuration helpers for the experimental VNC tunnel.
#[cfg(feature = "net")]
pub mod vnc_tunnel;

/// Public-room directory — topic derivation, advertisement store, and
/// gossip subscription for discovering public rooms on the same relay.
#[cfg(feature = "net")]
pub mod directory;

/// Bounded local cache of discovered public rooms (PDF Phase 4, Task 4.1).
///
/// Keyed by stable room_id; stores the latest valid advertisement plus
/// provenance (publisher, auth verdict, first/last seen, expiry,
/// compatibility, local join state); enforces entry-count + metadata-size
/// bounds with deterministic replacement and withdrawal handling. Owned by
/// the discovery/control-plane layer — never creates conversation records
/// or subscribes to room topics.
#[cfg(feature = "net")]
pub mod room_directory;

/// Bounded dynamic peer joiner — joins discovered peers into the gossip mesh
/// with dedup, backoff, retries, and concurrency limits.
#[cfg(feature = "net")]
pub mod dynamic_joiner;
/// Rolling, bounded candidate admission policy for DHT discovery loops.
///
/// Replaces the hard lifetime `max_candidates_per_session` cap with a bounded
/// remembered set, cooldown/stale TTL, short-term rolling-window abuse bound,
/// and per-cycle cap (PDF Task 3).
#[cfg(feature = "net")]
pub mod candidate_admission;
/// Safety and rate-limit enforcement for untrusted public-room message flows.
///
/// Wraps [`PublicRoomConfig`](crate::public_room_config::PublicRoomConfig) with per-peer state for message size, nickname
/// length, message rate, blob announcements, and download-queue bounds.
/// Pass `None` for private rooms to skip every check.
#[cfg(feature = "net")]
pub mod public_room_safety;
/// Boru-specific public-room topic tracker that wraps a [`TopicDiscoveryBackend`](crate::discovery_backend::TopicDiscoveryBackend)
/// with boru's identity model for publish-once / discover-once operations.
#[cfg(feature = "net")]
pub mod public_room_tracker;
pub mod topic_derivation;

/// Thread targeting, timeline filtering, unread state, and durable thread
/// projections shared by the network and GUI layers.
pub mod threads;

/// Versioned internal discovery topic identifier — the single gossip topic
/// every Boru node joins at startup as networking infrastructure (peer
/// discovery / presence / connectivity bootstrap). Not a conversation: it is
/// never rendered, persisted, or routed through chat payload paths.
///
/// Always available (no feature gate) so the derivation and its known-answer
/// vectors mirror [`topic_derivation`](crate::topic_derivation) and
/// [`public_room`](crate::public_room).
pub mod discovery_topic;

/// Discovery protocol message types — Hello / Presence / PeerAdvertisement.
///
/// The payloads exchanged on the internal discovery topic
/// ([`discovery_topic`](crate::discovery_topic)). A dedicated enum distinct
/// from the chat [`Message`](crate::chat_core::Message) type, so discovery
/// traffic can never be confused with chat payloads and chat payloads are
/// never routed through the discovery topic.
///
/// Always available (no feature gate) so the wire format, roundtrips, and
/// version gate mirror [`discovery_topic`](crate::discovery_topic); the
/// separation-from-chat tests that need the chat type are gated on `net`.
pub mod discovery_message;

/// Versioned, typed control-plane message envelope (BORU-CP-01).
///
/// The hidden-discovery control plane (PDF Phase 1) — a compact, magic-
/// prefixed wire format for discovery metadata (HELLO / PRESENCE /
/// CAPABILITIES / DIAGNOSTIC_HINT) that can never be confused with a chat
/// message. The envelope is versioned and forward-compatible: unknown
/// message types and unknown payload fields are ignored safely, while a
/// strict decoder rejects malformed frames without touching the gossip
/// actor or chat processing.
///
/// Always available (no feature gate) so the wire format, roundtrips, and
/// strict-decoder tests run without the `net` feature; the
/// separation-from-chat tests that need the chat type are gated on `net`.
pub mod control_plane;

/// Independent runtime gates for optional roadmap features.
pub mod feature_gates;

/// Focused discovery modules extracted from
/// [`DiscoveryService`](crate::discovery_service::DiscoveryService)
/// (BORU-DISC-004..): each owns a single architectural concern with explicit
/// owned state, while `DiscoveryService` stays the facade/coordinator.
///
/// Always available (no feature gate) so the pure registry/dedup logic and
/// its tests run without the `net` feature, mirroring
/// [`discovery_topic`](crate::discovery_topic) and
/// [`discovery_message`](crate::discovery_message).
pub mod discovery;

/// Internal discovery subsystem — the service API for the hidden discovery
/// gossip topic.
///
/// Owns the discovery join, publish, receive-path dispatch, and the
/// in-process peer registry behind a dedicated service that never creates or
/// touches conversation state
/// ([`ConversationEntry`](crate::conversations::ConversationEntry) /
/// [`ConversationStore`](crate::conversations::ConversationStore)) and never
/// touches chat persistence or rendering. Receive-path logic is testable
/// without any network (feed postcard bytes into
/// [`DiscoveryService::handle_incoming`](crate::discovery_service::DiscoveryService::handle_incoming)).
#[cfg(feature = "net")]
pub mod discovery_service;

/// Local, privacy-preserving public-address and GeoIP resolution.  This module
/// is deliberately independent of rendering and presence broadcasting.
#[cfg(feature = "net")]
pub mod network_location;
/// Pure projection of active presence records for the Network Status map.
#[cfg(feature = "net")]
pub mod network_map;
/// Versioned, privacy-preserving support-bundle export.
#[cfg(feature = "net")]
pub mod support_bundle;
/// In-memory sender-side registry for direct file offers.
#[cfg(feature = "net")]
pub mod file_offer;

/// Dedicated versioned QUIC protocol for streaming announced direct file offers.
#[cfg(feature = "net")]
pub mod file_offer_protocol;

/// Per-room discovery secrets — cryptographically random 32-byte keys
/// that isolate private rooms on the DHT.
///
/// Always available (no feature gate) so that [`RoomStore`](crate::room::RoomStore) can
/// (de)serialize secrets without the `net` feature.
pub mod discovery_secret;

/// Private-room topic tracker — thin wrapper over [`TopicDiscoveryBackend`](crate::discovery_backend::TopicDiscoveryBackend)
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
/// The [`ChatCallbacks`](crate::chat_callbacks::ChatCallbacks) trait is the interface that frontend state structs
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

/// Bounded, persistent replay-marker store backing group event replay
/// protection (BORU-AUDIT-16).
#[cfg(feature = "net")]
pub mod group_replay;

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

/// Startup migration that removes the stale saved lobby conversation.
///
/// Older Boru versions auto-joined the canonical public lobby on startup and
/// persisted it like any other room: a [`ConversationEntry`](crate::conversations::ConversationEntry)
/// in the conversation store and per-topic message history. This migration
/// detects those persisted entries at startup and removes them without
/// touching unrelated public rooms (only the exact canonical lobby topic is
/// matched).
#[cfg(feature = "net")]
pub mod lobby_migration;

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

/// Canonical file-admission policy: size limits and extension allowlists.
///
/// The single authoritative implementation of the "may this file be
/// shared/announced?" gate.  All intake boundaries must call
/// [`file_policy::admission`] instead of re-implementing the rule inline
/// (BORU-AUDIT-20).
#[cfg(feature = "net")]
pub mod file_policy;

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
/// Always available (no feature gate).  Use [`Diagnostics`](crate::diagnostics::Diagnostics) to record
/// [`DiagnosticEvent`](crate::diagnostics::DiagnosticEvent)s and [`ReceivedProbe`](crate::diagnostics::ReceivedProbe)s.  Oldest entries are
/// automatically evicted when storage limits are exceeded.
pub mod diagnostics;

/// Relational storage layer with managed migrations.
pub mod storage;

/// Named-ring permission groups for file resources (iroh-rings borrow).
///
/// A ring is a named set of peers sharing typed Read/Write/Delete
/// permissions on file resources.  Persisted in SQLite via
/// [`crate::storage::Storage`] and enforced request-time in the
/// file-access handler.  Always available (no feature gate) so storage
/// migrations can reference the types unconditionally.
pub mod rings;
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
/// Always available (no feature gate).  Defines [`CatalogWireRequest`](crate::catalogue_protocol::CatalogWireRequest),
/// [`CatalogWireResponse`](crate::catalogue_protocol::CatalogWireResponse), inner [`CatalogRequest`](crate::catalogue_protocol::CatalogRequest)/[`CatalogResponse`](crate::catalogue_protocol::CatalogResponse)
/// enums, and wire-safe [`CatalogErrorCode`](crate::catalogue_protocol::CatalogErrorCode).
pub mod catalogue_protocol;

/// File access protocol — versioned request/response wire wrappers.
///
/// Always available (no feature gate).  Defines [`FileAccessWireRequest`](crate::file_access_protocol::FileAccessWireRequest),
/// [`FileAccessWireResponse`](crate::file_access_protocol::FileAccessWireResponse), inner [`FileAccessRequest`](crate::file_access_protocol::FileAccessRequest)/[`FileAccessResponse`](crate::file_access_protocol::FileAccessResponse)
/// types, and wire-safe [`FileAccessErrorCode`](crate::file_access_protocol::FileAccessErrorCode).
pub mod file_access_protocol;

// ── New modules (catalogue + file access) ────────────────────────────────────

/// Versioned wire-frame protocol helpers — `read_frame` / `write_frame`.
pub mod protocol_version;

/// Canonical signed-object framing shared by every Ed25519-authenticated
/// protocol object (BORU-AUDIT-27).
pub mod protocol_signing;

/// Central size and count limits for catalogue protocol traffic.
pub mod catalogue_limits;

/// Per-peer and global rate limiting for catalogue protocol connections.
pub mod catalogue_rate_limits;

/// Catalogue retrieval protocol handler — server side.
pub mod catalogue_handler;
pub mod catalogue_policy;
pub mod catalogue_wire;

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

/// Path containment helpers shared by download/export safety checks.
pub mod path_containment;

/// BlobTicket wormhole sharing — copy a ticket string to share a file outside
/// the friend graph; paste a ticket to receive.  Provides address-info
/// trimming (Id-only vs RelayAndAddresses) and a connect-based preflight.
#[cfg(feature = "net")]
pub mod ticket_share;

/// PAKE-style short-code file shares — a 7-character code that resolves to a
/// blob ticket on the sharing peer, with expiry and single-use replay
/// rejection, plus the signed announcement envelope broadcast over a
/// code-derived rendezvous gossip topic.
#[cfg(feature = "net")]
pub mod short_code;

/// Text sanitisation for safe display in the UI and logs.
///
/// Strips or replaces control characters, Unicode format characters
/// (bidi overrides, zero-width spaces, etc.), and truncates to a
/// reasonable length.  See the module docs for full details.
pub mod abuse_controls;

/// Human-friendly deterministic peer names derived from [`PublicKey`](iroh::PublicKey).
///
/// Provides [`generate_friendly_name`](crate::peer_names::generate_friendly_name) for stable adjective‑noun names
/// (e.g. "Blue Falcon") and [`fmt_truncated`](crate::peer_names::fmt_truncated) for short identifiers
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

/// KLIPY (external GIF search) configuration and API-key security.
///
/// Reads the provider API key from `KLIPY_API_KEY` at runtime; never
/// hardcodes, commits, logs, or transmits the key to peers.  Always
/// available (no feature gate) so the auth seam can be reused by any
/// GIF provider adapter.
pub mod klipy_config;

/// Group encryption — p2panda-based end-to-end encrypted group messaging.
///
/// Provides type bridges between iroh and p2panda cryptographic types,
/// newtype wrappers for peer/operation identities, and the scaffolding
/// for key management, message encryption, and membership tracking.
#[cfg(feature = "net")]
pub mod group_encryption;

/// SPAKE2 short-code pairing — a password-authenticated complement to the
/// QR/URI peer-invitation flow.  Both peers agree on a short numeric code;
/// the PAKE proves they share it, then each side authenticates its
/// [`PeerInvitation`](crate::peer_invitation::PeerInvitation) with the derived key and feeds it into
/// [`pairing_service::accept_peer_invitation`].
#[cfg(feature = "net")]
pub mod spake2_pairing;

/// Bounded blocking file hasher — wraps blake3 hashing in
/// [`tokio::task::spawn_blocking`] with configurable concurrency.
///
/// Always available (no feature gate).  Used by [`file_indexer`] and
/// [`file_access_handler`] to avoid blocking the async runtime with
/// synchronous file I/O and CPU-bound blake3 computation.
#[cfg(feature = "net")]
pub mod file_hasher;

/// Provider-neutral GIF domain model — the [`GifProvider`](crate::gif_provider::GifProvider) trait and its
/// neutral request/response types.
///
/// Always available (no feature gate) because it is pure data plus a
/// trait: no networking, no provider credentials.  Provider-specific
/// wire models live inside the adapter module that implements
/// [`GifProvider`](gif_provider::GifProvider).
pub mod gif_provider;

/// KLIPY GIF provider adapter — a concrete [`GifProvider`](crate::gif_provider::GifProvider) implementation
/// for the KLIPY HTTP API.
///
/// All KLIPY-specific wire models and request/response types stay inside
/// this module; nothing leaks into the neutral domain model or app code.
/// Gated behind `gui` because it uses the optional `reqwest`/`url`/
/// `serde_json` dependencies.
#[cfg(feature = "gui")]
pub mod klipy_provider;

/// Build the default configured GIF provider (currently KLIPY) as a
/// provider-neutral `Arc<dyn GifProvider>` trait object.
///
/// Re-exported at the crate root so UI code can obtain the configured
/// provider without importing provider-specific modules or types.  Returns
/// [`GifProviderError::NotConfigured`](gif_provider::GifProviderError::NotConfigured)
/// when no API key is configured.
#[cfg(feature = "gui")]
pub use klipy_provider::default_gif_provider;
