//! End-to-end test for room creation, peer join, and metadata/roster sync.
//!
//! Spawns two local iroh peers connected via a relay server.  Peer A opens a
//! room (creating metadata and roster docs), peer B joins the same topic,
//! and the test verifies that both peers converge on the same metadata
//! and that the roster includes both peers.
//!
//! Assertions use the same pattern as the `/room info` CLI command:
//! read_metadata() for room metadata fields and list_members() for roster.

use std::time::Duration;

use boru_core::{
    net::{Gossip, GOSSIP_ALPN},
    proto::TopicId,
    room_docs::{
        self, create_metadata_doc, create_roster_doc, list_members, read_metadata, RoomMetadata,
    },
};
use iroh::{
    address_lookup::memory::MemoryLookup, endpoint::presets, protocol::Router, tls::CaTlsConfig,
    Endpoint, RelayMode, SecretKey,
};
use n0_error::Result;
use n0_future::{time::sleep, StreamExt};
use rand::{RngExt, SeedableRng};

async fn create_test_endpoint(
    rng: &mut rand::rngs::ChaCha12Rng,
    relay_map: iroh::RelayMap,
    memory: Option<MemoryLookup>,
) -> Result<Endpoint> {
    let ep = Endpoint::builder(presets::Minimal)
        .relay_mode(RelayMode::Custom(relay_map))
        .secret_key(SecretKey::from_bytes(&rng.random()))
        .alpns(vec![GOSSIP_ALPN.to_vec()])
        .ca_tls_config(CaTlsConfig::insecure_skip_verify())
        .bind()
        .await?;
    if let Some(m) = memory {
        ep.address_lookup().unwrap().add(m);
    }
    ep.online().await;
    Ok(ep)
}

/// Drain buffered events from a gossip receiver into the doc handles,
/// stopping after `idle` of inactivity.
async fn drain(
    md: &room_docs::RoomMetadataDoc,
    roster: &room_docs::RosterDoc,
    rx: &mut boru_core::api::GossipReceiver,
    idle: Duration,
    max: usize,
) {
    for _ in 0..max {
        match tokio::time::timeout(idle, rx.next()).await {
            Ok(Some(Ok(ev))) => {
                let _ = room_docs::process_gossip_event(md, Ok(ev.clone())).await;
                let _ = room_docs::process_roster_event(roster, Ok(ev)).await;
            }
            Ok(Some(Err(_))) => continue,
            _ => return,
        }
    }
}

/// Format room info output in the same style as the `/room info` CLI command.
fn format_room_info(
    md: &RoomMetadata,
    members: &std::collections::HashMap<String, boru_core::room_docs::RosterMember>,
) -> String {
    let mut out = format!(
        "Room: {} | Description: {} | Rules: {}",
        md.name.as_deref().unwrap_or("unnamed"),
        md.description.as_deref().unwrap_or("none"),
        md.rules.as_deref().unwrap_or("none"),
    );
    out.push_str(&format!("\nMembers ({}):", members.len()));
    for (pk, member) in members {
        out.push_str(&format!(
            "\n  {} ({}) — joined at {}",
            member.display_name,
            &pk[..16.min(pk.len())],
            member.joined_at,
        ));
    }
    out
}

#[tokio::test]
#[n0_tracing_test::traced_test]
async fn room_create_and_join_metadata_roster_sync() -> Result<()> {
    let mut rng = rand::rngs::ChaCha12Rng::seed_from_u64(42);
    let (relay_map, relay_url, _guard) = iroh::test_utils::run_relay_server().await.unwrap();

    let memory = MemoryLookup::new();

    // ── Create two peers ────────────────────────────────────────────────
    let ep_a = create_test_endpoint(&mut rng, relay_map.clone(), Some(memory.clone()))
        .await
        .unwrap();
    let ep_b = create_test_endpoint(&mut rng, relay_map.clone(), Some(memory.clone()))
        .await
        .unwrap();

    let gossip_a = Gossip::builder().spawn(ep_a.clone());
    let gossip_b = Gossip::builder().spawn(ep_b.clone());

    let _router_a = Router::builder(ep_a.clone())
        .accept(GOSSIP_ALPN, gossip_a.clone())
        .spawn();
    let _router_b = Router::builder(ep_b.clone())
        .accept(GOSSIP_ALPN, gossip_b.clone())
        .spawn();

    // Register both peers in the shared address lookup so gossip can
    // discover them via the relay.
    memory.add_endpoint_info(ep_a.addr().with_relay_url(relay_url.clone()));
    memory.add_endpoint_info(ep_b.addr().with_relay_url(relay_url));

    let topic = TopicId::from_bytes(blake3::hash(b"room-e2e-test-topic").into());

    // ── Subscribe both peers concurrently ───────────────────────────────
    // subscribe_and_join waits for at least one active connection.
    // When bootstrapping from an empty list, the subscribe will never
    // complete
    // unless another peer connects simultaneously.  Using try_join!
    // ensures
    // both subscriptions happen concurrently so they can discover each
    // other.
    let (topic_a, topic_b) = tokio::try_join!(
        gossip_a.subscribe_and_join(topic, vec![]),
        gossip_b.subscribe_and_join(topic, vec![ep_a.id()]),
    )?;
    let (sender_a, rx_a) = topic_a.split();
    let (sender_b, mut rx_b) = topic_b.split();

    eprintln!("subscribed: A={}, B={}", ep_a.id(), ep_b.id());

    // Give gossip a moment to settle the initial connections.
    sleep(Duration::from_millis(500)).await;
    eprintln!("settled");

    // ── Create room docs ────────────────────────────────────────────────

    // A creates metadata + roster
    let _md_a = create_metadata_doc(
        topic,
        &sender_a,
        RoomMetadata {
            name: Some("E2E Test Room".into()),
            description: Some("End-to-end test".into()),
            rules: Some("Be excellent".into()),
        },
    )
    .await
    .unwrap();

    let roster_a = create_roster_doc(
        topic,
        &sender_a,
        ep_a.secret_key().public().to_string(),
        "PeerA".into(),
    )
    .await
    .unwrap();

    // B creates metadata + roster
    let md_b = create_metadata_doc(topic, &sender_b, RoomMetadata::empty())
        .await
        .unwrap();

    let roster_b = create_roster_doc(
        topic,
        &sender_b,
        ep_b.secret_key().public().to_string(),
        "PeerB".into(),
    )
    .await
    .unwrap();

    eprintln!("docs created");

    // Short pause + drain any messages that arrived before B's docs existed.
    sleep(Duration::from_millis(300)).await;
    drain(&md_b, &roster_b, &mut rx_b, Duration::from_millis(500), 30).await;
    eprintln!("drained initial sync");

    // ── Add B to A's roster ─────────────────────────────────────────────
    room_docs::add_member(
        &roster_a,
        &sender_a,
        ep_b.secret_key().public().to_string(),
        "PeerB".into(),
    )
    .await
    .unwrap();

    eprintln!("added B to A's roster");

    // ── Wait for gossip to propagate, draining on B ─────────────────────
    // Maximum wait: 12 × 500ms = 6 seconds before assert failure.
    let pk_a = ep_a.secret_key().public().to_string();
    let pk_b = ep_b.secret_key().public().to_string();

    let mut propagated = false;
    for i in 0..12 {
        sleep(Duration::from_millis(500)).await;
        drain(&md_b, &roster_b, &mut rx_b, Duration::from_millis(300), 20).await;

        // Check if B has converged on the expected state.
        let b_md = read_metadata(&md_b).await;
        let b_roster = list_members(&roster_b).await;
        if b_md.name.as_deref() == Some("E2E Test Room")
            && b_roster.len() == 2
            && b_roster.contains_key(&pk_a)
            && b_roster.contains_key(&pk_b)
        {
            eprintln!("converged on iteration {i}");
            propagated = true;
            break;
        }
        eprintln!(
            "iteration {i}: metadata={:?}, roster_size={}",
            b_md.name,
            b_roster.len()
        );
    }

    assert!(
        propagated,
        "B did not converge on A's metadata and combined roster within the timeout"
    );

    // Also drain any outstanding A events (for completeness).
    drop(rx_a);
    drop(sender_a);
    drop(sender_b);

    // ── Assertions (in /room info style) ─────────────────────────────────

    let a_roster = list_members(&roster_a).await;
    let b_roster = list_members(&roster_b).await;

    eprintln!("A roster size: {}", a_roster.len());
    eprintln!("B roster size: {}", b_roster.len());

    // B should have A's metadata.
    let b_md = read_metadata(&md_b).await;
    eprintln!("B metadata: {:?}", b_md);
    assert_eq!(b_md.name.as_deref(), Some("E2E Test Room"));
    assert_eq!(b_md.description.as_deref(), Some("End-to-end test"));
    assert_eq!(b_md.rules.as_deref(), Some("Be excellent"));

    // A's roster should have both peers.
    assert_eq!(a_roster.len(), 2);
    assert!(a_roster.contains_key(&pk_a));
    assert!(a_roster.contains_key(&pk_b));

    // B's roster should have both peers.
    assert_eq!(b_roster.len(), 2);
    assert!(b_roster.contains_key(&pk_a));
    assert!(b_roster.contains_key(&pk_b));

    // Also verify the /room info format produces correct output.
    let a_info = format_room_info(&b_md, &b_roster);
    eprintln!("===== /room info output =====");
    for line in a_info.lines() {
        eprintln!("{line}");
    }
    eprintln!("===== end =====");

    // The formatted output should contain the room name and both peers.
    assert!(a_info.contains("E2E Test Room"));
    assert!(a_info.contains("End-to-end test"));
    assert!(a_info.contains("Be excellent"));
    assert!(a_info.contains("Members (2):"));
    assert!(a_info.contains("PeerA"));
    assert!(a_info.contains("PeerB"));

    eprintln!("ALL ASSERTIONS PASSED");
    Ok(())
}
