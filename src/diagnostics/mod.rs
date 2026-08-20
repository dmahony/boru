//! Core diagnostics — bounded event and probe storage for Boru.
//!
//! Provides a thread-safe [`Diagnostics`](crate::diagnostics::Diagnostics) singleton that records
//! [`DiagnosticEvent`](crate::diagnostics::DiagnosticEvent)s and [`ReceivedProbe`](crate::diagnostics::ReceivedProbe)s with bounded capacity.
//! Oldest records are automatically evicted when limits are exceeded.
//!
//! # Event types
//!
//! See [`DiagnosticEventKind`](crate::diagnostics::DiagnosticEventKind) for all supported event variants, including
//! the extended lifecycle stages (discovery, address lookup, connection,
//! subscription, probes).
//!
//! # Catalogue lifecycle event contract
//!
//! The catalogue protocol emits seven structured events that trace a
//! catalogue advertisement from receipt through fetch, verification,
//! and local installation.
//!
//! | Event | Required fields | Optional fields | Description |
//! |---|---|---|---|
//! | [`DiagnosticEventKind::CatalogueNoticeReceived`](crate::diagnostics::DiagnosticEventKind::CatalogueNoticeReceived) | — | `known_revision` | A remote peer advertised a catalogue (e.g. via gossip). Peer identity is carried by the `record_with_peer` context. |
//! | [`DiagnosticEventKind::CatalogueFetchStarted`](crate::diagnostics::DiagnosticEventKind::CatalogueFetchStarted) | — | `known_revision` | A fetch connection to a remote peer was initiated. |
//! | [`DiagnosticEventKind::CatalogueFetchCompleted`](crate::diagnostics::DiagnosticEventKind::CatalogueFetchCompleted) | `revision`, `file_count`, `collection_count` | — | A fetch completed successfully with a validated response. |
//! | [`DiagnosticEventKind::CatalogueSignatureRejected`](crate::diagnostics::DiagnosticEventKind::CatalogueSignatureRejected) | `error` | — | The fetched catalogue failed signature or owner-id verification. |
//! | [`DiagnosticEventKind::CatalogueFetchFailed`](crate::diagnostics::DiagnosticEventKind::CatalogueFetchFailed) | `error` | — | The fetch failed (timeout, connection error, protocol violation). |
//! | [`DiagnosticEventKind::CatalogueRevisionInstalled`](crate::diagnostics::DiagnosticEventKind::CatalogueRevisionInstalled) | `revision`, `file_count`, `collection_count` | — | A validated catalogue revision was persisted to local storage. |
//! | [`DiagnosticEventKind::CatalogueCachedDataUsed`](crate::diagnostics::DiagnosticEventKind::CatalogueCachedDataUsed) | `cached_revision` | — | Local cached data was served instead of a remote fetch (cache hit or known-revision match). |
//!
//! None of these events carry secrets, full catalogue contents, or raw
//! error internals.  All peer identities are provided via the caller's
//! `record_with_peer` parameter, not embedded in the event payload.
//!
//! # Peer state
//!
//! [`PeerDiagnosticState`](crate::diagnostics::PeerDiagnosticState) tracks the per-peer diagnostic lifecycle — what
//! stage each peer has reached.  The [`classify_discovery_test`](crate::diagnostics::classify_discovery_test) function
//! produces a structured failure classification from the collected evidence.
//!
//! # Probe types
//!
//! [`ReceivedProbe`](crate::diagnostics::ReceivedProbe) tracks probes received from peers with full metadata
//! (latency, message hash, duplicate count).  [`DiagnosticProbe`](crate::diagnostics::DiagnosticProbe) is the
//! wire-format probe sent through the gossip mesh.

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, LazyLock, Mutex};

use chrono::{DateTime, Utc};
use iroh_base::PublicKey;
use serde::{Deserialize, Serialize};

use crate::control_plane::advertisement::RoomVisibility;
use crate::TopicId;

// ── Submodules ─────────────────────────────────────────────
mod counters;
#[cfg(test)]
mod counters_tests;
mod events;
mod gui;
mod probes;
mod reporting;
mod safety;
mod snapshots;
mod store;
#[cfg(test)]
mod tests;

// ── Re-exports (public facade, BORU-CORE-002) ──────────────
pub use counters::*;
pub use events::*;
pub use gui::*;
pub use probes::*;
pub use reporting::*;
pub use safety::{redact_endpoint, redact_error, redact_payload};
pub use snapshots::*;
pub use store::*;
