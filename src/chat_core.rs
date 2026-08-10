//! Shared chat core — reusable state machine, protocol types, and network event handling.
//!
//! This module is a thin composition surface over responsibility-scoped
//! submodules (BORU-AUDIT-23):
//!
//! - [`protocol`](crate::chat_core::protocol) — pure wire types (`Message`, `SignedMessage`, `Ticket`,
//!   `RoomInvitation`, `NetEvent`) and codec helpers.
//! - [`composer`](crate::chat_core::composer), [`entries`](crate::chat_core::entries), [`status`](crate::chat_core::status), [`state`](crate::chat_core::state) — UI-free state types
//!   (`Composer`, `ChatEntry`, `StatusContext`, `AppState`).
//! - [`dedup`](crate::chat_core::dedup) — transport deduplication and signed-payload cache.
//! - [`net_event`](crate::chat_core::net_event) — network event processing (`handle_net_event`,
//!   `forward_gossip_events`, public-room safety filter).
//! - [`downloads`](crate::chat_core::downloads) — blob transfer execution with progress reporting.
//! - [`bootstrap`](crate::chat_core::bootstrap), [`util`](crate::chat_core::util) — bootstrap peer resolution and small helpers.
//!
//! Everything is re-exported at this level so existing import paths
//! (`iroh_gossip::chat_core::Message`, `chat_core::handle_net_event`, …)
//! keep working unchanged.  The module has **no** terminal/ratatui/crossterm
//! dependencies, making it usable from any frontend (TUI, GUI, headless).
//!
//! The [`ChatCallbacks`](crate::chat_callbacks::ChatCallbacks) trait is defined in [`crate::chat_callbacks`].

pub mod atomic_write;
pub mod bootstrap;
pub mod composer;
pub mod dedup;
pub mod downloads;
pub mod entries;
pub mod friend_ping;
pub mod net_event;
pub mod protocol;
pub mod state;
pub mod status;
pub mod util;

use std::sync::LazyLock;

use crate::diagnostics::Diagnostics;
use crate::transfer_telemetry::TransferTelemetry;

/// Global diagnostics store for recording network events and probes.
///
/// Lazily initialised on first access with default capacities
/// (5 000 events, 1 000 received probes).
pub static DIAGNOSTICS: LazyLock<Diagnostics> = LazyLock::new(Diagnostics::new);

/// Global transfer lifecycle telemetry store.
///
/// Lazily initialised on first access.  Wraps the shared [`DIAGNOSTICS`]
/// store and adds per-transfer sequence tracking.
pub static TRANSFER_TELEMETRY: LazyLock<TransferTelemetry> =
    LazyLock::new(|| TransferTelemetry::new(DIAGNOSTICS.clone()));

/// Re-export the callback trait for convenience — existing import paths
/// (`iroh_gossip::chat_core::ChatCallbacks`) continue to work.
pub use crate::chat_callbacks::ChatCallbacks;
pub use crate::chat_callbacks::{TransferId, TransferKind, TransferProgress};

/// Pure protocol/wire types, extracted to [`protocol`] so the
/// codec can be tested without network or storage.  Re-exported so existing
/// import paths (`iroh_gossip::chat_core::Message`) keep working.
pub use protocol::{
    message_hash, sign_advertisement, verify_advertisement, Hash, Message, MessageHash, NetEvent,
    RoomAdvertisement, RoomInvitation, RoomInviteV2, SharedFileMeta, SignedMessage, Ticket,
    ROOM_ADVERTISEMENT_PROTOCOL, ROOM_ADVERTISEMENT_VERSION, SIGNED_MESSAGE_PROTOCOL,
    SIGNED_MESSAGE_VERSION, DEFAULT_MESSAGE_TTL,
};

/// UI/state types, extracted to submodules so the state machine can be tested
/// without network or storage.  Re-exported for existing import paths.
pub use composer::Composer;
pub use entries::{ChatEntry, ChatKind};
pub use state::AppState;
pub use status::{ConnectionType, MeshHealth, StatusContext};

/// Transport deduplication and signed-payload cache, extracted to [`dedup`].
/// The cache functions are part of the public API; the shared statics stay
/// `pub(crate)` and are re-exported so `chat_core`'s net-event handler and
/// tests can keep reaching them.
pub use dedup::{get_signed_message, remember_signed_message, take_signed_message};
pub(crate) use dedup::{prune_seen_messages, DEDUP_SWEEP_THRESHOLD, DIAGNOSTIC_SEEN_MESSAGES, SEEN_MESSAGES};

/// Network event processing and the gossip→[`NetEvent`] bridge, extracted to
/// [`net_event`].  Re-exported for existing import paths.
pub use net_event::{
    broadcast_diagnostic_probe, check_peer_connection_type, filter_net_event_with_safety,
    forward_gossip_events, forward_gossip_events_with_safety, handle_net_event,
    handle_net_event_for_topic, handle_net_event_with_safety,
    handle_net_event_with_safety_for_topic, now_ms, now_secs, update_connection_counts,
};

/// Blob transfer execution, extracted to [`downloads`].
/// Re-exported for existing import paths; the internals stay `pub(crate)` so
/// `chat_core`'s tests can exercise them directly.
pub use downloads::{
    download_blob_to_file, download_blob_with_progress, download_blob_with_safety,
    download_candidates,
};

/// Bootstrap peer resolution, extracted to [`bootstrap`].
pub use bootstrap::{
    collect_bootstrap_peers, merge_bootstrap_peer_addrs, refresh_bootstrap_peers,
    seed_memory_lookup,
};

/// Shared formatting helpers, extracted to [`util`].
pub use util::fmt_relay_mode;

/// Unit tests for the chat core — kept in a separate file so `chat_core.rs`
/// stays a thin composition module.  `super` still resolves to `chat_core`,
/// so the tests exercise the exact same items as before the split.
#[cfg(test)]
mod tests;
