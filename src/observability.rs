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
//! | `debug!` | Success path details | "discovery returned no peers", "continuous publish succeeded", publication policy decisions |
//! | `info!` | Lifecycle transitions | Tracker start/shutdown, peers discovered, publish completed, background loop start/stop |
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
//! | Room discovery secret (raw 32-byte key) | Leaks the room's DHT signing credential. Even truncated values are avoided. | Log the truncated topic hash only. |
//! | Invitation strings (`boru1:...`) | Contains the base32-encoded discovery secret. | Log `topic` short prefix only. |
//! | Decrypted record content bytes | Even if the payload format changes, raw content bytes are never logged. | Log validation counters instead. |
//!
//! # Safe identifiers
//!
//! Use these consistently across all public-room tracking:
//!
//! | Identifier | Source | Example log field |
//! |------------|--------|-------------------|
//! | Room short ID | [`PublicRoomIdentity::short_id()`] | `room = "room-a1b2"` |
//! | Local peer (truncated) | [`EndpointId::fmt_short()`] | `local = "a1b2..."` |
//! | Remote peer (truncated) | [`EndpointId::fmt_short()`] | (via caller; remote peers are not logged directly by the tracker) |
//! | Private room topic (truncated) | `hex::encode(&topic.as_bytes()[..4])` | `topic = "ab12"` |
//! | Private room namespace (truncated) | `hex::encode(&namespace.as_bytes()[..4])` | `namespace = "cd34"` |
//!
//! # Logged events checklist
//!
//! Every private-room lifecycle event should be traceable.  The table below
//! documents where each event is logged and at what level.
//!
//! | Event | Level | Module/Function |
//! |-------|-------|-----------------|
//! | Tracker construction | `info!` | [`PrivateRoomTracker::new()`](crate::private_room_tracker::PrivateRoomTracker::new) |
//! | Tracker shutdown | `info!` | [`PrivateRoomTracker::shutdown()`](crate::private_room_tracker::PrivateRoomTracker::shutdown) |
//! | Publish attempt outcome + duration | `info!` / `warn!` | [`PrivateRoomTracker::publish_once()`](crate::private_room_tracker::PrivateRoomTracker::publish_once) |
//! | Discovery lookup outcome + counters | `info!` / `debug!` | [`PrivateRoomTracker::discover_once()`](crate::private_room_tracker::PrivateRoomTracker::discover_once) |
//! | Per-record validation rejection | `trace!` | [`filter_and_build()`](crate::discovery_validation::DiscoveryRecordValidator::filter_and_build) |
//! | Continuous startup | `info!` | [`PrivateContinuousTracker::start()`](crate::private_room_tracker::PrivateContinuousTracker::start) |
//! | Continuous shutdown | `info!` | [`PrivateContinuousTracker::shutdown()`](crate::private_room_tracker::PrivateContinuousTracker::shutdown) |
//! | Continuous publish success/failure | `debug!` / `warn!` | [`private_publish_loop()`](crate::private_room_tracker) |
//! | Continuous publish skipped/decided | `debug!` | [`private_publish_loop()`](crate::private_room_tracker) (policy decision + reason) |
//! | Continuous publish degraded DHT | `warn!` | [`private_publish_loop()`](crate::private_room_tracker) (3+ consecutive failures) |
//! | Continuous discover loop started | `info!` | [`private_discover_loop()`](crate::private_room_tracker) |
//! | Continuous discover new peers | `info!` | [`private_discover_loop()`](crate::private_room_tracker) (with cumulative + known counts) |
//! | Continuous discover no peers | (silent — loop continues) | [`private_discover_loop()`](crate::private_room_tracker) |
//! | Continuous discover all known | (silent — loop continues) | [`private_discover_loop()`](crate::private_room_tracker) |
//! | Continuous discover degraded DHT | `warn!` | [`private_discover_loop()`](crate::private_room_tracker) (3+ consecutive failures) |
//! | Candidate admitted for join | `trace!` | [`private_discover_loop()`](crate::private_room_tracker) |
//! | Stale peer eviction | `trace!` | [`private_discover_loop()`](crate::private_room_tracker) (stale-ttl eviction count) |
//! | Retry/backoff step | `debug!` | [`retry_with_backoff()`](crate::public_room_continuous) |
//! | Backend publish operation span | `info` span | `tracker.publish` (public/private) |
//! | Backend lookup operation span | `info` span | `tracker.lookup` (public/private) |
//!
//! # Adding new tracing
//!
//! When adding a new trace event:
//!
//! 1. Choose the appropriate level from the table above.
//! 2. Use `topic = %tracker.topic_short()` for the private-room identifier.
//! 3. Use [`fmt_short()`] for any [`EndpointId`] you include.
//! 4. Never format the full discovery key, secret, invitation, or record payload.
//! 5. Use counters, not payload contents (`accepted = 3`, not `peers = [id1, id2, id3]`).
//! 6. For private-room events, always use truncated hex identifiers, never raw bytes.
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
