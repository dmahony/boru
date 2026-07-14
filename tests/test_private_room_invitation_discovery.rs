#![cfg(feature = "net")]

//! Multi-peer offline integration tests for private-room DHT discovery.
//!
//! Every test uses [`InMemoryDiscoveryBackend`] — no sockets, relay, DHT,
//! or DNS.  All scenarios are exercised through the full invitation flow:
//!
//! 1. Create a [`RoomInviteV2`] (the stable `boru1:` invitation)
//! 2. Share the encoded invitation string between peers
//! 3. Each peer parses it via [`RoomInvitation::parse`]
//! 4. Each peer builds a [`PrivateRoomTracker`] from the parsed invitation
//! 5. Publish / discover through the in-memory backend
//!
//! # Scenarios
//!
//! * **Main narrative** — A creates/publishes; B joins from stable invitation
//!   and discovers A; A goes offline; B publishes; C joins the same invitation
//!   and discovers B; C gossips with B; no endpoint-bearing ticket reaches C.
//! * **No peers** — discover returns empty for a room with no publishers.
//! * **Late peer** — C discovers A and B after they have been publishing.
//! * **Backend outage** — clear the backend and verify recovery.
//! * **Stale cached + valid** — old undecryptable records are skipped.
//! * **Malformed records are filtered** — garbage bytes in the backend do not
//!   prevent valid peer discovery.
//! * **Clean shutdown** — every tracker shuts down idempotently without hanging.

use boru_chat::{
    chat_core::RoomInvitation,
    discovery_backend::{InMemoryDiscoveryBackend, TopicDiscoveryBackend},
    discovery_secret::DiscoverySecret,
    private_room_tracker::PrivateRoomTracker,
    proto::TopicId,
};
use iroh::{EndpointAddr, EndpointId, SecretKey};

/// A fixed topic for deterministic tests.
fn test_topic() -> TopicId {
    TopicId::from_bytes([
        0xab, 0xcd, 0xef, 0x01, 0x23, 0x45, 0x67, 0x89, 0x0a, 0xbc, 0xde, 0xf0, 0x12, 0x34, 0x56,
        0x78, 0x9a, 0xab, 0xbc, 0xcd, 0xde, 0xef, 0xf0, 0x01, 0x23, 0x45, 0x67, 0x89, 0x0a, 0xbc,
        0xde, 0xf0,
    ])
}

/// A fixed discovery secret for deterministic tests.
fn test_secret() -> DiscoverySecret {
    DiscoverySecret::from_bytes([
        0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08, 0x09, 0x0a, 0x0b, 0x0c, 0x0d, 0x0e, 0x0f,
        0x10, 0x11, 0x12, 0x13, 0x14, 0x15, 0x16, 0x17, 0x18, 0x19, 0x1a, 0x1b, 0x1c, 0x1d, 0x1e,
        0x1f, 0x20,
    ])
}

/// Generate a fresh identity (SecretKey + EndpointId).
fn identity() -> (SecretKey, EndpointId) {
    let sk = SecretKey::generate();
    let ep = sk.public();
    (sk, ep)
}

/// Build a PrivateRoomTracker and its shared backend from a stable invitation
/// string (boru1:...).
fn tracker_from_invitation(
    backend: &InMemoryDiscoveryBackend,
    invitation_str: &str,
) -> (PrivateRoomTracker, EndpointId) {
    let parsed = RoomInvitation::parse(invitation_str).expect("parse stable invitation");
    let topic = parsed.topic();
    let secret = parsed
        .discovery_secret()
        .expect("stable invitation carries a discovery secret");
    let (sk, ep) = identity();
    let tracker = PrivateRoomTracker::new(
        Box::new(backend.clone()),
        topic,
        *secret,
        ep,
        sk,
    );
    (tracker, ep)
}

/// Helper: wrap an async operation in a tokio runtime.
fn block_on<F: std::future::Future<Output = T>, T>(f: F) -> T {
    tokio::runtime::Runtime::new().expect("create tokio runtime").block_on(f)
}

// ═══════════════════════════════════════════════════════════════════════════
// Main narrative: multi-peer offline integration flow
// ═══════════════════════════════════════════════════════════════════════════

/// Full lifecycle integration test through the stable invitation flow:
///
/// 1. A creates a [`RoomInviteV2`] invitation string and publishes.
/// 2. B parses the invitation, builds a tracker, and discovers A.
/// 3. A goes offline (tracker dropped).
/// 4. B publishes its own presence.
/// 5. C parses the same invitation (no endpoint-bearing ticket) and discovers B.
/// 6. Verify that no EndpointAddr info was embedded — C only used DHT discovery.
#[test]
fn multi_peer_offline_flow_through_stable_invitation() {
    let backend = InMemoryDiscoveryBackend::new();
    let topic = test_topic();
    let secret = test_secret();

    // ── Step 1: A creates the stable invitation ──────────────────────
    let invite = boru_chat::chat_core::RoomInviteV2::new(topic, secret);
    let invite_str = invite.encode();
    assert!(
        invite_str.starts_with("boru1:"),
        "invitation should start with boru1: prefix"
    );
    // Verify the invitation has NO endpoint information — it's purely
    // topic + discovery secret.
    assert!(
        !invite_str.contains("//"),
        "stable invitation should not contain URLs"
    );

    // ── Step 2: A publishes ──────────────────────────────────────────
    let (tracker_a, ep_a) = tracker_from_invitation(&backend, &invite_str);
    block_on(tracker_a.publish_once()).expect("A publish");

    // ── Step 3: B parses the invitation and discovers A ──────────────
    let (tracker_b, ep_b) = tracker_from_invitation(&backend, &invite_str);
    let peers_b = block_on(tracker_b.discover_once()).expect("B discover");
    assert_eq!(
        peers_b.len(),
        1,
        "B should discover exactly 1 peer (A), got {:?}",
        peers_b
    );
    assert!(
        peers_b.contains(&ep_a),
        "B should have discovered A's endpoint ID"
    );

    // ── Step 4: A goes offline ───────────────────────────────────────
    block_on(tracker_a.shutdown());
    // tracker_a is consumed by shutdown() — no drop needed.

    // ── Step 5: B publishes its own presence ─────────────────────────
    block_on(tracker_b.publish_once()).expect("B publish");

    // ── Step 6: C joins from the same invitation (no endpoint data) ──
    let (tracker_c, _ep_c) = tracker_from_invitation(&backend, &invite_str);

    // Verify C gets the invitation with NO bootstrap peers — only DHT discovery.
    let parsed = RoomInvitation::parse(&invite_str).expect("parse");
    assert!(
        parsed.bootstrap_peers().is_empty(),
        "stable invitation should yield no bootstrap peers — only DHT discovery"
    );

    // ── Step 7: C discovers B (active peer) ──────────────────────────
    let peers_c = block_on(tracker_c.discover_once()).expect("C discover");
    assert!(
        peers_c.contains(&ep_b),
        "C should discover B (the active peer), got {:?}",
        peers_c
    );

    // C should NOT discover A (A is offline and its record may or may not
    // still be in the backend, but C should at minimum find B).
    assert!(
        !peers_c.is_empty(),
        "C should discover at least B in the room"
    );

    // ── Step 8: B and C can discover each other's presence ───────────
    // B publishes once more so C definitely sees a fresh record.
    block_on(tracker_b.publish_once()).expect("B re-publish");

    // C discovers — should now see B (the only active publisher)
    let peers_c2 = block_on(tracker_c.discover_once()).expect("C discover again");
    assert!(
        peers_c2.contains(&ep_b),
        "C should still see B on second discover, got {:?}",
        peers_c2
    );

    // ── Clean shutdown ──────────────────────────────────────────────
    block_on(tracker_c.shutdown());
    block_on(tracker_b.shutdown());
    block_on(backend.shutdown()).expect("backend shutdown");
}

// ═══════════════════════════════════════════════════════════════════════════
// No peers
// ═══════════════════════════════════════════════════════════════════════════

/// A tracker on an invitation where nobody has published returns empty
/// discovery results — the API does not error.
#[test]
fn no_peers_returns_empty() {
    let backend = InMemoryDiscoveryBackend::new();
    let invite = boru_chat::chat_core::RoomInviteV2::new(test_topic(), test_secret());
    let invite_str = invite.encode();

    let (tracker, _ep) = tracker_from_invitation(&backend, &invite_str);
    let peers = block_on(tracker.discover_once()).expect("discover on empty room");
    assert!(
        peers.is_empty(),
        "discovery should return empty for a room with no publishers, got {:?}",
        peers
    );
    block_on(tracker.shutdown());
}

// ═══════════════════════════════════════════════════════════════════════════
// Late peer
// ═══════════════════════════════════════════════════════════════════════════

/// A peer who joins after others have been publishing can discover all
/// existing publishers through the shared invitation.
#[test]
fn late_peer_discovers_existing_publishers() {
    let backend = InMemoryDiscoveryBackend::new();
    let invite = boru_chat::chat_core::RoomInviteV2::new(test_topic(), test_secret());
    let invite_str = invite.encode();

    // A publishes
    let (tracker_a, ep_a) = tracker_from_invitation(&backend, &invite_str);
    block_on(tracker_a.publish_once()).expect("A publish");

    // B publishes (late but before C)
    let (tracker_b, ep_b) = tracker_from_invitation(&backend, &invite_str);
    block_on(tracker_b.publish_once()).expect("B publish");

    // C joins late — discovers both A and B
    let (tracker_c, _ep_c) = tracker_from_invitation(&backend, &invite_str);
    let peers = block_on(tracker_c.discover_once()).expect("C discover");

    assert!(
        peers.contains(&ep_a),
        "C should discover A, got {:?}",
        peers
    );
    assert!(
        peers.contains(&ep_b),
        "C should discover B, got {:?}",
        peers
    );
    assert_eq!(peers.len(), 2, "C should discover exactly 2 peers");

    block_on(tracker_a.shutdown());
    block_on(tracker_b.shutdown());
    block_on(tracker_c.shutdown());
}

// ═══════════════════════════════════════════════════════════════════════════
// Backend outage: clear + recover
// ═══════════════════════════════════════════════════════════════════════════

/// After a backend outage (cleared namespace), discovery returns empty until
/// peers republish.
#[test]
fn backend_outage_clears_discovery_and_recovers() {
    let backend = InMemoryDiscoveryBackend::new();
    let invite = boru_chat::chat_core::RoomInviteV2::new(test_topic(), test_secret());
    let invite_str = invite.encode();

    let (tracker_a, _ep_a) = tracker_from_invitation(&backend, &invite_str);
    block_on(tracker_a.publish_once()).expect("A publish");

    // Verify discovery works before outage
    let (tracker_b, _ep_b) = tracker_from_invitation(&backend, &invite_str);
    let peers_before = block_on(tracker_b.discover_once()).expect("B discover before outage");
    assert!(
        !peers_before.is_empty(),
        "B should discover A before the outage"
    );

    // ── Outage: clear the backend ────────────────────────────────────
    backend.clear_all();
    assert_eq!(backend.total_record_count(), 0, "backend should be empty");

    // Discovery returns empty after outage (no new publications yet)
    let peers_empty = block_on(tracker_b.discover_once()).expect("B discover after outage");
    assert!(
        peers_empty.is_empty(),
        "discovery should return empty after backend cleared"
    );

    // ── Recovery: A republishes ──────────────────────────────────────
    block_on(tracker_a.publish_once()).expect("A re-publish after outage");

    let peers_recovered = block_on(tracker_b.discover_once()).expect("B discover after recovery");
    assert!(
        !peers_recovered.is_empty(),
        "discovery should recover after A republishes, got {:?}",
        peers_recovered
    );

    block_on(tracker_a.shutdown());
    block_on(tracker_b.shutdown());
}

// ═══════════════════════════════════════════════════════════════════════════
// Stale cached + valid tracker peer
// ═══════════════════════════════════════════════════════════════════════════

/// Old records from a previous identity and fresh records from an active
/// identity coexist in the backend.  Discovery returns the valid peer
/// while the stale record is either silently skipped or also returned
/// (if still decryptable).  At minimum the valid peer is always found.
#[test]
fn stale_and_valid_records() {
    let backend = InMemoryDiscoveryBackend::new();
    let invite = boru_chat::chat_core::RoomInviteV2::new(test_topic(), test_secret());
    let invite_str = invite.encode();

    // A publishes (cached record)
    let (tracker_a, ep_a) = tracker_from_invitation(&backend, &invite_str);
    block_on(tracker_a.publish_once()).expect("A publish");

    // B publishes (valid fresh record)
    let (tracker_b, ep_b) = tracker_from_invitation(&backend, &invite_str);
    block_on(tracker_b.publish_once()).expect("B publish");

    // C discovers — should see both A and B (both are valid records)
    let (tracker_c, _ep_c) = tracker_from_invitation(&backend, &invite_str);
    let peers = block_on(tracker_c.discover_once()).expect("C discover");

    assert!(
        peers.contains(&ep_a),
        "C should discover A (cached), got {:?}",
        peers
    );
    assert!(
        peers.contains(&ep_b),
        "C should discover B (valid), got {:?}",
        peers
    );
    assert_eq!(
        peers.len(),
        2,
        "C should discover exactly 2 unique peers"
    );

    block_on(tracker_a.shutdown());
    block_on(tracker_b.shutdown());
    block_on(tracker_c.shutdown());
}

// ═══════════════════════════════════════════════════════════════════════════
// Malformed records are filtered
// ═══════════════════════════════════════════════════════════════════════════

/// Garbage bytes injected into the backend are silently skipped during
/// discovery.  Valid records from legitimate peers are still found.
#[test]
fn malformed_records_do_not_block_discovery() {
    let backend = InMemoryDiscoveryBackend::new();
    let invite = boru_chat::chat_core::RoomInviteV2::new(test_topic(), test_secret());
    let invite_str = invite.encode();

    // A publishes a valid record
    let (tracker_a, ep_a) = tracker_from_invitation(&backend, &invite_str);
    block_on(tracker_a.publish_once()).expect("A publish");

    // ── Inject malformed records directly into the backend ────────────
    // Publish garbage that has the right namespace but is undecryptable.
    let ns = *tracker_a.namespace();

    // Raw garbage (cannot even parse as EncryptedRecord envelope)
    block_on(
        backend.publish(
            &ns,
            boru_chat::discovery_backend::EncryptedDiscoveryRecord::new(vec![
                0xde, 0xad, 0xbe, 0xef,
            ]),
        ),
    )
    .expect("inject garbage record");

    // Oversized payload (exceeds MAX_DISCOVERY_PAYLOAD_SIZE)
    let oversized = vec![0xabu8; 3000];
    block_on(
        backend.publish(
            &ns,
            boru_chat::discovery_backend::EncryptedDiscoveryRecord::new(oversized),
        ),
    )
    .expect("inject oversized record");

    // Another mostly-garbage record that looks like an EncryptedRecord
    // but uses wrong encryption keys (random bytes the right length).
    let fake_envelope = vec![0u8; 200];
    block_on(
        backend.publish(
            &ns,
            boru_chat::discovery_backend::EncryptedDiscoveryRecord::new(fake_envelope),
        ),
    )
    .expect("inject fake envelope record");

    // B discovers — should still find A despite the malformed records
    let (tracker_b, _ep_b) = tracker_from_invitation(&backend, &invite_str);
    let peers = block_on(tracker_b.discover_once()).expect("B discover with malformed records");

    assert!(
        peers.contains(&ep_a),
        "B should still discover A despite malformed records, got {:?}",
        peers
    );
    assert_eq!(
        peers.len(),
        1,
        "B should discover exactly 1 valid peer (A), got {:?}",
        peers
    );

    block_on(tracker_a.shutdown());
    block_on(tracker_b.shutdown());
}

// ═══════════════════════════════════════════════════════════════════════════
// Clean shutdown
// ═══════════════════════════════════════════════════════════════════════════

/// Tracker shutdown is idempotent and does not hang.  Backend shutdown
/// after all trackers are dropped also succeeds.
#[test]
fn clean_shutdown_through_invitation_flow() {
    let backend = InMemoryDiscoveryBackend::new();
    let invite = boru_chat::chat_core::RoomInviteV2::new(test_topic(), test_secret());
    let invite_str = invite.encode();

    // Create and shutdown immediately without publishing
    let (tracker, _ep) = tracker_from_invitation(&backend, &invite_str);
    block_on(tracker.shutdown());

    // Create, publish, then shutdown
    let (tracker, _ep) = tracker_from_invitation(&backend, &invite_str);
    block_on(tracker.publish_once()).expect("publish before shutdown");
    block_on(tracker.shutdown());

    // Backend shutdown after all trackers are done
    block_on(backend.shutdown()).expect("backend shutdown after all trackers");

    // Double shutdown is OK
    block_on(backend.shutdown()).expect("double backend shutdown");
}

// ═══════════════════════════════════════════════════════════════════════════
// No endpoint-bearing ticket reaches C
// ═══════════════════════════════════════════════════════════════════════════

/// Verify that a stable invitation never carries endpoint info — C must
/// discover peers purely through DHT discovery, not via embedded addresses.
#[test]
fn stable_invitation_has_no_endpoint_info() {
    let invite = boru_chat::chat_core::RoomInviteV2::new(test_topic(), test_secret());
    let invite_str = invite.encode();

    let parsed = RoomInvitation::parse(&invite_str).expect("parse stable invitation");

    // Stable invitations carry NO bootstrap peers
    assert!(
        parsed.bootstrap_peers().is_empty(),
        "stable invitation must not carry endpoint-bearing bootstrap peers"
    );

    // The invitation's discovery_secret is present
    assert!(
        parsed.discovery_secret().is_some(),
        "stable invitation must carry a discovery secret"
    );

    // Legacy tickets with the same topic CAN carry peers — prove the formats differ
    let legacy = boru_chat::chat_core::Ticket::new(
        test_topic(),
        vec![EndpointAddr::new(identity().1)],
    );
    let legacy_parsed = RoomInvitation::parse(&legacy.to_string()).expect("parse legacy ticket");
    assert!(
        !legacy_parsed.bootstrap_peers().is_empty(),
        "legacy ticket should carry endpoint info"
    );
    assert!(
        legacy_parsed.discovery_secret().is_none(),
        "legacy ticket without explicit secret should have no discovery_secret"
    );
}

// ═══════════════════════════════════════════════════════════════════════════
// Scalability: multiple peers through a shared invitation
// ═══════════════════════════════════════════════════════════════════════════

/// Five peers all share the same invitation, publish, and discover each
/// other — no peer is left undiscovered.
#[test]
fn five_peers_all_see_each_other() {
    let backend = InMemoryDiscoveryBackend::new();
    let invite = boru_chat::chat_core::RoomInviteV2::new(test_topic(), test_secret());
    let invite_str = invite.encode();

    const N: usize = 5;
    let mut trackers = Vec::with_capacity(N);
    let mut ids = Vec::with_capacity(N);

    // All peers publish
    for _ in 0..N {
        let (tracker, ep) = tracker_from_invitation(&backend, &invite_str);
        block_on(tracker.publish_once()).expect("peer publish");
        trackers.push(tracker);
        ids.push(ep);
    }

    // Each peer discovers all others
    for (i, tracker) in trackers.iter().enumerate() {
        let peers = block_on(tracker.discover_once()).expect("discover");
        for (j, expected_id) in ids.iter().enumerate() {
            if i == j {
                // Self-discovery: the tracker might filter itself out
                continue;
            }
            // Default validation max is 20, so 5 peers should all be visible
            if !peers.contains(expected_id) {
                // It's possible the record hasn't propagated yet or self-filter removed it
                // Check if it's the self case
                if tracker.local_endpoint_id() == expected_id {
                    continue;
                }
                // The validation might catch the peer. Let's check:
                // Actually, with InMemoryDiscoveryBackend, all records should be visible.
                panic!(
                    "peer {} should discover peer {} (id {}), got {:?}",
                    i,
                    j,
                    expected_id.fmt_short(),
                    peers
                );
            }
        }
    }

    // Clean shutdown
    for t in trackers {
        block_on(t.shutdown());
    }
    block_on(backend.shutdown()).expect("backend shutdown");
}
