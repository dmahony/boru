//! BORU-CP-13: structured per-peer diagnostics (PDF Phase 5, Task 5.1).
//!
//! Builds a share-safe [`PeerDiagnosticsSnapshot`] for every tracked peer
//! so a developer can tell whether a networking failure is in
//! **discovery**, **endpoint connectivity**, **topic join /
//! subscription**, **gossip**, **decoding**, or **application delivery** —
//! without relying on generic "connected" logs.
//!
//! The snapshot is derived from the BORU-CP-05 connectivity state machine
//! ([`PeerConnectivityStore`]), which is fed only from real networking
//! events, plus the BORU-CP-13 timestamp-only events
//! ([`ConnectivityEvent::DirectMessageSent`],
//! [`ConnectivityEvent::InboundGossipEvent`],
//! [`ConnectivityEvent::ApplicationMessageDecoded`]).
//!
//! # Share-safety
//!
//! Every field is safe to paste into a bug report after normal review:
//! peer ids are truncated (`fmt_short`), the direct-topic id is only a
//! short hash prefix, errors are free-form strings that never contain
//! message content, and no secret keys, private tokens, message contents,
//! or payload bytes are ever stored here. The discovery topic itself never
//! carries chat payloads (control-plane rule), so a snapshot can never leak
//! chat contents by construction.
//!
//! # Stages
//!
//! | Stage | Field | Source |
//! |-------|-------|--------|
//! | Discovery | [`PeerDiagnosticsSnapshot::discovery_last_seen_ms`] | `DiscoverySeen` event |
//! | Endpoint | [`PeerDiagnosticsSnapshot::endpoint`] / [`PeerDiagnosticsSnapshot::endpoint_observed_ms`] | connectivity state |
//! | Path | [`PeerDiagnosticsSnapshot::path_kind`] / [`PeerDiagnosticsSnapshot::relay_involved`] | `PathChangedDirect` / `PathChangedRelay` |
//! | Direct topic | [`PeerDiagnosticsSnapshot::direct_topic_id_prefix`] / [`PeerDiagnosticsSnapshot::topic_join_status`] / [`PeerDiagnosticsSnapshot::subscription_ready`] | `TopicJoined` / `TopicJoinFailed` + deterministic `direct_topic()` |
//! | Outbound broadcast | [`PeerDiagnosticsSnapshot::last_outbound_direct_ms`] | `DirectMessageSent` |
//! | Inbound gossip | [`PeerDiagnosticsSnapshot::last_inbound_gossip_ms`] | `InboundGossipEvent` |
//! | Decode / delivery | [`PeerDiagnosticsSnapshot::last_decoded_message_ms`] / [`PeerDiagnosticsSnapshot::last_inbound_direct_ms`] | `ApplicationMessageDecoded` / `DirectMessageReceived` |
//! | Errors | [`PeerDiagnosticsSnapshot::last_error`] / [`PeerDiagnosticsSnapshot::last_error_stage`] | `EndpointFailed` / `TopicJoinFailed` (+ trail) |

use std::fmt;
use std::time::Instant;

use iroh_base::PublicKey;
use serde::{Deserialize, Serialize};

use super::connectivity::{
    ConnectivityEvent, DirectTopicState, PathKind, PeerConnectivityEntry, PeerConnectivityState,
    PeerConnectivityStore,
};
use crate::contact::direct_topic;

/// One entry of the bounded transition trail, rendered with stable labels
/// and an elapsed duration (most recent first in the snapshot).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TrailSnapshotEntry {
    /// Elapsed milliseconds since this transition happened.
    pub elapsed_ms: u64,
    /// Stable label of the state before the transition.
    pub from: String,
    /// Stable label of the state after the transition.
    pub to: String,
    /// Stable label of the event that caused the transition.
    pub event: String,
}

/// BORU-CP-13: a structured, share-safe per-peer diagnostic snapshot.
///
/// Every stage from PDF Task 5.1 is represented with an elapsed duration
/// (or a stable state label when the stage has not been reached). The
/// snapshot is deliberately safe to share in bug reports: peer ids are
/// truncated, the direct-topic id is only a short hash prefix, errors are
/// free-form strings that never contain message content, and no secret
/// keys / tokens / payloads are ever stored here.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PeerDiagnosticsSnapshot {
    /// Short stable peer id (`fmt_short`), safe to share.
    pub peer_id: String,
    /// Connectivity state-machine label: `unknown | discovered | connecting
    /// | reachable | direct-topic-ready | degraded | offline-stale`.
    pub state: String,
    /// Elapsed ms since the peer was last seen on the discovery topic, or
    /// `None` when it has never been seen there.
    pub discovery_last_seen_ms: Option<u64>,
    /// Endpoint connectivity stage label derived from the state machine:
    /// `not-started | connecting | connected | failed | disconnected`.
    pub endpoint: String,
    /// Elapsed ms since the endpoint was last observed (any connectivity
    /// event), or `None` when the peer is not tracked yet.
    pub endpoint_observed_ms: Option<u64>,
    /// Current path kind: `unknown | direct | relay | transitioning`.
    ///
    /// Diagnostic/optimization metadata only (BORU-CP-14): it never proves
    /// application-level success, and a relay path is still considered
    /// reachable.
    pub path_kind: String,
    /// Whether a relay server is currently involved (`path_kind == "relay"`).
    pub relay_involved: bool,
    /// Short hex prefix (first 5 bytes / 10 hex chars) of the deterministic
    /// direct topic id, for correlating logs. Derived from both public keys,
    /// never a secret.
    pub direct_topic_id_prefix: Option<String>,
    /// Direct-topic join status: `not_attempted | ready | failed`.
    pub topic_join_status: String,
    /// Whether the direct topic is subscribed/ready for this peer.
    pub subscription_ready: bool,
    /// Elapsed ms since the last outbound direct broadcast to this peer, or
    /// `None` when none has been recorded.
    pub last_outbound_direct_ms: Option<u64>,
    /// Elapsed ms since the last inbound gossip event from this peer, or
    /// `None` when none has been recorded.
    pub last_inbound_gossip_ms: Option<u64>,
    /// Elapsed ms since the last successfully decoded application message
    /// from this peer, or `None` when none has been recorded.
    pub last_decoded_message_ms: Option<u64>,
    /// Elapsed ms since the last direct (non-discovery) message arrived from
    /// this peer, or `None` when none has been recorded.
    pub last_inbound_direct_ms: Option<u64>,
    /// Last recorded failure (dial / topic / path), if any. Sanitised —
    /// never contains message content or secrets.
    pub last_error: Option<String>,
    /// The stage at which the last failure occurred, if known:
    /// `endpoint | topic-join | unknown`.
    pub last_error_stage: Option<String>,
    /// Bounded transition trail (most recent first) with elapsed durations.
    pub trail: Vec<TrailSnapshotEntry>,
}

impl PeerDiagnosticsSnapshot {
    /// Build a snapshot for one tracked peer entry.
    ///
    /// `local_node` is the local public key, used to derive the
    /// deterministic direct topic id; `now` anchors every elapsed duration.
    pub fn from_entry(
        entry: &PeerConnectivityEntry,
        local_node: &PublicKey,
        now: Instant,
    ) -> Self {
        let state = entry.state;
        let endpoint = endpoint_label(state);
        let path_kind = path_label(entry.path_kind);
        let topic_join_status = topic_join_label(entry.direct_topic_state);
        let relay_involved = entry.path_kind == PathKind::Relay;
        let subscription_ready = entry.direct_topic_state == DirectTopicState::Ready;

        // The deterministic direct topic exists for every (local, peer)
        // key pair; the prefix is a share-safe correlator, never a secret.
        let direct_topic_id_prefix = Some(direct_topic(local_node, &entry.peer_id).fmt_short());

        let last_error_stage = entry
            .trail
            .iter()
            .rev()
            .find_map(|r| match r.event {
                ConnectivityEvent::EndpointFailed => Some("endpoint"),
                ConnectivityEvent::TopicJoinFailed => Some("topic-join"),
                _ => None,
            })
            .map(str::to_string)
            .or_else(|| {
                if entry.last_error.is_some() {
                    Some("unknown".to_string())
                } else {
                    None
                }
            });

        let trail = entry
            .trail
            .iter()
            .rev()
            .map(|r| TrailSnapshotEntry {
                elapsed_ms: now.saturating_duration_since(r.at).as_millis() as u64,
                from: r.from.label().to_string(),
                to: r.to.label().to_string(),
                event: r.event.label().to_string(),
            })
            .collect();

        Self {
            peer_id: entry.peer_id.fmt_short().to_string(),
            state: state.label().to_string(),
            discovery_last_seen_ms: elapsed_ms(entry.discovery_last_seen, now),
            endpoint: endpoint.to_string(),
            endpoint_observed_ms: elapsed_ms(Some(entry.last_seen), now),
            path_kind: path_kind.to_string(),
            relay_involved,
            direct_topic_id_prefix,
            topic_join_status: topic_join_status.to_string(),
            subscription_ready,
            last_outbound_direct_ms: elapsed_ms(entry.last_outbound_direct, now),
            last_inbound_gossip_ms: elapsed_ms(entry.last_inbound_gossip, now),
            last_decoded_message_ms: elapsed_ms(entry.last_decoded_message, now),
            last_inbound_direct_ms: elapsed_ms(entry.last_inbound_direct, now),
            last_error: entry.last_error.clone(),
            last_error_stage,
            trail,
        }
    }

    /// Render the snapshot as a single stable, tab-separated line suitable
    /// for debug logs and side-by-side comparison of two machines.
    pub fn render(&self) -> String {
        format!(
            "peer={} state={} discovery={} endpoint={} endpoint_obs={} path={} relay={} \
             topic={} topic_prefix={} subscribed={} outbound={} inbound_gossip={} decoded={} \
             inbound_direct={} error_stage={} error={}",
            self.peer_id,
            self.state,
            render_elapsed(self.discovery_last_seen_ms),
            self.endpoint,
            render_elapsed(self.endpoint_observed_ms),
            self.path_kind,
            self.relay_involved,
            self.topic_join_status,
            self.direct_topic_id_prefix
                .as_deref()
                .unwrap_or("none"),
            self.subscription_ready,
            render_elapsed(self.last_outbound_direct_ms),
            render_elapsed(self.last_inbound_gossip_ms),
            render_elapsed(self.last_decoded_message_ms),
            render_elapsed(self.last_inbound_direct_ms),
            self.last_error_stage.as_deref().unwrap_or("none"),
            self.last_error.as_deref().unwrap_or("none"),
        )
    }
}

impl fmt::Display for PeerDiagnosticsSnapshot {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.render())
    }
}

/// Build share-safe snapshots for every tracked peer, sorted by peer id for
/// stable side-by-side comparison.
pub fn snapshots_for(
    store: &PeerConnectivityStore,
    local_node: &PublicKey,
    now: Instant,
) -> Vec<PeerDiagnosticsSnapshot> {
    let mut out: Vec<_> = store
        .peers()
        .map(|(_, entry)| PeerDiagnosticsSnapshot::from_entry(entry, local_node, now))
        .collect();
    out.sort_by(|a, b| a.peer_id.cmp(&b.peer_id));
    out
}

// ---------------------------------------------------------------------------
// Internals
// ---------------------------------------------------------------------------

/// Stable endpoint stage label derived from the connectivity state.
fn endpoint_label(state: PeerConnectivityState) -> &'static str {
    match state {
        PeerConnectivityState::Unknown | PeerConnectivityState::Discovered => "not-started",
        PeerConnectivityState::Connecting => "connecting",
        PeerConnectivityState::Reachable | PeerConnectivityState::DirectTopicReady => "connected",
        PeerConnectivityState::Degraded => "failed",
        PeerConnectivityState::OfflineStale => "disconnected",
    }
}

/// Stable path-kind label.
fn path_label(kind: PathKind) -> &'static str {
    match kind {
        PathKind::Unknown => "unknown",
        PathKind::Direct => "direct",
        PathKind::Relay => "relay",
        PathKind::Transitioning => "transitioning",
    }
}

/// Stable direct-topic join status label.
fn topic_join_label(state: DirectTopicState) -> &'static str {
    match state {
        DirectTopicState::NotAttempted => "not_attempted",
        DirectTopicState::Ready => "ready",
        DirectTopicState::Failed => "failed",
    }
}

/// Elapsed milliseconds since `since`, or `None` when never observed.
fn elapsed_ms(since: Option<Instant>, now: Instant) -> Option<u64> {
    since.map(|t| now.saturating_duration_since(t).as_millis() as u64)
}

/// Human-friendly elapsed rendering: `never` or `123ms` / `1.2s` / `5m`.
fn render_elapsed(ms: Option<u64>) -> String {
    match ms {
        None => "never".to_string(),
        Some(ms) if ms < 1000 => format!("{ms}ms"),
        Some(ms) if ms < 60_000 => format!("{:.1}s", ms as f64 / 1000.0),
        Some(ms) => format!("{}m{}s", ms / 60_000, (ms % 60_000) / 1000),
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;
    use crate::control_plane::connectivity::{ConnectivityEvent as E, PeerConnectivityStore};

    fn key(byte: u8) -> PublicKey {
        let mut seed = [0u8; 32];
        seed[0] = byte;
        iroh_base::SecretKey::from_bytes(&seed).public()
    }

    /// A fully-healthy peer exposes every stage with an elapsed duration and
    /// the direct topic derived from both keys.
    #[test]
    fn healthy_peer_snapshot_has_all_stages() {
        let local = key(0x01);
        let peer = key(0x02);
        let t0 = Instant::now();
        let mut store = PeerConnectivityStore::new();

        store.apply(peer, E::DiscoverySeen, t0);
        store.apply(peer, E::EndpointConnecting, t0 + Duration::from_millis(1));
        store.apply(peer, E::EndpointConnected, t0 + Duration::from_millis(2));
        store.apply(peer, E::PathChangedDirect, t0 + Duration::from_millis(3));
        store.apply(peer, E::TopicJoined, t0 + Duration::from_millis(4));
        store.apply(peer, E::DirectMessageSent, t0 + Duration::from_millis(5));
        store.apply(peer, E::InboundGossipEvent, t0 + Duration::from_millis(6));
        store.apply(peer, E::ApplicationMessageDecoded, t0 + Duration::from_millis(7));
        store.apply(peer, E::DirectMessageReceived, t0 + Duration::from_millis(8));

        let now = t0 + Duration::from_millis(10_000);
        let snap = PeerDiagnosticsSnapshot::from_entry(store.get(&peer).unwrap(), &local, now);

        assert_eq!(snap.state, "direct-topic-ready");
        assert_eq!(snap.endpoint, "connected");
        assert_eq!(snap.path_kind, "direct");
        assert!(!snap.relay_involved);
        assert_eq!(snap.topic_join_status, "ready");
        assert!(snap.subscription_ready);

        // discovery seen at t0 → ~10s elapsed
        assert_eq!(snap.discovery_last_seen_ms, Some(10_000));
        assert_eq!(snap.last_outbound_direct_ms, Some(10_000 - 5));
        assert_eq!(snap.last_inbound_gossip_ms, Some(10_000 - 6));
        assert_eq!(snap.last_decoded_message_ms, Some(10_000 - 7));
        assert_eq!(snap.last_inbound_direct_ms, Some(10_000 - 8));
        assert_eq!(snap.last_error, None);
        assert_eq!(snap.last_error_stage, None);

        // Direct topic prefix is deterministic and derived from both keys.
        let expected = direct_topic(&local, &peer).fmt_short();
        assert_eq!(snap.direct_topic_id_prefix.as_deref(), Some(expected.as_str()));

        // Trail is rendered newest-first with stable labels. Only the four
        // real transitions are recorded — PathChangedDirect (from Reachable)
        // and DirectMessageReceived (from DirectTopicReady) are idempotent
        // no-ops that refresh hints/timestamps without trail records.
        assert_eq!(snap.trail.len(), 4);
        assert_eq!(snap.trail[0].event, "topic-joined");
        assert_eq!(snap.trail[3].event, "discovery-seen");
    }

    /// A peer stuck at discovery exposes the failure clearly: no endpoint,
    /// no topic, no subscription readiness, no inbound traffic.
    #[test]
    fn discovery_only_peer_is_not_subscription_ready() {
        let local = key(0x03);
        let peer = key(0x04);
        let t0 = Instant::now();
        let mut store = PeerConnectivityStore::new();

        store.apply(peer, E::DiscoverySeen, t0);

        let snap = PeerDiagnosticsSnapshot::from_entry(store.get(&peer).unwrap(), &local, t0);

        assert_eq!(snap.state, "discovered");
        assert_eq!(snap.endpoint, "not-started");
        assert_eq!(snap.topic_join_status, "not_attempted");
        assert!(!snap.subscription_ready);
        assert_eq!(snap.last_outbound_direct_ms, None);
        assert_eq!(snap.last_inbound_gossip_ms, None);
        assert_eq!(snap.last_decoded_message_ms, None);
        assert!(snap.direct_topic_id_prefix.is_some(), "topic prefix is derivable");
    }

    /// A topic-join failure is visible as `failed` + a stage-labelled error.
    #[test]
    fn topic_join_failure_is_stage_labelled() {
        let local = key(0x05);
        let peer = key(0x06);
        let t0 = Instant::now();
        let mut store = PeerConnectivityStore::new();

        store.apply(peer, E::DiscoverySeen, t0);
        store.apply(peer, E::EndpointConnecting, t0 + Duration::from_millis(1));
        store.apply(peer, E::EndpointConnected, t0 + Duration::from_millis(2));
        store.apply_with_error(
            peer,
            E::TopicJoinFailed,
            Some("subscribe_and_join timed out".to_string()),
            t0 + Duration::from_millis(3),
        );

        let snap = PeerDiagnosticsSnapshot::from_entry(store.get(&peer).unwrap(), &local, t0);

        assert_eq!(snap.state, "degraded");
        assert_eq!(snap.topic_join_status, "failed");
        assert!(!snap.subscription_ready);
        assert_eq!(snap.last_error.as_deref(), Some("subscribe_and_join timed out"));
        assert_eq!(snap.last_error_stage.as_deref(), Some("topic-join"));
    }

    /// An endpoint failure is stage-labelled `endpoint`.
    #[test]
    fn endpoint_failure_is_stage_labelled() {
        let local = key(0x07);
        let peer = key(0x08);
        let t0 = Instant::now();
        let mut store = PeerConnectivityStore::new();

        store.apply(peer, E::DiscoverySeen, t0);
        store.apply_with_error(
            peer,
            E::EndpointFailed,
            Some("connection refused".to_string()),
            t0 + Duration::from_millis(1),
        );

        let snap = PeerDiagnosticsSnapshot::from_entry(store.get(&peer).unwrap(), &local, t0);

        assert_eq!(snap.state, "degraded");
        assert_eq!(snap.endpoint, "failed");
        assert_eq!(snap.last_error.as_deref(), Some("connection refused"));
        assert_eq!(snap.last_error_stage.as_deref(), Some("endpoint"));
    }

    /// The rendered output is stable and share-safe: it contains only
    /// truncated ids and never the full 64-hex-char peer id.
    #[test]
    fn render_is_stable_and_share_safe() {
        let local = key(0x09);
        let peer = key(0x0A);
        let t0 = Instant::now();
        let mut store = PeerConnectivityStore::new();

        store.apply(peer, E::DiscoverySeen, t0);
        store.apply(peer, E::EndpointConnected, t0 + Duration::from_millis(1));
        store.apply(peer, E::TopicJoined, t0 + Duration::from_millis(2));

        let snap = PeerDiagnosticsSnapshot::from_entry(store.get(&peer).unwrap(), &local, t0);
        let full_hex = hex::encode(peer.as_bytes());

        let line = snap.render();
        assert!(line.contains(&snap.peer_id));
        assert!(
            !line.contains(&full_hex),
            "full peer id must never appear in the rendered snapshot"
        );
        // Stable field labels are present.
        for label in [
            "peer=", "state=", "discovery=", "endpoint=", "path=", "relay=", "topic=",
            "topic_prefix=", "subscribed=", "outbound=", "inbound_gossip=", "decoded=",
            "error_stage=", "error=",
        ] {
            assert!(line.contains(label), "missing label {label} in {line}");
        }
    }

    /// `snapshots_for` sorts peers by id and skips nothing.
    #[test]
    fn snapshots_for_sorts_by_peer_id() {
        let local = key(0x0B);
        let a = key(0x0C);
        let b = key(0x0D);
        let t0 = Instant::now();
        let mut store = PeerConnectivityStore::new();

        store.apply(b, E::DiscoverySeen, t0);
        store.apply(a, E::DiscoverySeen, t0);

        let snaps = snapshots_for(&store, &local, t0);
        assert_eq!(snaps.len(), 2);
        assert_eq!(snaps[0].peer_id, a.fmt_short().to_string());
        assert_eq!(snaps[1].peer_id, b.fmt_short().to_string());
    }

    /// A relay-only peer reports relay involvement and is still reachable
    /// (BORU-CP-14 acceptance: a relay connection can still be considered
    /// reachable).
    #[test]
    fn relay_path_is_reported() {
        let local = key(0x0E);
        let peer = key(0x0F);
        let t0 = Instant::now();
        let mut store = PeerConnectivityStore::new();

        store.apply(peer, E::DiscoverySeen, t0);
        store.apply(peer, E::EndpointConnected, t0 + Duration::from_millis(1));
        store.apply(peer, E::PathChangedRelay, t0 + Duration::from_millis(2));

        let snap = PeerDiagnosticsSnapshot::from_entry(store.get(&peer).unwrap(), &local, t0);

        assert_eq!(snap.path_kind, "relay");
        assert!(snap.relay_involved);
        assert_eq!(snap.state, "reachable", "relay-only is still reachable");
    }

    /// A transitioning path is reported as `transitioning` with no relay
    /// involvement, and the peer's connectivity state is untouched.
    #[test]
    fn transitioning_path_is_reported() {
        let local = key(0x10);
        let peer = key(0x11);
        let t0 = Instant::now();
        let mut store = PeerConnectivityStore::new();

        store.apply(peer, E::DiscoverySeen, t0);
        store.apply(peer, E::EndpointConnected, t0 + Duration::from_millis(1));
        store.apply(peer, E::PathChangedTransitioning, t0 + Duration::from_millis(2));

        let snap = PeerDiagnosticsSnapshot::from_entry(store.get(&peer).unwrap(), &local, t0);

        assert_eq!(snap.path_kind, "transitioning");
        assert!(!snap.relay_involved);
        assert_eq!(snap.state, "reachable");
        assert_eq!(snap.trail.len(), 2, "path events never add trail records");
    }
}
