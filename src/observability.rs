//! Observability conventions, redaction rules, and tracing guidelines.
//!
//! This module documents what is logged, what is never logged, and how to
//! safely add tracing without leaking sensitive data.  It is intentionally
//! a documentation-only module — runtime observability lives in the other
//! public-room modules.
//!
//! # Tracing levels
//!
//! | Level | When | Example |
//! |-------|------|---------|
//! | `trace!` | Per-record decisions | Rejection reason for each discovery record |
//! | `debug!` | Success path details | "discovery returned no peers", "continuous publish succeeded" |
//! | `info!` | Lifecycle transitions | Tracker start/shutdown, peers discovered, publish completed |
//! | `warn!` | Recoverable failures | Publish or discovery failure after retries, degraded DHT state |
//! | `error!` | Unrecoverable errors | Backend shutdown failure (currently no `error!` in public-room code) |
//!
//! # Redaction rules
//!
//! The following data is **never** written to logs:
//!
//! | Data | Rationale | Safe alternative |
//! |------|-----------|-----------------|
//! | Full discovery key (32-byte topic hash) | Although technically public (derived from room name), logging it leaks the room's DHT namespace to anyone with log access. | [`short_id()`] — the first 4 hex characters of the topic hash (`room-{prefix}`). |
//! | Decrypted record payloads (`DiscoveryRecordPayload`) | Payloads carry the publisher's [`EndpointId`] which is public, but the principle is zero payload content in logs. | Counters only: number of accepted/rejected records, not their contents. |
//! | Full invitation or ticket material | Tickets contain connection secrets. | Log the room identity's [`short_id()`] instead. |
//! | Unnecessary full [`EndpointId`] values | Full peer IDs (32 bytes) are public, but verbose in logs. | [`EndpointId::fmt_short()`] — a compact truncated representation. |
//!
//! # Safe identifiers
//!
//! Use these consistently across all public-room tracing:
//!
//! | Identifier | Source | Example log field |
//! |------------|--------|-------------------|
//! | Room short ID | [`PublicRoomIdentity::short_id()`] | `room = "room-a1b2"` |
//! | Local peer (truncated) | [`EndpointId::fmt_short()`] | `local = "a1b2..."` |
//! | Remote peer (truncated) | [`EndpointId::fmt_short()`] | (via caller; remote peers are not logged directly by the tracker) |
//!
//! # Logged events checklist
//!
//! Every public-room lifecycle event should be traceable.  The table below
//! documents where each event is logged and at what level.
//!
//! | Event | Level | Module/Function |
//! |-------|-------|-----------------|
//! | Public room network selection | `info!` | [`PublicRoomTracker::start()`] — the network is implied by identity derivation |
//! | Tracker startup | `info!` | [`PublicRoomTracker::start()`] |
//! | Tracker shutdown | `info!` | [`PublicRoomTracker::shutdown()`] |
//! | Publish attempt outcome + duration | `info!` / `warn!` | [`PublicRoomTracker::publish_once()`] |
//! | Discovery lookup outcome + counters | `info!` / `debug!` | [`PublicRoomTracker::discover_once()`] |
//! | Per-record validation rejection | `trace!` | [`filter_and_build()`](crate::discovery_validation::DiscoveryRecordValidator::filter_and_build) |
//! | Continuous tracker startup | (implied by `ContinuousTracker::start`) | [`public_room_continuous`] |
//! | Continuous tracker shutdown | `info!` | [`ContinuousTracker::shutdown()`](crate::public_room_continuous::ContinuousTracker::shutdown) |
//! | Continuous publish success/failure | `debug!` / `warn!` | [`publish_loop()`](crate::public_room_continuous) |
//! | Continuous publish degraded DHT | `warn!` | [`publish_loop()`](crate::public_room_continuous) (3+ consecutive failures) |
//! | Continuous discover new peers | `info!` | [`discover_loop()`](crate::public_room_continuous) |
//! | Continuous discover no peers | `debug!` | [`discover_loop()`](crate::public_room_continuous) |
//! | Continuous discover all known | `trace!` | [`discover_loop()`](crate::public_room_continuous) |
//! | Continuous discover degraded DHT | `warn!` | [`discover_loop()`](crate::public_room_continuous) (3+ consecutive failures) |
//! | Retry/backoff step | `debug!` | [`retry_with_backoff()`](crate::public_room_continuous) |
//! | Room leave | `info!` | [`PublicRoomTracker::shutdown()`](crate::public_room_tracker::PublicRoomTracker::shutdown) |
//!
//! # Adding new tracing
//!
//! When adding a new trace event:
//!
//! 1. Choose the appropriate level from the table above.
//! 2. Use the `room` field with [`short_id()`] for the room identifier.
//! 3. Use [`fmt_short()`] for any [`EndpointId`] you include.
//! 4. Never format the full discovery key or record payload.
//! 5. Use counters, not payload contents (`accepted = 3`, not `peers = [id1, id2, id3]`).
//!
//! [`short_id()`]: crate::public_room::PublicRoomIdentity::short_id
//! [`EndpointId::fmt_short()`]: https://docs.rs/iroh/latest/iroh/struct.PublicKey.html#method.fmt_short
//! [`PublicRoomIdentity::short_id()`]: crate::public_room::PublicRoomIdentity::short_id
//! [`PublicRoomTracker::start()`]: crate::public_room_tracker::PublicRoomTracker::start
//! [`PublicRoomTracker::shutdown()`]: crate::public_room_tracker::PublicRoomTracker::shutdown
//! [`PublicRoomTracker::publish_once()`]: crate::public_room_tracker::PublicRoomTracker::publish_once
//! [`PublicRoomTracker::discover_once()`]: crate::public_room_tracker::PublicRoomTracker::discover_once
//! [`public_room_continuous`]: crate::public_room_continuous
//! [`fmt_short()`]: https://docs.rs/iroh/latest/iroh/struct.PublicKey.html#method.fmt_short
