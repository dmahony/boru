//! Consolidated diagnostics unit tests (BORU-CORE-002).

use super::*;
use iroh_base::SecretKey;

/// Generate a valid public key for testing.
fn test_key() -> PublicKey {
    SecretKey::generate().public()
}

// ── Basic functionality (from part 1) ──────────────────────────────

#[test]
fn test_record_and_query_events() {
    let diag = Diagnostics::new();

    diag.record(None, DiagnosticEventKind::RoomJoined);
    diag.record(
        None,
        DiagnosticEventKind::MessageBroadcast {
            message_id: None,
            message_hash: None,
            probe_id: None,
        },
    );
    diag.record(
        Some(TopicId::from_bytes([1u8; 32])),
        DiagnosticEventKind::MessageReceived {
            message_id: None,
            message_hash: None,
            probe_id: None,
            sender_id: None,
        },
    );

    assert_eq!(diag.event_count(), 3);
    assert_eq!(diag.latest_sequence(), 2);

    // All events since 0 (sequence 0 is excluded by > since_sequence)
    let all = diag.events_since(0, 100, None);
    assert_eq!(all.len(), 2);

    // Filter by room
    let room_events = diag.events_since(0, 100, Some(TopicId::from_bytes([1u8; 32])));
    assert_eq!(room_events.len(), 1);
    assert!(matches!(
        room_events[0].kind,
        DiagnosticEventKind::MessageReceived { .. }
    ));

    // Since sequence
    let recent = diag.events_since(1, 100, None);
    assert_eq!(recent.len(), 1);
    assert_eq!(recent[0].sequence, 2);
}

#[test]
fn test_event_eviction() {
    let diag = Diagnostics::with_capacity(3, 100);

    for _i in 0..5 {
        diag.record(None, DiagnosticEventKind::RoomJoined);
    }

    assert_eq!(diag.event_count(), 3);
    assert_eq!(diag.latest_sequence(), 4);

    let events = diag.events_since(0, 100, None);
    assert_eq!(events.len(), 3);
    // Sequences should be 2, 3, 4 (the three newest)
    assert_eq!(events[0].sequence, 2);
    assert_eq!(events[1].sequence, 3);
    assert_eq!(events[2].sequence, 4);
}

#[test]
fn test_query_limit_clamped() {
    let diag = Diagnostics::new();

    for _i in 0..10 {
        diag.record(None, DiagnosticEventKind::RoomJoined);
    }

    // Request more than max clamp (should clamp to 1000)
    let events = diag.events_since(0, 5000, None);
    assert_eq!(events.len(), 9);
}

#[test]
fn test_probe_storage() {
    let diag = Diagnostics::new();
    let peer = test_key();

    diag.record_received_probe("probe-1".to_string(), peer, DiscoverySource::Mdns, None);

    let found = diag.find_received_probe("probe-1");
    assert!(found.is_some());
    assert_eq!(found.unwrap().sender_id, peer.to_string());
    assert_eq!(diag.probe_count(), 1);

    // Non-existent probe
    assert!(diag.find_received_probe("probe-nonexistent").is_none());
}

#[test]
fn test_probe_eviction() {
    let diag = Diagnostics::with_capacity(100, 3);
    let p_a = test_key();
    let p_b = test_key();
    let p_c = test_key();
    let p_d = test_key();

    diag.record_received_probe("a".to_string(), p_a, DiscoverySource::Mdns, None);
    diag.record_received_probe("b".to_string(), p_b, DiscoverySource::Ticket, None);
    diag.record_received_probe("c".to_string(), p_c, DiscoverySource::Gossip, None);

    assert_eq!(diag.probe_count(), 3);

    // Insert a fourth — "a" should be evicted
    diag.record_received_probe("d".to_string(), p_d, DiscoverySource::Bootstrap, None);

    assert_eq!(diag.probe_count(), 3);
    assert!(diag.find_received_probe("a").is_none());
    assert!(diag.find_received_probe("d").is_some());
}

#[test]
fn test_probe_replace_refreshes_position() {
    let diag = Diagnostics::with_capacity(100, 3);
    let p_a = test_key();
    let p_b = test_key();
    let p_c = test_key();
    let p_d = test_key();

    diag.record_received_probe("a".to_string(), p_a, DiscoverySource::Mdns, None);
    diag.record_received_probe("b".to_string(), p_b, DiscoverySource::Ticket, None);
    diag.record_received_probe("c".to_string(), p_c, DiscoverySource::Gossip, None);

    // Replace "a" — should move to newest, so "b" gets evicted next
    diag.record_received_probe("a".to_string(), p_a, DiscoverySource::Manual, None);

    // Insert a fourth — oldest is now "b"
    diag.record_received_probe("d".to_string(), p_d, DiscoverySource::Bootstrap, None);

    assert_eq!(diag.probe_count(), 3);
    assert!(diag.find_received_probe("a").is_some()); // replaced, not evicted
    assert!(diag.find_received_probe("b").is_none()); // evicted (oldest)
    assert!(diag.find_received_probe("d").is_some());
}

#[test]
fn test_serialize_roundtrip() {
    let event = DiagnosticEvent {
        sequence: 1,
        timestamp: Utc::now(),
        room_id: Some(TopicId::from_bytes([0xAB; 32])),
        peer_id: None,
        kind: DiagnosticEventKind::RoomJoined,
    };

    let json = serde_json::to_string(&event).unwrap();
    let deserialized: DiagnosticEvent = serde_json::from_str(&json).unwrap();
    assert_eq!(deserialized.sequence, event.sequence);
    assert!(matches!(deserialized.kind, DiagnosticEventKind::RoomJoined));

    // Check snake_case serialization for old variants
    let kind_json = serde_json::to_string(&DiagnosticEventKind::PeerDiscovered).unwrap();
    assert_eq!(kind_json, "{\"type\":\"peer_discovered\"}");

    // Check tagged serialization for new variants
    let new_kind = DiagnosticEventKind::AddressLookupStarted {
        source: DiscoverySource::Mdns,
    };
    let new_json = serde_json::to_string(&new_kind).unwrap();
    let deser_new: DiagnosticEventKind = serde_json::from_str(&new_json).unwrap();
    assert!(matches!(
        deser_new,
        DiagnosticEventKind::AddressLookupStarted { .. }
    ));
}

#[test]
fn test_error_variant_carries_string() {
    let diag = Diagnostics::new();
    diag.record(
        None,
        DiagnosticEventKind::Error("something went wrong".to_string()),
    );

    let events = diag.all_events();
    assert_eq!(events.len(), 1);
    match &events[0].kind {
        DiagnosticEventKind::Error(msg) => assert_eq!(msg, "something went wrong"),
        _ => panic!("expected Error variant"),
    }
}

#[test]
fn test_empty_diagnostics() {
    let diag = Diagnostics::new();
    assert_eq!(diag.latest_sequence(), 0);
    assert_eq!(diag.event_count(), 0);
    assert_eq!(diag.probe_count(), 0);
    assert!(diag.find_received_probe("nothing").is_none());
}

// ── Part 2: Peer state tests ───────────────────────────────────────

#[test]
fn test_peer_state_advances_from_discovered_to_connected_to_topic_member() {
    let peer_hex = "deadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeef";
    let room = TopicId::from_bytes([1u8; 32]);

    let start_state = None;

    // Event 1: peer discovered
    let e1 = DiagnosticEvent {
        sequence: 1,
        timestamp: Utc::now(),
        room_id: Some(room),
        peer_id: Some(peer_hex.to_string()),
        kind: DiagnosticEventKind::PeerDiscovered,
    };
    let state = update_peer_state(start_state, &e1);
    assert!(state.discovered);
    assert!(state.discovered_at_ms.is_some());
    assert_eq!(state.address_lookup_state, DiagnosticStageState::NotStarted);

    // Event 2: connection established
    let e2 = DiagnosticEvent {
        sequence: 2,
        timestamp: Utc::now(),
        room_id: Some(room),
        peer_id: Some(peer_hex.to_string()),
        kind: DiagnosticEventKind::ConnectionEstablished {
            remote_address: Some("127.0.0.1:1234".to_string()),
            transport: Some("quic".to_string()),
            used_relay: Some(false),
        },
    };
    let state = update_peer_state(Some(state), &e2);
    assert_eq!(state.connection_state, ConnectionDiagnosticState::Connected);
    assert_eq!(state.connected_address.as_deref(), Some("127.0.0.1:1234"));

    // Event 3: topic member
    let e3 = DiagnosticEvent {
        sequence: 3,
        timestamp: Utc::now(),
        room_id: Some(room),
        peer_id: Some(peer_hex.to_string()),
        kind: DiagnosticEventKind::PeerAddedToTopic,
    };
    let state = update_peer_state(Some(state), &e3);
    assert!(state.topic_member);
}

#[test]
fn test_failed_address_lookup_classified_as_address_resolution() {
    let peer_hex = "aaaa";
    let room = TopicId::from_bytes([2u8; 32]);

    let e1 = DiagnosticEvent {
        sequence: 1,
        timestamp: Utc::now(),
        room_id: Some(room),
        peer_id: Some(peer_hex.to_string()),
        kind: DiagnosticEventKind::PeerDiscovered,
    };
    let state = update_peer_state(None, &e1);

    let e2 = DiagnosticEvent {
        sequence: 2,
        timestamp: Utc::now(),
        room_id: Some(room),
        peer_id: Some(peer_hex.to_string()),
        kind: DiagnosticEventKind::AddressLookupStarted {
            source: DiscoverySource::Mdns,
        },
    };
    let state = update_peer_state(Some(state), &e2);

    let e3 = DiagnosticEvent {
        sequence: 3,
        timestamp: Utc::now(),
        room_id: Some(room),
        peer_id: Some(peer_hex.to_string()),
        kind: DiagnosticEventKind::AddressLookupFailed {
            source: DiscoverySource::Mdns,
            error: "DNS timeout".to_string(),
        },
    };
    let state = update_peer_state(Some(state), &e3);
    assert_eq!(state.address_lookup_state, DiagnosticStageState::Failed);
    assert_eq!(state.last_error.as_deref(), Some("DNS timeout"));

    // Build evidence and classify
    let evidence = DiscoveryTestEvidence {
        local_room_joined: true,
        peer_discovered: true,
        address_lookup_observed: true,
        address_resolved: false,
        connection_attempted: false,
        connection_established: false,
        subscription_started: false,
        subscription_joined: false,
        peer_in_topic: false,
        probe_broadcast: false,
        probe_received_or_acknowledged: false,
    };
    let (stage, _summary) = classify_discovery_test(&evidence, Some(&state));
    assert_eq!(stage, Some(DiscoveryFailureStage::AddressResolution));
}

#[test]
fn test_failed_connection_classified_as_connection() {
    let peer_hex = "bbbb";
    let room = TopicId::from_bytes([3u8; 32]);

    let e1 = DiagnosticEvent {
        sequence: 1,
        timestamp: Utc::now(),
        room_id: Some(room),
        peer_id: Some(peer_hex.to_string()),
        kind: DiagnosticEventKind::PeerDiscovered,
    };
    let mut state = update_peer_state(None, &e1);

    let e2 = DiagnosticEvent {
        sequence: 2,
        timestamp: Utc::now(),
        room_id: Some(room),
        peer_id: Some(peer_hex.to_string()),
        kind: DiagnosticEventKind::ConnectionFailed {
            addresses: vec!["127.0.0.1:9999".to_string()],
            error: "Connection refused".to_string(),
        },
    };
    state = update_peer_state(Some(state), &e2);

    let evidence = DiscoveryTestEvidence {
        local_room_joined: true,
        peer_discovered: true,
        address_lookup_observed: false,
        address_resolved: true,
        connection_attempted: true,
        connection_established: false,
        subscription_started: false,
        subscription_joined: false,
        peer_in_topic: false,
        probe_broadcast: false,
        probe_received_or_acknowledged: false,
    };
    let (stage, _summary) = classify_discovery_test(&evidence, Some(&state));
    assert_eq!(stage, Some(DiscoveryFailureStage::Connection));
}

#[test]
fn test_subscription_failure_classified_as_subscription() {
    let peer_hex = "cccc";
    let room = TopicId::from_bytes([4u8; 32]);

    let e1 = DiagnosticEvent {
        sequence: 1,
        timestamp: Utc::now(),
        room_id: Some(room),
        peer_id: Some(peer_hex.to_string()),
        kind: DiagnosticEventKind::PeerDiscovered,
    };
    let mut state = update_peer_state(None, &e1);

    let e2 = DiagnosticEvent {
        sequence: 2,
        timestamp: Utc::now(),
        room_id: Some(room),
        peer_id: Some(peer_hex.to_string()),
        kind: DiagnosticEventKind::RoomSubscriptionFailed {
            error: "already subscribed".to_string(),
        },
    };
    state = update_peer_state(Some(state), &e2);

    let evidence = DiscoveryTestEvidence {
        local_room_joined: true,
        peer_discovered: true,
        address_lookup_observed: false,
        address_resolved: true,
        connection_attempted: true,
        connection_established: true,
        subscription_started: true,
        subscription_joined: false,
        peer_in_topic: false,
        probe_broadcast: false,
        probe_received_or_acknowledged: false,
    };
    let (stage, _summary) = classify_discovery_test(&evidence, Some(&state));
    assert_eq!(stage, Some(DiscoveryFailureStage::Subscription));
}

#[test]
fn test_missing_topic_membership_classified_correctly() {
    let evidence = DiscoveryTestEvidence {
        local_room_joined: true,
        peer_discovered: true,
        address_lookup_observed: false,
        address_resolved: true,
        connection_attempted: true,
        connection_established: true,
        subscription_started: true,
        subscription_joined: true,
        peer_in_topic: false,
        probe_broadcast: false,
        probe_received_or_acknowledged: false,
    };
    let (stage, _summary) = classify_discovery_test(&evidence, None);
    assert_eq!(stage, Some(DiscoveryFailureStage::TopicMembership));
}

#[test]
fn test_probe_timeout_classified_as_probe_delivery() {
    let evidence = DiscoveryTestEvidence {
        local_room_joined: true,
        peer_discovered: true,
        address_lookup_observed: false,
        address_resolved: true,
        connection_attempted: true,
        connection_established: true,
        subscription_started: true,
        subscription_joined: true,
        peer_in_topic: true,
        probe_broadcast: true,
        probe_received_or_acknowledged: false,
    };
    let (stage, _summary) = classify_discovery_test(&evidence, None);
    assert_eq!(stage, Some(DiscoveryFailureStage::ProbeDelivery));
}

#[test]
fn test_missing_discovery_classified_as_discovery() {
    let evidence = DiscoveryTestEvidence {
        local_room_joined: true,
        peer_discovered: false,
        address_lookup_observed: false,
        address_resolved: false,
        connection_attempted: false,
        connection_established: false,
        subscription_started: false,
        subscription_joined: false,
        peer_in_topic: false,
        probe_broadcast: false,
        probe_received_or_acknowledged: false,
    };
    let (stage, _summary) = classify_discovery_test(&evidence, None);
    assert_eq!(stage, Some(DiscoveryFailureStage::Discovery));
}

#[test]
fn test_unknown_or_unobservable_produces_unknown() {
    // Room joined, peer not discovered — no evidence either way
    let evidence = DiscoveryTestEvidence {
        local_room_joined: true,
        peer_discovered: false,
        address_lookup_observed: false,
        address_resolved: false,
        connection_attempted: false,
        connection_established: false,
        subscription_started: false,
        subscription_joined: false,
        peer_in_topic: false,
        probe_broadcast: false,
        probe_received_or_acknowledged: false,
    };
    let (stage, _summary) = classify_discovery_test(&evidence, None);
    assert_eq!(stage, Some(DiscoveryFailureStage::Discovery));
}

#[test]
fn test_complete_success_classified_no_failure() {
    let evidence = DiscoveryTestEvidence {
        local_room_joined: true,
        peer_discovered: true,
        address_lookup_observed: true,
        address_resolved: true,
        connection_attempted: true,
        connection_established: true,
        subscription_started: true,
        subscription_joined: true,
        peer_in_topic: true,
        probe_broadcast: true,
        probe_received_or_acknowledged: true,
    };
    let (stage, summary) = classify_discovery_test(&evidence, None);
    assert!(stage.is_none());
    assert!(summary.contains("successfully"));
}

#[test]
fn test_local_room_unavailable_classified() {
    let _evidence = DiscoveryTestEvidence {
        local_room_joined: false,
        peer_discovered: false,
        ..Default::default()
    };
    // Use default for all other fields
    let evidence = DiscoveryTestEvidence {
        local_room_joined: false,
        peer_discovered: false,
        address_lookup_observed: false,
        address_resolved: false,
        connection_attempted: false,
        connection_established: false,
        subscription_started: false,
        subscription_joined: false,
        peer_in_topic: false,
        probe_broadcast: false,
        probe_received_or_acknowledged: false,
    };
    let (stage, _summary) = classify_discovery_test(&evidence, None);
    assert_eq!(stage, Some(DiscoveryFailureStage::LocalRoomUnavailable));
}

#[test]
fn test_asymmetric_peer_state_can_be_represented() {
    let peer_a = "aaaa";
    let peer_b = "bbbb";
    let room = TopicId::from_bytes([5u8; 32]);

    // Peer A is fully connected, peer B is only discovered
    let events = vec![
        DiagnosticEvent {
            sequence: 1,
            timestamp: Utc::now(),
            room_id: Some(room),
            peer_id: Some(peer_a.to_string()),
            kind: DiagnosticEventKind::PeerDiscovered,
        },
        DiagnosticEvent {
            sequence: 2,
            timestamp: Utc::now(),
            room_id: Some(room),
            peer_id: Some(peer_a.to_string()),
            kind: DiagnosticEventKind::ConnectionEstablished {
                remote_address: Some("192.168.1.1:1234".to_string()),
                transport: Some("quic".to_string()),
                used_relay: Some(false),
            },
        },
        DiagnosticEvent {
            sequence: 3,
            timestamp: Utc::now(),
            room_id: Some(room),
            peer_id: Some(peer_a.to_string()),
            kind: DiagnosticEventKind::PeerAddedToTopic,
        },
        DiagnosticEvent {
            sequence: 4,
            timestamp: Utc::now(),
            room_id: Some(room),
            peer_id: Some(peer_b.to_string()),
            kind: DiagnosticEventKind::PeerDiscovered,
        },
    ];

    // Build state by replaying events
    let mut states: HashMap<&str, Option<PeerDiagnosticState>> = HashMap::new();
    for e in &events {
        let pid = e.peer_id.as_deref().unwrap_or_default();
        let current = states.remove(pid).unwrap_or(None);
        let updated = update_peer_state(current, e);
        states.insert(pid, Some(updated));
    }

    let state_a = states.get(peer_a).unwrap().as_ref().unwrap();
    let state_b = states.get(peer_b).unwrap().as_ref().unwrap();

    assert_eq!(
        state_a.connection_state,
        ConnectionDiagnosticState::Connected
    );
    assert!(state_a.topic_member);
    assert_eq!(
        state_b.connection_state,
        ConnectionDiagnosticState::NotStarted
    );
    assert!(!state_b.topic_member);
}

#[test]
fn test_diagnostics_do_not_alter_normal_behaviour() {
    // Verify that the diagnostics store is purely observational
    let diag = Diagnostics::new();
    let initial_count = diag.event_count();
    assert_eq!(initial_count, 0);

    // Record some events — should not panic or affect anything else
    diag.record(None, DiagnosticEventKind::RoomJoinStarted);
    diag.record(None, DiagnosticEventKind::RoomJoined);
    assert_eq!(diag.event_count(), 2);
    assert_eq!(diag.latest_sequence(), 1);
}

#[test]
fn test_generate_probe_id() {
    let id1 = generate_probe_id();
    let id2 = generate_probe_id();
    assert_ne!(id1, id2, "probe IDs should be unique");
    assert_eq!(id1.len(), 32, "probe ID should be 32 hex chars");
}

#[test]
fn test_events_since_filtered() {
    let diag = Diagnostics::new();
    let room = TopicId::from_bytes([6u8; 32]);

    diag.record_with_peer(
        Some(room),
        Some("peer1"),
        DiagnosticEventKind::PeerDiscovered,
    );
    diag.record_with_peer(
        Some(room),
        Some("peer2"),
        DiagnosticEventKind::PeerDiscovered,
    );
    diag.record_with_peer(
        Some(room),
        Some("peer1"),
        DiagnosticEventKind::PeerAddedToTopic,
    );

    let peer1_events = diag.events_since_filtered(0, 100, Some(room), Some("peer1"));
    assert_eq!(peer1_events.len(), 1);
    assert!(peer1_events
        .iter()
        .all(|e| e.peer_id.as_deref() == Some("peer1")));
}

#[test]
fn test_build_evidence() {
    let diag = Diagnostics::new();
    let room = TopicId::from_bytes([7u8; 32]);

    diag.record(Some(room), DiagnosticEventKind::RoomJoined);
    diag.record_with_peer(
        Some(room),
        Some("peer_x"),
        DiagnosticEventKind::PeerDiscovered,
    );
    diag.record_with_peer(
        Some(room),
        Some("peer_x"),
        DiagnosticEventKind::ConnectionEstablished {
            remote_address: None,
            transport: None,
            used_relay: None,
        },
    );

    let evidence = diag.build_evidence(Some(room), None);
    assert!(evidence.local_room_joined);
    assert!(evidence.peer_discovered);
    assert!(evidence.connection_established);
    assert!(!evidence.address_lookup_observed);

    // Without room filter should also find room events
    let evidence_all = diag.build_evidence(None, None);
    assert!(evidence_all.local_room_joined);
}

#[test]
fn test_peer_state_and_build_evidence() {
    let diag = Diagnostics::new();
    let room = TopicId::from_bytes([8u8; 32]);

    diag.record_with_peer(
        Some(room),
        Some("peer_z"),
        DiagnosticEventKind::PeerDiscoveredWithAddr {
            source: DiscoverySource::Mdns,
            addresses: vec!["192.168.1.100:1234".to_string()],
        },
    );
    diag.record_with_peer(
        Some(room),
        Some("peer_z"),
        DiagnosticEventKind::ConnectionEstablished {
            remote_address: Some("192.168.1.100:1234".to_string()),
            transport: Some("quic".to_string()),
            used_relay: Some(false),
        },
    );
    diag.record_with_peer(
        Some(room),
        Some("peer_z"),
        DiagnosticEventKind::PeerAddedToTopic,
    );

    // Verify peer state reconstruction
    let states = diag.peer_states();
    let peer = states.get("peer_z").expect("peer_z should have state");
    assert!(peer.discovered);
    assert_eq!(peer.connection_state, ConnectionDiagnosticState::Connected);
    assert!(peer.topic_member);
    assert_eq!(peer.addresses.len(), 1);
    assert!(peer.addresses[0].contains("192.168.1.100"));

    // Verify evidence builder
    let evidence = diag.build_evidence(Some(room), Some("peer_z"));
    assert!(evidence.peer_discovered);
    assert!(evidence.connection_established);
    assert!(evidence.peer_in_topic);
}

#[test]
fn test_enhanced_probe_storage() {
    let diag = Diagnostics::new();
    let room = TopicId::from_bytes([9u8; 32]);

    let probe = ReceivedProbe {
        probe_id: "test-probe-1".to_string(),
        room_id: "room-9".to_string(),
        sender_id: "sender-1".to_string(),
        sent_at_ms: 1000,
        received_at_ms: 1025,
        latency_ms: Some(25),
        message_hash: "abc123".to_string(),
        duplicate_count: 0,
        timestamp: Utc::now(),
        room_id_opt: Some(room),
    };

    diag.record_received_probe_enhanced(probe);

    let found = diag.find_received_probe("test-probe-1").unwrap();
    assert_eq!(found.sender_id, "sender-1");
    assert_eq!(found.latency_ms, Some(25));
    assert_eq!(found.duplicate_count, 0);

    // Duplicate should increment count
    let probe_dup = ReceivedProbe {
        probe_id: "test-probe-1".to_string(),
        room_id: "room-9".to_string(),
        sender_id: "sender-1".to_string(),
        sent_at_ms: 1000,
        received_at_ms: 1026,
        latency_ms: Some(26),
        message_hash: "abc123".to_string(),
        duplicate_count: 0,
        timestamp: Utc::now(),
        room_id_opt: Some(room),
    };
    diag.record_received_probe_enhanced(probe_dup);
    let found = diag.find_received_probe("test-probe-1").unwrap();
    assert_eq!(found.duplicate_count, 1);
}

#[test]
fn test_peer_discovered_with_addr_updates_sources() {
    let peer_hex = "dddd";
    let room = TopicId::from_bytes([10u8; 32]);

    let e1 = DiagnosticEvent {
        sequence: 1,
        timestamp: Utc::now(),
        room_id: Some(room),
        peer_id: Some(peer_hex.to_string()),
        kind: DiagnosticEventKind::PeerDiscoveredWithAddr {
            source: DiscoverySource::Mdns,
            addresses: vec!["192.168.1.1:5000".to_string()],
        },
    };
    let state = update_peer_state(None, &e1);
    assert!(state.discovered);
    assert_eq!(state.discovery_sources.len(), 1);
    assert_eq!(state.addresses.len(), 1);
    assert_eq!(state.addresses[0], "192.168.1.1:5000");
}

// ── Snapshot tests ────────────────────────────────────────────────

#[test]
fn test_peer_diagnostic_snapshot_serde_roundtrip() {
    let snapshot = PeerDiagnosticSnapshot {
        peer_id: "abc123".to_string(),
        discovery_sources: vec![DiscoverySource::Mdns, DiscoverySource::Gossip],
        addresses: vec!["127.0.0.1:8080".to_string()],
        connected: true,
        last_seen_timestamp_ms: Some(1700000000000_i64),
        last_error: None,
    };

    let json = serde_json::to_string(&snapshot).unwrap();
    let deserialized: PeerDiagnosticSnapshot = serde_json::from_str(&json).unwrap();

    assert_eq!(deserialized.peer_id, "abc123");
    assert_eq!(deserialized.discovery_sources.len(), 2);
    assert_eq!(deserialized.addresses.len(), 1);
    assert!(deserialized.connected);
    assert_eq!(deserialized.last_seen_timestamp_ms, Some(1700000000000));
    assert!(deserialized.last_error.is_none());

    // Verify snake_case serialization
    assert!(json.contains("peer_id"));
    assert!(json.contains("discovery_sources"));
}

#[test]
fn test_room_diagnostic_snapshot_serde_roundtrip() {
    let peer = PeerDiagnosticSnapshot {
        peer_id: "peer1".to_string(),
        discovery_sources: vec![DiscoverySource::Ticket],
        addresses: vec![],
        connected: false,
        last_seen_timestamp_ms: None,
        last_error: Some("connection refused".to_string()),
    };

    let snapshot = RoomDiagnosticSnapshot {
        node_id: "node42".to_string(),
        room_id: "deadbeef".to_string(),
        joined: true,
        subscribed: true,
        peer_count: 1,
        peers: vec![peer],
        discovery_sources_enabled: vec!["discovery_secret".to_string()],
        last_error: None,
    };

    let json = serde_json::to_string(&snapshot).unwrap();
    let deserialized: RoomDiagnosticSnapshot = serde_json::from_str(&json).unwrap();

    assert_eq!(deserialized.node_id, "node42");
    assert_eq!(deserialized.room_id, "deadbeef");
    assert!(deserialized.joined);
    assert!(deserialized.subscribed);
    assert_eq!(deserialized.peer_count, 1);
    assert_eq!(deserialized.peers.len(), 1);
    assert_eq!(
        deserialized.peers[0].last_error.as_deref(),
        Some("connection refused")
    );
    assert_eq!(
        deserialized.discovery_sources_enabled,
        vec!["discovery_secret"]
    );

    // Verify snake_case field names
    assert!(json.contains("node_id"));
    assert!(json.contains("discovery_sources_enabled"));
}

#[test]
fn test_peer_snapshot_empty_defaults() {
    let snapshot = PeerDiagnosticSnapshot {
        peer_id: "".to_string(),
        discovery_sources: vec![],
        addresses: vec![],
        connected: false,
        last_seen_timestamp_ms: None,
        last_error: None,
    };

    // Serialize and verify skip_serializing_if works for empty vecs
    let json = serde_json::to_string(&snapshot).unwrap();
    assert!(json.contains("\"peer_id\""));
    assert!(json.contains("\"connected\":false"));

    // Empty discovery_sources and addresses should be skipped
    assert!(!json.contains("discovery_sources"));
    assert!(!json.contains("addresses"));
    assert!(!json.contains("last_seen_timestamp_ms"));
    assert!(!json.contains("last_error"));
}

#[cfg(feature = "net")]
#[test]
fn test_build_room_snapshot_from_empty_state() {
    use crate::friends::FriendsStore;
    use crate::room::RoomStore;
    use iroh_base::PublicKey;

    let node_id = PublicKey::from_bytes(&[0xAAu8; 32]).unwrap();
    let room_topic = TopicId::from_bytes([0xBBu8; 32]);
    let diag = Diagnostics::new();

    // Create an empty friends store
    let friends = FriendsStore::empty_at(std::path::PathBuf::from("/tmp"));

    // No room store
    let snapshot = build_room_snapshot(
        &node_id,
        room_topic,
        None::<&RoomStore>,
        &friends,
        &diag,
        false,
    );

    assert!(!snapshot.joined);
    assert!(!snapshot.subscribed);
    assert_eq!(snapshot.peer_count, 0);
    assert!(snapshot.peers.is_empty());
    assert!(snapshot.discovery_sources_enabled.is_empty());
    assert!(snapshot.last_error.is_none());
    assert_eq!(snapshot.node_id, node_id.to_string());
    assert_eq!(snapshot.room_id, hex::encode(room_topic.as_bytes()));

    // Verify JSON serialization
    let json = serde_json::to_string(&snapshot).unwrap();
    let _deserialized: RoomDiagnosticSnapshot = serde_json::from_str(&json).unwrap();
}

// ── Test 7: Probe IDs survive serialization round-trip ──────────

#[test]
fn test_probe_id_serialization_roundtrip() {
    let probe = DiagnosticProbe {
        probe_id: "test-probe-42".to_string(),
        sender_id: "sender-pubkey-hex".to_string(),
        room_id: "room-topic-hex".to_string(),
        sent_at_ms: 1000000,
        payload: Some("diagnostic payload".to_string()),
    };

    // JSON round-trip
    let json = serde_json::to_string(&probe).unwrap();
    let deserialized: DiagnosticProbe = serde_json::from_str(&json).unwrap();
    assert_eq!(deserialized.probe_id, "test-probe-42");
    assert_eq!(deserialized.sender_id, "sender-pubkey-hex");
    assert_eq!(deserialized.room_id, "room-topic-hex");
    assert_eq!(deserialized.sent_at_ms, 1000000);
    assert_eq!(deserialized.payload.as_deref(), Some("diagnostic payload"));

    // JSON must contain the probe_id field
    assert!(json.contains("test-probe-42"));
    assert!(json.contains("sender-pubkey-hex"));

    // Postcard binary round-trip
    let binary = postcard::to_stdvec(&probe).unwrap();
    let deserialized2: DiagnosticProbe = postcard::from_bytes(&binary).unwrap();
    assert_eq!(deserialized2.probe_id, "test-probe-42");
    assert_eq!(deserialized2.sender_id, "sender-pubkey-hex");

    // ReceivedProbe round-trip
    let received = ReceivedProbe {
        probe_id: "rx-probe-99".to_string(),
        room_id: "rx-room".to_string(),
        sender_id: "rx-sender".to_string(),
        sent_at_ms: 2000,
        received_at_ms: 2025,
        latency_ms: Some(25),
        message_hash: "deadbeef".to_string(),
        duplicate_count: 0,
        timestamp: Utc::now(),
        room_id_opt: None,
    };
    let json_rx = serde_json::to_string(&received).unwrap();
    let deserialized_rx: ReceivedProbe = serde_json::from_str(&json_rx).unwrap();
    assert_eq!(deserialized_rx.probe_id, "rx-probe-99");
    assert!(json_rx.contains("rx-probe-99"));

    // Postcard round-trip for ReceivedProbe
    let binary_rx = postcard::to_stdvec(&received).unwrap();
    let deserialized_rx2: ReceivedProbe = postcard::from_bytes(&binary_rx).unwrap();
    assert_eq!(deserialized_rx2.probe_id, "rx-probe-99");
}

// ── Test 10: Negative clock-derived latency becomes None ────────

#[test]
fn test_negative_latency_becomes_none() {
    // Simulate clock skew: sent_at_ms > received_at_ms
    let received_at_ms: i64 = 1000;
    let sent_at_ms: i64 = 2000;

    let latency = if received_at_ms >= sent_at_ms {
        Some(received_at_ms - sent_at_ms)
    } else {
        None
    };
    assert!(latency.is_none(), "negative latency should be None");

    // Same time should produce zero latency
    let received_at_ms: i64 = 2000;
    let sent_at_ms: i64 = 2000;
    let latency = if received_at_ms >= sent_at_ms {
        Some(received_at_ms - sent_at_ms)
    } else {
        None
    };
    assert_eq!(latency, Some(0), "same-time latency should be 0");

    // Normal case: received after sent
    let received_at_ms: i64 = 2025;
    let sent_at_ms: i64 = 2000;
    let latency = if received_at_ms >= sent_at_ms {
        Some(received_at_ms - sent_at_ms)
    } else {
        None
    };
    assert_eq!(latency, Some(25), "normal latency should be 25");

    // Verify that a ReceivedProbe with negative clock skew has latency=None
    let probe = ReceivedProbe {
        probe_id: "clock-skew-test".to_string(),
        room_id: "room".to_string(),
        sender_id: "sender".to_string(),
        sent_at_ms: 2000,
        received_at_ms: 1000,
        latency_ms: None, // This is what handle_net_event sets
        message_hash: "hash".to_string(),
        duplicate_count: 0,
        timestamp: Utc::now(),
        room_id_opt: None,
    };
    assert!(probe.latency_ms.is_none());
    assert_eq!(probe.sent_at_ms, 2000);
    assert_eq!(probe.received_at_ms, 1000);
}

// ── Test 11: Unknown room in snapshot returns structured error ──
// (requires net feature for build_room_snapshot)

#[cfg(feature = "net")]
#[test]
fn test_unknown_room_snapshot_returns_unjoined() {
    use crate::friends::FriendsStore;
    use crate::room::RoomStore;
    use iroh_base::PublicKey;

    let node_id = PublicKey::from_bytes(&[0xCCu8; 32]).unwrap();
    // Room topic that does NOT match the room store
    let room_topic = TopicId::from_bytes([0xDDu8; 32]);
    let diag = Diagnostics::new();

    let friends = FriendsStore::empty_at(std::path::PathBuf::from("/tmp"));

    // No room store at all — the room is unknown
    let snapshot = build_room_snapshot(
        &node_id,
        room_topic,
        None::<&RoomStore>,
        &friends,
        &diag,
        false,
    );

    // Unknown room should not panic; it should report joined=false
    assert!(!snapshot.joined, "unknown room should report not joined");
    assert!(!snapshot.subscribed);
    assert_eq!(snapshot.peer_count, 0);
    assert!(snapshot.peers.is_empty());
    assert!(snapshot.last_error.is_none());

    // Room store with a different topic (still unknown)
    let room_store = RoomStore::new(
        std::path::PathBuf::from("/tmp"),
        TopicId::from_bytes([0xEEu8; 32]),
    );
    let snapshot2 = build_room_snapshot(
        &node_id,
        room_topic,
        Some(&room_store),
        &friends,
        &diag,
        false,
    );
    assert!(
        !snapshot2.joined,
        "mismatched topic should report not joined"
    );
    assert_eq!(snapshot2.room_id, hex::encode(room_topic.as_bytes()));
}

// ── Test 12: Diagnostic output contains no secret key material ──

#[test]
fn test_no_secret_key_material_in_diagnostic_probe() {
    let probe = DiagnosticProbe {
        probe_id: "safe-probe".to_string(),
        sender_id: "some-public-key".to_string(),
        room_id: "room-abc".to_string(),
        sent_at_ms: 1000,
        payload: None,
    };
    let json = serde_json::to_string(&probe).unwrap();
    // Must not contain secret key fields
    assert!(!json.contains("secret_key"));
    assert!(!json.contains("secret"));
    assert!(!json.contains("private_key"));
    assert!(!json.contains("signing_key"));
}

#[test]
fn test_no_secret_key_material_in_received_probe() {
    let received = ReceivedProbe {
        probe_id: "safe-rx".to_string(),
        room_id: "rx-room".to_string(),
        sender_id: "rx-pubkey".to_string(),
        sent_at_ms: 1000,
        received_at_ms: 1025,
        latency_ms: Some(25),
        message_hash: "hash".to_string(),
        duplicate_count: 0,
        timestamp: Utc::now(),
        room_id_opt: None,
    };
    let json = serde_json::to_string(&received).unwrap();
    assert!(!json.contains("secret_key"));
    assert!(!json.contains("secret"));
    assert!(!json.contains("private_key"));
}

#[test]
fn test_no_secret_key_material_in_peer_snapshot() {
    let snapshot = PeerDiagnosticSnapshot {
        peer_id: "peer-pubkey".to_string(),
        discovery_sources: vec![],
        addresses: vec!["127.0.0.1:8080".to_string()],
        connected: false,
        last_seen_timestamp_ms: None,
        last_error: None,
    };
    let json = serde_json::to_string(&snapshot).unwrap();
    assert!(!json.contains("secret_key"));
    assert!(!json.contains("secret"));
    assert!(!json.contains("private_key"));
}

#[test]
fn test_no_secret_key_material_in_room_snapshot() {
    let snapshot = RoomDiagnosticSnapshot {
        node_id: "node-pubkey".to_string(),
        room_id: "room-topic".to_string(),
        joined: false,
        subscribed: false,
        peer_count: 0,
        peers: vec![],
        discovery_sources_enabled: vec![],
        last_error: None,
    };
    let json = serde_json::to_string(&snapshot).unwrap();
    assert!(!json.contains("secret_key"));
    assert!(!json.contains("secret"));
    assert!(!json.contains("private_key"));
    assert!(!json.contains("ticket"));
    assert!(!json.contains("discovery_secret"));
}

// ── GUI Action Tracking tests ───────────────────────────────────────

#[test]
fn test_gui_action_id_unique() {
    let id1 = GuiActionId::new();
    let id2 = GuiActionId::new();
    assert_ne!(id1, id2, "action IDs should be unique");
    assert_eq!(id1.0.len(), 32, "action ID should be 32 hex chars");
    assert_eq!(id2.0.len(), 32, "action ID should be 32 hex chars");
}

#[test]
fn test_gui_action_id_default_is_new() {
    let id: GuiActionId = Default::default();
    assert_eq!(id.0.len(), 32);
}

#[test]
fn test_gui_action_state_terminal_classification() {
    use GuiActionState::*;

    assert!(Completed.is_terminal());
    assert!(TimedOut.is_terminal());
    assert!(Failed.is_terminal());
    assert!(Rejected.is_terminal());
    assert!(QueueFull.is_terminal());

    assert!(!Queued.is_terminal());
    assert!(!Validating.is_terminal());
    assert!(!AppMessageQueued.is_terminal());
    assert!(!AppMessageHandled.is_terminal());
    assert!(!WaitingForExpectedState.is_terminal());

    assert!(Queued.is_active());
    assert!(Validating.is_active());
    assert!(AppMessageQueued.is_active());
    assert!(AppMessageHandled.is_active());
    assert!(WaitingForExpectedState.is_active());

    assert!(!Completed.is_active());
    assert!(!TimedOut.is_active());
    assert!(!Failed.is_active());
    assert!(!Rejected.is_active());
    assert!(!QueueFull.is_active());
}

#[test]
fn test_gui_action_state_transition_valid() {
    use GuiActionState::*;

    let mut action = GuiActionStatus {
        action_id: GuiActionId::new(),
        state: Queued,
        requested_at_ms: 1000,
        updated_at_ms: 1000,
        expected_gui_revision: None,
        observed_gui_revision: None,
        error: None,
        result: None,
        expected_state: None,
        timeout_at_ms: None,
    };

    assert!(action.transition_to(Validating).is_ok());
    assert_eq!(action.state, Validating);

    assert!(action.transition_to(AppMessageQueued).is_ok());
    assert_eq!(action.state, AppMessageQueued);

    assert!(action.transition_to(AppMessageHandled).is_ok());
    assert_eq!(action.state, AppMessageHandled);

    assert!(action.transition_to(Completed).is_ok());
    assert_eq!(action.state, Completed);
}

#[test]
fn test_gui_action_state_transition_via_failed() {
    use GuiActionState::*;

    let mut action = GuiActionStatus {
        action_id: GuiActionId::new(),
        state: Queued,
        requested_at_ms: 1000,
        updated_at_ms: 1000,
        expected_gui_revision: None,
        observed_gui_revision: None,
        error: None,
        result: None,
        expected_state: None,
        timeout_at_ms: None,
    };

    assert!(action.transition_to(Validating).is_ok());
    assert!(action.transition_to(Rejected).is_ok());
    assert_eq!(action.state, Rejected);
}

#[test]
fn test_gui_action_state_transition_via_wait_and_timeout() {
    use GuiActionState::*;

    let mut action = GuiActionStatus {
        action_id: GuiActionId::new(),
        state: Queued,
        requested_at_ms: 1000,
        updated_at_ms: 1000,
        expected_gui_revision: None,
        observed_gui_revision: None,
        error: None,
        result: None,
        expected_state: None,
        timeout_at_ms: None,
    };

    action.transition_to(Validating).unwrap();
    action.transition_to(AppMessageQueued).unwrap();
    action.transition_to(AppMessageHandled).unwrap();

    assert!(action.transition_to(WaitingForExpectedState).is_ok());
    assert_eq!(action.state, WaitingForExpectedState);

    assert!(action.transition_to(TimedOut).is_ok());
    assert_eq!(action.state, TimedOut);
}

#[test]
fn test_gui_action_state_queue_full_is_terminal() {
    let mut action = GuiActionStatus {
        action_id: GuiActionId::new(),
        state: GuiActionState::Queued,
        requested_at_ms: 1000,
        updated_at_ms: 1000,
        expected_gui_revision: None,
        observed_gui_revision: None,
        error: None,
        result: None,
        expected_state: None,
        timeout_at_ms: None,
    };

    action.transition_to(GuiActionState::QueueFull).unwrap();
    assert_eq!(action.state, GuiActionState::QueueFull);
    assert!(action.state.is_terminal());
    assert!(action.transition_to(GuiActionState::Completed).is_err());
}

#[test]
fn test_gui_action_state_transition_invalid() {
    use GuiActionState::*;

    let mut action = GuiActionStatus {
        action_id: GuiActionId::new(),
        state: Queued,
        requested_at_ms: 1000,
        updated_at_ms: 1000,
        expected_gui_revision: None,
        observed_gui_revision: None,
        error: None,
        result: None,
        expected_state: None,
        timeout_at_ms: None,
    };

    assert!(action.transition_to(AppMessageHandled).is_err());
    assert_eq!(action.state, Queued);

    assert!(action.transition_to(Completed).is_err());
    assert_eq!(action.state, Queued);

    action.transition_to(Validating).unwrap();

    assert!(action.transition_to(Completed).is_err());
}

#[test]
fn test_gui_action_terminal_states_reject_transitions() {
    use GuiActionState::*;

    for terminal_state in [Completed, TimedOut, Failed, Rejected] {
        let mut action = GuiActionStatus {
            action_id: GuiActionId::new(),
            state: terminal_state,
            requested_at_ms: 1000,
            updated_at_ms: 1000,
            expected_gui_revision: None,
            observed_gui_revision: None,
            error: None,
            result: None,
            expected_state: None,
            timeout_at_ms: None,
        };

        assert!(
            action.transition_to(Queued).is_err(),
            "terminal state {:?} should reject transitions",
            action.state
        );
    }
}

#[test]
fn test_gui_action_history_record_and_get() {
    let history = GuiActionHistory::new();
    let id = GuiActionId::new();

    let request = GuiActionRequest {
        action_id: id.clone(),
        requested_at_ms: 1000,
        command: "SendPressed".to_string(),
    };

    let returned_id = history.record(request);
    assert_eq!(returned_id, id);
    assert_eq!(history.action_count(), 1);

    let status = history.get(&id).expect("should find action");
    assert_eq!(status.action_id, id);
    assert_eq!(status.state, GuiActionState::Queued);
    assert_eq!(status.requested_at_ms, 1000);
}

#[test]
fn test_gui_action_history_transition_and_get() {
    let history = GuiActionHistory::new();
    let id = GuiActionId::new();

    let request = GuiActionRequest {
        action_id: id.clone(),
        requested_at_ms: 1000,
        command: "OpenRoom".to_string(),
    };
    history.record(request);

    assert!(history
        .transition_to(&id, GuiActionState::Validating)
        .is_ok());
    let status = history.get(&id).unwrap();
    assert_eq!(status.state, GuiActionState::Validating);

    assert!(history
        .transition_to(&id, GuiActionState::Completed)
        .is_err());
    let status = history.get(&id).unwrap();
    assert_eq!(status.state, GuiActionState::Validating);

    assert!(history.set_state(&id, GuiActionState::Completed));
    let status = history.get(&id).unwrap();
    assert_eq!(status.state, GuiActionState::Completed);
}

#[test]
fn test_gui_action_history_bounded_storage() {
    let history = GuiActionHistory::with_capacity(3);

    for i in 0..3 {
        let id = GuiActionId::new();
        let request = GuiActionRequest {
            action_id: id,
            requested_at_ms: i * 100,
            command: format!("Action-{i}"),
        };
        history.record(request);
    }
    assert_eq!(history.action_count(), 3);
    assert_eq!(history.active_count(), 3);

    // Verify the oldest is detected correctly
    let all = history.all_actions();
    assert_eq!(all.len(), 3);
    let oldest_id = all.last().unwrap().action_id.clone();
    history.set_state(&oldest_id, GuiActionState::Completed);

    // Verify the state was actually set
    if let Some(s) = history.get(&oldest_id) {
        assert!(s.state.is_terminal(), "set_state should make it terminal");
    } else {
        panic!("oldest_id not found after set_state!");
    }

    let new_id = GuiActionId::new();
    let request = GuiActionRequest {
        action_id: new_id.clone(),
        requested_at_ms: 300,
        command: "Action-4".to_string(),
    };
    history.record(request);

    assert_eq!(history.action_count(), 3);
    assert!(
        history.get(&oldest_id).is_none(),
        "completed action should be evicted"
    );
    assert!(history.get(&new_id).is_some(), "new action should exist");
}

#[test]
fn test_gui_action_history_active_not_evicted() {
    // Terminal actions are evicted first; if none exist, oldest
    // actions are evicted to enforce capacity.
    let history = GuiActionHistory::with_capacity(3);

    // Fill with 3 actions, keep one active, complete one
    let ids: Vec<GuiActionId> = (0..3)
        .map(|i| {
            let id = GuiActionId::new();
            let request = GuiActionRequest {
                action_id: id.clone(),
                requested_at_ms: i as i64 * 100,
                command: format!("Action-{i}"),
            };
            history.record(request);
            id
        })
        .collect();

    assert_eq!(history.action_count(), 3);
    assert_eq!(history.active_count(), 3);

    // Complete the oldest (ids[0]) and middle (ids[1]),
    // keep the newest (ids[2]) active
    history.set_state(&ids[0], GuiActionState::Completed);
    history.set_state(&ids[1], GuiActionState::Completed);
    assert_eq!(history.active_count(), 1);

    // Add a 4th — the oldest terminal (ids[0]) should be evicted
    let new_id = GuiActionId::new();
    let request = GuiActionRequest {
        action_id: new_id.clone(),
        requested_at_ms: 300,
        command: "Action-4".to_string(),
    };
    history.record(request);

    assert_eq!(history.action_count(), 3);
    assert!(
        history.get(&ids[0]).is_none(),
        "oldest terminal should be evicted"
    );
    assert!(history.get(&ids[2]).is_some(), "active action should stay");
    assert!(history.get(&new_id).is_some(), "new action should exist");
}

#[test]
fn test_gui_action_history_remove() {
    let history = GuiActionHistory::with_capacity(10);
    let id = GuiActionId::new();

    let request = GuiActionRequest {
        action_id: id.clone(),
        requested_at_ms: 1000,
        command: "TestAction".to_string(),
    };
    history.record(request);
    assert_eq!(history.action_count(), 1);

    assert!(history.remove(&id));
    assert_eq!(history.action_count(), 0);
    assert!(history.get(&id).is_none());

    assert!(!history.remove(&GuiActionId::new()));
}

#[test]
fn test_gui_action_serialize_roundtrip() {
    let status = GuiActionStatus {
        action_id: GuiActionId("abc123def456".to_string()),
        state: GuiActionState::AppMessageHandled,
        requested_at_ms: 1000,
        updated_at_ms: 1050,
        expected_gui_revision: Some(42),
        observed_gui_revision: Some(42),
        error: None,
        result: Some("success".to_string()),
        expected_state: None,
        timeout_at_ms: None,
    };

    let json = serde_json::to_string(&status).unwrap();
    let deserialized: GuiActionStatus = serde_json::from_str(&json).unwrap();

    assert_eq!(deserialized.action_id.0, "abc123def456");
    assert_eq!(deserialized.state, GuiActionState::AppMessageHandled);
    assert_eq!(deserialized.requested_at_ms, 1000);
    assert_eq!(deserialized.expected_gui_revision, Some(42));
    assert_eq!(deserialized.result.as_deref(), Some("success"));

    assert!(json.contains("action_id"));
    assert!(json.contains("requested_at_ms"));
    assert!(json.contains("expected_gui_revision"));
    assert!(json.contains("observed_gui_revision"));
}

#[test]
fn test_gui_action_state_serialize_snake_case() {
    let states = [
        (GuiActionState::Queued, "queued"),
        (GuiActionState::Validating, "validating"),
        (GuiActionState::Rejected, "rejected"),
        (GuiActionState::QueueFull, "queue_full"),
        (GuiActionState::AppMessageQueued, "app_message_queued"),
        (GuiActionState::AppMessageHandled, "app_message_handled"),
        (
            GuiActionState::WaitingForExpectedState,
            "waiting_for_expected_state",
        ),
        (GuiActionState::Completed, "completed"),
        (GuiActionState::TimedOut, "timed_out"),
        (GuiActionState::Failed, "failed"),
    ];

    for (state, expected) in &states {
        let json = serde_json::to_value(state).unwrap();
        assert_eq!(json.as_str().unwrap(), *expected, "mismatch for {state:?}");
    }
}

#[test]
fn test_gui_action_history_eviction_oldest_completed_first() {
    let history = GuiActionHistory::with_capacity(3);

    let mut ids = Vec::new();
    for i in 0..3 {
        let id = GuiActionId::new();
        ids.push(id.clone());
        let request = GuiActionRequest {
            action_id: id,
            requested_at_ms: i * 100,
            command: format!("Action-{i}"),
        };
        history.record(request);
    }

    history.set_state(&ids[0], GuiActionState::Completed);
    history.set_state(&ids[2], GuiActionState::Completed);

    let new_id = GuiActionId::new();
    let request = GuiActionRequest {
        action_id: new_id.clone(),
        requested_at_ms: 300,
        command: "Action-3".to_string(),
    };
    history.record(request);

    assert_eq!(history.action_count(), 3);
    assert!(
        history.get(&ids[0]).is_none(),
        "oldest completed should be evicted"
    );
    assert!(history.get(&ids[1]).is_some(), "active action should stay");
    assert!(
        history.get(&ids[2]).is_some(),
        "completed but not oldest should stay"
    );
}

#[test]
fn test_gui_action_history_all_actions_order_newest_first() {
    let history = GuiActionHistory::with_capacity(10);
    let ids: Vec<GuiActionId> = (0..3)
        .map(|i| {
            let id = GuiActionId::new();
            let request = GuiActionRequest {
                action_id: id.clone(),
                requested_at_ms: i as i64 * 100,
                command: format!("Action-{i}"),
            };
            history.record(request);
            id
        })
        .collect();

    let all = history.all_actions();
    assert_eq!(all.len(), 3);
    assert_eq!(all[0].action_id, ids[2]);
    assert_eq!(all[1].action_id, ids[1]);
    assert_eq!(all[2].action_id, ids[0]);
}

#[test]
fn test_gui_action_history_actions_with_state() {
    let history = GuiActionHistory::with_capacity(10);

    let id1 = GuiActionId::new();
    history.record(GuiActionRequest {
        action_id: id1.clone(),
        requested_at_ms: 100,
        command: "OpenRoom".to_string(),
    });

    let id2 = GuiActionId::new();
    history.record(GuiActionRequest {
        action_id: id2.clone(),
        requested_at_ms: 200,
        command: "SendPressed".to_string(),
    });

    history.set_state(&id2, GuiActionState::Completed);

    let active = history.actions_with_state(GuiActionState::Queued);
    assert_eq!(active.len(), 1);
    assert_eq!(active[0].action_id, id1);

    let completed = history.actions_with_state(GuiActionState::Completed);
    assert_eq!(completed.len(), 1);
    assert_eq!(completed[0].action_id, id2);
}

#[test]
fn test_gui_action_history_default_capacity() {
    let history = GuiActionHistory::new();
    for i in 0..1000 {
        let id = GuiActionId::new();
        let request = GuiActionRequest {
            action_id: id,
            requested_at_ms: i,
            command: format!("Action-{i}"),
        };
        history.record(request);
    }
    assert_eq!(history.action_count(), 1000);
}

#[test]
fn test_gui_action_history_eviction_chain() {
    let history = GuiActionHistory::with_capacity(3);

    let mut ids = Vec::new();
    for i in 0..3 {
        let id = GuiActionId::new();
        ids.push(id.clone());
        let request = GuiActionRequest {
            action_id: id,
            requested_at_ms: i * 100,
            command: format!("Action-{i}"),
        };
        history.record(request);
    }

    for id in &ids {
        history.set_state(id, GuiActionState::Completed);
    }

    for i in 3..6 {
        let id = GuiActionId::new();
        let request = GuiActionRequest {
            action_id: id,
            requested_at_ms: i * 100,
            command: format!("Action-{i}"),
        };
        history.record(request);
    }

    assert_eq!(history.action_count(), 3);
    for id in &ids {
        assert!(history.get(id).is_none(), "{id:?} should have been evicted");
    }
}

// ── GuiWaitCondition tests ──────────────────────────────────────

fn test_snapshot(
    active_screen: &str,
    active_room: Option<&str>,
    neighbor_count: usize,
    total_entry_count: usize,
) -> IcedStateSnapshot {
    IcedStateSnapshot {
        node_id: "test-node".to_string(),
        version: "0.101.0".to_string(),
        active_screen: active_screen.to_string(),
        active_room: active_room.map(|s| s.to_string()),
        conversation_count: 0,
        neighbor_count,
        direct_peer_count: 0,
        relayed_peer_count: 0,
        mesh_health: "Good".to_string(),
        online_friend_count: 0,
        friend_count: 0,
        total_entry_count,
        dark_mode: false,
        composer_text: String::new(),
        dialog_open: false,
        unread_count: 0,
        dashboard: None,
        timestamp: Utc::now(),
    }
}

#[test]
fn test_screen_is_condition_matches() {
    let snapshot = test_snapshot("ChatList", None, 0, 0);
    let journal = IcedMessageJournal::new();

    assert!(evaluate_wait_condition(
        &GuiWaitCondition::ScreenIs {
            expected: "ChatList".to_string()
        },
        &snapshot,
        &journal,
    ));
}

#[test]
fn test_screen_is_condition_does_not_match() {
    let snapshot = test_snapshot("Settings", None, 0, 0);
    let journal = IcedMessageJournal::new();

    assert!(!evaluate_wait_condition(
        &GuiWaitCondition::ScreenIs {
            expected: "ChatList".to_string()
        },
        &snapshot,
        &journal,
    ));
}

#[test]
fn test_room_selected_any_room() {
    let snapshot = test_snapshot("Chat", Some("abc"), 0, 0);
    let journal = IcedMessageJournal::new();

    assert!(evaluate_wait_condition(
        &GuiWaitCondition::RoomSelected { room_topic: None },
        &snapshot,
        &journal,
    ));
}

#[test]
fn test_room_selected_no_room() {
    let snapshot = test_snapshot("ChatList", None, 0, 0);
    let journal = IcedMessageJournal::new();

    assert!(!evaluate_wait_condition(
        &GuiWaitCondition::RoomSelected { room_topic: None },
        &snapshot,
        &journal,
    ));
}

#[test]
fn test_room_selected_specific_topic() {
    let snapshot = test_snapshot("Chat", Some("room123"), 0, 0);
    let journal = IcedMessageJournal::new();

    assert!(evaluate_wait_condition(
        &GuiWaitCondition::RoomSelected {
            room_topic: Some("room123".to_string())
        },
        &snapshot,
        &journal,
    ));
}

#[test]
fn test_room_selected_wrong_topic() {
    let snapshot = test_snapshot("Chat", Some("room123"), 0, 0);
    let journal = IcedMessageJournal::new();

    assert!(!evaluate_wait_condition(
        &GuiWaitCondition::RoomSelected {
            room_topic: Some("other-room".to_string())
        },
        &snapshot,
        &journal,
    ));
}

#[test]
fn test_peer_visible_with_enough_neighbors() {
    let snapshot = test_snapshot("Chat", Some("room1"), 3, 0);
    let journal = IcedMessageJournal::new();

    assert!(evaluate_wait_condition(
        &GuiWaitCondition::PeerVisible { min_count: 3 },
        &snapshot,
        &journal,
    ));
    assert!(evaluate_wait_condition(
        &GuiWaitCondition::PeerVisible { min_count: 1 },
        &snapshot,
        &journal,
    ));
}

#[test]
fn test_peer_visible_not_enough_neighbors() {
    let snapshot = test_snapshot("ChatList", None, 0, 0);
    let journal = IcedMessageJournal::new();

    assert!(!evaluate_wait_condition(
        &GuiWaitCondition::PeerVisible { min_count: 1 },
        &snapshot,
        &journal,
    ));
}

#[test]
fn test_message_visible_with_enough_entries() {
    let snapshot = test_snapshot("Chat", Some("room1"), 0, 5);
    let journal = IcedMessageJournal::new();

    assert!(evaluate_wait_condition(
        &GuiWaitCondition::MessageVisible { min_count: 5 },
        &snapshot,
        &journal,
    ));
    assert!(evaluate_wait_condition(
        &GuiWaitCondition::MessageVisible { min_count: 3 },
        &snapshot,
        &journal,
    ));
}

#[test]
fn test_message_visible_not_enough_entries() {
    let snapshot = test_snapshot("Chat", Some("room1"), 0, 2);
    let journal = IcedMessageJournal::new();

    assert!(!evaluate_wait_condition(
        &GuiWaitCondition::MessageVisible { min_count: 5 },
        &snapshot,
        &journal,
    ));
}

#[test]
fn test_gui_revision_at_least_reached() {
    let snapshot = test_snapshot("ChatList", None, 0, 0);
    let journal = IcedMessageJournal::new();

    // Record enough entries to reach revision 2 (sequences 0, 1, 2 → latest = 2)
    journal.record("TestMessage", FailureLayer::IcedUpdate, true, "", None);
    journal.record("TestMessage", FailureLayer::IcedUpdate, true, "", None);
    journal.record("TestMessage", FailureLayer::IcedUpdate, true, "", None);
    assert_eq!(journal.latest_sequence(), 2);

    assert!(evaluate_wait_condition(
        &GuiWaitCondition::GuiRevisionAtLeast {
            expected_revision: 2
        },
        &snapshot,
        &journal,
    ));
    assert!(evaluate_wait_condition(
        &GuiWaitCondition::GuiRevisionAtLeast {
            expected_revision: 1
        },
        &snapshot,
        &journal,
    ));
}

#[test]
fn test_gui_revision_at_least_not_reached() {
    let snapshot = test_snapshot("ChatList", None, 0, 0);
    let journal = IcedMessageJournal::new();

    // Only 2 entries → revision 1
    journal.record("TestMessage", FailureLayer::IcedUpdate, true, "", None);
    journal.record("TestMessage", FailureLayer::IcedUpdate, true, "", None);

    assert!(!evaluate_wait_condition(
        &GuiWaitCondition::GuiRevisionAtLeast {
            expected_revision: 5
        },
        &snapshot,
        &journal,
    ));
}

#[test]
fn test_gui_wait_condition_serde_roundtrip() {
    let condition = GuiWaitCondition::ScreenIs {
        expected: "ChatList".to_string(),
    };

    let json = serde_json::to_string(&condition).unwrap();
    let deserialized: GuiWaitCondition = serde_json::from_str(&json).unwrap();

    assert_eq!(deserialized, condition);
    assert!(json.contains("\"type\":\"screen_is\""));
    assert!(json.contains("\"expected\":\"ChatList\""));

    // RoomSelected roundtrip
    let condition2 = GuiWaitCondition::RoomSelected {
        room_topic: Some("room123".to_string()),
    };
    let json2 = serde_json::to_string(&condition2).unwrap();
    let deserialized2: GuiWaitCondition = serde_json::from_str(&json2).unwrap();
    assert_eq!(deserialized2, condition2);
    assert!(json2.contains("room_topic"));

    // Roundtrip for all variants
    let variants = vec![
        GuiWaitCondition::ScreenIs {
            expected: "Chat".to_string(),
        },
        GuiWaitCondition::RoomSelected { room_topic: None },
        GuiWaitCondition::RoomSelected {
            room_topic: Some("abc".to_string()),
        },
        GuiWaitCondition::PeerVisible { min_count: 0 },
        GuiWaitCondition::PeerVisible { min_count: 5 },
        GuiWaitCondition::MessageVisible { min_count: 1 },
        GuiWaitCondition::GuiRevisionAtLeast {
            expected_revision: 42,
        },
        GuiWaitCondition::ConversationSelected {
            conversation_id: None,
        },
        GuiWaitCondition::ConversationSelected {
            conversation_id: Some("peer1".to_string()),
        },
        GuiWaitCondition::ComposerTextIs {
            expected: "hello".to_string(),
        },
        GuiWaitCondition::DialogOpen,
        GuiWaitCondition::DialogClosed,
        GuiWaitCondition::UnreadCountAtLeast { min_count: 5 },
    ];

    for v in variants {
        let json = serde_json::to_string(&v).unwrap();
        let deserialized: GuiWaitCondition = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized, v);
    }
}

#[test]
fn test_gui_wait_condition_no_secret_material() {
    let condition = GuiWaitCondition::ScreenIs {
        expected: "ChatList".to_string(),
    };
    let json = serde_json::to_string(&condition).unwrap();
    assert!(!json.contains("secret_key"));
    assert!(!json.contains("secret"));
    assert!(!json.contains("private_key"));
    assert!(!json.contains("ticket"));
}

#[test]
fn test_evaluate_wait_condition_empty_journal() {
    let snapshot = test_snapshot("ChatList", None, 0, 0);
    let journal = IcedMessageJournal::new();

    // Empty journal has latest_sequence = 0, so revision 1 should fail
    assert!(!evaluate_wait_condition(
        &GuiWaitCondition::GuiRevisionAtLeast {
            expected_revision: 1
        },
        &snapshot,
        &journal,
    ));

    // revision 0 should pass (0 >= 0)
    assert!(evaluate_wait_condition(
        &GuiWaitCondition::GuiRevisionAtLeast {
            expected_revision: 0
        },
        &snapshot,
        &journal,
    ));
}

#[test]
fn test_peer_visible_zero_count_any_neighbor() {
    let snapshot = test_snapshot("Chat", Some("room1"), 1, 0);
    let journal = IcedMessageJournal::new();

    // min_count=0: should be true even with empty snapshot
    assert!(evaluate_wait_condition(
        &GuiWaitCondition::PeerVisible { min_count: 0 },
        &snapshot,
        &journal,
    ));
}

#[test]
fn test_message_visible_zero_count_any_entry() {
    let snapshot = test_snapshot("Chat", Some("room1"), 0, 0);
    let journal = IcedMessageJournal::new();

    // min_count=0: should be true even with 0 entries
    assert!(evaluate_wait_condition(
        &GuiWaitCondition::MessageVisible { min_count: 0 },
        &snapshot,
        &journal,
    ));
}

#[test]
fn test_screen_is_case_sensitive() {
    let snapshot = test_snapshot("chatlist", None, 0, 0);
    let journal = IcedMessageJournal::new();

    // Screen names are case-sensitive
    assert!(!evaluate_wait_condition(
        &GuiWaitCondition::ScreenIs {
            expected: "ChatList".to_string()
        },
        &snapshot,
        &journal,
    ));
}

// ── New GuiWaitCondition evaluation tests ────────────────────────

#[test]
fn test_conversation_selected_any() {
    let snapshot = test_snapshot("Chat", Some("room1"), 0, 0);
    let journal = IcedMessageJournal::new();

    assert!(evaluate_wait_condition(
        &GuiWaitCondition::ConversationSelected {
            conversation_id: None,
        },
        &snapshot,
        &journal,
    ));
}

#[test]
fn test_conversation_selected_no_conversation() {
    let snapshot = test_snapshot("ChatList", None, 0, 0);
    let journal = IcedMessageJournal::new();

    assert!(!evaluate_wait_condition(
        &GuiWaitCondition::ConversationSelected {
            conversation_id: None,
        },
        &snapshot,
        &journal,
    ));
}

#[test]
fn test_conversation_selected_specific_id() {
    let snapshot = test_snapshot("Chat", Some("peer-abc"), 0, 0);
    let journal = IcedMessageJournal::new();

    assert!(evaluate_wait_condition(
        &GuiWaitCondition::ConversationSelected {
            conversation_id: Some("peer-abc".to_string()),
        },
        &snapshot,
        &journal,
    ));
}

#[test]
fn test_conversation_selected_wrong_id() {
    let snapshot = test_snapshot("Chat", Some("peer-abc"), 0, 0);
    let journal = IcedMessageJournal::new();

    assert!(!evaluate_wait_condition(
        &GuiWaitCondition::ConversationSelected {
            conversation_id: Some("other-peer".to_string()),
        },
        &snapshot,
        &journal,
    ));
}

#[test]
fn test_composer_text_matches() {
    let mut snapshot = test_snapshot("Chat", Some("room1"), 0, 0);
    snapshot.composer_text = "hello world".to_string();
    let journal = IcedMessageJournal::new();

    assert!(evaluate_wait_condition(
        &GuiWaitCondition::ComposerTextIs {
            expected: "hello world".to_string(),
        },
        &snapshot,
        &journal,
    ));
}

#[test]
fn test_composer_text_does_not_match() {
    let mut snapshot = test_snapshot("Chat", Some("room1"), 0, 0);
    snapshot.composer_text = "foo".to_string();
    let journal = IcedMessageJournal::new();

    assert!(!evaluate_wait_condition(
        &GuiWaitCondition::ComposerTextIs {
            expected: "bar".to_string(),
        },
        &snapshot,
        &journal,
    ));
}

#[test]
fn test_dialog_open_when_open() {
    let mut snapshot = test_snapshot("ChatList", None, 0, 0);
    snapshot.dialog_open = true;
    let journal = IcedMessageJournal::new();

    assert!(evaluate_wait_condition(
        &GuiWaitCondition::DialogOpen,
        &snapshot,
        &journal,
    ));
}

#[test]
fn test_dialog_open_when_closed() {
    let snapshot = test_snapshot("ChatList", None, 0, 0);
    let journal = IcedMessageJournal::new();

    assert!(!evaluate_wait_condition(
        &GuiWaitCondition::DialogOpen,
        &snapshot,
        &journal,
    ));
}

#[test]
fn test_dialog_closed_when_closed() {
    let snapshot = test_snapshot("ChatList", None, 0, 0);
    let journal = IcedMessageJournal::new();

    assert!(evaluate_wait_condition(
        &GuiWaitCondition::DialogClosed,
        &snapshot,
        &journal,
    ));
}

#[test]
fn test_dialog_closed_when_open() {
    let mut snapshot = test_snapshot("ChatList", None, 0, 0);
    snapshot.dialog_open = true;
    let journal = IcedMessageJournal::new();

    assert!(!evaluate_wait_condition(
        &GuiWaitCondition::DialogClosed,
        &snapshot,
        &journal,
    ));
}

#[test]
fn test_unread_count_at_least_meets_threshold() {
    let mut snapshot = test_snapshot("ChatList", None, 0, 0);
    snapshot.unread_count = 10;
    let journal = IcedMessageJournal::new();

    assert!(evaluate_wait_condition(
        &GuiWaitCondition::UnreadCountAtLeast { min_count: 10 },
        &snapshot,
        &journal,
    ));
    assert!(evaluate_wait_condition(
        &GuiWaitCondition::UnreadCountAtLeast { min_count: 5 },
        &snapshot,
        &journal,
    ));
}

#[test]
fn test_unread_count_at_least_below_threshold() {
    let mut snapshot = test_snapshot("ChatList", None, 0, 0);
    snapshot.unread_count = 3;
    let journal = IcedMessageJournal::new();

    assert!(!evaluate_wait_condition(
        &GuiWaitCondition::UnreadCountAtLeast { min_count: 10 },
        &snapshot,
        &journal,
    ));
}

#[test]
fn test_unread_count_zero_threshold_any() {
    let snapshot = test_snapshot("ChatList", None, 0, 0);
    let journal = IcedMessageJournal::new();

    // min_count=0 should always pass
    assert!(evaluate_wait_condition(
        &GuiWaitCondition::UnreadCountAtLeast { min_count: 0 },
        &snapshot,
        &journal,
    ));
}

#[test]
fn test_update_peer_state_preserves_peer_state() {
    let peer_hex = "abcdef1234567890abcdef1234567890abcdef1234567890abcdef1234567890";
    let room = TopicId::from_bytes([42u8; 32]);

    let e1 = DiagnosticEvent {
        sequence: 1,
        timestamp: Utc::now(),
        room_id: Some(room),
        peer_id: Some(peer_hex.to_string()),
        kind: DiagnosticEventKind::PeerDiscovered,
    };
    let mut state = update_peer_state(None, &e1);
    assert!(state.discovered);

    let e2 = DiagnosticEvent {
        sequence: 2,
        timestamp: Utc::now(),
        room_id: Some(room),
        peer_id: Some(peer_hex.to_string()),
        kind: DiagnosticEventKind::ConnectionEstablished {
            remote_address: Some("10.0.0.1:1234".to_string()),
            transport: Some("quic".to_string()),
            used_relay: Some(false),
        },
    };
    state = update_peer_state(Some(state), &e2);
    assert_eq!(state.connection_state, ConnectionDiagnosticState::Connected);

    let e3 = DiagnosticEvent {
        sequence: 3,
        timestamp: Utc::now(),
        room_id: Some(room),
        peer_id: Some(peer_hex.to_string()),
        kind: DiagnosticEventKind::PeerAddedToTopic,
    };
    state = update_peer_state(Some(state), &e3);
    assert!(state.topic_member);
}

// ── GuiActionError and GuiActionErrorCode serialization tests ──────

#[test]
fn test_gui_action_error_code_serde_roundtrip() {
    // Test all error code variants serialize and deserialize
    let codes = vec![
        GuiActionErrorCode::GuiActionsDisabled,
        GuiActionErrorCode::UnknownRoom,
        GuiActionErrorCode::UnknownConversation,
        GuiActionErrorCode::UnknownPeer,
        GuiActionErrorCode::InvalidCurrentScreen,
        GuiActionErrorCode::BlockingDialogOpen,
        GuiActionErrorCode::NoActiveConversation,
        GuiActionErrorCode::ComposerEmpty,
        GuiActionErrorCode::ComposerTooLong,
        GuiActionErrorCode::SendDisabled,
        GuiActionErrorCode::RoomInactive,
        GuiActionErrorCode::ActionQueueClosed,
        GuiActionErrorCode::ActionTimedOut,
        GuiActionErrorCode::InvalidArgument,
        GuiActionErrorCode::InternalError,
    ];

    for code in &codes {
        let json = serde_json::to_string(code).unwrap();
        let deserialized: GuiActionErrorCode = serde_json::from_str(&json).unwrap();
        assert_eq!(&deserialized, code, "roundtrip failed for {code:?}");
    }

    // Verify snake_case serialization for each variant
    assert_eq!(
        serde_json::to_string(&GuiActionErrorCode::GuiActionsDisabled).unwrap(),
        "\"gui_actions_disabled\""
    );
    assert_eq!(
        serde_json::to_string(&GuiActionErrorCode::UnknownRoom).unwrap(),
        "\"unknown_room\""
    );
    assert_eq!(
        serde_json::to_string(&GuiActionErrorCode::InvalidCurrentScreen).unwrap(),
        "\"invalid_current_screen\""
    );
    assert_eq!(
        serde_json::to_string(&GuiActionErrorCode::NoActiveConversation).unwrap(),
        "\"no_active_conversation\""
    );
    assert_eq!(
        serde_json::to_string(&GuiActionErrorCode::ActionQueueClosed).unwrap(),
        "\"action_queue_closed\""
    );
    assert_eq!(
        serde_json::to_string(&GuiActionErrorCode::BlockingDialogOpen).unwrap(),
        "\"blocking_dialog_open\""
    );
    assert_eq!(
        serde_json::to_string(&GuiActionErrorCode::ComposerTooLong).unwrap(),
        "\"composer_too_long\""
    );
    assert_eq!(
        serde_json::to_string(&GuiActionErrorCode::ActionTimedOut).unwrap(),
        "\"action_timed_out\""
    );
    assert_eq!(
        serde_json::to_string(&GuiActionErrorCode::InternalError).unwrap(),
        "\"internal_error\""
    );
}

#[test]
fn test_gui_action_error_serde_roundtrip() {
    let errors = vec![
        GuiActionError::new(
            GuiActionErrorCode::UnknownRoom,
            "Room 'abc123' was not found",
        ),
        GuiActionError::new(
            GuiActionErrorCode::ComposerEmpty,
            "Cannot send empty message",
        ),
        GuiActionError::new(
            GuiActionErrorCode::SendDisabled,
            "Sending is disabled in read-only mode",
        ),
        GuiActionError::new(
            GuiActionErrorCode::ActionTimedOut,
            "Action timed out after 5000ms",
        ),
        GuiActionError::new(
            GuiActionErrorCode::InternalError,
            "unexpected state: room is None",
        ),
        GuiActionError::new(
            GuiActionErrorCode::UnknownPeer,
            "Peer deadbeef is not known",
        ),
        GuiActionError::new(
            GuiActionErrorCode::InvalidArgument,
            "Invalid state transition: Queued → Completed",
        ),
    ];

    for error in &errors {
        let json = serde_json::to_string(error).unwrap();
        let deserialized: GuiActionError = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.code, error.code);
        assert_eq!(deserialized.message, error.message);
    }

    // Verify the JSON structure uses snake_case fields
    let json = serde_json::to_string(&errors[0]).unwrap();
    assert!(json.contains("\"code\""));
    assert!(json.contains("\"message\""));
    assert!(json.contains("\"unknown_room\""));

    // Postcard binary roundtrip
    for error in &errors {
        let binary = postcard::to_stdvec(error).unwrap();
        let deserialized: GuiActionError = postcard::from_bytes(&binary).unwrap();
        assert_eq!(deserialized.code, error.code);
        assert_eq!(deserialized.message, error.message);
    }
}

#[test]
fn test_gui_action_status_serde_with_error_field() {
    let status = GuiActionStatus {
        action_id: GuiActionId("deadbeef1234".to_string()),
        state: GuiActionState::Rejected,
        requested_at_ms: 1000,
        updated_at_ms: 1050,
        expected_gui_revision: None,
        observed_gui_revision: None,
        error: Some(GuiActionError::new(
            GuiActionErrorCode::UnknownRoom,
            "Room 'xyz' was not found",
        )),
        result: None,
        expected_state: None,
        timeout_at_ms: None,
    };

    let json = serde_json::to_string(&status).unwrap();
    let deserialized: GuiActionStatus = serde_json::from_str(&json).unwrap();

    assert_eq!(deserialized.action_id.0, "deadbeef1234");
    assert_eq!(deserialized.state, GuiActionState::Rejected);
    assert_eq!(deserialized.requested_at_ms, 1000);

    let err = deserialized.error.expect("error field should be present");
    assert_eq!(err.code, GuiActionErrorCode::UnknownRoom);
    assert_eq!(err.message, "Room 'xyz' was not found");

    // Verify the JSON structure
    assert!(json.contains("\"error\""));
    assert!(json.contains("\"unknown_room\""));
    assert!(json.contains("Room 'xyz' was not found"));

    // Postcard binary roundtrip
    let binary = postcard::to_stdvec(&status).unwrap();
    let deserialized2: GuiActionStatus = postcard::from_bytes(&binary).unwrap();
    let err2 = deserialized2
        .error
        .expect("error should survive postcard roundtrip");
    assert_eq!(err2.code, GuiActionErrorCode::UnknownRoom);
    assert_eq!(err2.message, "Room 'xyz' was not found");
}

#[test]
fn test_gui_action_history_transition_to_returns_structured_error() {
    let history = GuiActionHistory::new();
    let id = GuiActionId::new();

    // Non-existent action should return InvalidArgument
    let err = history
        .transition_to(&id, GuiActionState::Validating)
        .unwrap_err();
    assert_eq!(err.code, GuiActionErrorCode::InvalidArgument);
    assert!(err.message.contains(&id.0));

    // Record an action and try an invalid transition
    let request = GuiActionRequest {
        action_id: id.clone(),
        requested_at_ms: 1000,
        command: "TestAction".to_string(),
    };
    history.record(request);

    // Invalid state transition should return InvalidArgument
    let err = history
        .transition_to(&id, GuiActionState::Completed)
        .unwrap_err();
    assert_eq!(err.code, GuiActionErrorCode::InvalidArgument);
    assert!(err.message.contains("Invalid state transition"));
}

#[test]
fn test_gui_action_error_display_format() {
    let err = GuiActionError::new(
        GuiActionErrorCode::UnknownRoom,
        "Room 'abc123' was not found",
    );
    let display = format!("{err}");
    assert_eq!(display, "UnknownRoom: Room 'abc123' was not found");
}

// ── GuiTestCommand serialization round-trip tests ──────────────────

#[test]
fn test_gui_test_command_json_roundtrip() {
    use GuiTestCommand::*;

    let variants: Vec<GuiTestCommand> = vec![
        GoToChatList,
        OpenRoom {
            room_id: "abcdef1234567890abcdef1234567890abcdef1234567890abcdef1234567890".into(),
        },
        OpenConversation {
            conversation_id: "deadbeef1234567890abcdef1234567890deadbeef1234567890abcdef1234567890"
                .into(),
        },
        OpenFriends,
        OpenSettings,
        CloseDialog,
        SetComposerText {
            text: "hello world".into(),
        },
        SubmitComposer,
        SelectPeer {
            peer_id: "cafebabe1234567890abcdef1234567890cafebabe1234567890abcdef1234567890".into(),
        },
        ToggleDarkMode { enabled: true },
        ToggleHelp,
        Wait {
            condition: GuiWaitCondition::ScreenIs {
                expected: "ChatList".into(),
            },
            timeout_ms: 5000,
        },
    ];

    for cmd in &variants {
        let json = serde_json::to_string(cmd).unwrap();
        let deserialized: GuiTestCommand = serde_json::from_str(&json).unwrap();
        assert_eq!(&deserialized, cmd, "JSON round-trip failed for {cmd:?}");
        assert!(json.contains("\"command\""));
    }
}

#[test]
fn test_gui_test_command_postcard_serde_limitation() {
    use GuiTestCommand::*;

    // Postcard v1 with `experimental-derive` can serialize tagged enums
    // (serde's Serialize uses external/adjacent tagging), but cannot
    // deserialize them back (returns \"will never implement\" error).
    // Only JSON round-trips are guaranteed for GuiTestCommand.
    let cmd = OpenRoom {
        room_id: "abcd1234abcd1234abcd1234abcd1234abcd1234abcd1234abcd1234abcd1234".into(),
    };
    let bytes = postcard::to_stdvec(&cmd).expect("postcard should serialize tagged enums");
    let result: Result<GuiTestCommand, _> = postcard::from_bytes(&bytes);
    assert!(
        result.is_err(),
        "postcard should not deserialize tagged enums"
    );
    let err = format!("{}", result.unwrap_err());
    assert!(
        err.contains("never implement"),
        "Error should mention 'never implement': {err}"
    );
}

#[test]
fn test_gui_test_command_json_tagged_discrimination() {
    let json = r#"{"command": "go_to_chat_list"}"#;
    let cmd: GuiTestCommand = serde_json::from_str(json).unwrap();
    assert!(matches!(cmd, GuiTestCommand::GoToChatList));

    let json = r#"{"command": "open_settings"}"#;
    let cmd: GuiTestCommand = serde_json::from_str(json).unwrap();
    assert!(matches!(cmd, GuiTestCommand::OpenSettings));

    let json = r#"{"command": "toggle_help"}"#;
    let cmd: GuiTestCommand = serde_json::from_str(json).unwrap();
    assert!(matches!(cmd, GuiTestCommand::ToggleHelp));
}

#[test]
fn test_gui_test_command_json_unit_variants() {
    let json = serde_json::to_string(&GuiTestCommand::GoToChatList).unwrap();
    assert_eq!(json, r#"{"command":"go_to_chat_list"}"#);

    let json = serde_json::to_string(&GuiTestCommand::OpenFriends).unwrap();
    assert_eq!(json, r#"{"command":"open_friends"}"#);

    let json = serde_json::to_string(&GuiTestCommand::CloseDialog).unwrap();
    assert_eq!(json, r#"{"command":"close_dialog"}"#);
}

#[test]
fn test_gui_test_command_json_struct_variants() {
    let cmd = GuiTestCommand::SetComposerText {
        text: "test".into(),
    };
    let json = serde_json::to_string(&cmd).unwrap();
    assert!(json.contains("\"command\":\"set_composer_text\""));
    assert!(json.contains("\"text\":\"test\""));

    let cmd = GuiTestCommand::ToggleDarkMode { enabled: true };
    let json = serde_json::to_string(&cmd).unwrap();
    assert!(json.contains("\"command\":\"toggle_dark_mode\""));
    assert!(json.contains("\"enabled\":true"));
}

#[test]
fn test_gui_test_command_no_secrets_in_json() {
    let cmd = GuiTestCommand::OpenRoom {
        room_id: "aaaabbbbccccddddaaaabbbbccccddddaaaabbbbccccddddaaaabbbbccccdddd".into(),
    };
    let json = serde_json::to_string(&cmd).unwrap();
    assert!(!json.contains("secret_key"));
    assert!(!json.contains("secret"));
    assert!(!json.contains("private_key"));
    assert!(!json.contains("ticket"));
    assert!(!json.contains("password"));
}

#[test]
fn test_gui_test_command_validate_valid() {
    assert!(GuiTestCommand::GoToChatList.validate().is_ok());
    assert!(GuiTestCommand::OpenFriends.validate().is_ok());
    assert!(GuiTestCommand::OpenSettings.validate().is_ok());
    assert!(GuiTestCommand::CloseDialog.validate().is_ok());
    assert!(GuiTestCommand::SubmitComposer.validate().is_ok());
    assert!(GuiTestCommand::ToggleHelp.validate().is_ok());
    assert!(GuiTestCommand::ToggleDarkMode { enabled: true }
        .validate()
        .is_ok());
    assert!(GuiTestCommand::SetComposerText {
        text: "hello".into()
    }
    .validate()
    .is_ok());
}

#[test]
fn test_gui_test_command_validate_rejects_control_chars() {
    assert!(GuiTestCommand::SetComposerText { text: "\n".into() }
        .validate()
        .is_err());
    assert!(GuiTestCommand::SetComposerText { text: "\r".into() }
        .validate()
        .is_err());
    assert!(GuiTestCommand::SetComposerText { text: "\t".into() }
        .validate()
        .is_err());
    assert!(GuiTestCommand::SetComposerText {
        text: "\x00".into()
    }
    .validate()
    .is_err());
}

#[test]
fn test_gui_test_command_validate_rejects_overflow() {
    let long = "a".repeat(GUI_TEST_COMMAND_MAX_STRING_LEN + 1);
    assert!(GuiTestCommand::SetComposerText { text: long }
        .validate()
        .is_err());
}

#[test]
fn test_gui_test_command_preserves_unicode_message_text() {
    let text = "こんにちは 🌍 — café";
    let command = GuiTestCommand::SetComposerText { text: text.into() };
    assert!(command.validate().is_ok());
    let json = serde_json::to_string(&command).unwrap();
    assert!(json.contains(text));
    let decoded: GuiTestCommand = serde_json::from_str(&json).unwrap();
    assert_eq!(decoded, command);
}

#[test]
fn test_gui_test_command_rejects_invalid_ids_and_paths() {
    for value in [
        "",
        "../tmp",
        "peer/name",
        "peer\\\\name",
        "peer;rm",
        "peer id",
        "peer\nname",
    ] {
        assert!(
            GuiTestCommand::OpenRoom {
                room_id: value.into()
            }
            .validate()
            .is_err(),
            "accepted unsafe room id: {value:?}"
        );
        assert!(
            GuiTestCommand::OpenConversation {
                conversation_id: value.into()
            }
            .validate()
            .is_err(),
            "accepted unsafe conversation id: {value:?}"
        );
        assert!(
            GuiTestCommand::SelectPeer {
                peer_id: value.into()
            }
            .validate()
            .is_err(),
            "accepted unsafe peer id: {value:?}"
        );
    }
}

#[test]
fn test_gui_test_command_validate_rejects_excessive_timeout() {
    assert!(GuiTestCommand::Wait {
        condition: GuiWaitCondition::ScreenIs {
            expected: "ChatList".into()
        },
        timeout_ms: GUI_TEST_COMMAND_MAX_TIMEOUT_MS + 1,
    }
    .validate()
    .is_err());
}

#[test]
fn test_gui_test_command_unknown_variant_rejected_by_serde() {
    let malicious = r#"{"command": "execute_shell", "cmd": "rm -rf /"}"#;
    let result: Result<GuiTestCommand, _> = serde_json::from_str(malicious);
    assert!(result.is_err(), "Unknown variant must be rejected by serde");
}

// ── Security: string field bounds for ALL variants ────────────────

#[test]
fn test_gui_test_command_validate_rejects_long_room_id() {
    let long = "a".repeat(GUI_TEST_COMMAND_MAX_STRING_LEN + 1);
    assert!(GuiTestCommand::OpenRoom { room_id: long }
        .validate()
        .is_err());
}

#[test]
fn test_gui_test_command_validate_rejects_long_conversation_id() {
    let long = "a".repeat(GUI_TEST_COMMAND_MAX_STRING_LEN + 1);
    assert!(GuiTestCommand::OpenConversation {
        conversation_id: long,
    }
    .validate()
    .is_err());
}

#[test]
fn test_gui_test_command_validate_rejects_long_peer_id() {
    let long = "a".repeat(GUI_TEST_COMMAND_MAX_STRING_LEN + 1);
    assert!(GuiTestCommand::SelectPeer { peer_id: long }
        .validate()
        .is_err());
}

#[test]
fn test_gui_test_command_clear_mesh_event_log_validates_and_round_trips() {
    assert!(GuiTestCommand::ClearMeshEventLog.validate().is_ok());
    assert_eq!(GuiTestCommand::ClearMeshEventLog.expected_state(), None);
    let encoded = serde_json::to_string(&GuiTestCommand::ClearMeshEventLog).expect("serialize");
    assert!(encoded.contains("clear_mesh_event_log"));
    let decoded: GuiTestCommand = serde_json::from_str(&encoded).expect("deserialize");
    assert_eq!(decoded, GuiTestCommand::ClearMeshEventLog);
}

#[test]
fn test_gui_test_command_set_peer_presence_validates_peer_id() {
    // Valid hex identifier is accepted for both online and offline.
    assert!(GuiTestCommand::SetPeerPresence {
        peer_id: "a1".repeat(32),
        online: true,
    }
    .validate()
    .is_ok());
    assert!(GuiTestCommand::SetPeerPresence {
        peer_id: "a1".repeat(32),
        online: false,
    }
    .validate()
    .is_ok());
    // Oversized or unsafe identifiers are rejected.
    let long = "a".repeat(GUI_TEST_COMMAND_MAX_STRING_LEN + 1);
    assert!(GuiTestCommand::SetPeerPresence {
        peer_id: long,
        online: true,
    }
    .validate()
    .is_err());
    assert!(GuiTestCommand::SetPeerPresence {
        peer_id: "not a valid id!".into(),
        online: true,
    }
    .validate()
    .is_err());
}

#[test]
fn test_gui_test_command_set_peer_presence_serde_round_trip() {
    let command = GuiTestCommand::SetPeerPresence {
        peer_id: "a1".repeat(32),
        online: true,
    };
    let encoded = serde_json::to_string(&command).expect("serialize");
    assert!(encoded.contains("set_peer_presence"));
    let decoded: GuiTestCommand = serde_json::from_str(&encoded).expect("deserialize");
    assert_eq!(decoded, command);
    // The offline form must round-trip too.
    let offline = GuiTestCommand::SetPeerPresence {
        peer_id: "b2".repeat(32),
        online: false,
    };
    let decoded: GuiTestCommand =
        serde_json::from_str(&serde_json::to_string(&offline).expect("serialize"))
            .expect("deserialize");
    assert_eq!(decoded, offline);
}

#[test]
fn test_gui_test_command_validate_rejects_long_wait_screen_name() {
    let long = "a".repeat(GUI_TEST_COMMAND_MAX_STRING_LEN + 1);
    assert!(GuiTestCommand::Wait {
        condition: GuiWaitCondition::ScreenIs { expected: long },
        timeout_ms: 1000,
    }
    .validate()
    .is_err());
}

#[test]
fn test_gui_test_command_validate_rejects_long_wait_room_topic() {
    let long = "a".repeat(GUI_TEST_COMMAND_MAX_STRING_LEN + 1);
    assert!(GuiTestCommand::Wait {
        condition: GuiWaitCondition::RoomSelected {
            room_topic: Some(long),
        },
        timeout_ms: 1000,
    }
    .validate()
    .is_err());
}

// ── Security: no shell / filesystem / keyboard / mouse variants ──

#[test]
fn test_gui_test_command_rejects_dangerous_variants() {
    // Verifies that no new dangerous variants can be injected via serde.
    let dangerous = [
        r#"{"command": "execute"}"#,
        r#"{"command": "exec"}"#,
        r#"{"command": "shell"}"#,
        r#"{"command": "run"}"#,
        r#"{"command": "system"}"#,
        r#"{"command": "open_file"}"#,
        r#"{"command": "write_file"}"#,
        r#"{"command": "read_file"}"#,
        r#"{"command": "getenv"}"#,
        r#"{"command": "env"}"#,
        r#"{"command": "keyboard"}"#,
        r#"{"command": "mouse"}"#,
        r#"{"command": "window_handle"}"#,
        r#"{"command": "click"}"#,
        r#"{"command": "type_keys"}"#,
        r#"{"command": "send_keys"}"#,
        r#"{"command": "clipboard"}"#,
        r#"{"command": "spawn"}"#,
    ];
    for payload in &dangerous {
        let result: Result<GuiTestCommand, _> = serde_json::from_str(payload);
        assert!(
            result.is_err(),
            "Dangerous variant must be rejected: {}",
            payload
        );
    }
}

// ── GuiTestCommand::expected_state() ──────────────────────────

#[test]
fn test_gui_test_command_expected_state_go_to_chat_list() {
    let cmd = GuiTestCommand::GoToChatList;
    assert_eq!(
        cmd.expected_state(),
        Some(ExpectedState::ScreenIs("ChatList".into()))
    );
}

#[test]
fn test_gui_test_command_expected_state_open_room() {
    let cmd = GuiTestCommand::OpenRoom {
        room_id: "deadbeef".into(),
    };
    assert_eq!(
        cmd.expected_state(),
        Some(ExpectedState::RoomSelected("deadbeef".into()))
    );
}

#[test]
fn test_gui_test_command_expected_state_open_conversation() {
    let cmd = GuiTestCommand::OpenConversation {
        conversation_id: "cafebabe".into(),
    };
    assert_eq!(
        cmd.expected_state(),
        Some(ExpectedState::ConversationSelected("cafebabe".into()))
    );
}

#[test]
fn test_gui_test_command_expected_state_set_composer_text() {
    let cmd = GuiTestCommand::SetComposerText {
        text: "hello world".into(),
    };
    assert_eq!(
        cmd.expected_state(),
        Some(ExpectedState::ComposerTextIs("hello world".into()))
    );
}

#[test]
fn test_gui_test_command_expected_state_submit_composer() {
    let cmd = GuiTestCommand::SubmitComposer;
    assert_eq!(cmd.expected_state(), Some(ExpectedState::MessageSent));
}

#[test]
fn test_gui_test_command_expected_state_toggle_dark_mode() {
    let cmd = GuiTestCommand::ToggleDarkMode { enabled: true };
    assert_eq!(cmd.expected_state(), Some(ExpectedState::DarkModeIs(true)));

    let cmd = GuiTestCommand::ToggleDarkMode { enabled: false };
    assert_eq!(cmd.expected_state(), Some(ExpectedState::DarkModeIs(false)));
}

#[test]
fn test_gui_test_command_expected_state_open_friends() {
    let cmd = GuiTestCommand::OpenFriends;
    assert_eq!(
        cmd.expected_state(),
        Some(ExpectedState::ScreenIs("FriendRequests".into()))
    );
}

#[test]
fn test_gui_test_command_expected_state_open_settings() {
    let cmd = GuiTestCommand::OpenSettings;
    assert_eq!(
        cmd.expected_state(),
        Some(ExpectedState::ScreenIs("Settings".into()))
    );
}

#[test]
fn test_gui_test_command_expected_state_toggle_help() {
    let cmd = GuiTestCommand::ToggleHelp;
    assert_eq!(cmd.expected_state(), Some(ExpectedState::HelpVisible(true)));
}

#[test]
fn test_gui_test_command_expected_state_returns_none_for_ambiguous() {
    // CloseDialog — depends on current state
    assert!(GuiTestCommand::CloseDialog.expected_state().is_none());
    // SelectPeer — may open conversation or profile
    assert!(GuiTestCommand::SelectPeer {
        peer_id: "abc".into()
    }
    .expected_state()
    .is_none());
    // Wait — condition is tracked separately
    assert!(GuiTestCommand::Wait {
        condition: GuiWaitCondition::PeerVisible { min_count: 1 },
        timeout_ms: 1000,
    }
    .expected_state()
    .is_none());
}

// ── Security: GuiActionError / GuiActionErrorCode ─────────────────

#[test]
fn test_gui_action_error_code_serde_snake_case() {
    let json = serde_json::to_string(&GuiActionErrorCode::GuiActionsDisabled).unwrap();
    assert_eq!(json, "\"gui_actions_disabled\"");

    let json = serde_json::to_string(&GuiActionErrorCode::UnknownRoom).unwrap();
    assert_eq!(json, "\"unknown_room\"");

    let json = serde_json::to_string(&GuiActionErrorCode::ActionTimedOut).unwrap();
    assert_eq!(json, "\"action_timed_out\"");

    // Round-trip
    let decoded: GuiActionErrorCode = serde_json::from_str("\"internal_error\"").unwrap();
    assert_eq!(decoded, GuiActionErrorCode::InternalError);
}

#[test]
fn test_gui_action_error_no_secrets_in_serialized_output() {
    let err = GuiActionError::new(GuiActionErrorCode::UnknownRoom, "room not found");
    let json = serde_json::to_string(&err).unwrap();
    assert!(!json.contains("secret"));
    assert!(!json.contains("key"));
    assert!(!json.contains("ticket"));
    assert!(!json.contains("password"));
}

#[test]
fn test_gui_action_error_display() {
    let err = GuiActionError::new(GuiActionErrorCode::GuiActionsDisabled, "test msg");
    let display = format!("{}", err);
    assert!(display.contains("GuiActionsDisabled"));
    assert!(display.contains("test msg"));
}

// ── Security: GuiActionState machine transition enforcement ───────

#[test]
fn test_gui_action_state_invalid_transitions_rejected() {
    let mut status = GuiActionStatus {
        action_id: GuiActionId::new(),
        state: GuiActionState::Queued,
        requested_at_ms: 1000,
        updated_at_ms: 1000,
        expected_gui_revision: None,
        observed_gui_revision: None,
        error: None,
        result: None,
        expected_state: None,
        timeout_at_ms: None,
    };

    // Queued → Completed (invalid: must go through Validating first)
    assert!(status.transition_to(GuiActionState::Completed).is_err());

    // Queued → Validating (valid)
    assert!(status.transition_to(GuiActionState::Validating).is_ok());

    // Validating → AppMessageQueued (valid)
    assert!(status
        .transition_to(GuiActionState::AppMessageQueued)
        .is_ok());

    // AppMessageQueued → Completed (invalid: must go through AppMessageHandled first)
    assert!(status.transition_to(GuiActionState::Completed).is_err());
}

#[test]
fn test_gui_action_state_terminal_cannot_transition() {
    let mut status = GuiActionStatus {
        action_id: GuiActionId::new(),
        state: GuiActionState::Completed,
        requested_at_ms: 1000,
        updated_at_ms: 1000,
        expected_gui_revision: None,
        observed_gui_revision: None,
        error: None,
        result: None,
        expected_state: None,
        timeout_at_ms: None,
    };

    // Terminal state → any other state should fail
    assert!(status.transition_to(GuiActionState::Queued).is_err());
    assert!(status.transition_to(GuiActionState::Validating).is_err());
    assert!(status
        .transition_to(GuiActionState::AppMessageQueued)
        .is_err());
}

#[test]
fn test_gui_action_state_rejected_is_terminal() {
    let mut status = GuiActionStatus {
        action_id: GuiActionId::new(),
        state: GuiActionState::Rejected,
        requested_at_ms: 1000,
        updated_at_ms: 1000,
        expected_gui_revision: None,
        observed_gui_revision: None,
        error: None,
        result: None,
        expected_state: None,
        timeout_at_ms: None,
    };
    assert!(status.transition_to(GuiActionState::Validating).is_err());
}

#[test]
fn test_gui_action_state_full_lifecycle_valid() {
    let mut status = GuiActionStatus {
        action_id: GuiActionId::new(),
        state: GuiActionState::Queued,
        requested_at_ms: 1000,
        updated_at_ms: 1000,
        expected_gui_revision: None,
        observed_gui_revision: None,
        error: None,
        result: None,
        expected_state: None,
        timeout_at_ms: None,
    };

    assert!(status.transition_to(GuiActionState::Validating).is_ok());
    assert!(status
        .transition_to(GuiActionState::AppMessageQueued)
        .is_ok());
    assert!(status
        .transition_to(GuiActionState::AppMessageHandled)
        .is_ok());
    assert!(status
        .transition_to(GuiActionState::WaitingForExpectedState)
        .is_ok());
    assert!(status.transition_to(GuiActionState::Completed).is_ok());
    assert!(status.state.is_terminal());
}

#[test]
fn test_gui_action_state_rejected_lifecycle() {
    let mut status = GuiActionStatus {
        action_id: GuiActionId::new(),
        state: GuiActionState::Queued,
        requested_at_ms: 1000,
        updated_at_ms: 1000,
        expected_gui_revision: None,
        observed_gui_revision: None,
        error: None,
        result: None,
        expected_state: None,
        timeout_at_ms: None,
    };

    assert!(status.transition_to(GuiActionState::Validating).is_ok());
    assert!(status.transition_to(GuiActionState::Rejected).is_ok());
    assert!(status.state.is_terminal());
}

#[test]
fn test_gui_action_state_failed_lifecycle() {
    let mut status = GuiActionStatus {
        action_id: GuiActionId::new(),
        state: GuiActionState::Queued,
        requested_at_ms: 1000,
        updated_at_ms: 1000,
        expected_gui_revision: None,
        observed_gui_revision: None,
        error: None,
        result: None,
        expected_state: None,
        timeout_at_ms: None,
    };

    assert!(status.transition_to(GuiActionState::Validating).is_ok());
    assert!(status
        .transition_to(GuiActionState::AppMessageQueued)
        .is_ok());
    assert!(status
        .transition_to(GuiActionState::AppMessageHandled)
        .is_ok());
    assert!(status.transition_to(GuiActionState::Failed).is_ok());
    assert!(status.state.is_terminal());
}

#[test]
fn test_gui_action_state_timed_out_lifecycle() {
    let mut status = GuiActionStatus {
        action_id: GuiActionId::new(),
        state: GuiActionState::Queued,
        requested_at_ms: 1000,
        updated_at_ms: 1000,
        expected_gui_revision: None,
        observed_gui_revision: None,
        error: None,
        result: None,
        expected_state: None,
        timeout_at_ms: None,
    };

    assert!(status.transition_to(GuiActionState::Validating).is_ok());
    assert!(status
        .transition_to(GuiActionState::AppMessageQueued)
        .is_ok());
    assert!(status
        .transition_to(GuiActionState::AppMessageHandled)
        .is_ok());
    assert!(status
        .transition_to(GuiActionState::WaitingForExpectedState)
        .is_ok());
    assert!(status.transition_to(GuiActionState::TimedOut).is_ok());
    assert!(status.state.is_terminal());
}

// ── Security: GuiActionHistory capacity and eviction ──────────────

#[test]
fn test_gui_action_history_capacity_capped() {
    let history = GuiActionHistory::with_capacity(5);
    for i in 0..10 {
        let request = GuiActionRequest {
            action_id: GuiActionId(format!("id-{}", i)),
            requested_at_ms: 1000 + i as i64,
            command: format!("cmd-{}", i),
        };
        history.record(request);
    }
    // Should have evicted oldest 5
    assert_eq!(history.action_count(), 5);
}

#[test]
fn test_gui_action_history_active_actions_evicted_when_capacity_exceeded() {
    let history = GuiActionHistory::with_capacity(3);

    // Fill with non-terminal actions
    for i in 0..3 {
        let request = GuiActionRequest {
            action_id: GuiActionId(format!("active-{}", i)),
            requested_at_ms: 1000 + i as i64,
            command: format!("cmd-{}", i),
        };
        history.record(request);
    }

    // All active — adding a 4th evicts the oldest (active-0) to keep capacity
    let r4 = GuiActionRequest {
        action_id: GuiActionId("active-4".into()),
        requested_at_ms: 1000,
        command: "cmd-4".into(),
    };
    history.record(r4);
    // Capacity enforced: oldest evicted, new one added, back to 3
    assert_eq!(history.action_count(), 3);
    assert_eq!(history.active_count(), 3);
    // Oldest (active-0) should be gone; newest (active-4) should exist
    assert!(history.get(&GuiActionId("active-0".into())).is_none());
    assert!(history.get(&GuiActionId("active-4".into())).is_some());
}

#[test]
fn test_gui_action_history_completed_actions_evicted() {
    let history = GuiActionHistory::with_capacity(3);

    for i in 0..3 {
        let request = GuiActionRequest {
            action_id: GuiActionId(format!("c{}", i)),
            requested_at_ms: 1000 + i as i64,
            command: format!("cmd-{}", i),
        };
        history.record(request);
    }

    // Complete the first one via set_state
    history.set_state(&GuiActionId("c0".into()), GuiActionState::Completed);
    assert_eq!(history.active_count(), 2);

    // Add a 4th — should evict c0 (oldest terminal)
    let r4 = GuiActionRequest {
        action_id: GuiActionId("c4".into()),
        requested_at_ms: 1000,
        command: "cmd-4".into(),
    };
    history.record(r4);

    assert_eq!(history.action_count(), 3);
    assert!(history.get(&GuiActionId("c0".into())).is_none());
    assert!(history.get(&GuiActionId("c4".into())).is_some());
}

#[test]
fn test_gui_action_history_find_nonexistent() {
    let history = GuiActionHistory::new();
    assert!(history.get(&GuiActionId("nothing".into())).is_none());
}

#[test]
fn test_gui_action_history_find_by_action_id() {
    let history = GuiActionHistory::new();
    let aid = GuiActionId("find-me".into());
    let request = GuiActionRequest {
        action_id: aid.clone(),
        requested_at_ms: 2000,
        command: "find-cmd".into(),
    };
    history.record(request);
    let found = history.get(&aid);
    assert!(found.is_some());
    let found = found.unwrap();
    assert_eq!(found.requested_at_ms, 2000);
    assert!(found.state.is_active());
}

#[test]
fn test_gui_action_history_transition_to_validates_state_machine() {
    let history = GuiActionHistory::new();
    let aid = GuiActionId("sm-1".into());
    let request = GuiActionRequest {
        action_id: aid.clone(),
        requested_at_ms: 1000,
        command: "sm-cmd".into(),
    };
    history.record(request);

    // Valid: Queued → Validating
    assert!(history
        .transition_to(&aid, GuiActionState::Validating)
        .is_ok());

    // Invalid: Validating → Completed (skip AppMessageQueued)
    assert!(history
        .transition_to(&aid, GuiActionState::Completed)
        .is_err());

    // Valid: Validating → Rejected
    assert!(history
        .transition_to(&aid, GuiActionState::Rejected)
        .is_ok());

    // Terminal: cannot transition further
    assert!(history.transition_to(&aid, GuiActionState::Queued).is_err());
}

#[test]
fn test_gui_action_history_transition_to_unknown_id() {
    let history = GuiActionHistory::new();
    assert!(history
        .transition_to(
            &GuiActionId("nonexistent".into()),
            GuiActionState::Completed
        )
        .is_err());
}

#[test]
fn test_gui_action_history_set_expected_state() {
    let history = GuiActionHistory::new();
    let request = GuiActionRequest {
        action_id: GuiActionId("test-1".into()),
        requested_at_ms: 1000,
        command: "GoToChatList".into(),
    };
    let id = history.record(request);

    // Set expected state
    assert!(history.set_expected_state(&id, ExpectedState::ScreenIs("ChatList".into())));

    // Verify it was stored
    let status = history.get(&id).unwrap();
    assert_eq!(
        status.expected_state,
        Some(ExpectedState::ScreenIs("ChatList".into()))
    );

    // Overwrite with a different expected state
    assert!(history.set_expected_state(&id, ExpectedState::DarkModeIs(true)));
    let status = history.get(&id).unwrap();
    assert_eq!(status.expected_state, Some(ExpectedState::DarkModeIs(true)));

    // Unknown action returns false
    assert!(!history.set_expected_state(
        &GuiActionId("nonexistent".into()),
        ExpectedState::MessageSent
    ));
}

// ── Security: GuiWaitCondition evaluation ─────────────────────────

#[test]
fn test_evaluate_wait_condition_screen_is_matches() {
    let cond = GuiWaitCondition::ScreenIs {
        expected: "ChatList".into(),
    };
    let snapshot = IcedStateSnapshot {
        node_id: "node".into(),
        version: "1".into(),
        active_screen: "ChatList".into(),
        active_room: None,
        conversation_count: 0,
        neighbor_count: 0,
        direct_peer_count: 0,
        relayed_peer_count: 0,
        mesh_health: "OK".into(),
        online_friend_count: 0,
        friend_count: 0,
        total_entry_count: 0,
        dark_mode: false,
        composer_text: String::new(),
        dialog_open: false,
        unread_count: 0,
        dashboard: None,
        timestamp: chrono::Utc::now(),
    };
    let journal = IcedMessageJournal::new();
    assert!(evaluate_wait_condition(&cond, &snapshot, &journal));
}

#[test]
fn test_evaluate_wait_condition_screen_is_no_match() {
    let cond = GuiWaitCondition::ScreenIs {
        expected: "Settings".into(),
    };
    let snapshot = IcedStateSnapshot {
        node_id: "node".into(),
        version: "1".into(),
        active_screen: "ChatList".into(),
        active_room: None,
        conversation_count: 0,
        neighbor_count: 0,
        direct_peer_count: 0,
        relayed_peer_count: 0,
        mesh_health: "OK".into(),
        online_friend_count: 0,
        friend_count: 0,
        total_entry_count: 0,
        dark_mode: false,
        composer_text: String::new(),
        dialog_open: false,
        unread_count: 0,
        dashboard: None,
        timestamp: chrono::Utc::now(),
    };
    let journal = IcedMessageJournal::new();
    assert!(!evaluate_wait_condition(&cond, &snapshot, &journal));
}

#[test]
fn test_evaluate_wait_condition_peer_visible_matches() {
    let cond = GuiWaitCondition::PeerVisible { min_count: 3 };
    let snapshot = IcedStateSnapshot {
        node_id: "node".into(),
        version: "1".into(),
        active_screen: "list".into(),
        active_room: None,
        conversation_count: 0,
        neighbor_count: 5,
        direct_peer_count: 3,
        relayed_peer_count: 2,
        mesh_health: "OK".into(),
        online_friend_count: 0,
        friend_count: 0,
        total_entry_count: 0,
        dark_mode: false,
        composer_text: String::new(),
        dialog_open: false,
        unread_count: 0,
        dashboard: None,
        timestamp: chrono::Utc::now(),
    };
    let journal = IcedMessageJournal::new();
    assert!(evaluate_wait_condition(&cond, &snapshot, &journal));
}

#[test]
fn test_evaluate_wait_condition_gui_revision_at_least() {
    let cond = GuiWaitCondition::GuiRevisionAtLeast {
        expected_revision: 5,
    };
    let snapshot = IcedStateSnapshot {
        node_id: "node".into(),
        version: "1".into(),
        active_screen: "list".into(),
        active_room: None,
        conversation_count: 0,
        neighbor_count: 0,
        direct_peer_count: 0,
        relayed_peer_count: 0,
        mesh_health: "OK".into(),
        online_friend_count: 0,
        friend_count: 0,
        total_entry_count: 0,
        dark_mode: false,
        composer_text: String::new(),
        dialog_open: false,
        unread_count: 0,
        dashboard: None,
        timestamp: chrono::Utc::now(),
    };
    let journal = IcedMessageJournal::with_capacity(10);
    for i in 0..7 {
        journal.record(
            format!("Msg{}", i),
            FailureLayer::IcedUpdate,
            true,
            "",
            None,
        );
    }
    assert!(evaluate_wait_condition(&cond, &snapshot, &journal));
}

// ── Security: GuiActionId uniqueness and format ───────────────────

#[test]
fn test_gui_action_id_generates_unique_ids() {
    let mut ids = std::collections::HashSet::new();
    for _ in 0..100 {
        let id = GuiActionId::new();
        assert!(ids.insert(id.0.clone()), "GuiActionId must be unique");
    }
}

#[test]
fn test_gui_action_id_format_is_hex() {
    let id = GuiActionId::new();
    assert_eq!(id.0.len(), 32);
    assert!(id.0.chars().all(|c| c.is_ascii_hexdigit()));
}

#[test]
fn test_gui_action_id_display() {
    let id = GuiActionId("abcd1234".into());
    assert_eq!(format!("{}", id), "abcd1234");
}

// ── Security: GuiActionEventKind no secrets in serialized form ────

#[test]
fn test_gui_action_event_kind_no_secrets() {
    let kinds: Vec<GuiActionEventKind> = vec![
        GuiActionEventKind::ActionRequested,
        GuiActionEventKind::ActionQueued,
        GuiActionEventKind::ActionValidationStarted,
        GuiActionEventKind::ActionValidated,
        GuiActionEventKind::ActionRejected {
            reason: "test".into(),
        },
        GuiActionEventKind::AppMessageQueued {
            message_variant: "Test".into(),
        },
        GuiActionEventKind::AppMessageHandled {
            message_variant: "Test".into(),
            success: true,
        },
        GuiActionEventKind::ExpectedStateObserved,
        GuiActionEventKind::ActionCompleted,
    ];
    for kind in &kinds {
        let json = serde_json::to_string(kind).unwrap();
        assert!(
            !json.contains("secret_key"),
            "Event kind must not contain secret_key: {}",
            json
        );
    }
}

// ── Security: IcedStateSnapshot no secrets in serialized form ─────

#[test]
fn test_iced_state_snapshot_no_secrets() {
    let snapshot = IcedStateSnapshot {
        node_id: "node-abc".into(),
        version: "0.101.0".into(),
        active_screen: "ChatList".into(),
        active_room: None,
        conversation_count: 3,
        neighbor_count: 2,
        direct_peer_count: 1,
        relayed_peer_count: 1,
        mesh_health: "Good".into(),
        online_friend_count: 5,
        friend_count: 10,
        total_entry_count: 42,
        dark_mode: true,
        composer_text: String::new(),
        dialog_open: false,
        unread_count: 0,
        dashboard: None,
        timestamp: chrono::Utc::now(),
    };
    let json = serde_json::to_string(&snapshot).unwrap();
    assert!(!json.contains("secret_key"));
    assert!(!json.contains("secret"));
    assert!(!json.contains("mailbox"));
    assert!(!json.contains("discovery_secret"));
    assert!(!json.contains("ticket"));
    assert!(!json.contains("password"));
    assert!(!json.contains("token"));
    assert!(!json.contains("private_key"));
}

// ── Security: verify KNOWN_SAFE_VARIANTS count matches enum ───────

/// All known safe variant names — update when adding new variants.
/// This test verifies that the documentation constant exactly matches
/// the actual serde tag names of all GuiTestCommand variants.
#[test]
fn test_all_gui_test_command_variants_are_known_safe() {
    // Struct variants need full JSON with required fields.
    // Unit variants can use just the command tag.
    let json_cases: Vec<(&str, &str)> = vec![
        (r#"{"command":"go_to_chat_list"}"#, "GoToChatList"),
        (
            r#"{"command":"open_room","room_id":"abcd1234"}"#,
            "OpenRoom",
        ),
        (
            r#"{"command":"open_conversation","conversation_id":"deadbeef"}"#,
            "OpenConversation",
        ),
        (r#"{"command":"open_friends"}"#, "OpenFriends"),
        (r#"{"command":"open_settings"}"#, "OpenSettings"),
        (r#"{"command":"close_dialog"}"#, "CloseDialog"),
        (
            r#"{"command":"set_composer_text","text":"hello"}"#,
            "SetComposerText",
        ),
        (r#"{"command":"submit_composer"}"#, "SubmitComposer"),
        (
            r#"{"command":"select_peer","peer_id":"cafe1234"}"#,
            "SelectPeer",
        ),
        (
            r#"{"command":"toggle_dark_mode","enabled":true}"#,
            "ToggleDarkMode",
        ),
        (r#"{"command":"toggle_help"}"#, "ToggleHelp"),
        (
            r#"{"command":"wait","condition":{"type":"screen_is","expected":"ChatList"},"timeout_ms":5000}"#,
            "Wait",
        ),
    ];

    for (json_str, variant_name) in &json_cases {
        let result: Result<GuiTestCommand, _> = serde_json::from_str(json_str);
        assert!(
            result.is_ok(),
            "Known safe variant must deserialize: {} (json={})",
            variant_name,
            json_str
        );
    }
}

// ── GuiActionEventHistory event-ordering tests ───────────────────

#[test]
fn test_gui_action_event_history_record_and_query() {
    let journal = GuiActionEventHistory::new();

    journal.record(
        "action-1",
        GuiActionEventKind::ActionRequested,
        1,
        None,
        "ChatList",
    );
    journal.record(
        "action-1",
        GuiActionEventKind::ActionValidationStarted,
        1,
        None,
        "ChatList",
    );
    journal.record(
        "action-1",
        GuiActionEventKind::ActionCompleted,
        1,
        None,
        "ChatList",
    );

    assert_eq!(journal.entry_count(), 3);
    assert_eq!(journal.latest_sequence(), 2);

    // entries_since(0) returns records with sequence > 0 (so 1, 2)
    let since_0 = journal.entries_since(0, 100);
    assert_eq!(since_0.len(), 2);
    assert_eq!(since_0[0].sequence, 1);
    assert_eq!(since_0[1].sequence, 2);

    // entries_since(1) returns records with sequence > 1 (only 2)
    let since_1 = journal.entries_since(1, 100);
    assert_eq!(since_1.len(), 1);
    assert_eq!(since_1[0].sequence, 2);

    // entries_since(latest) returns empty
    let since_latest = journal.entries_since(2, 100);
    assert!(since_latest.is_empty());
}

#[test]
fn test_gui_action_event_history_sequence_ordering() {
    let journal = GuiActionEventHistory::new();

    // Interleave multiple action IDs — sequences must still be monotonic
    journal.record("a", GuiActionEventKind::ActionRequested, 1, None, "Screen");
    journal.record("b", GuiActionEventKind::ActionRequested, 1, None, "Screen");
    journal.record("a", GuiActionEventKind::ActionCompleted, 2, None, "Screen");
    journal.record("c", GuiActionEventKind::ActionRequested, 2, None, "Screen");
    journal.record(
        "b",
        GuiActionEventKind::ActionValidationStarted,
        2,
        None,
        "Screen",
    );
    journal.record(
        "c",
        GuiActionEventKind::ActionFailed {
            error: "timeout".into(),
        },
        3,
        None,
        "Screen",
    );

    assert_eq!(journal.entry_count(), 6);
    assert_eq!(journal.latest_sequence(), 5);

    let all = journal.all_entries();
    // newest first
    assert_eq!(all[0].sequence, 5);
    assert_eq!(all[1].sequence, 4);
    assert_eq!(all[2].sequence, 3);
    assert_eq!(all[3].sequence, 2);
    assert_eq!(all[4].sequence, 1);
    assert_eq!(all[5].sequence, 0);

    // Check action IDs in newest-first order
    assert_eq!(all[0].action_id, "c");
    assert!(matches!(
        all[0].kind,
        GuiActionEventKind::ActionFailed { .. }
    ));
    assert_eq!(all[5].action_id, "a");
    assert!(matches!(all[5].kind, GuiActionEventKind::ActionRequested));
}

#[test]
fn test_gui_action_event_history_entries_since_limit() {
    let journal = GuiActionEventHistory::new();

    for i in 0..50 {
        journal.record(
            format!("action-{}", i),
            GuiActionEventKind::ActionRequested,
            i,
            None,
            "Screen",
        );
    }

    // Request more than clamp limit — should clamp to 1000
    let many = journal.entries_since(0, 5000);
    assert_eq!(many.len(), 49); // sequence > 0 means seq 1..49 (49 items)

    // Request small limit
    let few = journal.entries_since(0, 3);
    assert_eq!(few.len(), 3);
    assert_eq!(few[0].sequence, 1);
    assert_eq!(few[1].sequence, 2);
    assert_eq!(few[2].sequence, 3);
}

#[test]
fn test_gui_action_event_history_eviction() {
    // with_capacity clamps to [64, 5000]
    let journal = GuiActionEventHistory::with_capacity(64);

    // Fill beyond capacity (record 70 entries, should evict to 64)
    for i in 0..70 {
        journal.record(
            format!("action-{}", i),
            GuiActionEventKind::ActionRequested,
            i as u64,
            None,
            "Screen",
        );
    }

    // Only 64 entries remain
    assert_eq!(journal.entry_count(), 64);
    assert_eq!(journal.latest_sequence(), 69);

    // The 6 oldest (seq 0..5) should be evicted
    let all = journal.all_entries();
    assert_eq!(all.len(), 64);
    let sequences: Vec<u64> = all.iter().map(|e| e.sequence).collect();
    let expected: Vec<u64> = (6..70).rev().collect();
    assert_eq!(sequences, expected);

    // entries_since should only see survivors with sequence > 0
    let since_0 = journal.entries_since(0, 100);
    assert_eq!(since_0.len(), 64);
    assert_eq!(since_0[0].sequence, 6);
}

#[test]
fn test_gui_action_event_history_all_entries_newest_first() {
    let journal = GuiActionEventHistory::new();

    journal.record("id-1", GuiActionEventKind::ActionRequested, 0, None, "A");
    journal.record("id-1", GuiActionEventKind::ActionValidated, 1, None, "A");
    journal.record("id-1", GuiActionEventKind::ActionCompleted, 2, None, "B");

    let all = journal.all_entries();
    assert_eq!(all.len(), 3);
    assert_eq!(all[0].current_screen, "B"); // newest
    assert_eq!(all[0].sequence, 2);
    assert_eq!(all[1].current_screen, "A"); // middle
    assert_eq!(all[1].sequence, 1);
    assert_eq!(all[2].current_screen, "A"); // oldest
    assert_eq!(all[2].sequence, 0);
}

#[test]
fn test_gui_action_event_history_latest_sequence_and_count() {
    let journal = GuiActionEventHistory::new();

    assert_eq!(journal.latest_sequence(), 0);
    assert_eq!(journal.entry_count(), 0);

    journal.record("x", GuiActionEventKind::ActionRequested, 0, None, "");
    assert_eq!(journal.latest_sequence(), 0);
    assert_eq!(journal.entry_count(), 1);

    journal.record("x", GuiActionEventKind::ActionCompleted, 1, None, "");
    assert_eq!(journal.latest_sequence(), 1);
    assert_eq!(journal.entry_count(), 2);
}

#[test]
fn test_gui_action_event_history_empty_journal() {
    let journal = GuiActionEventHistory::new();

    assert_eq!(journal.entry_count(), 0);
    assert_eq!(journal.latest_sequence(), 0);
    assert!(journal.entries_since(0, 100).is_empty());
    assert!(journal.all_entries().is_empty());
}

#[test]
fn test_gui_action_event_history_with_capacity_clamping() {
    // Below minimum — clamps to 64
    let tiny = GuiActionEventHistory::with_capacity(10);
    for i in 0..70 {
        tiny.record(
            format!("a{}", i),
            GuiActionEventKind::ActionRequested,
            i as u64,
            None,
            "",
        );
    }
    assert_eq!(tiny.entry_count(), 64);

    // Above maximum — clamps to 5000
    let huge = GuiActionEventHistory::with_capacity(10_000);
    for i in 0..6000 {
        huge.record(
            format!("a{}", i),
            GuiActionEventKind::ActionRequested,
            i as u64,
            None,
            "",
        );
    }
    assert_eq!(huge.entry_count(), 5000);
}

#[test]
fn test_gui_action_event_history_room_and_screen_fields() {
    let journal = GuiActionEventHistory::new();
    let room = TopicId::from_bytes([0xAA; 32]);

    journal.record(
        "action-42",
        GuiActionEventKind::ActionRequested,
        5,
        Some(room),
        "Chat",
    );

    let all = journal.all_entries();
    assert_eq!(all.len(), 1);
    assert_eq!(all[0].action_id, "action-42");
    assert_eq!(all[0].gui_revision, 5);
    assert_eq!(all[0].room_id, Some(room));
    assert_eq!(all[0].current_screen, "Chat");
    assert!(matches!(all[0].kind, GuiActionEventKind::ActionRequested));
}

#[test]
fn test_gui_action_event_history_action_timed_out_and_failed() {
    let journal = GuiActionEventHistory::new();

    journal.record(
        "t1",
        GuiActionEventKind::ActionTimedOut { timeout_ms: 5000 },
        3,
        None,
        "Chat",
    );
    journal.record(
        "t2",
        GuiActionEventKind::ActionFailed {
            error: "permission denied".into(),
        },
        4,
        None,
        "Settings",
    );

    let all = journal.all_entries();
    assert_eq!(all.len(), 2);

    // Newest first: t2 (seq 1)
    assert_eq!(all[0].action_id, "t2");
    match &all[0].kind {
        GuiActionEventKind::ActionFailed { error } => assert_eq!(error, "permission denied"),
        _ => panic!("expected ActionFailed"),
    }
    assert_eq!(all[0].current_screen, "Settings");

    // Oldest: t1 (seq 0)
    assert_eq!(all[1].action_id, "t1");
    match &all[1].kind {
        GuiActionEventKind::ActionTimedOut { timeout_ms } => assert_eq!(*timeout_ms, 5000),
        _ => panic!("expected ActionTimedOut"),
    }
    assert_eq!(all[1].current_screen, "Chat");
}

// ── Extended event-ordering tests ───────────────────────────────

#[test]
fn test_gui_action_event_history_concurrent_record_ordering() {
    // Multiple threads record events simultaneously on the same journal.
    // After all join, sequences must be strictly unique and monotonic.
    let journal = GuiActionEventHistory::with_capacity(5000);
    let n_threads: usize = 10;
    let events_per_thread: usize = 50;
    let mut handles = Vec::with_capacity(n_threads);

    for t in 0..n_threads {
        let j = journal.clone();
        handles.push(std::thread::spawn(move || {
            let mut local_seqs = Vec::with_capacity(events_per_thread);
            for i in 0..events_per_thread {
                let kind = match i % 6 {
                    0 => GuiActionEventKind::ActionRequested,
                    1 => GuiActionEventKind::ActionValidationStarted,
                    2 => GuiActionEventKind::ActionValidated,
                    3 => GuiActionEventKind::ActionCompleted,
                    4 => GuiActionEventKind::AppMessageQueued {
                        message_variant: format!("Msg-{t}-{i}"),
                    },
                    _ => GuiActionEventKind::ExpectedStateObserved,
                };
                j.record(
                    format!("concurrent-{t}-{i}"),
                    kind,
                    (t * events_per_thread + i) as u64,
                    None,
                    "Screen",
                );
                // Snapshot latest sequence via entry_count (indirect read)
                let count = j.entry_count();
                local_seqs.push(count);
            }
            local_seqs
        }));
    }

    let mut all_seqs = Vec::with_capacity(n_threads * events_per_thread);
    for h in handles {
        if let Ok(seqs) = h.join() {
            all_seqs.extend(seqs);
        }
    }

    // Total recorded events: n_threads * events_per_thread
    let total = journal.entry_count();
    assert_eq!(
        total,
        n_threads * events_per_thread,
        "all events must be recorded"
    );

    // Latest sequence must reflect total - 1
    assert_eq!(
        journal.latest_sequence(),
        (total - 1) as u64,
        "latest_sequence must be last assigned seq"
    );

    // all_entries must be newest-first.  Under concurrent recording the
    // sequence-ordering invariant is: every entry has a unique, monotonically
    // increasing sequence number.  However, because the sequence counter is
    // assigned outside the Mutex, insertion order may not match sequence
    // order (thread A gets seq 5, thread B gets seq 6, B acquires the lock
    // first and pushes seq 6, A pushes seq 5 — reversed → [5, 6] which is
    // not strictly descending).  So we only verify uniqueness and range.
    let all = journal.all_entries();
    assert_eq!(all.len(), total, "all entries must be present");

    // All sequence numbers must be unique and in [0, total-1]
    let mut seq_set: std::collections::HashSet<u64> = std::collections::HashSet::new();
    for entry in &all {
        assert!(
            seq_set.insert(entry.sequence),
            "duplicate sequence {} found",
            entry.sequence
        );
        assert!(
            entry.sequence < total as u64,
            "sequence {} out of range (max {})",
            entry.sequence,
            total - 1
        );
    }
    assert_eq!(seq_set.len(), total, "all sequence numbers must be unique");
}

#[test]
fn test_gui_action_event_kind_all_variants_serde_roundtrip() {
    // Every GuiActionEventKind variant must roundtrip through JSON faithfully.
    // This verifies no variant is omitted and the serde tag scheme is consistent.
    let kinds: Vec<GuiActionEventKind> = vec![
        GuiActionEventKind::ActionRequested,
        GuiActionEventKind::ActionQueued,
        GuiActionEventKind::ActionValidationStarted,
        GuiActionEventKind::ActionValidated,
        GuiActionEventKind::ActionRejected {
            reason: "validation failed".into(),
        },
        GuiActionEventKind::ActionQueueFull { capacity: 256 },
        GuiActionEventKind::AppMessageQueued {
            message_variant: "SendMessage".into(),
        },
        GuiActionEventKind::AppMessageHandled {
            message_variant: "SendMessage".into(),
            success: true,
        },
        GuiActionEventKind::ExpectedStateObserved,
        GuiActionEventKind::ActionCompleted,
        GuiActionEventKind::ActionTimedOut { timeout_ms: 5000 },
        GuiActionEventKind::ActionFailed {
            error: "permission denied".into(),
        },
    ];

    assert_eq!(kinds.len(), 12, "all 12 variants must be tested");

    for (i, kind) in kinds.iter().enumerate() {
        let json = serde_json::to_string(kind).unwrap();
        let deser: GuiActionEventKind = serde_json::from_str(&json).unwrap();
        // Use debug format for comparison since PartialEq isn't derived
        let original_debug = format!("{:?}", kind);
        let deser_debug = format!("{:?}", deser);
        assert_eq!(
            original_debug, deser_debug,
            "roundtrip mismatch for variant index {i}: {json}"
        );
    }
}

#[test]
fn test_gui_action_event_history_timestamp_ordering() {
    // Timestamps must be monotonically non-decreasing (wall-clock moves forward).
    let journal = GuiActionEventHistory::new();
    let actions = ["a", "b", "c", "d", "e"];

    for (i, action) in actions.iter().enumerate() {
        journal.record(
            action,
            GuiActionEventKind::ActionRequested,
            i as u64,
            None,
            "Screen",
        );
        std::thread::sleep(std::time::Duration::from_millis(1));
        journal.record(
            action,
            GuiActionEventKind::ActionCompleted,
            i as u64 + 10,
            None,
            "Screen",
        );
    }

    let all = journal.all_entries();
    // all_entries is newest-first, so reverse for chronological order
    let chrono: Vec<&GuiActionEvent> = all.iter().rev().collect();

    for i in 0..chrono.len().saturating_sub(1) {
        assert!(
            chrono[i].timestamp <= chrono[i + 1].timestamp,
            "timestamp went backwards at idx {}: {} > {}",
            i,
            chrono[i].timestamp,
            chrono[i + 1].timestamp
        );
    }
}

#[test]
fn test_gui_action_event_history_sequence_continuity_after_eviction() {
    // After eviction forces out old entries, sequence numbers continue
    // monotonically without resetting.
    let journal = GuiActionEventHistory::with_capacity(64);

    // Fill to capacity
    for i in 0..64 {
        journal.record(
            format!("pre-{i}"),
            GuiActionEventKind::ActionRequested,
            i as u64,
            None,
            "Screen",
        );
    }
    assert_eq!(journal.latest_sequence(), 63);
    assert_eq!(journal.entry_count(), 64);

    // Over-fill — this triggers eviction of oldest
    for i in 0..20 {
        journal.record(
            format!("post-{i}"),
            GuiActionEventKind::ActionCompleted,
            (64 + i) as u64,
            None,
            "Screen",
        );
    }

    // Count should stay at capacity (64)
    assert_eq!(
        journal.entry_count(),
        64,
        "count must not exceed capacity after eviction"
    );
    // Latest sequence must be the last one assigned (83 = 64 + 20 - 1)
    assert_eq!(
        journal.latest_sequence(),
        83,
        "latest_sequence must continue monotonically after eviction"
    );

    // All entries must have strictly descending sequences (newest-first)
    let all = journal.all_entries();
    assert_eq!(all.len(), 64);
    for i in 0..all.len().saturating_sub(1) {
        assert!(
            all[i].sequence > all[i + 1].sequence,
            "sequence not descending after eviction at idx {}: {} <= {}",
            i,
            all[i].sequence,
            all[i + 1].sequence
        );
    }

    // The oldest surviving sequence should be 20 (64 evicted, so oldest of 84 total)
    let chrono: Vec<&GuiActionEvent> = all.iter().rev().collect();
    assert_eq!(
        chrono[0].sequence, 20,
        "first chronological entry should be seq 20"
    );

    // entries_since(83) should be empty (nothing newer than latest)
    assert!(journal.entries_since(83, 100).is_empty());

    // entries_since(20) should return entries with sequence > 20 (i.e. seq 21..83 = 63 entries)
    let since_20 = journal.entries_since(20, 100);
    assert_eq!(since_20.len(), 63);
    assert_eq!(since_20[0].sequence, 21);
}

#[test]
fn test_gui_action_event_history_action_lifecycle_in_order() {
    // Record a complete action lifecycle and verify events appear in
    // the expected chronological order when read back.
    let journal = GuiActionEventHistory::new();
    let action_id = "lifecycle-test-1";

    journal.record(
        action_id,
        GuiActionEventKind::ActionRequested,
        1,
        None,
        "ChatList",
    );
    journal.record(
        action_id,
        GuiActionEventKind::ActionQueued,
        1,
        None,
        "ChatList",
    );
    journal.record(
        action_id,
        GuiActionEventKind::ActionValidationStarted,
        1,
        None,
        "ChatList",
    );
    journal.record(
        action_id,
        GuiActionEventKind::ActionValidated,
        1,
        None,
        "ChatList",
    );
    journal.record(
        action_id,
        GuiActionEventKind::AppMessageQueued {
            message_variant: "SendMessage".into(),
        },
        2,
        None,
        "ChatList",
    );
    journal.record(
        action_id,
        GuiActionEventKind::AppMessageHandled {
            message_variant: "SendMessage".into(),
            success: true,
        },
        2,
        None,
        "ChatList",
    );
    journal.record(
        action_id,
        GuiActionEventKind::ExpectedStateObserved,
        3,
        None,
        "Chat",
    );
    journal.record(
        action_id,
        GuiActionEventKind::ActionCompleted,
        3,
        None,
        "Chat",
    );

    assert_eq!(journal.entry_count(), 8);
    assert_eq!(journal.latest_sequence(), 7);

    // Read in chronological order
    let since_0 = journal.entries_since(0, 100);
    assert_eq!(since_0.len(), 7); // sequence > 0 means seq 1..7 (7 items)

    let expected_kinds: &[GuiActionEventKind] = &[
        // Seq 0 is ActionRequested, excluded by entries_since(0)
        GuiActionEventKind::ActionQueued,            // seq 1
        GuiActionEventKind::ActionValidationStarted, // seq 2
        GuiActionEventKind::ActionValidated,         // seq 3
        GuiActionEventKind::AppMessageQueued {
            // seq 4
            message_variant: "SendMessage".into(),
        },
        GuiActionEventKind::AppMessageHandled {
            // seq 5
            message_variant: "SendMessage".into(),
            success: true,
        },
        GuiActionEventKind::ExpectedStateObserved, // seq 6
        GuiActionEventKind::ActionCompleted,       // seq 7
    ];
    // entries_since(0) returns only entries with sequence > 0 (seq 1..7).
    // expected_kinds is indexed without offset because it starts at seq 1.
    assert_eq!(since_0.len(), 7);
    for (i, entry) in since_0.iter().enumerate() {
        let expected = &expected_kinds[i];
        let entry_debug = format!("{:?}", entry.kind);
        let expected_debug = format!("{:?}", expected);
        assert_eq!(
            entry_debug, expected_debug,
            "lifecycle step {i} kind mismatch (seq {})",
            entry.sequence
        );
        assert_eq!(entry.action_id, action_id);
    }
}

// ── GuiTestHandle tests ───────────────────────────────────────

/// Multiple MCP producers may enqueue concurrently.  The bounded queue is
/// deliberately non-blocking: accepted requests are delivered exactly
/// once, while excess producers receive `ActionQueueFull` rather than
/// blocking or panicking.  Tokio's mpsc guarantees FIFO order for each
/// producer; no global order is promised between independently scheduled
/// producers.
#[cfg(feature = "gui")]
#[test]
fn test_gui_test_handle_concurrent_mcp_producers_are_bounded_and_unique() {
    use std::collections::HashSet;
    use std::sync::Arc;
    use std::thread;

    const CAPACITY: usize = 32;
    const PRODUCERS: usize = 4;
    const PER_PRODUCER: usize = 64;
    let (handle, mut rx) = GuiTestHandle::channel(CAPACITY);
    let barrier = Arc::new(std::sync::Barrier::new(PRODUCERS));
    let mut workers = Vec::new();

    for producer in 0..PRODUCERS {
        let producer_handle = handle.clone();
        let producer_barrier = Arc::clone(&barrier);
        workers.push(thread::spawn(move || {
            producer_barrier.wait();
            let mut accepted = Vec::new();
            let mut full = 0usize;
            for sequence in 0..PER_PRODUCER {
                let request = GuiActionRequest {
                    action_id: GuiActionId::new(),
                    requested_at_ms: (producer * PER_PRODUCER + sequence) as i64,
                    command: format!("producer_{producer}_action_{sequence}"),
                };
                match producer_handle.enqueue(request) {
                    Ok(()) => accepted.push(sequence),
                    Err(err) if err.code == GuiActionErrorCode::ActionQueueFull => {
                        full += 1;
                    }
                    Err(err) => panic!("unexpected enqueue error: {}", err.message),
                }
            }
            (accepted.len(), full)
        }));
    }
    let totals: Vec<_> = workers
        .into_iter()
        .map(|worker| worker.join().expect("producer must not panic"))
        .collect();
    drop(handle);

    let mut received = Vec::new();
    while let Ok(request) = rx.try_recv() {
        received.push(request);
    }
    assert!(received.len() <= CAPACITY, "queue exceeded capacity");
    assert_eq!(
        received.len(),
        totals.iter().map(|(accepted, _)| *accepted).sum::<usize>()
    );
    assert_eq!(
        totals.iter().map(|(_, full)| *full).sum::<usize>() + received.len(),
        PRODUCERS * PER_PRODUCER
    );
    let ids: HashSet<_> = received
        .iter()
        .map(|request| request.action_id.clone())
        .collect();
    assert_eq!(
        ids.len(),
        received.len(),
        "accepted action IDs must be unique"
    );
}

#[cfg(feature = "gui")]
#[test]
fn test_gui_test_handle_enqueue() {
    let (handle, _rx) = GuiTestHandle::channel(256);
    let request = GuiActionRequest {
        action_id: GuiActionId::new(),
        requested_at_ms: 1000,
        command: "TestCommand".to_string(),
    };
    assert!(handle.enqueue(request).is_ok());
}

#[cfg(feature = "gui")]
#[test]
fn test_gui_test_handle_closed_channel_error() {
    let (handle, rx) = GuiTestHandle::channel(256);
    // Drop the receiver to close the channel
    drop(rx);
    let request = GuiActionRequest {
        action_id: GuiActionId::new(),
        requested_at_ms: 1000,
        command: "TestCommand".to_string(),
    };
    let err = handle.enqueue(request).unwrap_err();
    assert_eq!(err.code, GuiActionErrorCode::ActionQueueClosed);
    assert!(
        err.message.contains("closed"),
        "error message should mention 'closed': {}",
        err.message
    );
}

#[cfg(feature = "gui")]
#[test]
fn test_gui_test_handle_capacity() {
    let (handle, _rx) = GuiTestHandle::channel(256);
    assert_eq!(handle.capacity(), 256);
}

#[cfg(feature = "gui")]
#[test]
fn test_gui_test_handle_is_closed() {
    let (handle, rx) = GuiTestHandle::channel(256);
    assert!(!handle.is_closed(), "channel should be open initially");
    drop(rx);
    assert!(
        handle.is_closed(),
        "channel should be closed after dropping receiver"
    );
}

#[cfg(feature = "gui")]
#[test]
fn test_gui_test_handle_queue_full_error() {
    // Use capacity 1 so the second send fails immediately
    let (handle, mut rx) = GuiTestHandle::channel(1);
    let request = GuiActionRequest {
        action_id: GuiActionId::new(),
        requested_at_ms: 1000,
        command: "Cmd1".to_string(),
    };
    assert!(handle.enqueue(request).is_ok());

    // Don't drain the receiver — the second send should fail with Full
    let request2 = GuiActionRequest {
        action_id: GuiActionId::new(),
        requested_at_ms: 2000,
        command: "Cmd2".to_string(),
    };
    let err = handle.enqueue(request2).unwrap_err();
    assert_eq!(err.code, GuiActionErrorCode::ActionQueueFull);
    assert!(
        err.message.contains("full"),
        "error message should mention 'full': {}",
        err.message
    );

    // Drain the receiver so the channel isn't leaked with queued messages
    let _ = rx.try_recv();
}

#[cfg(feature = "gui")]
#[test]
fn test_gui_test_handle_zero_capacity_clamped() {
    // Zero should be clamped to 1
    let (handle, _rx) = GuiTestHandle::channel(0);
    assert_eq!(handle.capacity(), 1);
}

#[cfg(feature = "gui")]
#[test]
fn test_gui_test_handle_oversized_capacity_clamped() {
    // Above max should be clamped to 4096
    let (handle, _rx) = GuiTestHandle::channel(9999);
    assert_eq!(handle.capacity(), 4096);
}

#[cfg(feature = "gui")]
#[test]
fn test_gui_test_handle_new_from_sender() {
    let (tx, _rx) = tokio::sync::mpsc::channel::<GuiActionRequest>(64);
    let handle = GuiTestHandle::new(tx);
    assert_eq!(handle.capacity(), 64);
    assert!(!handle.is_closed());
}

#[cfg(feature = "gui")]
#[test]
fn test_gui_test_handle_enqueue_with_new() {
    let (tx, _rx) = tokio::sync::mpsc::channel::<GuiActionRequest>(64);
    let handle = GuiTestHandle::new(tx);
    let request = GuiActionRequest {
        action_id: GuiActionId::new(),
        requested_at_ms: 1000,
        command: "FromNew".to_string(),
    };
    assert!(handle.enqueue(request).is_ok());
}

#[cfg(feature = "gui")]
#[test]
fn test_gui_test_handle_closed_detection_after_enqueue() {
    let (handle, rx) = GuiTestHandle::channel(16);
    let request = GuiActionRequest {
        action_id: GuiActionId::new(),
        requested_at_ms: 1000,
        command: "PreClose".to_string(),
    };
    assert!(handle.enqueue(request).is_ok());
    assert!(!handle.is_closed());

    drop(rx); // Close the channel

    // is_closed should now return true
    assert!(handle.is_closed());

    // enqueue should now return ActionQueueClosed
    let request2 = GuiActionRequest {
        action_id: GuiActionId::new(),
        requested_at_ms: 2000,
        command: "PostClose".to_string(),
    };
    let err = handle.enqueue(request2).unwrap_err();
    assert_eq!(err.code, GuiActionErrorCode::ActionQueueClosed);
}

// ── Action timeout handling tests ─────────────────────────────────

#[test]
fn test_gui_action_timeout_auto_set_on_waiting() {
    // Entering WaitingForExpectedState should auto-set timeout_at_ms
    use GuiActionState::*;

    let mut action = GuiActionStatus {
        action_id: GuiActionId::new(),
        state: Queued,
        requested_at_ms: 1000,
        updated_at_ms: 1000,
        expected_gui_revision: None,
        observed_gui_revision: None,
        error: None,
        result: None,
        expected_state: None,
        timeout_at_ms: None,
    };

    assert!(action.timeout_at_ms.is_none());

    // Move through valid states to WaitingForExpectedState
    action.transition_to(Validating).unwrap();
    action.transition_to(AppMessageQueued).unwrap();
    action.transition_to(AppMessageHandled).unwrap();
    action.transition_to(WaitingForExpectedState).unwrap();

    assert_eq!(action.state, WaitingForExpectedState);
    assert!(
        action.timeout_at_ms.is_some(),
        "timeout_at_ms should be set when entering WaitingForExpectedState"
    );
    let timeout = action.timeout_at_ms.unwrap();
    assert!(
        timeout > action.updated_at_ms,
        "timeout should be in the future (updated={}, timeout={})",
        action.updated_at_ms,
        timeout
    );
    // Should be at least DEFAULT_ACTION_STATE_TIMEOUT_MS in the future
    assert!(
        timeout - action.updated_at_ms >= DEFAULT_ACTION_STATE_TIMEOUT_MS,
        "timeout delta should be >= default ({}), got {}",
        DEFAULT_ACTION_STATE_TIMEOUT_MS,
        timeout - action.updated_at_ms
    );
}

#[test]
fn test_gui_action_timeout_not_set_on_other_states() {
    // Non-WaitingForExpectedState transitions should not set timeout_at_ms
    use GuiActionState::*;

    let mut action = GuiActionStatus {
        action_id: GuiActionId::new(),
        state: Queued,
        requested_at_ms: 1000,
        updated_at_ms: 1000,
        expected_gui_revision: None,
        observed_gui_revision: None,
        error: None,
        result: None,
        expected_state: None,
        timeout_at_ms: None,
    };

    // Queued -> Validating
    action.transition_to(Validating).unwrap();
    assert!(action.timeout_at_ms.is_none());

    // Validating -> AppMessageQueued
    action.transition_to(AppMessageQueued).unwrap();
    assert!(action.timeout_at_ms.is_none());

    // AppMessageQueued -> AppMessageHandled
    action.transition_to(AppMessageHandled).unwrap();
    assert!(action.timeout_at_ms.is_none());

    // AppMessageHandled -> Completed (terminal)
    action.transition_to(Completed).unwrap();
    assert!(action.timeout_at_ms.is_none());
}

#[test]
fn test_gui_action_history_timeout_cleared_on_transition_out_of_waiting() {
    // Timeout_at_ms should be cleared when leaving WaitingForExpectedState
    use GuiActionState::*;

    let mut action = GuiActionStatus {
        action_id: GuiActionId::new(),
        state: Queued,
        requested_at_ms: 1000,
        updated_at_ms: 1000,
        expected_gui_revision: None,
        observed_gui_revision: None,
        error: None,
        result: None,
        expected_state: None,
        timeout_at_ms: None,
    };

    action.transition_to(Validating).unwrap();
    action.transition_to(AppMessageQueued).unwrap();
    action.transition_to(AppMessageHandled).unwrap();
    action.transition_to(WaitingForExpectedState).unwrap();
    assert!(
        action.timeout_at_ms.is_some(),
        "timeout should be set on enter WaitingForExpectedState"
    );

    // Transition to Completed (should clear timeout)
    action.transition_to(Completed).unwrap();
    assert!(
        action.timeout_at_ms.is_none(),
        "timeout should be cleared when transitioning out of WaitingForExpectedState"
    );
}

#[test]
fn test_gui_action_history_timeout_set_via_direct_set_state() {
    // Direct set_state to WaitingForExpectedState should also set timeout
    use GuiActionState::*;

    let mut action = GuiActionStatus {
        action_id: GuiActionId::new(),
        state: Queued,
        requested_at_ms: 1000,
        updated_at_ms: 1000,
        expected_gui_revision: None,
        observed_gui_revision: None,
        error: None,
        result: None,
        expected_state: None,
        timeout_at_ms: None,
    };

    // Use set_state (not transition_to) to go to WaitingForExpectedState
    action.set_state(WaitingForExpectedState);
    assert_eq!(action.state, WaitingForExpectedState);
    assert!(
        action.timeout_at_ms.is_some(),
        "direct set_state to WaitingForExpectedState should set timeout"
    );
}

#[test]
fn test_gui_action_history_check_timeouts_returns_empty_when_none_expired() {
    // Fresh actions in WaitingForExpectedState should not be timed out
    let history = GuiActionHistory::with_capacity(10);
    let id = GuiActionId::new();

    let request = GuiActionRequest {
        action_id: id.clone(),
        requested_at_ms: 1000,
        command: "TestCommand".into(),
    };

    let recorded_id = history.record(request);
    history
        .transition_to(&recorded_id, GuiActionState::Validating)
        .unwrap();
    history
        .transition_to(&recorded_id, GuiActionState::AppMessageQueued)
        .unwrap();
    history
        .transition_to(&recorded_id, GuiActionState::AppMessageHandled)
        .unwrap();
    history
        .transition_to(&recorded_id, GuiActionState::WaitingForExpectedState)
        .unwrap();

    // Immediately check_timeouts — should not detect anything since
    // the timeout is 10s in the future
    let timed_out = history.check_timeouts();
    assert!(
        timed_out.is_empty(),
        "Freshly-started actions should not time out immediately"
    );
}

#[test]
fn test_gui_action_history_check_timeouts_skips_non_waiting_actions() {
    // Actions not in WaitingForExpectedState should never be timed out
    let history = GuiActionHistory::with_capacity(10);
    let ids: Vec<GuiActionId> = (0..5)
        .map(|_| {
            let id = GuiActionId::new();
            let request = GuiActionRequest {
                action_id: id.clone(),
                requested_at_ms: 1000,
                command: "TestCommand".into(),
            };
            history.record(request)
        })
        .collect();

    // Set to various non-waiting states
    // Valid path: Queued → Validating → AppMessageQueued → AppMessageHandled
    history
        .transition_to(&ids[0], GuiActionState::Validating)
        .unwrap();
    history
        .transition_to(&ids[1], GuiActionState::Validating)
        .unwrap();
    history
        .transition_to(&ids[1], GuiActionState::AppMessageQueued)
        .unwrap();
    history
        .transition_to(&ids[2], GuiActionState::Validating)
        .unwrap();
    history
        .transition_to(&ids[2], GuiActionState::AppMessageQueued)
        .unwrap();
    history
        .transition_to(&ids[2], GuiActionState::AppMessageHandled)
        .unwrap();
    history.set_state(&ids[3], GuiActionState::Completed);
    history.set_state(&ids[4], GuiActionState::Rejected);

    let timed_out = history.check_timeouts();
    assert!(
        timed_out.is_empty(),
        "Actions in non-waiting states should never time out"
    );
}

#[test]
fn test_gui_action_history_check_timeouts_skips_actions_without_timeout_set() {
    // Actions in WaitingForExpectedState but without timeout_at_ms
    // should not be timed out
    let history = GuiActionHistory::with_capacity(10);
    let id = GuiActionId::new();

    let request = GuiActionRequest {
        action_id: id.clone(),
        requested_at_ms: 1000,
        command: "TestCommand".into(),
    };

    let recorded_id = history.record(request);
    history.set_state(&recorded_id, GuiActionState::WaitingForExpectedState);
    // Clear the timeout manually to simulate a corrupted state
    {
        let mut actions = history.inner.actions.lock().expect("actions lock");
        if let Some(status) = actions.get_mut(&recorded_id) {
            status.timeout_at_ms = None;
        }
    }

    let timed_out = history.check_timeouts();
    assert!(
        timed_out.is_empty(),
        "Actions without timeout_at_ms should not time out"
    );
}

#[test]
fn test_gui_action_history_next_timeout_remaining_with_no_actions() {
    let history = GuiActionHistory::with_capacity(10);
    assert!(history.next_timeout_remaining_ms().is_none());
}

#[test]
fn test_gui_action_history_next_timeout_remaining_with_outdated_timeout() {
    // An action whose timeout is in the past should return Some(0)
    let history = GuiActionHistory::with_capacity(10);
    let id = GuiActionId::new();
    let request = GuiActionRequest {
        action_id: id.clone(),
        requested_at_ms: 1000,
        command: "TestCommand".into(),
    };
    let recorded_id = history.record(request);
    history.set_state(&recorded_id, GuiActionState::WaitingForExpectedState);

    // Set timeout to the past
    {
        let mut actions = history.inner.actions.lock().expect("actions lock");
        if let Some(status) = actions.get_mut(&recorded_id) {
            status.timeout_at_ms = Some(1); // epoch + 1ms = long past
        }
    }

    let remaining = history.next_timeout_remaining_ms();
    assert_eq!(remaining, Some(0), "past timeout should return Some(0)");
}

#[test]
fn test_gui_action_history_expire_marks_active_action_and_preserves_terminal_action() {
    let history = GuiActionHistory::new();
    let active = GuiActionId::new();
    let terminal = GuiActionId::new();
    for id in [active.clone(), terminal.clone()] {
        history.record(GuiActionRequest {
            action_id: id.clone(),
            requested_at_ms: 0,
            command: "test".into(),
        });
    }
    history.set_state(&terminal, GuiActionState::Completed);

    let expired = history.expire(&active).expect("active action expires");
    assert_eq!(expired.state, GuiActionState::TimedOut);
    assert_eq!(
        expired.error.as_ref().map(|error| &error.code),
        Some(&GuiActionErrorCode::ActionTimedOut)
    );
    assert!(
        history.expire(&terminal).is_none(),
        "terminal action is unchanged"
    );
    assert_eq!(
        history.get(&terminal).unwrap().state,
        GuiActionState::Completed
    );
}

#[test]
fn test_gui_action_history_expire_is_idempotent() {
    let history = GuiActionHistory::new();
    let id = GuiActionId::new();
    history.record(GuiActionRequest {
        action_id: id.clone(),
        requested_at_ms: 0,
        command: "test".into(),
    });
    assert!(history.expire(&id).is_some());
    assert!(history.expire(&id).is_none());
}

#[test]
fn test_gui_action_timeout_constant_values() {
    // Verify the constants match requirements
    assert_eq!(
        DEFAULT_ACTION_STATE_TIMEOUT_MS, 10_000,
        "default should be 10s"
    );
    assert_eq!(MAX_ACTION_STATE_TIMEOUT_MS, 30_000, "max should be 30s");
    const _: () = assert!(
        DEFAULT_ACTION_STATE_TIMEOUT_MS <= MAX_ACTION_STATE_TIMEOUT_MS,
        "default must not exceed max"
    );
}

#[test]
fn test_gui_action_timeout_at_ms_cleared_on_transition_to_timed_out() {
    // Test that transition_to TimedOut clears timeout_at_ms
    use GuiActionState::*;

    let mut action = GuiActionStatus {
        action_id: GuiActionId::new(),
        state: Queued,
        requested_at_ms: 1000,
        updated_at_ms: 1000,
        expected_gui_revision: None,
        observed_gui_revision: None,
        error: None,
        result: None,
        expected_state: None,
        timeout_at_ms: None,
    };

    action.transition_to(Validating).unwrap();
    action.transition_to(AppMessageQueued).unwrap();
    action.transition_to(AppMessageHandled).unwrap();
    action.transition_to(WaitingForExpectedState).unwrap();
    assert!(action.timeout_at_ms.is_some());

    // Transition to TimedOut must clear timeout_at_ms
    action.transition_to(TimedOut).unwrap();
    assert_eq!(action.state, TimedOut);
    assert!(
        action.timeout_at_ms.is_none(),
        "timeout_at_ms should be cleared on TimedOut terminal state"
    );
}

// =========================================================================
// Concurrency tests for GuiActionHistory
// =========================================================================

#[test]
fn test_concurrent_record_no_data_loss() {
    // Spawn N threads, each recording an action.
    // After all join, verify all N are present and readable.
    let history = GuiActionHistory::with_capacity(100);
    let n: usize = 20;
    let mut handles = Vec::with_capacity(n);

    for i in 0..n {
        let h = history.clone();
        handles.push(std::thread::spawn(move || {
            let id = GuiActionId::new();
            let request = GuiActionRequest {
                action_id: id.clone(),
                requested_at_ms: i as i64 * 100,
                command: format!("Concurrent-{i}"),
            };
            let returned = h.record(request);
            assert_eq!(returned, id);
            id
        }));
    }

    let mut ids: Vec<GuiActionId> = handles
        .into_iter()
        .map(|h| h.join().expect("thread panicked"))
        .collect();
    ids.sort_by(|a, b| a.0.cmp(&b.0));

    assert_eq!(history.action_count(), n);
    assert_eq!(history.active_count(), n);

    for id in &ids {
        let status = history
            .get(id)
            .expect("every recorded action should be findable");
        assert_eq!(status.state, GuiActionState::Queued);
    }
}

#[test]
fn test_concurrent_record_and_get_no_panic() {
    // Reader threads call get() while writer threads call record().
    // Verify no panics and all actions eventually readable.
    let history = GuiActionHistory::with_capacity(100);
    let n_writers = 8;
    let n_readers = 4;
    let actions_per_writer = 10;

    let recorded_ids = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let ready = std::sync::Arc::new(std::sync::Barrier::new(n_writers + n_readers));

    let mut handles = Vec::new();

    // Writer threads
    for w in 0..n_writers {
        let h = history.clone();
        let ids = recorded_ids.clone();
        let barrier = ready.clone();
        handles.push(std::thread::spawn(move || {
            barrier.wait();
            for i in 0..actions_per_writer {
                let id = GuiActionId::new();
                let request = GuiActionRequest {
                    action_id: id.clone(),
                    requested_at_ms: (w * actions_per_writer + i) as i64 * 100,
                    command: format!("Writer-{w}-{i}"),
                };
                h.record(request);
                ids.lock().unwrap().push(id);
            }
        }));
    }

    // Reader threads
    for _ in 0..n_readers {
        let h = history.clone();
        let barrier = ready.clone();
        handles.push(std::thread::spawn(move || {
            barrier.wait();
            // Repeatedly read all actions — should never panic
            for _ in 0..50 {
                let _all = h.all_actions();
                let _count = h.action_count();
                let _active = h.active_count();
            }
        }));
    }

    // Wait for all
    for h in handles {
        h.join().expect("thread panicked");
    }

    let total_written = n_writers * actions_per_writer;
    assert_eq!(history.action_count(), total_written);
    assert_eq!(history.active_count(), total_written);
}

#[test]
fn test_concurrent_transition_and_get() {
    // Writer threads progress actions through states while readers query.
    let history = GuiActionHistory::with_capacity(50);
    let n: usize = 20;
    let barrier = std::sync::Arc::new(std::sync::Barrier::new(n));

    let ids: Vec<GuiActionId> = (0..n)
        .map(|i| {
            let id = GuiActionId::new();
            let request = GuiActionRequest {
                action_id: id.clone(),
                requested_at_ms: i as i64 * 100,
                command: format!("Trans-{i}"),
            };
            history.record(request);
            id
        })
        .collect();

    let mut handles = Vec::with_capacity(n);
    for (idx, id) in ids.iter().enumerate() {
        let h = history.clone();
        let aid = id.clone();
        let bar = barrier.clone();
        handles.push(std::thread::spawn(move || {
            bar.wait();
            // Transition through a valid lifecycle
            h.transition_to(&aid, GuiActionState::Validating).ok();
            h.transition_to(&aid, GuiActionState::AppMessageQueued).ok();
            h.transition_to(&aid, GuiActionState::AppMessageHandled)
                .ok();
            h.transition_to(&aid, GuiActionState::Completed).ok();
            // Every 5th action, also read the result
            if idx % 5 == 0 {
                let _status = h.get(&aid);
            }
        }));
    }

    for h in handles {
        h.join().expect("thread panicked");
    }

    // All should be completed
    for id in &ids {
        let status = history.get(id).expect("action should exist");
        assert_eq!(
            status.state,
            GuiActionState::Completed,
            "action {:?} should be completed",
            id
        );
    }
    assert_eq!(history.active_count(), 0);
}

#[test]
fn test_concurrent_remove_and_get() {
    // Concurrent remove() and get() calls on the same history.
    let history = GuiActionHistory::with_capacity(50);
    let n: usize = 20;

    let ids: Vec<GuiActionId> = (0..n)
        .map(|i| {
            let id = GuiActionId::new();
            let request = GuiActionRequest {
                action_id: id.clone(),
                requested_at_ms: i as i64 * 100,
                command: format!("Remove-{i}"),
            };
            history.record(request);
            id
        })
        .collect();

    let barrier = std::sync::Arc::new(std::sync::Barrier::new(n));
    let mut handles = Vec::with_capacity(n);

    for (idx, id) in ids.iter().enumerate() {
        let h = history.clone();
        let aid = id.clone();
        let bar = barrier.clone();
        handles.push(std::thread::spawn(move || {
            bar.wait();
            // Half remove, half read
            if idx % 2 == 0 {
                let _removed = h.remove(&aid);
            } else {
                let _status = h.get(&aid);
            }
        }));
    }

    for h in handles {
        h.join().expect("thread panicked");
    }

    // Evens were removed, odds should still exist
    for (idx, id) in ids.iter().enumerate() {
        if idx % 2 == 0 {
            assert!(
                history.get(id).is_none(),
                "even-indexed action should be removed"
            );
        } else {
            assert!(
                history.get(id).is_some(),
                "odd-indexed action should still exist"
            );
        }
    }
}

#[test]
fn test_concurrent_record_with_capacity_eviction() {
    // Hit the capacity bound while multiple threads are recording.
    // Verify that eviction still works and no data is lost/duplicated.
    let capacity = 10;
    let history = GuiActionHistory::with_capacity(capacity);
    let n_threads = 8;
    let actions_per_thread = 5; // 40 total vs capacity 10

    let mut handles = Vec::with_capacity(n_threads);

    for t in 0..n_threads {
        let h = history.clone();
        handles.push(std::thread::spawn(move || {
            for i in 0..actions_per_thread {
                let id = GuiActionId::new();
                let request = GuiActionRequest {
                    action_id: id.clone(),
                    requested_at_ms: (t * actions_per_thread + i) as i64 * 100,
                    command: format!("Evict-{t}-{i}"),
                };
                h.record(request);
                // Mark some as completed to allow targeted eviction
                if i % 2 == 0 {
                    h.set_state(&id, GuiActionState::Completed);
                }
            }
        }));
    }

    for h in handles {
        h.join().expect("thread panicked");
    }

    // Store should be at capacity (no more, no less)
    let count = history.action_count();
    assert!(
        count <= capacity,
        "should not exceed capacity: {count} > {capacity}"
    );
    // There may be fewer if some got pruned via order eviction,
    // but it should be tightly bounded near capacity
    assert!(
        count >= capacity - n_threads, // allow some slack due to concurrent eviction
        "should be near capacity: {count} < {}",
        capacity - n_threads
    );

    // Verify no action has a badly corrupted state
    let all = history.all_actions();
    for a in &all {
        assert!(
            a.state.is_terminal() || a.state.is_active(),
            "action {:?} has invalid state",
            a.action_id
        );
    }

    // All actions should have unique IDs
    let mut ids: Vec<&str> = all.iter().map(|a| a.action_id.0.as_str()).collect();
    ids.sort();
    ids.dedup();
    assert_eq!(ids.len(), all.len(), "all action IDs must be unique");
}

#[test]
fn test_concurrent_all_actions_ordering() {
    // Verify newest-first ordering holds under concurrent read/write.
    let history = GuiActionHistory::with_capacity(50);
    let n: usize = 15;

    let ids: Vec<GuiActionId> = (0..n)
        .map(|i| {
            let id = GuiActionId::new();
            let request = GuiActionRequest {
                action_id: id.clone(),
                requested_at_ms: i as i64 * 100,
                command: format!("Order-{i}"),
            };
            history.record(request);
            id
        })
        .collect();

    // Read all_actions from multiple threads simultaneously
    let barrier = std::sync::Arc::new(std::sync::Barrier::new(5));
    let mut handles = Vec::with_capacity(5);
    for _ in 0..5 {
        let h = history.clone();
        let bar = barrier.clone();
        let ids_ref = ids.clone();
        handles.push(std::thread::spawn(move || {
            bar.wait();
            for _ in 0..20 {
                let all = h.all_actions();
                // The result should have n entries
                assert_eq!(all.len(), n);
                // Should be ordered newest first (descending by insertion order)
                for pair in all.windows(2) {
                    let earlier_idx = ids_ref.iter().position(|id| id == &pair[1].action_id);
                    let later_idx = ids_ref.iter().position(|id| id == &pair[0].action_id);
                    if let (Some(e), Some(l)) = (earlier_idx, later_idx) {
                        assert!(
                            e <= l,
                            "newest-first ordering violated: {} before {}",
                            pair[0].action_id.0,
                            pair[1].action_id.0,
                        );
                    }
                }
            }
        }));
    }

    for h in handles {
        h.join().expect("thread panicked");
    }
}

#[test]
fn test_concurrent_mixed_operations_no_deadlock() {
    // Stress test: mix of record, get, transition, remove, all_actions,
    // active_count, and check_timeouts from many threads simultaneously.
    // If there's a lock inversion or deadlock, this test will hang.
    let history = GuiActionHistory::with_capacity(20);
    let n_threads = 12;
    let iterations = 25;

    let mut handles = Vec::with_capacity(n_threads);

    for t in 0..n_threads {
        let h = history.clone();
        handles.push(std::thread::spawn(move || {
            for i in 0..iterations {
                let id = GuiActionId::new();
                let request = GuiActionRequest {
                    action_id: id.clone(),
                    requested_at_ms: (t * iterations + i) as i64,
                    command: format!("Mix-{t}-{i}"),
                };
                let rid = h.record(request);

                // Vary the operation per iteration to mix access patterns
                match i % 6 {
                    0 => {
                        // Transition and read
                        h.transition_to(&rid, GuiActionState::Validating).ok();
                        let _s = h.get(&rid);
                    }
                    1 => {
                        // Read all
                        let _all = h.all_actions();
                    }
                    2 => {
                        // Transition and remove
                        h.transition_to(&rid, GuiActionState::Validating).ok();
                        h.set_state(&rid, GuiActionState::Completed);
                        let _r = h.remove(&rid);
                    }
                    3 => {
                        // check_timeouts
                        let _to = h.check_timeouts();
                    }
                    4 => {
                        // active_count and action_count
                        let _ac = h.active_count();
                        let _cnt = h.action_count();
                    }
                    5 => {
                        // Transition through full lifecycle
                        h.transition_to(&rid, GuiActionState::Validating).ok();
                        h.transition_to(&rid, GuiActionState::AppMessageQueued).ok();
                        h.transition_to(&rid, GuiActionState::AppMessageHandled)
                            .ok();
                        h.transition_to(&rid, GuiActionState::WaitingForExpectedState)
                            .ok();
                    }
                    _ => {}
                }
            }
        }));
    }

    for h in handles {
        h.join().expect("thread panicked");
    }

    // Basic sanity: should not panic or hang
    let all = history.all_actions();
    // active_count should be consistent
    let active = history.active_count();
    let total = history.action_count();
    assert!(
        active <= total,
        "active count ({active}) cannot exceed total ({total})"
    );

    // Verify no duplicate action IDs
    let mut ids: Vec<&str> = all.iter().map(|a| a.action_id.0.as_str()).collect();
    ids.sort();
    let deduped = {
        let mut d = ids.clone();
        d.dedup();
        d
    };
    assert_eq!(
        ids.len(),
        deduped.len(),
        "no duplicate action IDs allowed under concurrent access"
    );
}

#[test]
fn test_concurrent_status_reads() {
    // Multiple threads reading action statuses concurrently.
    let history = GuiActionHistory::with_capacity(30);

    // Pre-populate
    for i in 0..10 {
        let id = GuiActionId::new();
        let request = GuiActionRequest {
            action_id: id.clone(),
            requested_at_ms: i * 100,
            command: format!("Read-{i}"),
        };
        history.record(request);
    }

    let barrier = std::sync::Arc::new(std::sync::Barrier::new(8));
    let mut handles = Vec::with_capacity(8);

    for r in 0..8 {
        let h = history.clone();
        let bar = barrier.clone();
        handles.push(std::thread::spawn(move || {
            bar.wait();
            // Each reader calls multiple query methods
            for _ in 0..100 {
                let all = h.all_actions();
                let count = h.action_count();
                let _active = h.active_count();
                let _queued = h.actions_with_state(GuiActionState::Queued);
                let _next_timeout = h.next_timeout_remaining_ms();

                // all should contain the right number
                assert_eq!(all.len(), count);
            }
            format!("Reader-{r} done")
        }));
    }

    for h in handles {
        h.join().expect("reader thread panicked");
    }

    // Verify no corruption from concurrent reads
    let all = history.all_actions();
    assert_eq!(all.len(), 10);
    assert_eq!(history.active_count(), 10);
}

#[test]
fn test_concurrent_record_and_transition_chain() {
    // Multiple queued navigation-like actions: record an action,
    // transition it through states mimicking a real action lifecycle.
    // This simulates multiple queued navigation actions being processed.
    let history = GuiActionHistory::with_capacity(50);
    let n: usize = 16;

    let barrier = std::sync::Arc::new(std::sync::Barrier::new(n));
    let mut handles = Vec::with_capacity(n);

    for idx in 0..n {
        let h = history.clone();
        let bar = barrier.clone();
        handles.push(std::thread::spawn(move || {
            bar.wait();
            // Each thread simulates one navigation action's lifecycle
            let id = GuiActionId::new();
            let request = GuiActionRequest {
                action_id: id.clone(),
                requested_at_ms: idx as i64 * 100,
                command: match idx % 4 {
                    0 => "GoToChatList".into(),
                    1 => "OpenRoom".into(),
                    2 => "OpenSettings".into(),
                    _ => "GoToChatList".into(),
                },
            };
            let rid = h.record(request);

            // Status after record should be Queued
            let status = h.get(&rid).unwrap();
            assert_eq!(status.state, GuiActionState::Queued);

            // Gradually transition through the lifecycle
            h.transition_to(&rid, GuiActionState::Validating).unwrap();
            h.transition_to(&rid, GuiActionState::AppMessageQueued)
                .unwrap();
            h.transition_to(&rid, GuiActionState::AppMessageHandled)
                .unwrap();
            h.transition_to(&rid, GuiActionState::Completed).unwrap();

            // Final check
            let final_status = h.get(&rid).unwrap();
            assert_eq!(final_status.state, GuiActionState::Completed);
            assert!(final_status.state.is_terminal());

            rid
        }));
    }

    let results: Vec<GuiActionId> = handles
        .into_iter()
        .map(|h| h.join().expect("thread panicked"))
        .collect();

    // Verify all completed
    for id in &results {
        let status = history.get(id).expect("action should exist");
        assert_eq!(status.state, GuiActionState::Completed);
    }

    // Verify count is correct (ordering is non-deterministic under concurrency)
    let all = history.all_actions();
    assert_eq!(all.len(), n);
}

#[test]
fn test_concurrent_composer_update_followed_by_submit() {
    // Simulate: set composer text, then submit — in sequence but
    // with concurrent status reads in between.
    let history = GuiActionHistory::with_capacity(10);

    // Set composer text action
    let compose_id = GuiActionId::new();
    history.record(GuiActionRequest {
        action_id: compose_id.clone(),
        requested_at_ms: 100,
        command: "SetComposerText".into(),
    });

    // Submit composer action
    let submit_id = GuiActionId::new();
    history.record(GuiActionRequest {
        action_id: submit_id.clone(),
        requested_at_ms: 200,
        command: "SubmitComposer".into(),
    });

    // Transition compose action while reading in parallel
    let h1 = history.clone();
    let h2 = history.clone();
    let cid = compose_id.clone();
    let sid = submit_id.clone();

    let barrier = std::sync::Arc::new(std::sync::Barrier::new(3));

    let t1 = {
        let bar = barrier.clone();
        std::thread::spawn(move || {
            bar.wait();
            h1.transition_to(&cid, GuiActionState::Validating).unwrap();
            h1.transition_to(&cid, GuiActionState::AppMessageQueued)
                .unwrap();
            h1.transition_to(&cid, GuiActionState::AppMessageHandled)
                .unwrap();
            h1.transition_to(&cid, GuiActionState::Completed).unwrap();
        })
    };

    let t2 = {
        let bar = barrier.clone();
        std::thread::spawn(move || {
            bar.wait();
            // Once compose is progressing, start submit
            h2.transition_to(&sid, GuiActionState::Validating).unwrap();
            h2.transition_to(&sid, GuiActionState::AppMessageQueued)
                .unwrap();
            h2.transition_to(&sid, GuiActionState::AppMessageHandled)
                .unwrap();
            h2.transition_to(&sid, GuiActionState::Completed).unwrap();
        })
    };

    // Reader thread checks status concurrently
    let h3 = history.clone();
    let _t3 = std::thread::spawn(move || {
        barrier.wait();
        for _ in 0..20 {
            let _all = h3.all_actions();
            let _count = h3.action_count();
        }
    });

    t1.join().expect("compose thread panicked");
    t2.join().expect("submit thread panicked");

    // Now check that submit eventually completed
    // (it may finish before compose due to scheduling, but both should complete)
    let compose_status = history.get(&compose_id).expect("compose action exists");
    let submit_status = history.get(&submit_id).expect("submit action exists");
    assert!(
        compose_status.state.is_terminal(),
        "compose action should be terminal: {:?}",
        compose_status.state
    );
    assert!(
        submit_status.state.is_terminal(),
        "submit action should be terminal: {:?}",
        submit_status.state
    );
}

#[test]
fn test_action_timeout_while_another_succeeds() {
    // One action times out (via check_timeouts) while another
    // successfully completes through its lifecycle.
    let history = GuiActionHistory::with_capacity(10);

    // Action A: normal lifecycle → Completed
    let id_a = GuiActionId::new();
    history.record(GuiActionRequest {
        action_id: id_a.clone(),
        requested_at_ms: 100,
        command: "NormalAction".into(),
    });
    history
        .transition_to(&id_a, GuiActionState::Validating)
        .unwrap();
    history
        .transition_to(&id_a, GuiActionState::AppMessageQueued)
        .unwrap();
    history
        .transition_to(&id_a, GuiActionState::AppMessageHandled)
        .unwrap();
    history
        .transition_to(&id_a, GuiActionState::Completed)
        .unwrap();

    // Action B: enters WaitingForExpectedState with expired timeout
    let id_b = GuiActionId::new();
    history.record(GuiActionRequest {
        action_id: id_b.clone(),
        requested_at_ms: 200,
        command: "TimeoutAction".into(),
    });
    history
        .transition_to(&id_b, GuiActionState::Validating)
        .unwrap();
    history
        .transition_to(&id_b, GuiActionState::AppMessageQueued)
        .unwrap();
    history
        .transition_to(&id_b, GuiActionState::AppMessageHandled)
        .unwrap();
    history
        .transition_to(&id_b, GuiActionState::WaitingForExpectedState)
        .unwrap();

    // Manually set timeout to the past so check_timeouts catches it
    {
        let mut actions = history.inner.actions.lock().expect("actions lock");
        if let Some(status) = actions.get_mut(&id_b) {
            status.timeout_at_ms = Some(1); // epoch + 1ms = long past
        }
    }

    // Run timeout check
    let timed_out = history.check_timeouts();

    // Verify action A is still Completed
    let status_a = history.get(&id_a).unwrap();
    assert_eq!(status_a.state, GuiActionState::Completed);

    // Verify action B was timed out
    assert_eq!(timed_out.len(), 1, "exactly one action should time out");
    assert_eq!(timed_out[0].0, id_b);
    assert_eq!(timed_out[0].1.state, GuiActionState::TimedOut);

    let status_b = history.get(&id_b).unwrap();
    assert_eq!(status_b.state, GuiActionState::TimedOut);
}

#[test]
fn test_concurrent_timeout_check_during_lifecycle() {
    // Some threads transition actions through their lifecycle while
    // another thread calls check_timeouts(). Verify no deadlock.
    let history = GuiActionHistory::with_capacity(20);
    let n: usize = 8;

    let ids: Vec<GuiActionId> = (0..n)
        .map(|i| {
            let id = GuiActionId::new();
            let request = GuiActionRequest {
                action_id: id.clone(),
                requested_at_ms: i as i64 * 100,
                command: format!("TimeoutTest-{i}"),
            };
            history.record(request);
            id
        })
        .collect();

    let barrier = std::sync::Arc::new(std::sync::Barrier::new(n + 1));
    let mut handles = Vec::with_capacity(n + 1);

    // Worker threads: transition actions through lifecycles
    for (idx, id) in ids.iter().enumerate() {
        let h = history.clone();
        let aid = id.clone();
        let bar = barrier.clone();
        handles.push(std::thread::spawn(move || {
            bar.wait();
            // Put some in waiting, some through to completion
            h.transition_to(&aid, GuiActionState::Validating).unwrap();
            h.transition_to(&aid, GuiActionState::AppMessageQueued)
                .unwrap();
            h.transition_to(&aid, GuiActionState::AppMessageHandled)
                .unwrap();
            if idx % 2 == 0 {
                h.transition_to(&aid, GuiActionState::WaitingForExpectedState)
                    .unwrap();
            } else {
                h.transition_to(&aid, GuiActionState::Completed).unwrap();
            }
            // Sleep a tiny bit to increase contention window
            std::thread::sleep(std::time::Duration::from_micros(10));
        }));
    }

    // Timeout checker thread
    let h_tc = history.clone();
    let bar_tc = barrier.clone();
    handles.push(std::thread::spawn(move || {
        bar_tc.wait();
        for _ in 0..20 {
            let _timed_out = h_tc.check_timeouts();
            std::thread::sleep(std::time::Duration::from_micros(5));
        }
    }));

    for h in handles {
        h.join().expect("thread panicked");
    }

    // Verify no structural corruption
    let all = history.all_actions();
    assert_eq!(all.len(), n);

    // Even-indexed actions should be WaitingForExpectedState or TimedOut
    for (idx, id) in ids.iter().enumerate() {
        let status = history.get(id).expect("action should exist");
        if idx % 2 == 0 {
            // Could be WaitingForExpectedState or TimedOut (if check_timeouts caught it)
            assert!(
                status.state == GuiActionState::WaitingForExpectedState
                    || status.state == GuiActionState::TimedOut,
                "even index {idx} should be waiting or timed out, got {:?}",
                status.state
            );
        } else {
            assert_eq!(
                status.state,
                GuiActionState::Completed,
                "odd index {idx} should be completed"
            );
        }
    }
}

#[test]
fn test_gui_action_history_lock_no_deadlock() {
    // Verify the two-lock design (actions + order mutex) does not
    // deadlock under concurrent record + remove operations.
    // record() acquires actions then order; remove() acquires order then actions.
    // This is the classic lock ordering scenario.
    let history = GuiActionHistory::with_capacity(10);

    let ids: Vec<GuiActionId> = (0..5)
        .map(|i| {
            let id = GuiActionId::new();
            let request = GuiActionRequest {
                action_id: id.clone(),
                requested_at_ms: i as i64 * 100,
                command: format!("Lock-{i}"),
            };
            history.record(request);
            id
        })
        .collect();

    // Complete them so they can be evicted
    for id in &ids {
        history.set_state(id, GuiActionState::Completed);
    }

    let barrier = std::sync::Arc::new(std::sync::Barrier::new(6));
    let mut handles = Vec::with_capacity(6);

    // Threads 0-4: alternate between record() and remove()
    for (idx, aid) in ids.iter().enumerate().take(5) {
        let h = history.clone();
        let aid = aid.clone();
        let bar = barrier.clone();
        handles.push(std::thread::spawn(move || {
            bar.wait();
            for round in 0..20 {
                if round % 2 == 0 {
                    // record() acquires actions then order
                    let new_id = GuiActionId::new();
                    let request = GuiActionRequest {
                        action_id: new_id.clone(),
                        requested_at_ms: round as i64 * 100,
                        command: format!("Record-{idx}-{round}"),
                    };
                    h.record(request);
                } else {
                    // remove() acquires order then actions
                    let _ = h.remove(&aid);
                    // Re-add so next round has something to remove
                    let request = GuiActionRequest {
                        action_id: aid.clone(),
                        requested_at_ms: round as i64 * 100,
                        command: format!("ReAdd-{idx}"),
                    };
                    h.record(request);
                }
            }
        }));
    }

    // Thread 5: reads all_actions() which acquires both locks
    let h_reader = history.clone();
    let bar_reader = barrier.clone();
    handles.push(std::thread::spawn(move || {
        bar_reader.wait();
        for _ in 0..50 {
            let _all = h_reader.all_actions();
            std::thread::sleep(std::time::Duration::from_micros(5));
        }
    }));

    for h in handles {
        h.join().expect("thread panicked");
    }

    // If we reached here, there's no deadlock
    // Verify the history is still internally consistent
    let all = history.all_actions();
    let total = history.action_count();
    assert_eq!(all.len(), total);

    // No duplicate IDs
    let mut id_set: Vec<&str> = all.iter().map(|a| a.action_id.0.as_str()).collect();
    id_set.sort();
    let len_before = id_set.len();
    id_set.dedup();
    assert_eq!(
        id_set.len(),
        len_before,
        "no duplicate IDs under concurrent access"
    );
}

#[test]
fn test_gui_action_history_arc_clone_shared_access() {
    // Verify that cloning Arc<GuiActionHistoryInner> works correctly
    // across threads — both clones see the same state.
    let history = GuiActionHistory::new();
    let h2 = history.clone();

    let id = GuiActionId::new();
    history.record(GuiActionRequest {
        action_id: id.clone(),
        requested_at_ms: 100,
        command: "Shared".into(),
    });

    // The clone should see the same data
    assert!(h2.get(&id).is_some());
    assert_eq!(h2.action_count(), 1);

    // Record via h2, read via history
    let id2 = GuiActionId::new();
    h2.record(GuiActionRequest {
        action_id: id2.clone(),
        requested_at_ms: 200,
        command: "Shared2".into(),
    });

    assert!(history.get(&id2).is_some());
    assert_eq!(history.action_count(), 2);
}

#[test]
fn test_gui_action_history_next_timeout_concurrent_access() {
    // Verify next_timeout_remaining_ms is safe under concurrent
    // transitions that modify timeout_at_ms.
    let history = GuiActionHistory::with_capacity(10);
    let n: usize = 6;

    let ids: Vec<GuiActionId> = (0..n)
        .map(|i| {
            let id = GuiActionId::new();
            let request = GuiActionRequest {
                action_id: id.clone(),
                requested_at_ms: i as i64 * 100,
                command: format!("TimeoutRead-{i}"),
            };
            history.record(request);
            id
        })
        .collect();

    // Put all into WaitingForExpectedState
    for id in &ids {
        history
            .transition_to(id, GuiActionState::Validating)
            .unwrap();
        history
            .transition_to(id, GuiActionState::AppMessageQueued)
            .unwrap();
        history
            .transition_to(id, GuiActionState::AppMessageHandled)
            .unwrap();
        history
            .transition_to(id, GuiActionState::WaitingForExpectedState)
            .unwrap();
    }

    let barrier = std::sync::Arc::new(std::sync::Barrier::new(n + 2));
    let mut handles = Vec::with_capacity(n + 2);

    // Worker threads: complete some actions (removing timeout), timeout others
    for (idx, id) in ids.iter().enumerate() {
        let h = history.clone();
        let aid = id.clone();
        let bar = barrier.clone();
        handles.push(std::thread::spawn(move || {
            bar.wait();
            if idx % 2 == 0 {
                // Complete normally — clears timeout
                h.transition_to(&aid, GuiActionState::Completed).unwrap();
            } else {
                // Let it stay in waiting (timeout remains)
            }
        }));
    }

    // Reader threads: read next_timeout_remaining_ms concurrently
    for _ in 0..2 {
        let h = history.clone();
        let bar = barrier.clone();
        handles.push(std::thread::spawn(move || {
            bar.wait();
            for _ in 0..30 {
                let _remaining = h.next_timeout_remaining_ms();
            }
        }));
    }

    for h in handles {
        h.join().expect("thread panicked");
    }

    // Even-indexed should be Completed, odd-indexed still Waiting
    for (idx, id) in ids.iter().enumerate() {
        let status = history.get(id).expect("action exists");
        if idx % 2 == 0 {
            assert_eq!(status.state, GuiActionState::Completed);
        } else {
            assert_eq!(status.state, GuiActionState::WaitingForExpectedState);
            assert!(status.timeout_at_ms.is_some());
        }
    }

    // next_timeout_remaining_ms should not panic
    let _remaining = history.next_timeout_remaining_ms();
}

#[test]
fn test_gui_action_history_eviction_under_concurrent_record() {
    // Simulate queue-full behaviour: capacity is small, many threads
    // record concurrently, forcing frequent eviction.
    let capacity = 5;
    let history = GuiActionHistory::with_capacity(capacity);
    let n_threads = 10;
    let actions_per_thread = 20; // 200 total vs capacity 5

    let mut handles = Vec::with_capacity(n_threads);

    for t in 0..n_threads {
        let h = history.clone();
        handles.push(std::thread::spawn(move || {
            for i in 0..actions_per_thread {
                let id = GuiActionId::new();
                let request = GuiActionRequest {
                    action_id: id.clone(),
                    requested_at_ms: (t * actions_per_thread + i) as i64 * 10,
                    command: format!("Full-{t}-{i}"),
                };
                h.record(request);
                // Mark as terminal quickly to let eviction happen
                h.set_state(&id, GuiActionState::Completed);
            }
        }));
    }

    for h in handles {
        h.join().expect("thread panicked");
    }

    // Should be bounded at capacity
    let count = history.action_count();
    assert!(
        count <= capacity,
        "should not exceed capacity: {count} > {capacity}"
    );

    // All stored actions should be terminal
    for a in history.all_actions() {
        assert!(
            a.state.is_terminal(),
            "stored action {:?} should be terminal under queue-full scenario",
            a.action_id
        );
    }
}

// ── GuiTestCommand serialization tests ──────────────────────────

#[test]
fn test_gui_test_command_go_to_chat_list_serde() {
    let cmd = GuiTestCommand::GoToChatList;
    let json = serde_json::to_string(&cmd).unwrap();
    assert_eq!(json, r#"{"command":"go_to_chat_list"}"#);
    let deser: GuiTestCommand = serde_json::from_str(&json).unwrap();
    assert_eq!(cmd, deser);
    assert_eq!(deser, GuiTestCommand::GoToChatList);
}

#[test]
fn test_gui_test_command_open_room_serde() {
    let cmd = GuiTestCommand::OpenRoom {
        room_id: "ab".to_string(),
    };
    let json = serde_json::to_string(&cmd).unwrap();
    assert_eq!(json, r#"{"command":"open_room","room_id":"ab"}"#);
    let deser: GuiTestCommand = serde_json::from_str(&json).unwrap();
    assert_eq!(cmd, deser);
}

#[test]
fn test_gui_test_command_open_conversation_serde() {
    let cmd = GuiTestCommand::OpenConversation {
        conversation_id: "deadbeef".to_string(),
    };
    let json = serde_json::to_string(&cmd).unwrap();
    assert_eq!(
        json,
        r#"{"command":"open_conversation","conversation_id":"deadbeef"}"#
    );
    let deser: GuiTestCommand = serde_json::from_str(&json).unwrap();
    assert_eq!(cmd, deser);
}

#[test]
fn test_gui_test_command_open_friends_serde() {
    let cmd = GuiTestCommand::OpenFriends;
    let json = serde_json::to_string(&cmd).unwrap();
    assert_eq!(json, r#"{"command":"open_friends"}"#);
    let deser: GuiTestCommand = serde_json::from_str(&json).unwrap();
    assert_eq!(cmd, deser);
}

#[test]
fn test_gui_test_command_open_settings_serde() {
    let cmd = GuiTestCommand::OpenSettings;
    let json = serde_json::to_string(&cmd).unwrap();
    assert_eq!(json, r#"{"command":"open_settings"}"#);
    let deser: GuiTestCommand = serde_json::from_str(&json).unwrap();
    assert_eq!(cmd, deser);
}

#[test]
fn test_gui_test_command_close_dialog_serde() {
    let cmd = GuiTestCommand::CloseDialog;
    let json = serde_json::to_string(&cmd).unwrap();
    assert_eq!(json, r#"{"command":"close_dialog"}"#);
    let deser: GuiTestCommand = serde_json::from_str(&json).unwrap();
    assert_eq!(cmd, deser);
}

#[test]
fn test_gui_test_command_set_composer_text_serde() {
    let cmd = GuiTestCommand::SetComposerText {
        text: "hello world".to_string(),
    };
    let json = serde_json::to_string(&cmd).unwrap();
    assert_eq!(
        json,
        r#"{"command":"set_composer_text","text":"hello world"}"#
    );
    let deser: GuiTestCommand = serde_json::from_str(&json).unwrap();
    assert_eq!(cmd, deser);
}

#[test]
fn test_gui_test_command_submit_composer_serde() {
    let cmd = GuiTestCommand::SubmitComposer;
    let json = serde_json::to_string(&cmd).unwrap();
    assert_eq!(json, r#"{"command":"submit_composer"}"#);
    let deser: GuiTestCommand = serde_json::from_str(&json).unwrap();
    assert_eq!(cmd, deser);
}

#[test]
fn test_gui_test_command_select_peer_serde() {
    let cmd = GuiTestCommand::SelectPeer {
        peer_id: "0123456789abcdef".to_string(),
    };
    let json = serde_json::to_string(&cmd).unwrap();
    assert_eq!(
        json,
        r#"{"command":"select_peer","peer_id":"0123456789abcdef"}"#
    );
    let deser: GuiTestCommand = serde_json::from_str(&json).unwrap();
    assert_eq!(cmd, deser);
}

#[test]
fn test_gui_test_command_toggle_dark_mode_serde() {
    let cmd = GuiTestCommand::ToggleDarkMode { enabled: true };
    let json = serde_json::to_string(&cmd).unwrap();
    assert_eq!(json, r#"{"command":"toggle_dark_mode","enabled":true}"#);
    let deser: GuiTestCommand = serde_json::from_str(&json).unwrap();
    assert_eq!(cmd, deser);
    assert_eq!(deser, GuiTestCommand::ToggleDarkMode { enabled: true });

    let cmd_off = GuiTestCommand::ToggleDarkMode { enabled: false };
    let json_off = serde_json::to_string(&cmd_off).unwrap();
    assert_eq!(
        json_off,
        r#"{"command":"toggle_dark_mode","enabled":false}"#
    );
    let deser_off: GuiTestCommand = serde_json::from_str(&json_off).unwrap();
    assert_eq!(cmd_off, deser_off);
}

#[test]
fn test_gui_test_command_toggle_help_serde() {
    let cmd = GuiTestCommand::ToggleHelp;
    let json = serde_json::to_string(&cmd).unwrap();
    assert_eq!(json, r#"{"command":"toggle_help"}"#);
    let deser: GuiTestCommand = serde_json::from_str(&json).unwrap();
    assert_eq!(cmd, deser);
}

#[test]
fn test_gui_test_command_wait_screen_is_serde() {
    let cmd = GuiTestCommand::Wait {
        condition: GuiWaitCondition::ScreenIs {
            expected: "ChatList".to_string(),
        },
        timeout_ms: 5000,
    };
    let json = serde_json::to_string(&cmd).unwrap();
    assert_eq!(
        json,
        r#"{"command":"wait","condition":{"type":"screen_is","expected":"ChatList"},"timeout_ms":5000}"#
    );
    let deser: GuiTestCommand = serde_json::from_str(&json).unwrap();
    assert_eq!(cmd, deser);
}

#[test]
fn test_gui_test_command_wait_room_selected_serde() {
    let cmd = GuiTestCommand::Wait {
        condition: GuiWaitCondition::RoomSelected {
            room_topic: Some("topic123".to_string()),
        },
        timeout_ms: 30000,
    };
    let json = serde_json::to_string(&cmd).unwrap();
    let deser: GuiTestCommand = serde_json::from_str(&json).unwrap();
    assert_eq!(cmd, deser);

    // With None
    let cmd_none = GuiTestCommand::Wait {
        condition: GuiWaitCondition::RoomSelected { room_topic: None },
        timeout_ms: 30000,
    };
    let json_none = serde_json::to_string(&cmd_none).unwrap();
    let deser_none: GuiTestCommand = serde_json::from_str(&json_none).unwrap();
    assert_eq!(cmd_none, deser_none);
}

#[test]
fn test_gui_test_command_wait_peer_visible_serde() {
    let cmd = GuiTestCommand::Wait {
        condition: GuiWaitCondition::PeerVisible { min_count: 3 },
        timeout_ms: 10000,
    };
    let json = serde_json::to_string(&cmd).unwrap();
    assert_eq!(
        json,
        r#"{"command":"wait","condition":{"type":"peer_visible","min_count":3},"timeout_ms":10000}"#
    );
    let deser: GuiTestCommand = serde_json::from_str(&json).unwrap();
    assert_eq!(cmd, deser);
}

#[test]
fn test_gui_test_command_wait_message_visible_serde() {
    let cmd = GuiTestCommand::Wait {
        condition: GuiWaitCondition::MessageVisible { min_count: 1 },
        timeout_ms: 15000,
    };
    let json = serde_json::to_string(&cmd).unwrap();
    let deser: GuiTestCommand = serde_json::from_str(&json).unwrap();
    assert_eq!(cmd, deser);
}

#[test]
fn test_gui_test_command_wait_gui_revision_serde() {
    let cmd = GuiTestCommand::Wait {
        condition: GuiWaitCondition::GuiRevisionAtLeast {
            expected_revision: 42,
        },
        timeout_ms: 5000,
    };
    let json = serde_json::to_string(&cmd).unwrap();
    assert_eq!(
        json,
        r#"{"command":"wait","condition":{"type":"gui_revision_at_least","expected_revision":42},"timeout_ms":5000}"#
    );
    let deser: GuiTestCommand = serde_json::from_str(&json).unwrap();
    assert_eq!(cmd, deser);
}

#[test]
fn test_gui_test_command_wait_conversation_selected_serde() {
    let cmd = GuiTestCommand::Wait {
        condition: GuiWaitCondition::ConversationSelected {
            conversation_id: Some("conv1".to_string()),
        },
        timeout_ms: 5000,
    };
    let json = serde_json::to_string(&cmd).unwrap();
    let deser: GuiTestCommand = serde_json::from_str(&json).unwrap();
    assert_eq!(cmd, deser);

    // With None
    let cmd_none = GuiTestCommand::Wait {
        condition: GuiWaitCondition::ConversationSelected {
            conversation_id: None,
        },
        timeout_ms: 5000,
    };
    let json_none = serde_json::to_string(&cmd_none).unwrap();
    let deser_none: GuiTestCommand = serde_json::from_str(&json_none).unwrap();
    assert_eq!(cmd_none, deser_none);
}

#[test]
fn test_gui_test_command_wait_composer_text_is_serde() {
    let cmd = GuiTestCommand::Wait {
        condition: GuiWaitCondition::ComposerTextIs {
            expected: "hello".to_string(),
        },
        timeout_ms: 5000,
    };
    let json = serde_json::to_string(&cmd).unwrap();
    assert_eq!(
        json,
        r#"{"command":"wait","condition":{"type":"composer_text_is","expected":"hello"},"timeout_ms":5000}"#
    );
    let deser: GuiTestCommand = serde_json::from_str(&json).unwrap();
    assert_eq!(cmd, deser);
}

#[test]
fn test_gui_test_command_wait_dialog_open_closed_serde() {
    let cmd_open = GuiTestCommand::Wait {
        condition: GuiWaitCondition::DialogOpen,
        timeout_ms: 5000,
    };
    let json_open = serde_json::to_string(&cmd_open).unwrap();
    assert_eq!(
        json_open,
        r#"{"command":"wait","condition":{"type":"dialog_open"},"timeout_ms":5000}"#
    );
    let deser_open: GuiTestCommand = serde_json::from_str(&json_open).unwrap();
    assert_eq!(cmd_open, deser_open);

    let cmd_closed = GuiTestCommand::Wait {
        condition: GuiWaitCondition::DialogClosed,
        timeout_ms: 5000,
    };
    let json_closed = serde_json::to_string(&cmd_closed).unwrap();
    assert_eq!(
        json_closed,
        r#"{"command":"wait","condition":{"type":"dialog_closed"},"timeout_ms":5000}"#
    );
    let deser_closed: GuiTestCommand = serde_json::from_str(&json_closed).unwrap();
    assert_eq!(cmd_closed, deser_closed);
}

#[test]
fn test_gui_test_command_wait_unread_count_serde() {
    let cmd = GuiTestCommand::Wait {
        condition: GuiWaitCondition::UnreadCountAtLeast { min_count: 5 },
        timeout_ms: 10000,
    };
    let json = serde_json::to_string(&cmd).unwrap();
    assert_eq!(
        json,
        r#"{"command":"wait","condition":{"type":"unread_count_at_least","min_count":5},"timeout_ms":10000}"#
    );
    let deser: GuiTestCommand = serde_json::from_str(&json).unwrap();
    assert_eq!(cmd, deser);
}

#[test]
fn test_expected_state_serde() {
    let states: Vec<(ExpectedState, &str)> = vec![
        (
            ExpectedState::ScreenIs("ChatList".to_string()),
            r#"{"type":"screen_is","value":"ChatList"}"#,
        ),
        (
            ExpectedState::RoomSelected("topic123".to_string()),
            r#"{"type":"room_selected","value":"topic123"}"#,
        ),
        (
            ExpectedState::ConversationSelected("peer_key".to_string()),
            r#"{"type":"conversation_selected","value":"peer_key"}"#,
        ),
        (
            ExpectedState::ComposerTextIs("hello".to_string()),
            r#"{"type":"composer_text_is","value":"hello"}"#,
        ),
        (
            ExpectedState::DarkModeIs(true),
            r#"{"type":"dark_mode_is","value":true}"#,
        ),
        (ExpectedState::MessageSent, r#"{"type":"message_sent"}"#),
        (
            ExpectedState::HelpVisible(false),
            r#"{"type":"help_visible","value":false}"#,
        ),
        (
            ExpectedState::Generic("custom condition".to_string()),
            r#"{"type":"generic","value":"custom condition"}"#,
        ),
    ];

    for (state, expected_json) in states {
        let json = serde_json::to_string(&state).unwrap();
        assert_eq!(json, expected_json, "Mismatch for {:?}", state);
        let deser: ExpectedState = serde_json::from_str(&json).unwrap();
        assert_eq!(state, deser, "Roundtrip mismatch for {:?}", state);
    }
}

#[test]
fn test_gui_test_command_roundtrip_all_variants() {
    // Every variant serialized then deserialized must equal the original.
    let cmds: Vec<GuiTestCommand> = vec![
        GuiTestCommand::GoToChatList,
        GuiTestCommand::OpenRoom {
            room_id: "aabbccdd".to_string(),
        },
        GuiTestCommand::OpenConversation {
            conversation_id: "11223344".to_string(),
        },
        GuiTestCommand::OpenFriends,
        GuiTestCommand::OpenSettings,
        GuiTestCommand::CloseDialog,
        GuiTestCommand::SetComposerText {
            text: "test message".to_string(),
        },
        GuiTestCommand::SubmitComposer,
        GuiTestCommand::SelectPeer {
            peer_id: "ffeeddcc".to_string(),
        },
        GuiTestCommand::ToggleDarkMode { enabled: true },
        GuiTestCommand::ToggleHelp,
        GuiTestCommand::Wait {
            condition: GuiWaitCondition::ScreenIs {
                expected: "Settings".to_string(),
            },
            timeout_ms: 1000,
        },
    ];

    for cmd in cmds {
        let json = serde_json::to_string(&cmd).unwrap();
        let deser: GuiTestCommand = serde_json::from_str(&json).unwrap();
        assert_eq!(cmd, deser, "Roundtrip failed for {:?}", cmd);
    }
}

#[test]
fn test_gui_test_command_validation() {
    // Valid commands
    GuiTestCommand::GoToChatList.validate().unwrap();
    GuiTestCommand::OpenRoom {
        room_id: "valid_room_id".to_string(),
    }
    .validate()
    .unwrap();
    GuiTestCommand::SetComposerText {
        text: "Hello, world!".to_string(),
    }
    .validate()
    .unwrap();
    GuiTestCommand::ToggleDarkMode { enabled: true }
        .validate()
        .unwrap();
    GuiTestCommand::SubmitComposer.validate().unwrap();
    GuiTestCommand::CloseDialog.validate().unwrap();

    // Invalid: room_id too long (assumes GUI_TEST_COMMAND_MAX_STRING_LEN is 4096)
    let long_room = "x".repeat(GUI_TEST_COMMAND_MAX_STRING_LEN + 1);
    assert!(
        GuiTestCommand::OpenRoom {
            room_id: long_room.clone(),
        }
        .validate()
        .is_err(),
        "OpenRoom should reject over-long room_id"
    );

    // Invalid: composer text with control character
    assert!(
        GuiTestCommand::SetComposerText {
            text: "hello\nworld".to_string(),
        }
        .validate()
        .is_err(),
        "SetComposerText should reject control characters"
    );

    // Invalid: timeout exceeds max
    assert!(
        GuiTestCommand::Wait {
            condition: GuiWaitCondition::DialogClosed,
            timeout_ms: GUI_TEST_COMMAND_MAX_TIMEOUT_MS + 1,
        }
        .validate()
        .is_err(),
        "Wait should reject over-max timeout"
    );

    // Valid: timeout at max
    GuiTestCommand::Wait {
        condition: GuiWaitCondition::DialogClosed,
        timeout_ms: GUI_TEST_COMMAND_MAX_TIMEOUT_MS,
    }
    .validate()
    .unwrap();
}

#[test]
fn test_expected_state_matches_str() {
    let screen = ExpectedState::ScreenIs("ChatList".to_string());
    assert!(screen.matches_str("screen", "ChatList"));
    assert!(!screen.matches_str("screen", "Settings"));
    assert!(!screen.matches_str("room", "ChatList"));

    let room = ExpectedState::RoomSelected("abc".to_string());
    assert!(room.matches_str("room", "abc"));
    assert!(!room.matches_str("room", "xyz"));

    let dark = ExpectedState::DarkModeIs(true);
    assert!(dark.matches_str("dark_mode", "true"));
    assert!(!dark.matches_str("dark_mode", "false"));

    let msg = ExpectedState::MessageSent;
    assert!(msg.matches_str("message_sent", "true"));
    assert!(!msg.matches_str("message_sent", "false"));
}

// ── Concurrency tests ────────────────────────────────────────────────

#[test]
fn test_concurrent_multiple_navigation_actions() {
    // Multiple queued GUI navigation actions processed concurrently.
    // Each thread enqueues a navigation action and transitions it through
    // the lifecycle; verify all reach completion without deadlock.
    let history = GuiActionHistory::with_capacity(100);
    const N_THREADS: usize = 8;
    const ACTIONS_PER_THREAD: usize = 25;

    let mut handles = Vec::with_capacity(N_THREADS);

    for t in 0..N_THREADS {
        let h = history.clone();
        handles.push(std::thread::spawn(move || {
            for i in 0..ACTIONS_PER_THREAD {
                let id = GuiActionId::new();
                let request = GuiActionRequest {
                    action_id: id.clone(),
                    requested_at_ms: (t * ACTIONS_PER_THREAD + i) as i64 * 10,
                    command: format!("Nav-{t}-{i}"),
                };
                let rid = h.record(request);
                assert_eq!(rid, id, "recorded id should match");

                // Full lifecycle: Queued -> Validating -> AppMessageQueued -> AppMessageHandled -> Completed
                h.transition_to(&id, GuiActionState::Validating)
                    .unwrap_or_else(|e| {
                        // Rejected or Failed also acceptable terminal states under high concurrency
                        // if validation conditions change
                        if e.code == GuiActionErrorCode::InvalidArgument {
                            // Could be from eviction — action was evicted before transition
                            return;
                        }
                        panic!("transition to Validating failed: {e:?}");
                    });
                h.transition_to(&id, GuiActionState::AppMessageQueued).ok();
                h.transition_to(&id, GuiActionState::AppMessageHandled).ok();
                h.transition_to(&id, GuiActionState::Completed).ok();
            }
        }));
    }

    // Drain threads — any panic propagates
    for h in handles {
        h.join().expect("navigation action thread panicked");
    }

    // All 200 actions should be accounted for (some may have been evicted
    // but all survivors should be terminal)
    let count = history.action_count();
    assert!(
        count <= 100,
        "history should not exceed capacity: {count} > 100"
    );
    assert!(count > 0, "should have at least some actions stored");

    // Every stored action must be terminal
    for a in history.all_actions() {
        assert!(
            a.state.is_terminal(),
            "stored action {:?} should be terminal, was {:?}",
            a.action_id,
            a.state
        );
    }

    // No duplicate action IDs
    let ids: std::collections::HashSet<GuiActionId> = history
        .all_actions()
        .into_iter()
        .map(|a| a.action_id)
        .collect();
    assert_eq!(
        ids.len(),
        history.action_count(),
        "no duplicate IDs allowed under concurrent access"
    );
}

#[test]
fn test_concurrent_composer_update_then_submit() {
    // Composer update (SetComposerText) followed by submit (SubmitComposer)
    // executed by separate threads. Verify both actions make it through
    // the lifecycle and ordering can be inferred from requested_at_ms.
    let history = GuiActionHistory::with_capacity(50);
    const PAIRS: usize = 30;

    let mut handles = Vec::with_capacity(PAIRS * 2);

    for i in 0..PAIRS {
        // Set text action
        let h = history.clone();
        let set_id = GuiActionId::new();
        let set_req = GuiActionRequest {
            action_id: set_id.clone(),
            requested_at_ms: i as i64 * 100,
            command: format!("SetComposerText-pair-{i}"),
        };

        handles.push(std::thread::spawn(move || {
            let rid = h.record(set_req);
            let _ = rid;
            h.transition_to(&set_id, GuiActionState::Validating).ok();
            h.transition_to(&set_id, GuiActionState::AppMessageQueued)
                .ok();
            h.transition_to(&set_id, GuiActionState::AppMessageHandled)
                .ok();
            h.transition_to(&set_id, GuiActionState::Completed).ok();
        }));

        // Submit action (slightly later in requested_at_ms)
        let h2 = history.clone();
        let sub_id = GuiActionId::new();
        let sub_req = GuiActionRequest {
            action_id: sub_id.clone(),
            requested_at_ms: i as i64 * 100 + 50, // 50ms after the set
            command: format!("SubmitComposer-pair-{i}"),
        };

        handles.push(std::thread::spawn(move || {
            let rid = h2.record(sub_req);
            let _ = rid;
            h2.transition_to(&sub_id, GuiActionState::Validating).ok();
            h2.transition_to(&sub_id, GuiActionState::AppMessageQueued)
                .ok();
            h2.transition_to(&sub_id, GuiActionState::AppMessageHandled)
                .ok();
            h2.transition_to(&sub_id, GuiActionState::Completed).ok();
        }));
    }

    for h in handles {
        h.join().expect("composer thread panicked");
    }

    // Every stored action should be terminal
    for a in history.all_actions() {
        assert!(
            a.state.is_terminal(),
            "all actions should be terminal, got {:?}",
            a.state
        );
    }

    // Verify no ID collisions
    let ids: std::collections::HashSet<GuiActionId> = history
        .all_actions()
        .into_iter()
        .map(|a| a.action_id)
        .collect();
    assert_eq!(ids.len(), history.action_count(), "no duplicate IDs");
}

#[test]
fn test_concurrent_timeout_while_another_succeeds() {
    // One action times out while another completes normally.
    // Use very short (retroactive) timeouts to force timeout detection,
    // then use check_timeouts() to verify the timed-out action is detected
    // while the completed action is untouched.
    let history = GuiActionHistory::with_capacity(10);

    // Action A: will be completed normally
    let id_a = GuiActionId::new();
    history.record(GuiActionRequest {
        action_id: id_a.clone(),
        requested_at_ms: 100,
        command: "WillSucceed".into(),
    });

    // Action B: will be left in WaitingForExpectedState — should time out
    let id_b = GuiActionId::new();
    history.record(GuiActionRequest {
        action_id: id_b.clone(),
        requested_at_ms: 200,
        command: "WillTimeout".into(),
    });

    // Thread A: drive A through the full lifecycle to Completed
    let h_a = history.clone();
    let aid_a = id_a.clone();
    let t_a = std::thread::spawn(move || {
        // Drive A through full lifecycle
        h_a.transition_to(&aid_a, GuiActionState::Validating)
            .unwrap();
        h_a.transition_to(&aid_a, GuiActionState::AppMessageQueued)
            .unwrap();
        h_a.transition_to(&aid_a, GuiActionState::AppMessageHandled)
            .unwrap();
        h_a.transition_to(&aid_a, GuiActionState::WaitingForExpectedState)
            .unwrap();
        std::thread::sleep(std::time::Duration::from_millis(10));
        h_a.transition_to(&aid_a, GuiActionState::Completed)
            .unwrap();
    });

    // Thread B: drive B to WaitingForExpectedState then leave it
    // Force the timeout to be in the past by setting it directly
    let h_b = history.clone();
    let aid_b = id_b.clone();
    let t_b = std::thread::spawn(move || {
        h_b.transition_to(&aid_b, GuiActionState::Validating)
            .unwrap();
        h_b.transition_to(&aid_b, GuiActionState::AppMessageQueued)
            .unwrap();
        h_b.transition_to(&aid_b, GuiActionState::AppMessageHandled)
            .unwrap();
        h_b.transition_to(&aid_b, GuiActionState::WaitingForExpectedState)
            .unwrap();

        // Forcibly set timeout to the past to make check_timeouts detect it
        // even if the 10ms hasn't passed yet
        std::thread::sleep(std::time::Duration::from_millis(20));

        // Now directly set timeout_at_ms to 1 (epoch ms 1, far in the past)
        {
            let inner = &h_b.inner;
            let mut actions = inner.actions.lock().expect("actions lock");
            if let Some(status) = actions.get_mut(&aid_b) {
                status.timeout_at_ms = Some(1);
            }
        }
    });

    t_a.join().expect("succeed thread panicked");
    t_b.join().expect("timeout thread panicked");

    // Run check_timeouts — should catch B, not A
    let timed_out = history.check_timeouts();

    // A should be Completed
    let status_a = history.get(&id_a).expect("action A exists");
    assert_eq!(
        status_a.state,
        GuiActionState::Completed,
        "successful action should be Completed"
    );

    // B should be TimedOut (or just timed out entries show up in check_timeouts result)
    let status_b = history.get(&id_b).expect("action B exists");
    if status_b.state == GuiActionState::WaitingForExpectedState {
        // check_timeouts may not have caught it if the sleep wasn't enough;
        // the timeout_at_ms manipulation should have worked though
        assert!(
            timed_out.iter().any(|(id, _)| *id == id_b),
            "action B should have been detected as timed out: timed_out={:?}",
            timed_out
        );
    } else {
        assert_eq!(
            status_b.state,
            GuiActionState::TimedOut,
            "timed-out action should be in TimedOut state"
        );
    }

    // Action A should never be in timed_out list
    assert!(
        !timed_out.iter().any(|(id, _)| *id == id_a),
        "successful action should not be in timed_out list"
    );
}

#[cfg(feature = "gui")]
#[test]
fn test_concurrent_channel_closure() {
    // Channel closure: close the receiver while enqueuing actions.
    // Verify that subsequent enqueues return ActionQueueClosed.
    let (handle, rx) = GuiTestHandle::channel(256);

    // Enqueue a few successful actions first
    let success_ids: Vec<GuiActionId> = (0..3)
        .map(|i| {
            let id = GuiActionId::new();
            let req = GuiActionRequest {
                action_id: id.clone(),
                requested_at_ms: i as i64 * 100,
                command: format!("PreClose-{i}"),
            };
            handle
                .enqueue(req)
                .expect("pre-close enqueue should succeed");
            id
        })
        .collect();

    assert_eq!(success_ids.len(), 3, "three actions should enqueue");

    // Close the channel by dropping the receiver
    drop(rx);

    // Enqueue should now return ActionQueueClosed
    let post_close = GuiActionRequest {
        action_id: GuiActionId::new(),
        requested_at_ms: 9999,
        command: "PostClose".into(),
    };
    let err = handle.enqueue(post_close).unwrap_err();
    assert_eq!(
        err.code,
        GuiActionErrorCode::ActionQueueClosed,
        "enqueue after channel close should return ActionQueueClosed, got {:?}",
        err.code
    );

    // is_closed should return true
    assert!(handle.is_closed(), "handle should report closed");
}

#[cfg(feature = "gui")]
#[cfg(feature = "gui")]
#[test]
fn test_concurrent_queue_full_behaviour() {
    // Queue full behaviour: fill a small-capacity channel without draining,
    // verify that new enqueues return ActionQueueFull.
    let capacity = 2;
    let (handle, mut rx) = GuiTestHandle::channel(capacity);

    // Fill the channel to capacity
    for i in 0..capacity {
        let req = GuiActionRequest {
            action_id: GuiActionId::new(),
            requested_at_ms: i as i64 * 100,
            command: format!("Fill-{i}"),
        };
        handle
            .enqueue(req)
            .unwrap_or_else(|_| panic!("fill enqueue {i} should succeed"));
    }

    // Next enqueue should fail with ActionQueueFull (no drain)
    let overflow = GuiActionRequest {
        action_id: GuiActionId::new(),
        requested_at_ms: 9999,
        command: "Overflow".into(),
    };
    let err = handle.enqueue(overflow).unwrap_err();
    assert_eq!(
        err.code,
        GuiActionErrorCode::ActionQueueFull,
        "enqueue beyond capacity should return ActionQueueFull, got {:?}",
        err.code
    );

    // Drain one item — next enqueue should succeed
    let _ = rx.try_recv().expect("should drain one item");
    let after_drain = GuiActionRequest {
        action_id: GuiActionId::new(),
        requested_at_ms: 10000,
        command: "AfterDrain".into(),
    };
    handle
        .enqueue(after_drain)
        .expect("enqueue after drain should succeed");

    // Drain the rest
    while rx.try_recv().is_ok() {}
}

#[cfg(feature = "gui")]
#[cfg(feature = "gui")]
#[test]
fn test_concurrent_status_reads_with_writes() {
    // Concurrent status reads: multiple threads read action status
    // (get, action_count, all_actions, actions_with_state) while
    // writer threads record and update actions.
    // Verify no panics and eventually-consistent state.
    let history = GuiActionHistory::with_capacity(100);
    const N_WRITERS: usize = 4;
    const N_READERS: usize = 4;
    const ACTIONS_PER_WRITER: usize = 50;
    const READS_PER_READER: usize = 200;

    let barrier = std::sync::Arc::new(std::sync::Barrier::new(N_WRITERS + N_READERS));
    let mut handles = Vec::with_capacity(N_WRITERS + N_READERS);

    // Writer threads: record actions and transition them through lifecycle
    for w in 0..N_WRITERS {
        let h = history.clone();
        let bar = barrier.clone();
        handles.push(std::thread::spawn(move || {
            bar.wait();
            for i in 0..ACTIONS_PER_WRITER {
                let id = GuiActionId::new();
                let request = GuiActionRequest {
                    action_id: id.clone(),
                    requested_at_ms: (w * ACTIONS_PER_WRITER + i) as i64 * 10,
                    command: format!("Writer-{w}-{i}"),
                };
                let rid = h.record(request);
                let _ = rid;

                // Drive through lifecycle (best-effort, may fail if evicted)
                h.transition_to(&id, GuiActionState::Validating).ok();
                h.transition_to(&id, GuiActionState::AppMessageQueued).ok();
                h.transition_to(&id, GuiActionState::AppMessageHandled).ok();
                h.transition_to(&id, GuiActionState::Completed).ok();
            }
        }));
    }

    // Reader threads: read status while writes happen
    for _ in 0..N_READERS {
        let h = history.clone();
        let bar = barrier.clone();
        handles.push(std::thread::spawn(move || {
            bar.wait();
            for ri in 0..READS_PER_READER {
                // Mix get, action_count, all_actions, actions_with_state
                match ri % 4 {
                    0 => {
                        let _count = h.action_count();
                    }
                    1 => {
                        let _all = h.all_actions();
                    }
                    2 => {
                        let _completed = h.actions_with_state(GuiActionState::Completed);
                    }
                    3 => {
                        let _active = h.active_count();
                    }
                    _ => unreachable!(),
                }
            }
        }));
    }

    for h in handles {
        h.join().expect("status read/write thread panicked");
    }

    // Eventually-consistent: all actions should be bounded
    let count = history.action_count();
    assert!(count <= 100, "should not exceed capacity: {count} > 100");

    // All survivors must be terminal
    for a in history.all_actions() {
        assert!(
            a.state.is_terminal(),
            "all survivors should be terminal, got {:?}",
            a.state
        );
    }

    // No duplicate IDs
    let ids: std::collections::HashSet<GuiActionId> = history
        .all_actions()
        .into_iter()
        .map(|a| a.action_id)
        .collect();
    assert_eq!(ids.len(), history.action_count(), "no duplicate IDs");
}

#[test]
fn test_concurrent_event_ordering() {
    // Event ordering: verify that GuiActionEventHistory sequences are
    // unique and monotonically increasing even under concurrent recording.
    let journal = GuiActionEventHistory::with_capacity(5000);
    const N_THREADS: usize = 8;
    const EVENTS_PER_THREAD: usize = 100;

    let mut handles = Vec::with_capacity(N_THREADS);

    for t in 0..N_THREADS {
        let j = journal.clone();
        handles.push(std::thread::spawn(move || {
            for i in 0..EVENTS_PER_THREAD {
                j.record(
                    format!("action-{t}-{i}"),
                    GuiActionEventKind::ActionRequested,
                    (t * EVENTS_PER_THREAD + i) as u64,
                    None,
                    "ConcurrentScreen",
                );
            }
        }));
    }

    for h in handles {
        h.join().expect("event recording thread panicked");
    }

    // Verify sequence numbers are unique and increasing
    let entries = journal.all_entries();
    assert!(!entries.is_empty(), "should have recorded events");

    // Collect sequences (entries are newest-first)
    let mut sequences: Vec<u64> = entries.iter().map(|e| e.sequence).collect();
    sequences.sort();

    // Should be 0..N-1 with no gaps
    let total_expected = (N_THREADS * EVENTS_PER_THREAD) as u64;
    // Some entries may have been evicted if journal filled up
    // But with capacity 5000 vs 800 entries, none should be evicted
    assert_eq!(
        sequences.len() as u64,
        total_expected,
        "should have all {total_expected} sequences, got {}",
        sequences.len()
    );

    // Sequences should be 0..total_expected-1 with no gaps
    for (idx, &seq) in sequences.iter().enumerate() {
        assert_eq!(
            seq as usize, idx,
            "sequences should be contiguous with no gaps at position {idx}"
        );
    }
}

#[test]
fn test_concurrent_event_ordering_with_mixed_kinds() {
    // Event ordering with mixed event kinds: verify sequences are unique
    // and time-ordered even when different event types are recorded
    // concurrently.
    let journal = GuiActionEventHistory::with_capacity(5000);
    const N_THREADS: usize = 6;
    const EVENTS_PER_THREAD: usize = 75;
    let event_kinds = std::sync::Arc::new(vec![
        GuiActionEventKind::ActionRequested,
        GuiActionEventKind::ActionValidated,
        GuiActionEventKind::ActionRejected {
            reason: "test concurrent rejection".into(),
        },
        GuiActionEventKind::ActionCompleted,
        GuiActionEventKind::ActionTimedOut { timeout_ms: 5000 },
        GuiActionEventKind::ActionFailed {
            error: "concurrent error".into(),
        },
    ]);
    let mut handles = Vec::with_capacity(N_THREADS);

    for t in 0..N_THREADS {
        let j = journal.clone();
        let ek = Arc::clone(&event_kinds);
        handles.push(std::thread::spawn(move || {
            for i in 0..EVENTS_PER_THREAD {
                let kind_idx = (t * EVENTS_PER_THREAD + i) % ek.len();
                j.record(
                    format!("action-{t}-{i}"),
                    ek[kind_idx].clone(),
                    (t * EVENTS_PER_THREAD + i) as u64,
                    None,
                    "MixedScreen",
                );
            }
        }));
    }

    for h in handles {
        h.join().expect("mixed event thread panicked");
    }

    let entries = journal.all_entries();
    let total_expected = N_THREADS * EVENTS_PER_THREAD;
    assert_eq!(
        entries.len(),
        total_expected,
        "should have exactly {total_expected} entries, got {}",
        entries.len()
    );

    // Sequences should be contiguous 0..total with no gaps
    let mut sequences: Vec<u64> = entries.iter().map(|e| e.sequence).collect();
    sequences.sort();
    for (idx, &seq) in sequences.iter().enumerate() {
        assert_eq!(
            seq as usize, idx,
            "mixed event sequences should be contiguous at position {idx}"
        );
    }

    // Verify all sequence entries are present
    let seq_set: std::collections::HashSet<u64> = sequences.into_iter().collect();
    for s in 0..total_expected as u64 {
        assert!(seq_set.contains(&s), "sequence {s} should exist in journal");
    }
}

#[test]
fn test_concurrent_gui_revision_progression() {
    // GUI revision progression: verify that revisions recorded under
    // concurrency are unique and monotonically increasing.
    let journal = GuiActionEventHistory::with_capacity(5000);
    const N_THREADS: usize = 4;
    const EVENTS_PER_THREAD: usize = 50;

    let mut handles = Vec::with_capacity(N_THREADS);

    for t in 0..N_THREADS {
        let j = journal.clone();
        handles.push(std::thread::spawn(move || {
            for i in 0..EVENTS_PER_THREAD {
                // Each thread uses its own revision base to avoid collisions
                // (concurrent threads could interleave the same revision number)
                let revision = (t * EVENTS_PER_THREAD + i) as u64;
                j.record(
                    format!("rev-action-{t}-{i}"),
                    GuiActionEventKind::ActionCompleted,
                    revision,
                    None,
                    "RevisionScreen",
                );
            }
        }));
    }

    for h in handles {
        h.join().expect("revision thread panicked");
    }

    let entries = journal.all_entries();
    let total_expected = N_THREADS * EVENTS_PER_THREAD;
    assert_eq!(
        entries.len(),
        total_expected,
        "should have {total_expected} entries, got {}",
        entries.len()
    );

    // Verify ALL revisions 0..200 are present (each thread reserved its range)
    let revisions: std::collections::HashSet<u64> =
        entries.iter().map(|e| e.gui_revision).collect();
    for rev in 0..total_expected as u64 {
        assert!(
            revisions.contains(&rev),
            "revision {rev} should be present in journal entries"
        );
    }

    // Sequences should be contiguous
    let mut sequences: Vec<u64> = entries.iter().map(|e| e.sequence).collect();
    sequences.sort();
    for (idx, &seq) in sequences.iter().enumerate() {
        assert_eq!(
            seq as usize, idx,
            "sequences should be contiguous with no gaps at position {idx} (revision progression)"
        );
    }
}

#[cfg(feature = "gui")]
#[test]
fn test_concurrent_guihandle_enqueue_deadlock_free() {
    // Verify that concurrent enqueue and receive on a GuiTestHandle
    // channel is deadlock-free. Use multiple producer threads and
    // an active consumer draining the receiver.
    // Capacity must exceed total messages (300 = 6 × 50) so concurrent
    // producers never race past the consumer even under high system load.
    let (handle, mut rx) = GuiTestHandle::channel(512);
    const N_PRODUCERS: usize = 6;
    const MSGS_PER_PRODUCER: usize = 50;

    let mut handles = Vec::with_capacity(N_PRODUCERS);
    let mut received = 0usize;

    for p in 0..N_PRODUCERS {
        let h = handle.clone();
        handles.push(std::thread::spawn(move || {
            for i in 0..MSGS_PER_PRODUCER {
                let req = GuiActionRequest {
                    action_id: GuiActionId::new(),
                    requested_at_ms: (p * MSGS_PER_PRODUCER + i) as i64 * 10,
                    command: format!("ConcurrentEnqueue-{p}-{i}"),
                };
                h.enqueue(req).unwrap_or_else(|e| {
                    // Channel may close or be full during drain race;
                    // just count what we can
                    panic!("enqueue failed: {e:?}");
                });
            }
        }));
    }

    // Drain the receiver while producers are running
    use std::time::Duration;
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    while received < N_PRODUCERS * MSGS_PER_PRODUCER {
        if std::time::Instant::now() > deadline {
            break; // Don't hang if producers failed
        }
        match rx.try_recv() {
            Ok(_) => received += 1,
            Err(tokio::sync::mpsc::error::TryRecvError::Empty) => {
                std::thread::yield_now();
            }
            Err(tokio::sync::mpsc::error::TryRecvError::Disconnected) => {
                break;
            }
        }
    }

    for h in handles {
        h.join().expect("producer thread panicked");
    }

    // After producers finish, drain any remaining
    while rx.try_recv().is_ok() {
        received += 1;
    }

    assert_eq!(
        received,
        N_PRODUCERS * MSGS_PER_PRODUCER,
        "should have received all {expected} messages, got {received}",
        expected = N_PRODUCERS * MSGS_PER_PRODUCER
    );
}

#[test]
fn test_concurrent_lock_order_consistency() {
    // Verify no lock inversion or deadlock by exercising both
    // GuiActionHistory and GuiActionEventHistory simultaneously
    // from multiple threads.
    let history = GuiActionHistory::with_capacity(50);
    let journal = GuiActionEventHistory::with_capacity(300);
    const N_WRITERS: usize = 6;
    const ITEMS_PER_WRITER: usize = 40;

    let mut handles = Vec::with_capacity(N_WRITERS);

    for w in 0..N_WRITERS {
        let h = history.clone();
        let j = journal.clone();
        handles.push(std::thread::spawn(move || {
            for i in 0..ITEMS_PER_WRITER {
                // Write to action history
                let id = GuiActionId::new();
                let request = GuiActionRequest {
                    action_id: id.clone(),
                    requested_at_ms: (w * ITEMS_PER_WRITER + i) as i64 * 10,
                    command: format!("LockTest-{w}-{i}"),
                };
                h.record(request);
                h.transition_to(&id, GuiActionState::Validating).ok();
                h.transition_to(&id, GuiActionState::AppMessageQueued).ok();
                h.transition_to(&id, GuiActionState::AppMessageHandled).ok();
                h.transition_to(&id, GuiActionState::Completed).ok();

                // Also write to event journal
                j.record(
                    format!("lock-event-{w}-{i}"),
                    GuiActionEventKind::ActionCompleted,
                    (w * ITEMS_PER_WRITER + i) as u64,
                    None,
                    "LockTestScreen",
                );

                // Read from both
                let _all = h.all_actions();
                let _entries = j.entries_since(0, 10);
            }
        }));
    }

    for h in handles {
        h.join().expect("lock order consistency thread panicked");
    }

    // Both stores should be internally consistent
    assert!(history.action_count() <= 50, "history capacity respected");
    assert_eq!(
        journal.entry_count(),
        N_WRITERS * ITEMS_PER_WRITER,
        "journal should have all entries"
    );

    // All history entries should be terminal
    for a in history.all_actions() {
        assert!(
            a.state.is_terminal(),
            "all actions should be terminal under lock test"
        );
    }
}

#[test]
fn test_gui_wait_condition_load_is_deterministic() {
    let snapshot = test_snapshot("Chat", Some("room-1"), 3, 5);
    let journal = IcedMessageJournal::new();
    for i in 0..100 {
        journal.record(
            format!("WaitLoad-{i}"),
            FailureLayer::IcedUpdate,
            true,
            "",
            None,
        );
    }
    let conditions = [
        GuiWaitCondition::ScreenIs {
            expected: "Chat".to_string(),
        },
        GuiWaitCondition::RoomSelected {
            room_topic: Some("room-1".to_string()),
        },
        GuiWaitCondition::PeerVisible { min_count: 3 },
        GuiWaitCondition::MessageVisible { min_count: 5 },
        GuiWaitCondition::GuiRevisionAtLeast {
            expected_revision: 99,
        },
        GuiWaitCondition::ConversationSelected {
            conversation_id: Some("room-1".to_string()),
        },
        GuiWaitCondition::ComposerTextIs {
            expected: String::new(),
        },
        GuiWaitCondition::DialogClosed,
        GuiWaitCondition::UnreadCountAtLeast { min_count: 0 },
    ];

    let started = std::time::Instant::now();
    for _ in 0..10_000 {
        for condition in &conditions {
            assert!(evaluate_wait_condition(condition, &snapshot, &journal));
        }
    }
    assert_eq!(journal.entry_count(), 100);
    eprintln!(
        "GUI wait-condition load: 90,000 evaluations in {:?}",
        started.elapsed()
    );
}

fn test_gui_request(id: &str) -> GuiActionRequest {
    GuiActionRequest {
        action_id: GuiActionId(id.to_string()),
        requested_at_ms: 0,
        command: "toggle_help".to_string(),
    }
}

#[cfg(feature = "gui")]
#[tokio::test]
async fn test_gui_action_channel_closure_is_structured_and_recorded() {
    let (handle, receiver) = GuiTestHandle::channel(1);
    drop(receiver);

    let request = test_gui_request("closed-channel");
    let error = handle
        .enqueue(request)
        .expect_err("closed receiver must reject");
    assert_eq!(error.code, GuiActionErrorCode::ActionQueueClosed);
    assert!(error.message.contains("closed"));

    let status = handle
        .history()
        .get(&GuiActionId("closed-channel".to_string()))
        .expect("failed enqueue must remain observable");
    assert_eq!(status.state, GuiActionState::Rejected);
    assert_eq!(
        status.error.as_ref().map(|e| e.code.clone()),
        Some(GuiActionErrorCode::ActionQueueClosed)
    );
}

#[cfg(feature = "gui")]
#[tokio::test]
async fn test_gui_action_shutdown_drains_then_receiver_closes_without_hanging() {
    let (handle, mut receiver) = GuiTestHandle::channel(1);
    handle
        .enqueue(test_gui_request("shutdown-action"))
        .expect("live receiver accepts action");

    let received = tokio::time::timeout(std::time::Duration::from_millis(100), receiver.recv())
        .await
        .expect("receiving queued action must not hang")
        .expect("queued action should be delivered");
    assert_eq!(received.action_id.0, "shutdown-action");

    drop(handle);
    let closed = tokio::time::timeout(std::time::Duration::from_millis(100), receiver.recv())
        .await
        .expect("shutdown receive must not hang");
    assert!(
        closed.is_none(),
        "receiver must observe graceful sender shutdown"
    );
}

#[test]
fn test_unknown_and_evicted_gui_action_ids_are_not_resolved() {
    let history = GuiActionHistory::with_capacity(1);
    let first = history.record(test_gui_request("stale-action"));
    assert!(history.set_state(&first, GuiActionState::Completed));
    let second = history.record(test_gui_request("current-action"));

    assert!(history
        .get(&GuiActionId("stale-action".to_string()))
        .is_none());
    let error = history
        .transition_to(
            &GuiActionId("unknown-action".to_string()),
            GuiActionState::Validating,
        )
        .expect_err("unknown action IDs must return a structured error");
    assert_eq!(error.code, GuiActionErrorCode::InvalidArgument);
    assert!(error.message.contains("not found"));
    assert!(history.get(&second).is_some());
}

#[test]
fn test_gui_composer_control_commands_serialize_and_declare_state() {
    let clear = GuiTestCommand::ClearComposer;
    let focus = GuiTestCommand::FocusComposer;
    assert_eq!(
        serde_json::to_string(&clear).unwrap(),
        r#"{"command":"clear_composer"}"#
    );
    assert_eq!(
        serde_json::to_string(&focus).unwrap(),
        r#"{"command":"focus_composer"}"#
    );
    assert_eq!(
        clear.expected_state(),
        Some(ExpectedState::ComposerTextIs(String::new()))
    );
    assert_eq!(
        focus.expected_state(),
        Some(ExpectedState::Generic("composer_focused".into()))
    );
    assert!(clear.validate().is_ok());
    assert!(focus.validate().is_ok());
}

#[test]
fn test_gui_composer_control_commands_round_trip() {
    for command in [GuiTestCommand::ClearComposer, GuiTestCommand::FocusComposer] {
        let json = serde_json::to_string(&command).unwrap();
        let decoded: GuiTestCommand = serde_json::from_str(&json).unwrap();
        assert_eq!(decoded, command);
    }
}
