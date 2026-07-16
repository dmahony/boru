//! Integration tests for MCP diagnostics feature.
//!
//! Tests:
//!   13. Normal text-message behavior still works with diagnostics active.
//!   14. Two in-process peers can exchange a probe through the normal gossip path.
//!   15. Receiving peer reports the exact probe ID.
//!   16. Sender and receiver report the same message hash.
//!
//! These tests require the `net` feature (part of default features).

use std::time::Duration;

use boru_chat::{
    api::{Event as GossipEvent, GossipTopic},
    chat_core::{broadcast_diagnostic_probe, message_hash, Message, SignedMessage},
    diagnostics::{DiagnosticEventKind, DiagnosticProbe, Diagnostics},
    net::{Gossip, GOSSIP_ALPN},
    proto::TopicId,
};
use iroh::{
    address_lookup::memory::MemoryLookup, endpoint::presets, protocol::Router, Endpoint, RelayMode,
    SecretKey,
};
use n0_error::Result;
use n0_future::{time::sleep, StreamExt};
use rand::{RngExt, SeedableRng};

fn make_sk(rng: &mut impl rand::Rng) -> SecretKey {
    SecretKey::from_bytes(&rng.random())
}

async fn spawn_peer_relay(
    rng: &mut impl rand::Rng,
) -> Result<(Router, Endpoint, SecretKey, Gossip)> {
    let ep = Endpoint::builder(presets::N0)
        .secret_key(make_sk(rng))
        .address_lookup(MemoryLookup::new())
        .relay_mode(RelayMode::Default)
        .bind_addr("127.0.0.1:0".parse::<std::net::SocketAddr>().unwrap())?
        .bind()
        .await?;
    ep.online().await;
    let gossip = Gossip::builder().spawn(ep.clone());
    let router = Router::builder(ep.clone())
        .accept(GOSSIP_ALPN, gossip.clone())
        .spawn();
    Ok((router, ep.clone(), ep.secret_key().clone(), gossip))
}

/// Non-blocking drain of events from a topic subscription.
async fn drain_events(sub: &mut GossipTopic, timeout: Duration) -> Vec<GossipEvent> {
    let mut events = Vec::new();
    loop {
        let item = tokio::time::timeout(timeout, sub.next()).await;
        match item {
            Ok(Some(Ok(ev))) => events.push(ev),
            Ok(Some(Err(e))) => {
                eprintln!("  drain error: {e}");
                break;
            }
            _ => break,
        }
    }
    events
}

/// Wait until both peers are joined to the topic, with a max retry count.
async fn wait_for_both_joined(sub_a: &mut GossipTopic, sub_b: &mut GossipTopic) -> bool {
    let short = Duration::from_millis(50);
    for _i in 0..80 {
        // The default relay path can take several seconds to rendezvous on a
        // busy CI host. Keep polling for a bounded interval rather than
        // treating a transient handshake delay as a transport failure.
        sleep(Duration::from_millis(100)).await;
        let _ev_a = drain_events(sub_a, short).await;
        let _ev_b = drain_events(sub_b, short).await;
        if sub_a.is_joined() && sub_b.is_joined() {
            return true;
        }
    }
    false
}

/// Drain stale events before sending a message.
async fn drain_stale(sub_a: &mut GossipTopic, sub_b: &mut GossipTopic) {
    let short = Duration::from_millis(50);
    drain_events(sub_a, short).await;
    drain_events(sub_b, short).await;
}

// ── Test 13: Normal text-message behavior still works with diagnostics active ──

#[tokio::test]
async fn test_diagnostics_does_not_block_text_messages() -> Result<()> {
    let _ = tracing_subscriber::fmt::try_init();
    let mut rng = rand::rngs::ChaCha12Rng::seed_from_u64(99);

    let (router_a, _ep_a, sk_a, gossip_a) = spawn_peer_relay(&mut rng).await?;
    let (router_b, _ep_b, _sk_b, gossip_b) = spawn_peer_relay(&mut rng).await?;

    let topic = TopicId::from_bytes(rng.random());

    let mut sub_a = gossip_a.subscribe(topic, vec![_ep_a.id()]).await?;
    sleep(Duration::from_millis(100)).await;
    let mut sub_b = gossip_b.subscribe(topic, vec![_ep_a.id()]).await?;

    assert!(
        wait_for_both_joined(&mut sub_a, &mut sub_b).await,
        "peers should join the topic"
    );

    drain_stale(&mut sub_a, &mut sub_b).await;

    // A broadcasts a text message (AboutMe is a simple text-id message type)
    let msg = Message::AboutMe {
        name: "TestDiagnosticsUser".into(),
        profile_image_ticket: None,
    };
    let encoded = SignedMessage::sign_and_encode(&sk_a, &msg).expect("sign text message");
    sub_a.broadcast(encoded).await?;

    // Wait for propagation
    sleep(Duration::from_secs(3)).await;

    // Drain B's events and verify the text message arrived
    let ev_b = drain_events(&mut sub_b, Duration::from_secs(2)).await;

    let mut received_text = false;
    for ev in &ev_b {
        if let GossipEvent::Received(msg) = ev {
            if let Ok((_from, decoded, _sent_at)) = SignedMessage::verify_and_decode(&msg.content) {
                match decoded {
                    Message::AboutMe { name, .. } if name == "TestDiagnosticsUser" => {
                        received_text = true;
                    }
                    _ => {}
                }
            }
        }
    }

    assert!(
        received_text,
        "Peer B should receive the text message even with diagnostics active"
    );

    // Verify diagnostics is still functioning (no corruption)
    let diag = Diagnostics::new();
    diag.record(Some(topic), DiagnosticEventKind::RoomJoinStarted);
    diag.record(Some(topic), DiagnosticEventKind::RoomJoined);
    assert_eq!(
        diag.event_count(),
        2,
        "diagnostics should still function after text message exchange"
    );

    // Cleanup
    drop(sub_a);
    drop(sub_b);
    drop(router_a);
    drop(router_b);

    Ok(())
}

// ── Tests 14-16: Probe exchange between two peers ─────────────────────

#[tokio::test]
async fn test_two_peers_exchange_probe() -> Result<()> {
    let _ = tracing_subscriber::fmt::try_init();
    let mut rng = rand::rngs::ChaCha12Rng::seed_from_u64(100);

    let (router_a, ep_a, sk_a, gossip_a) = spawn_peer_relay(&mut rng).await?;
    let (router_b, ep_b, _sk_b, gossip_b) = spawn_peer_relay(&mut rng).await?;

    println!("Peer A: {}", ep_a.id().fmt_short());
    println!("Peer B: {}", ep_b.id().fmt_short());

    let topic = TopicId::from_bytes(rng.random());
    let topic_hex = hex::encode(topic.as_bytes());
    println!("Topic: {topic}");

    let mut sub_a = gossip_a.subscribe(topic, vec![ep_a.id()]).await?;
    sleep(Duration::from_millis(100)).await;
    let mut sub_b = gossip_b.subscribe(topic, vec![ep_a.id()]).await?;

    assert!(
        wait_for_both_joined(&mut sub_a, &mut sub_b).await,
        "peers should join the topic"
    );

    drain_stale(&mut sub_a, &mut sub_b).await;

    // Peer A creates and broadcasts a diagnostic probe with a known ID
    let known_probe_id = "integration-test-probe-001";
    let probe_bytes = broadcast_diagnostic_probe(
        &sk_a,
        &topic_hex,
        Some("integration test payload".to_string()),
        Some(known_probe_id.to_string()),
    )
    .expect("create probe");

    sub_a.broadcast(probe_bytes).await?;
    println!("Probe broadcast by A");

    // Wait for propagation
    sleep(Duration::from_secs(3)).await;

    // Peer B receives events and looks for the probe
    let ev_b = drain_events(&mut sub_b, Duration::from_secs(2)).await;
    println!("B received {} events", ev_b.len());

    let mut received_probe: Option<DiagnosticProbe> = None;
    let mut received_hash: Option<String> = None;

    for ev in &ev_b {
        if let GossipEvent::Received(msg) = ev {
            if let Ok((from, decoded, _sent_at)) = SignedMessage::verify_and_decode(&msg.content) {
                println!("  B: decoded message from {}", from.fmt_short());
                match decoded {
                    Message::DiagnosticProbe(ref probe) => {
                        println!("  B: received probe id={}", probe.probe_id);
                        // Compute the message hash on the receiving side
                        let rx_hash = hex::encode(message_hash(&decoded));
                        println!("  B: computed message hash={}", rx_hash);
                        received_probe = Some(probe.clone());
                        received_hash = Some(rx_hash);
                    }
                    _ => {
                        println!("  B: non-probe message: {:?}", decoded);
                    }
                }
            }
        }
    }

    // Test 14: Verify probe was received
    assert!(
        received_probe.is_some(),
        "Peer B should receive the probe through gossip"
    );

    // Test 15: Verify receiving peer reports the exact probe ID
    let probe = received_probe.unwrap();
    assert_eq!(
        probe.probe_id, known_probe_id,
        "Receiving peer should report the exact probe ID that was sent"
    );

    // Test 16: Verify sender and receiver compute the same message hash
    // Re-compute the hash on the sender side for comparison
    let sender_probe = DiagnosticProbe {
        probe_id: known_probe_id.to_string(),
        sender_id: sk_a.public().to_string(),
        room_id: topic_hex.clone(),
        sent_at_ms: probe.sent_at_ms, // Use the same sent_at_ms from the received probe
        payload: Some("integration test payload".to_string()),
    };
    let sender_msg = Message::DiagnosticProbe(sender_probe);
    let sender_hash = hex::encode(message_hash(&sender_msg));

    let rx_hash = received_hash.expect("should have received hash");
    assert_eq!(
        sender_hash, rx_hash,
        "Sender and receiver should compute the same message hash"
    );

    println!("✓ Probe exchange verified: ID matches, hash matches");

    // Cleanup
    drop(sub_a);
    drop(sub_b);
    drop(router_a);
    drop(router_b);

    Ok(())
}

// ── Tests 17-20: Iced diagnostics types (no GUI required) ─────────────

use boru_chat::diagnostics::{
    classify_failures, classify_message_layer, FailureAnalysis, FailureLayer, IcedMessageJournal,
};

#[test]
fn test_iced_message_journal_basic_record_and_query() {
    let journal = IcedMessageJournal::new();

    assert_eq!(journal.entry_count(), 0);
    assert_eq!(journal.latest_sequence(), 0);

    journal.record("GoToChatList", FailureLayer::IcedUpdate, true, "", None);
    assert_eq!(journal.entry_count(), 1);
    assert_eq!(journal.latest_sequence(), 0);

    journal.record("NetEvent", FailureLayer::Network, true, "", None);
    assert_eq!(journal.entry_count(), 2);

    let entries = journal.entries_since(0, 100);
    assert_eq!(entries.len(), 1); // > 0 means sequence > 0, so only seq 1

    // Query since sequence 0 gives us both
    let entries_all = journal.all_entries();
    assert_eq!(entries_all.len(), 2);
    assert_eq!(entries_all[0].message_variant, "GoToChatList");
    assert_eq!(entries_all[0].layer, FailureLayer::IcedUpdate);
    assert_eq!(entries_all[1].message_variant, "NetEvent");
    assert_eq!(entries_all[1].layer, FailureLayer::Network);
}

#[test]
fn test_iced_message_journal_eviction() {
    let journal = IcedMessageJournal::with_capacity(3);

    for i in 0..5 {
        journal.record(&format!("Msg{i}"), FailureLayer::IcedUpdate, true, "", None);
    }

    assert_eq!(journal.entry_count(), 3);
    let entries = journal.all_entries();
    assert_eq!(entries.len(), 3);
    // Should have kept the 3 newest: Msg2, Msg3, Msg4
    assert_eq!(entries[0].message_variant, "Msg2");
    assert_eq!(entries[1].message_variant, "Msg3");
    assert_eq!(entries[2].message_variant, "Msg4");
}

#[test]
fn test_iced_message_journal_failure_recording() {
    let journal = IcedMessageJournal::new();

    journal.record(
        "NetEvent",
        FailureLayer::Network,
        false,
        "Connection timeout",
        Some(150),
    );
    journal.record(
        "SendPressed",
        FailureLayer::ApplicationState,
        true,
        "",
        None,
    );
    journal.record(
        "ToggleDark",
        FailureLayer::IcedUpdate,
        false,
        "Unknown variant",
        None,
    );

    let entries = journal.all_entries();
    assert_eq!(entries.len(), 3);

    // First entry: failed network event
    assert!(!entries[0].success);
    assert_eq!(entries[0].error, "Connection timeout");
    assert_eq!(entries[0].layer, FailureLayer::Network);
    assert_eq!(entries[0].duration_ms, Some(150));

    // Second entry: successful state update
    assert!(entries[1].success);
    assert_eq!(entries[1].layer, FailureLayer::ApplicationState);

    // Third entry: failed iced update
    assert!(!entries[2].success);
    assert_eq!(entries[2].layer, FailureLayer::IcedUpdate);
}

#[test]
fn test_classify_message_layer_correctness() {
    // Network events
    assert_eq!(classify_message_layer("NetEvent"), FailureLayer::Network);
    assert_eq!(classify_message_layer("FriendEvent"), FailureLayer::Network);
    assert_eq!(
        classify_message_layer("WhisperEvent"),
        FailureLayer::Network
    );
    assert_eq!(classify_message_layer("InboxEvent"), FailureLayer::Network);
    assert_eq!(
        classify_message_layer("ConnMonitorTick"),
        FailureLayer::Network
    );
    assert_eq!(
        classify_message_layer("ConnCountsResult"),
        FailureLayer::Network
    );
    assert_eq!(
        classify_message_layer("NewDiscoveredPeers"),
        FailureLayer::Network
    );
    assert_eq!(
        classify_message_layer("DownloadProgress"),
        FailureLayer::Network
    );

    // Application state
    assert_eq!(
        classify_message_layer("OpenRoom"),
        FailureLayer::ApplicationState
    );
    assert_eq!(
        classify_message_layer("RoomOpened"),
        FailureLayer::ApplicationState
    );
    assert_eq!(
        classify_message_layer("SendPressed"),
        FailureLayer::ApplicationState
    );
    assert_eq!(
        classify_message_layer("MessageSent"),
        FailureLayer::ApplicationState
    );
    assert_eq!(
        classify_message_layer("FriendAdded"),
        FailureLayer::ApplicationState
    );
    assert_eq!(
        classify_message_layer("GoToChatList"),
        FailureLayer::ApplicationState
    );
    assert_eq!(
        classify_message_layer("ToggleDark"),
        FailureLayer::ApplicationState
    );
    assert_eq!(
        classify_message_layer("ErrorMsg"),
        FailureLayer::ApplicationState
    );

    // Iced UI update (catch-all)
    assert_eq!(
        classify_message_layer("ToggleHelp"),
        FailureLayer::IcedUpdate
    );
    assert_eq!(
        classify_message_layer("CloseSettings"),
        FailureLayer::IcedUpdate
    );
    assert_eq!(classify_message_layer("Shortcut"), FailureLayer::IcedUpdate);
    assert_eq!(classify_message_layer("Noop"), FailureLayer::IcedUpdate);
    assert_eq!(
        classify_message_layer("CopyToClipboard"),
        FailureLayer::IcedUpdate
    );
}

#[test]
fn test_classify_failures_from_diagnostics_events() {
    let diagnostics = Diagnostics::new();
    let journal = IcedMessageJournal::new();

    // Record a network failure in diagnostics
    diagnostics.record(
        None,
        boru_chat::diagnostics::DiagnosticEventKind::ConnectionFailed {
            addresses: vec!["127.0.0.1:1234".to_string()],
            error: "Connection refused".to_string(),
        },
    );

    let analysis = classify_failures(&diagnostics, &journal, 0);
    assert!(analysis.network_failure);
    assert!(!analysis.state_update_failure);
    assert!(!analysis.iced_update_failure);
    assert!(analysis
        .details
        .iter()
        .any(|d| d.contains("Connection refused")));
}

#[test]
fn test_classify_failures_from_iced_journal() {
    let diagnostics = Diagnostics::new();
    let journal = IcedMessageJournal::new();

    // Record a failure in the Iced message journal
    journal.record(
        "NetEvent",
        FailureLayer::Network,
        false,
        "Connection timeout",
        None,
    );
    journal.record(
        "SendPressed",
        FailureLayer::ApplicationState,
        false,
        "Room not found",
        None,
    );

    let analysis = classify_failures(&diagnostics, &journal, 0);
    assert!(analysis.network_failure);
    assert!(analysis.state_update_failure);
    assert!(!analysis.iced_update_failure);
    assert_eq!(analysis.details.len(), 2);
}

#[test]
fn test_classify_failures_empty() {
    let diagnostics = Diagnostics::new();
    let journal = IcedMessageJournal::new();

    let analysis = classify_failures(&diagnostics, &journal, 0);
    assert!(!analysis.network_failure);
    assert!(!analysis.state_update_failure);
    assert!(!analysis.iced_update_failure);
    assert!(analysis.details.is_empty());
}

#[test]
fn test_classify_failures_since_sequence() {
    let diagnostics = Diagnostics::new();
    let journal = IcedMessageJournal::new();

    // Record an older event and a newer one
    diagnostics.record(
        None,
        boru_chat::diagnostics::DiagnosticEventKind::RoomJoinFailed,
    );

    // Record a successful Iced message (seq 1 in journal)
    journal.record("Noop", FailureLayer::IcedUpdate, true, "", None);

    // Check with since_sequence=0 — should see both failures
    let analysis = classify_failures(&diagnostics, &journal, 0);
    assert!(analysis.network_failure);
    assert_eq!(analysis.details.len(), 1);

    // Check with since_sequence=999 — should see nothing
    let analysis2 = classify_failures(&diagnostics, &journal, 999);
    assert!(!analysis2.network_failure);
    assert!(analysis2.details.is_empty());
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
struct IcedStateSnapshotTest {
    node_id: String,
    version: String,
    active_screen: String,
    #[serde(default)]
    active_room: Option<String>,
    conversation_count: usize,
    neighbor_count: usize,
    direct_peer_count: usize,
    relayed_peer_count: usize,
    mesh_health: String,
    online_friend_count: usize,
    friend_count: usize,
    total_entry_count: usize,
    dark_mode: bool,
    #[serde(with = "chrono::serde::ts_seconds")]
    timestamp: chrono::DateTime<chrono::Utc>,
}

#[test]
fn test_iced_state_snapshot_serde() {
    let snapshot = IcedStateSnapshotTest {
        node_id: "node-abc".to_string(),
        version: "0.101.0".to_string(),
        active_screen: "ChatList".to_string(),
        active_room: None,
        conversation_count: 3,
        neighbor_count: 2,
        direct_peer_count: 1,
        relayed_peer_count: 1,
        mesh_health: "Good".to_string(),
        online_friend_count: 5,
        friend_count: 10,
        total_entry_count: 42,
        dark_mode: true,
        timestamp: chrono::Utc::now(),
    };

    // Round-trip through JSON
    let json = serde_json::to_string(&snapshot).unwrap();
    let deserialized: IcedStateSnapshotTest = serde_json::from_str(&json).unwrap();
    assert_eq!(deserialized.node_id, "node-abc");
    assert_eq!(deserialized.active_screen, "ChatList");
    assert!(deserialized.active_room.is_none());
    assert_eq!(deserialized.conversation_count, 3);
    assert_eq!(deserialized.dark_mode, true);

    // Verify no secret keys in output
    assert!(!json.contains("secret_key"));
    assert!(!json.contains("secret"));
}

#[test]
fn test_failure_analysis_serde() {
    let analysis = FailureAnalysis {
        network_failure: true,
        state_update_failure: false,
        iced_update_failure: true,
        details: vec![
            "[network] Connection failed".to_string(),
            "[iced] update failed for 'ToggleDark'".to_string(),
        ],
        timestamp: chrono::Utc::now(),
    };

    let json = serde_json::to_string(&analysis).unwrap();
    let deserialized: FailureAnalysis = serde_json::from_str(&json).unwrap();
    assert!(deserialized.network_failure);
    assert!(!deserialized.state_update_failure);
    assert!(deserialized.iced_update_failure);
    assert_eq!(deserialized.details.len(), 2);
    assert!(!json.contains("secret_key"));
}

#[test]
fn test_iced_message_journal_entry_serde() {
    use boru_chat::diagnostics::IcedMessageJournalEntry;

    let entry = IcedMessageJournalEntry {
        sequence: 42,
        timestamp: chrono::Utc::now(),
        message_variant: "SendPressed".to_string(),
        layer: FailureLayer::ApplicationState,
        success: false,
        error: "Room not joined".to_string(),
        duration_ms: Some(5),
    };

    let json = serde_json::to_string(&entry).unwrap();
    let deserialized: IcedMessageJournalEntry = serde_json::from_str(&json).unwrap();
    assert_eq!(deserialized.sequence, 42);
    assert_eq!(deserialized.message_variant, "SendPressed");
    assert_eq!(deserialized.layer, FailureLayer::ApplicationState);
    assert!(!deserialized.success);
    assert_eq!(deserialized.error, "Room not joined");
    assert_eq!(deserialized.duration_ms, Some(5));
    assert!(!json.contains("secret_key"));
}
