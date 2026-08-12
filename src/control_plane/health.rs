//! BORU-CP-15 (PDF Phase 5, Task 5.3): developer networking health view.
//!
//! Debug-only. Provides one place to inspect the control plane and data
//! plane independently: a per-peer health row with **separate, clearly
//! labelled indicators** for Discovery, Endpoint, Direct Topic, Inbound
//! Delivery, Outbound Delivery, and Path, plus recent state transitions
//! (the bounded trail from the connectivity state machine).
//!
//! Two output shapes are provided:
//!
//! * [`render_health_view`] — human-readable table for a terminal.
//! * [`render_copy_diagnostics`] — **copy-diagnostics** block with stable
//!   labels (format version + one line per peer), so two test machines
//!   can produce directly comparable dumps and diff them side by side.
//!   Discovery success is a separate indicator from direct-message
//!   success, and inbound/outbound delivery are separate per peer, so an
//!   asymmetric A→B vs B→A failure is obvious when the two dumps are
//!   compared.
//!
//! # Debug-only surface
//!
//! This module is **not** wired into the chat UI. It is consumed by the
//! `doctor` example's `--health` subcommand and by integration tests. The
//! format is not stable for user-facing use yet (PDF Task 5.3 step 4).
//!
//! # Share-safety
//!
//! All rendered values come from [`PeerDiagnosticsSnapshot`] (BORU-CP-13),
//! which only stores truncated peer ids, topic prefixes, stable labels,
//! and sanitised error strings — no secrets, tokens, or message contents.

use std::time::{Duration, Instant};

use bytes::Bytes;
use iroh_base::PublicKey;
use n0_future::StreamExt;

use super::diagnostics::{snapshots_for, PeerDiagnosticsSnapshot};
use crate::api::Event as GossipEvent;
use crate::contact::direct_topic;
use crate::control_plane::connectivity::{ConnectivityEvent, PeerConnectivityStore};
use crate::discovery_service::DiscoveryService;
use crate::net::Gossip;

/// Stable format version of the copy-diagnostics block. Bump when the
/// label set or line format changes incompatibly.
pub const HEALTH_FORMAT_VERSION: &str = "BORU-HEALTH-V1";

/// Fixed magic prefix of a health probe payload broadcast on a peer's
/// deterministic direct topic by the health harness. It is deliberately NOT
/// a chat `SignedMessage`; the chat decoder rejects it (unknown framing →
/// fail closed for that feature, never rendered as chat).
///
/// The full payload is `magic ‖ sender_public_key_bytes` (32 bytes after
/// the magic). Embedding the sender makes each side's probe bytes unique,
/// so the gossip layer's content-hash message ids differ between the two
/// peers — otherwise the two identical probes would be deduplicated as one
/// message and neither side would observe the other's inbound delivery.
pub const HEALTH_PROBE_MAGIC: &[u8] = b"BORU-HEALTH-PROBE-V1";

/// Build a unique health-probe payload for `local`.
pub fn health_probe_payload(local: &PublicKey) -> Vec<u8> {
    let mut out = Vec::with_capacity(HEALTH_PROBE_MAGIC.len() + 32);
    out.extend_from_slice(HEALTH_PROBE_MAGIC);
    out.extend_from_slice(local.as_bytes());
    out
}

/// Whether `content` is a health probe, and if so which peer sent it
/// (decoded from the trailing public key). `None` if not a probe.
pub fn health_probe_sender(content: &[u8]) -> Option<PublicKey> {
    let magic = HEALTH_PROBE_MAGIC;
    let key = content.get(magic.len()..)?;
    if !content.starts_with(magic) || key.len() != 32 {
        return None;
    }
    PublicKey::from_bytes(key.try_into().ok()?).ok()
}

/// Time budget for the direct-topic probe's join + echo wait.
const PROBE_TIMEOUT: Duration = Duration::from_secs(10);

/// One peer's health row: the six PDF 5.3 indicators as stable labels.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PeerHealthRow {
    /// Short stable peer id (share-safe).
    pub peer_id: String,
    /// Connectivity state-machine label (`unknown | discovered |
    /// connecting | reachable | direct-topic-ready | degraded |
    /// offline-stale`).
    pub state: String,
    /// Discovery indicator: `seen-<elapsed>` or `never`.
    pub discovery: String,
    /// Endpoint indicator: `connected-<elapsed> | connecting | failed |
    /// not-started | disconnected`.
    pub endpoint: String,
    /// Direct-topic indicator: `ready | not_attempted | failed`.
    pub direct_topic: String,
    /// Inbound-delivery indicator: `ok-<elapsed>` or `never`.
    pub inbound: String,
    /// Outbound-delivery indicator: `ok-<elapsed>` or `never`.
    pub outbound: String,
    /// Path indicator: `direct | relay | transitioning | unknown`.
    pub path: String,
    /// Recent transitions (newest first), stable `from→to (event)`
    /// strings.
    pub transitions: Vec<String>,
    /// Last recorded failure reason, if any (sanitised).
    pub last_error: Option<String>,
}

/// Build sorted health rows from per-peer diagnostic snapshots.
///
/// Rows are sorted by peer id so two machines' copy-diagnostics blocks
/// line up peer-for-peer.
pub fn build_health_rows(snapshots: &[PeerDiagnosticsSnapshot]) -> Vec<PeerHealthRow> {
    let mut rows: Vec<PeerHealthRow> = snapshots.iter().map(PeerHealthRow::from_snapshot).collect();
    rows.sort_by(|a, b| a.peer_id.cmp(&b.peer_id));
    rows
}

/// Convenience: build rows straight from the connectivity store.
pub fn health_rows_for(
    store: &PeerConnectivityStore,
    local_node: &PublicKey,
    now: Instant,
) -> Vec<PeerHealthRow> {
    let snaps = snapshots_for(store, local_node, now);
    build_health_rows(&snaps)
}

impl PeerHealthRow {
    /// Map one share-safe snapshot to the six PDF 5.3 indicators.
    pub fn from_snapshot(snap: &PeerDiagnosticsSnapshot) -> Self {
        let discovery = match snap.discovery_last_seen_ms {
            Some(ms) => format!("seen-{}", render_elapsed(ms)),
            None => "never".to_string(),
        };
        let endpoint = match snap.endpoint.as_str() {
            "connected" => format!("connected-{}", render_elapsed_opt(snap.endpoint_observed_ms)),
            other => other.to_string(),
        };
        let inbound = match snap.last_inbound_direct_ms {
            Some(ms) => format!("ok-{}", render_elapsed(ms)),
            None => "never".to_string(),
        };
        let outbound = match snap.last_outbound_direct_ms {
            Some(ms) => format!("ok-{}", render_elapsed(ms)),
            None => "never".to_string(),
        };
        let transitions = snap
            .trail
            .iter()
            .map(|t| format!("{}→{} ({})", t.from, t.to, t.event))
            .collect();
        Self {
            peer_id: snap.peer_id.clone(),
            state: snap.state.clone(),
            discovery,
            endpoint,
            direct_topic: snap.topic_join_status.clone(),
            inbound,
            outbound,
            path: snap.path_kind.clone(),
            transitions,
            last_error: snap.last_error.clone(),
        }
    }
}

/// Render the human-readable health view (debug terminal output).
pub fn render_health_view(local_node: &str, uptime: Duration, rows: &[PeerHealthRow]) -> String {
    let mut out = String::new();
    out.push_str(&format!(
        "BORU network health ({} · node {local_node} · uptime {}s · {} peer(s))\n",
        HEALTH_FORMAT_VERSION,
        uptime.as_secs(),
        rows.len(),
    ));
    out.push_str(
        "  peer     │ state             │ discovery │ endpoint      │ direct_topic │ inbound  │ outbound │ path\n",
    );
    if rows.is_empty() {
        out.push_str("  (no peers tracked yet)\n");
        return out;
    }
    for row in rows {
        out.push_str(&format!(
            "  {:<8} │ {:<17} │ {:<9} │ {:<13} │ {:<12} │ {:<8} │ {:<8} │ {}\n",
            row.peer_id,
            row.state,
            row.discovery,
            row.endpoint,
            row.direct_topic,
            row.inbound,
            row.outbound,
            row.path,
        ));
    }
    // Transitions + errors on their own lines so the table stays aligned.
    for row in rows {
        if !row.transitions.is_empty() || row.last_error.is_some() {
            out.push_str(&format!("  ── {}:\n", row.peer_id));
            for t in &row.transitions {
                out.push_str(&format!("      transition {t}\n"));
            }
            if let Some(err) = &row.last_error {
                out.push_str(&format!("      last_error: {err}\n"));
            }
        }
    }
    out
}

/// Render the **copy-diagnostics** block: stable labels, one line per
/// peer, sorted. Two machines running the same command produce directly
/// comparable output (the header line differs only in `node=`).
pub fn render_copy_diagnostics(
    local_node: &str,
    uptime: Duration,
    rows: &[PeerHealthRow],
) -> String {
    let mut out = String::new();
    out.push_str(&format!(
        "{}\nnode={} uptime={}s peers={}\n",
        HEALTH_FORMAT_VERSION,
        local_node,
        uptime.as_secs(),
        rows.len(),
    ));
    for row in rows {
        out.push_str(&format!(
            "peer={} discovery={} endpoint={} direct_topic={} inbound={} outbound={} path={} state={}\n",
            row.peer_id,
            row.discovery,
            row.endpoint,
            row.direct_topic,
            row.inbound,
            row.outbound,
            row.path,
            row.state,
        ));
    }
    out
}

/// Probe one peer's deterministic direct topic and feed the resulting
/// data-plane events into the connectivity store.
///
/// This is what makes the Inbound/Outbound Delivery indicators meaningful
/// in a standalone health run: it subscribes to `direct_topic(local,
/// peer)`, reports `TopicJoined`, broadcasts [`HEALTH_PROBE_PAYLOAD`], and
/// reports `DirectMessageSent`. If the peer is also running the health
/// harness (or the app is on the same direct topic and echoes), receiving
/// a probe reports `DirectMessageReceived`. When both machines run the
/// harness, both sides get real inbound+outbound evidence and the two
/// dumps are directly comparable.
///
/// Returns a summary of what actually happened (for tests / logging).
pub async fn probe_direct_topic(
    gossip: &Gossip,
    service: &DiscoveryService,
    local: PublicKey,
    peer: PublicKey,
) -> DirectTopicProbe {
    let topic = direct_topic(&local, &peer);
    let Ok(mut sub) = gossip.subscribe(topic, vec![peer]).await else {
        let _ = service.report_connectivity_failure(
            peer,
            ConnectivityEvent::TopicJoinFailed,
            "health probe: direct topic subscribe failed".to_string(),
        );
        return DirectTopicProbe {
            topic_joined: false,
            probe_sent: false,
            probe_received: false,
        };
    };

    // Wait for at least one connection on the direct topic (bounded).
    let joined = match tokio::time::timeout(PROBE_TIMEOUT, sub.joined()).await {
        Ok(Ok(())) => true,
        _ => {
            let _ = service.report_connectivity_failure(
                peer,
                ConnectivityEvent::TopicJoinFailed,
                "health probe: direct topic join timed out".to_string(),
            );
            return DirectTopicProbe {
                topic_joined: false,
                probe_sent: false,
                probe_received: false,
            };
        }
    };
    let _ = service.report_connectivity_event(peer, ConnectivityEvent::TopicJoined);

    let (sender, mut receiver) = sub.split();
    let probe = Bytes::from(health_probe_payload(&local));

    // Listen for a probe from the peer (inbound delivery evidence). The
    // broadcast is repeated on a short interval because the topic mesh may
    // still be forming when we join: a single early broadcast can be
    // silently dropped to an empty mesh (known gossip trap), so we keep
    // re-broadcasting until the peer's probe arrives or the window closes.
    // Payload is tiny + repeats are idempotent and bounded by PROBE_TIMEOUT.
    let mut probe_received = false;
    let mut sent_flag_sent = false;
    let deadline = tokio::time::Instant::now() + PROBE_TIMEOUT;
    let mut broadcast_at = tokio::time::Instant::now();
    loop {
        if tokio::time::Instant::now() >= deadline {
            break;
        }
        if tokio::time::Instant::now() >= broadcast_at {
            let ok = sender.broadcast(probe.clone()).await.is_ok();
            if ok && !sent_flag_sent {
                let _ = service.report_connectivity_event(peer, ConnectivityEvent::DirectMessageSent);
                sent_flag_sent = true;
            }
            broadcast_at = tokio::time::Instant::now() + Duration::from_millis(200);
        }
        match tokio::time::timeout(Duration::from_millis(200), receiver.next()).await {
            Ok(Some(Ok(GossipEvent::Received(msg)))) => {
                // Only accept a probe whose embedded sender is the peer we
                // are probing (never self, never a third party).
                if let Some(sender_pk) = health_probe_sender(&msg.content) {
                    if sender_pk == peer && sender_pk != local {
                        let _ = service
                            .report_connectivity_event(peer, ConnectivityEvent::DirectMessageReceived);
                        probe_received = true;
                        break;
                    }
                }
            }
            Ok(Some(Ok(_))) => {} // NeighborUp/Down, MissingMessages, Lagged
            Ok(Some(Err(_))) => break,
            Ok(None) => break,
            Err(_) => {}
        }
    }

    DirectTopicProbe {
        topic_joined: joined,
        probe_sent: sent_flag_sent,
        probe_received,
    }
}

/// Outcome of [`probe_direct_topic`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DirectTopicProbe {
    /// Whether the direct topic subscription reached `joined()`.
    pub topic_joined: bool,
    /// Whether the probe payload was broadcast (outbound attempt).
    pub probe_sent: bool,
    /// Whether a probe from the peer was received (inbound delivery).
    pub probe_received: bool,
}

/// `elapsed-ms` → `12ms` / `3s` / `1m5s` (stable, short).
fn render_elapsed(ms: u64) -> String {
    if ms < 1000 {
        format!("{ms}ms")
    } else if ms < 60_000 {
        format!("{}s", ms / 1000)
    } else {
        format!("{}m{}s", ms / 60_000, (ms % 60_000) / 1000)
    }
}

/// `Option<u64>` elapsed-ms → `never` or [`render_elapsed`].
fn render_elapsed_opt(ms: Option<u64>) -> String {
    match ms {
        Some(ms) => render_elapsed(ms),
        None => "never".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::control_plane::connectivity::{ConnectivityEvent as E, PeerConnectivityStore};
    use crate::control_plane::diagnostics::PeerDiagnosticsSnapshot;

    fn key(byte: u8) -> PublicKey {
        let mut seed = [0u8; 32];
        seed[0] = byte;
        iroh_base::SecretKey::from_bytes(&seed).public()
    }

    fn healthy_snapshot() -> PeerDiagnosticsSnapshot {
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
        store.apply(peer, E::DirectMessageReceived, t0 + Duration::from_millis(6));
        let now = t0 + Duration::from_secs(60);
        snapshots_for(&store, &local, now).into_iter().next().unwrap()
    }

    #[test]
    fn healthy_row_has_all_six_indicators() {
        let snap = healthy_snapshot();
        let row = PeerHealthRow::from_snapshot(&snap);

        assert!(row.discovery.starts_with("seen-"), "discovery: {}", row.discovery);
        assert!(row.endpoint.starts_with("connected-"), "endpoint: {}", row.endpoint);
        assert_eq!(row.direct_topic, "ready");
        assert!(row.inbound.starts_with("ok-"), "inbound: {}", row.inbound);
        assert!(row.outbound.starts_with("ok-"), "outbound: {}", row.outbound);
        assert_eq!(row.path, "direct");
        assert_eq!(row.state, "direct-topic-ready");
        // Transitions trail recorded with stable labels.
        assert!(row.transitions.iter().any(|t| t.contains("topic-joined")));
        assert!(row.transitions.iter().any(|t| t.contains("discovery-seen")));
    }

    #[test]
    fn discovery_only_peer_is_clearly_not_delivering() {
        let local = key(0x03);
        let peer = key(0x04);
        let t0 = Instant::now();
        let mut store = PeerConnectivityStore::new();
        store.apply(peer, E::DiscoverySeen, t0);
        let snap = snapshots_for(&store, &local, t0).into_iter().next().unwrap();
        let row = PeerHealthRow::from_snapshot(&snap);

        assert!(row.discovery.starts_with("seen-"));
        assert_eq!(row.endpoint, "not-started");
        assert_eq!(row.direct_topic, "not_attempted");
        assert_eq!(row.inbound, "never");
        assert_eq!(row.outbound, "never");
        assert_eq!(row.path, "unknown");
    }

    #[test]
    fn copy_diagnostics_is_stable_and_sorted() {
        let rows = build_health_rows(&[healthy_snapshot()]);
        let a = render_copy_diagnostics("node-a", Duration::from_secs(30), &rows);
        let b = render_copy_diagnostics("node-a", Duration::from_secs(30), &rows);
        assert_eq!(a, b, "identical input must produce identical output");

        assert!(a.starts_with("BORU-HEALTH-V1\nnode=node-a uptime=30s peers=1\n"));
        assert!(a.contains("discovery="), "missing discovery label");
        assert!(a.contains("endpoint="), "missing endpoint label");
        assert!(a.contains("direct_topic="), "missing direct_topic label");
        assert!(a.contains("inbound="), "missing inbound label");
        assert!(a.contains("outbound="), "missing outbound label");
        assert!(a.contains("path="), "missing path label");
        assert!(a.contains("state="), "missing state label");
    }

    #[test]
    fn asymmetric_failure_is_visible_in_the_dump() {
        // A sees B: discovery + endpoint + topic + outbound ok, but NO
        // inbound (B never sent anything back).
        let a_local = key(0x0A);
        let b_peer = key(0x0B);
        let t0 = Instant::now();
        let mut store_a = PeerConnectivityStore::new();
        store_a.apply(b_peer, E::DiscoverySeen, t0);
        store_a.apply(b_peer, E::EndpointConnected, t0 + Duration::from_millis(1));
        store_a.apply(b_peer, E::TopicJoined, t0 + Duration::from_millis(2));
        store_a.apply(b_peer, E::DirectMessageSent, t0 + Duration::from_millis(3));
        let row_a = PeerHealthRow::from_snapshot(
            &snapshots_for(&store_a, &a_local, t0).into_iter().next().unwrap(),
        );

        // B sees A: discovery + endpoint, but never joined the direct
        // topic and never received anything.
        let b_local = key(0x0B);
        let a_peer = key(0x0A);
        let mut store_b = PeerConnectivityStore::new();
        store_b.apply(a_peer, E::DiscoverySeen, t0);
        store_b.apply(a_peer, E::EndpointConnected, t0 + Duration::from_millis(1));
        let row_b = PeerHealthRow::from_snapshot(
            &snapshots_for(&store_b, &b_local, t0).into_iter().next().unwrap(),
        );

        // A claims it sent to B; B's dump shows it never received and
        // never even joined the direct topic. The labels make the
        // A→B asymmetry obvious.
        assert!(row_a.outbound.starts_with("ok-"), "A outbound: {}", row_a.outbound);
        assert_eq!(row_a.inbound, "never", "A must not invent inbound delivery");
        assert_eq!(row_b.inbound, "never", "B never received");
        assert_eq!(row_b.outbound, "never", "B never sent");
        assert_eq!(row_b.direct_topic, "not_attempted");

        let dump_a = render_copy_diagnostics("A", Duration::from_secs(10), &[row_a]);
        let dump_b = render_copy_diagnostics("B", Duration::from_secs(10), &[row_b]);
        assert!(dump_a.contains("outbound=ok-"), "{dump_a}");
        assert!(dump_a.contains("inbound=never"), "{dump_a}");
        assert!(dump_b.contains("inbound=never"), "{dump_b}");
        assert!(dump_b.contains("direct_topic=not_attempted"), "{dump_b}");
    }

    #[test]
    fn copy_diagnostics_is_share_safe() {
        // The renderer never emits a full 64-hex-char peer id: snapshots
        // already carry short ids, and the copy-diagnostics block passes
        // them through. Guard against any accidental long-id regression.
        let snap = healthy_snapshot();
        let row = PeerHealthRow::from_snapshot(&snap);
        let dump = render_copy_diagnostics("n", Duration::ZERO, &[row]);
        let has_long_hex = dump
            .split_whitespace()
            .any(|tok| tok.len() >= 64 && tok.chars().all(|c| c.is_ascii_hexdigit()));
        assert!(!has_long_hex, "copy-diagnostics must not contain a full 64-hex id: {dump}");
        // No probe payload bytes / message contents either.
        assert!(!dump.contains("BORU-HEALTH-PROBE"));
    }

    #[test]
    fn probe_payload_is_unique_per_sender_and_decodes() {
        let a = key(0x11);
        let b = key(0x12);
        let pa = health_probe_payload(&a);
        let pb = health_probe_payload(&b);
        // Each side's probe must differ so gossip content-hash message ids
        // are unique (otherwise both probes deduplicate into one).
        assert_ne!(pa, pb);
        assert_eq!(health_probe_sender(&pa), Some(a));
        assert_eq!(health_probe_sender(&pb), Some(b));
        // A non-probe payload (e.g. a chat message) is never a probe.
        assert_eq!(health_probe_sender(b"hello world"), None);
        // Truncated probe is rejected.
        assert_eq!(health_probe_sender(&pa[..HEALTH_PROBE_MAGIC.len() + 4]), None);
    }
}
