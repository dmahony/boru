#![cfg(feature = "net")]

//! # BORU-CP-15: two-node networking health view comparison
//!
//! Proves PDF Task 5.3 acceptance criteria in-process:
//!
//! 1. **Two machines produce directly comparable diagnostic dumps** — two
//!    in-process nodes (A and B) run the health probe harness; each side
//!    renders a `BORU-HEALTH-V1` copy-diagnostics block with identical
//!    stable labels and per-peer sorted rows.
//! 2. **Symmetric success** — when both nodes probe each other's
//!    deterministic direct topic, both dumps show the peer with
//!    `inbound=ok-… outbound=ok-… direct_topic=ready`.
//! 3. **Asymmetric A→B vs B→A failures are obvious** — when A's store has
//!    an outbound broadcast but B's store has no inbound delivery for A,
//!    the two rendered dumps differ exactly in the direction that failed.

use std::time::{Duration, Instant};

use boru_core::{
    control_plane::connectivity::{ConnectivityEvent, PeerConnectivityStore},
    control_plane::health::{
        build_health_rows, probe_direct_topic, render_copy_diagnostics, render_health_view,
        DirectTopicProbe,
    },
    discovery_service::DiscoveryService,
    discovery_topic::discovery_topic,
    net::{Gossip, GOSSIP_ALPN},
    public_room::PublicNetwork,
};
use iroh::{
    address_lookup::memory::MemoryLookup, endpoint::presets, protocol::Router, Endpoint, PublicKey,
    RelayMode, SecretKey,
};
use n0_error::{bail_any, Result};
use rand::{RngExt, SeedableRng};

const MESH_TIMEOUT: Duration = Duration::from_secs(20);
const POLL_TICK: Duration = Duration::from_millis(100);

// ---------------------------------------------------------------------------
// Helpers (mirror test_discovery_two_node's deterministic harness)
// ---------------------------------------------------------------------------

async fn spawn_node(
    rng: &mut impl rand::Rng,
    memory: MemoryLookup,
) -> Result<(Router, Endpoint, SecretKey, Gossip)> {
    let ep = Endpoint::builder(presets::N0DisableRelay)
        .secret_key(SecretKey::from_bytes(&rng.random()))
        .address_lookup(memory)
        .relay_mode(RelayMode::Disabled)
        .bind_addr("127.0.0.1:0".parse::<std::net::SocketAddr>().unwrap())?
        .bind()
        .await?;
    let gossip = Gossip::builder().spawn(ep.clone());
    let router = Router::builder(ep.clone())
        .accept(GOSSIP_ALPN, gossip.clone())
        .spawn();
    Ok((router, ep.clone(), ep.secret_key().clone(), gossip))
}

/// Consume the Arc and shut the discovery service down cleanly (the doctor
/// harness drains its probe JoinSet before doing this, so the Arc is unique).
async fn shutdown_service(service: std::sync::Arc<DiscoveryService>) {
    if let Ok(service) = std::sync::Arc::try_unwrap(service) {
        service.shutdown().await;
    }
}

async fn wait_for_peer(service: &DiscoveryService, peer: PublicKey, what: &str) -> Result<()> {
    let deadline = Instant::now() + MESH_TIMEOUT;
    while Instant::now() < deadline {
        if service.known_peers().iter().any(|(id, _)| *id == peer) {
            return Ok(());
        }
        tokio::time::sleep(POLL_TICK).await;
    }
    bail_any!("timed out waiting for {what}")
}

/// Wait until the connectivity store for `service` has an entry for `peer`.
async fn wait_for_connectivity(
    service: &DiscoveryService,
    peer: PublicKey,
    what: &str,
) -> Result<()> {
    let deadline = Instant::now() + MESH_TIMEOUT;
    while Instant::now() < deadline {
        let store = service.connectivity_store();
        if store.lock().unwrap().get(&peer).is_some() {
            return Ok(());
        }
        tokio::time::sleep(POLL_TICK).await;
    }
    bail_any!("timed out waiting for connectivity entry: {what}")
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

/// Both nodes discover each other, probe each other's direct topic, and
/// render copy-diagnostics blocks that are directly comparable and show
/// symmetric success.
#[tokio::test]
async fn two_nodes_produce_comparable_symmetric_dumps() -> Result<()> {
    let mut rng = rand::rngs::StdRng::seed_from_u64(0x5A17);
    let memory = MemoryLookup::new();
    let (_router_a, ep_a, sk_a, gossip_a) = spawn_node(&mut rng, memory.clone()).await?;
    let (_router_b, ep_b, sk_b, gossip_b) = spawn_node(&mut rng, memory.clone()).await?;
    memory.add_endpoint_info(ep_a.addr());
    memory.add_endpoint_info(ep_b.addr());

    let pk_a = sk_a.public();
    let pk_b = sk_b.public();
    let topic = discovery_topic(PublicNetwork::Mainnet);

    let service_a = std::sync::Arc::new(
        DiscoveryService::join(&gossip_a, topic, Vec::new(), pk_a, sk_a.clone())
            .await?
            .with_announce_min_interval(Duration::ZERO)
            .with_control_announce_min_interval(Duration::ZERO),
    );
    let service_b = std::sync::Arc::new(
        DiscoveryService::join(&gossip_b, topic, vec![ep_a.id()], pk_b, sk_b.clone())
            .await?
            .with_announce_min_interval(Duration::ZERO)
            .with_control_announce_min_interval(Duration::ZERO),
    );

    // Discovery must land on both sides before probing.
    wait_for_peer(&service_a, pk_b, "A discovers B").await?;
    wait_for_peer(&service_b, pk_a, "B discovers A").await?;

    // Both sides probe the deterministic direct topic concurrently.
    let (probe_a, probe_b) = tokio::join!(
        probe_direct_topic(&gossip_a, &service_a, pk_a, pk_b),
        probe_direct_topic(&gossip_b, &service_b, pk_b, pk_a),
    );
    assert_eq!(
        probe_a,
        DirectTopicProbe {
            topic_joined: true,
            probe_sent: true,
            probe_received: true,
        },
        "A→B probe should complete both directions"
    );
    assert_eq!(
        probe_b,
        DirectTopicProbe {
            topic_joined: true,
            probe_sent: true,
            probe_received: true,
        },
        "B→A probe should complete both directions"
    );

    // Give the store a beat to reflect the events, then render both sides.
    tokio::time::sleep(Duration::from_millis(300)).await;
    let rows_a = build_health_rows(&service_a.peer_diagnostics());
    let rows_b = build_health_rows(&service_b.peer_diagnostics());

    // Each dump must list exactly one peer (the other node).
    assert_eq!(rows_a.len(), 1, "A sees one peer: {rows_a:#?}");
    assert_eq!(rows_b.len(), 1, "B sees one peer: {rows_b:#?}");

    let row_a = &rows_a[0];
    let row_b = &rows_b[0];
    assert_eq!(row_a.peer_id, pk_b.fmt_short().to_string());
    assert_eq!(row_b.peer_id, pk_a.fmt_short().to_string());

    // Symmetric success: both sides report discovery + endpoint + direct
    // topic + inbound + outbound. The six indicators are separate.
    assert!(
        row_a.discovery.starts_with("seen-"),
        "A discovery: {}",
        row_a.discovery
    );
    assert!(
        row_a.endpoint.starts_with("connected-"),
        "A endpoint: {}",
        row_a.endpoint
    );
    assert_eq!(row_a.direct_topic, "ready", "A direct topic");
    assert!(
        row_a.inbound.starts_with("ok-"),
        "A inbound: {}",
        row_a.inbound
    );
    assert!(
        row_a.outbound.starts_with("ok-"),
        "A outbound: {}",
        row_a.outbound
    );
    // Path is diagnostic-only (BORU-CP-14): on loopback with a shared
    // MemoryLookup it classifies as `direct` once the 15s refresh sweep
    // runs, but the test window is shorter — `unknown` is also acceptable
    // (the label must simply be present and separate from delivery).
    assert!(
        row_a.path == "direct" || row_a.path == "unknown",
        "A path: {}",
        row_a.path
    );

    assert!(
        row_b.discovery.starts_with("seen-"),
        "B discovery: {}",
        row_b.discovery
    );
    assert!(
        row_b.endpoint.starts_with("connected-"),
        "B endpoint: {}",
        row_b.endpoint
    );
    assert_eq!(row_b.direct_topic, "ready", "B direct topic");
    assert!(
        row_b.inbound.starts_with("ok-"),
        "B inbound: {}",
        row_b.inbound
    );
    assert!(
        row_b.outbound.starts_with("ok-"),
        "B outbound: {}",
        row_b.outbound
    );

    // Copy-diagnostics blocks are directly comparable: identical header
    // shape, identical label set, sorted rows.
    let dump_a = render_copy_diagnostics("A", Duration::from_secs(10), &rows_a);
    let dump_b = render_copy_diagnostics("B", Duration::from_secs(10), &rows_b);
    for label in [
        "discovery=",
        "endpoint=",
        "direct_topic=",
        "inbound=",
        "outbound=",
        "path=",
        "state=",
    ] {
        assert!(dump_a.contains(label), "dump A missing {label}:\n{dump_a}");
        assert!(dump_b.contains(label), "dump B missing {label}:\n{dump_b}");
    }
    // The per-peer rows differ only in peer id / direction values, not in
    // which labels exist.
    let peer_line_a: Vec<_> = dump_a.lines().filter(|l| l.starts_with("peer=")).collect();
    let peer_line_b: Vec<_> = dump_b.lines().filter(|l| l.starts_with("peer=")).collect();
    assert_eq!(peer_line_a.len(), 1);
    assert_eq!(peer_line_b.len(), 1);

    // Human view renders too (debug surface).
    let human = render_health_view("A", Duration::from_secs(10), &rows_a);
    assert!(human.contains("discovery"));
    assert!(human.contains("inbound"));
    assert!(human.contains("outbound"));

    shutdown_service(service_a).await;
    shutdown_service(service_b).await;
    Ok(())
}

/// The dump format makes an asymmetric A→B failure obvious: A has sent
/// (outbound=ok) but B has never received anything from A (inbound=never).
/// Rendered side by side, the two blocks differ exactly in the failed
/// direction.
#[test]
fn asymmetric_failure_renders_obviously_in_both_dumps() {
    let t0 = Instant::now();
    let pk_a = {
        let mut seed = [0u8; 32];
        seed[0] = 0xAA;
        iroh_base::SecretKey::from_bytes(&seed).public()
    };
    let pk_b = {
        let mut seed = [0u8; 32];
        seed[0] = 0xBB;
        iroh_base::SecretKey::from_bytes(&seed).public()
    };

    // Machine A's store: it discovered B, connected, joined the direct
    // topic, and broadcast a probe (outbound) — but never received a reply.
    let mut store_a = PeerConnectivityStore::new();
    store_a.apply(pk_b, ConnectivityEvent::DiscoverySeen, t0);
    store_a.apply(
        pk_b,
        ConnectivityEvent::EndpointConnected,
        t0 + Duration::from_millis(1),
    );
    store_a.apply(
        pk_b,
        ConnectivityEvent::TopicJoined,
        t0 + Duration::from_millis(2),
    );
    store_a.apply(
        pk_b,
        ConnectivityEvent::DirectMessageSent,
        t0 + Duration::from_millis(3),
    );

    // Machine B's store: it discovered A and connected, but B never joined
    // the direct topic and never received A's probe.
    let mut store_b = PeerConnectivityStore::new();
    store_b.apply(pk_a, ConnectivityEvent::DiscoverySeen, t0);
    store_b.apply(
        pk_a,
        ConnectivityEvent::EndpointConnected,
        t0 + Duration::from_millis(1),
    );

    let rows_a = build_health_rows(&boru_core::control_plane::diagnostics::snapshots_for(
        &store_a,
        &pk_a,
        t0 + Duration::from_secs(10),
    ));
    let rows_b = build_health_rows(&boru_core::control_plane::diagnostics::snapshots_for(
        &store_b,
        &pk_b,
        t0 + Duration::from_secs(10),
    ));

    let dump_a = render_copy_diagnostics("A", Duration::from_secs(10), &rows_a);
    let dump_b = render_copy_diagnostics("B", Duration::from_secs(10), &rows_b);

    // A: outbound succeeded, inbound never (A→B broken).
    assert!(
        dump_a.contains("outbound=ok-"),
        "A dump shows outbound:\n{dump_a}"
    );
    assert!(
        dump_a.contains("inbound=never"),
        "A dump shows no inbound:\n{dump_a}"
    );
    // B: never joined the direct topic, never received anything from A.
    assert!(
        dump_b.contains("direct_topic=not_attempted"),
        "B dump:\n{dump_b}"
    );
    assert!(
        dump_b.contains("inbound=never"),
        "B dump shows no inbound:\n{dump_b}"
    );
    assert!(
        dump_b.contains("outbound=never"),
        "B dump shows no outbound:\n{dump_b}"
    );
}

/// A raw two-node probe harness spawns a probe task per peer and keeps the
/// service alive (guards the JoinSet pattern used by the doctor example).
#[tokio::test]
async fn probe_tasks_in_joinset_feed_the_store() -> Result<()> {
    let mut rng = rand::rngs::StdRng::seed_from_u64(0x5A18);
    let memory = MemoryLookup::new();
    let (_router_a, ep_a, sk_a, gossip_a) = spawn_node(&mut rng, memory.clone()).await?;
    let (_router_b, ep_b, sk_b, gossip_b) = spawn_node(&mut rng, memory.clone()).await?;
    memory.add_endpoint_info(ep_a.addr());
    memory.add_endpoint_info(ep_b.addr());

    let pk_a = sk_a.public();
    let pk_b = sk_b.public();
    let topic = discovery_topic(PublicNetwork::Mainnet);

    let service_a = std::sync::Arc::new(
        DiscoveryService::join(&gossip_a, topic, Vec::new(), pk_a, sk_a.clone())
            .await?
            .with_announce_min_interval(Duration::ZERO)
            .with_control_announce_min_interval(Duration::ZERO),
    );
    let service_b = std::sync::Arc::new(
        DiscoveryService::join(&gossip_b, topic, vec![ep_a.id()], pk_b, sk_b.clone())
            .await?
            .with_announce_min_interval(Duration::ZERO)
            .with_control_announce_min_interval(Duration::ZERO),
    );

    wait_for_peer(&service_a, pk_b, "A discovers B").await?;
    wait_for_peer(&service_b, pk_a, "B discovers A").await?;
    wait_for_connectivity(&service_a, pk_b, "A connectivity entry").await?;
    wait_for_connectivity(&service_b, pk_a, "B connectivity entry").await?;

    // Simulate the doctor harness: spawn one probe per peer into a JoinSet
    // (tasks need 'static data, so clone the gossip + Arc-clone the service).
    let mut tasks: tokio::task::JoinSet<DirectTopicProbe> = tokio::task::JoinSet::new();
    tasks.spawn({
        let gossip = gossip_a.clone();
        let service = std::sync::Arc::clone(&service_a);
        async move { probe_direct_topic(&gossip, &service, pk_a, pk_b).await }
    });
    tasks.spawn({
        let gossip = gossip_b.clone();
        let service = std::sync::Arc::clone(&service_b);
        async move { probe_direct_topic(&gossip, &service, pk_b, pk_a).await }
    });
    let mut ok = 0;
    while let Some(res) = tasks.join_next().await {
        let probe = res.map_err(|e| anyhow::anyhow!("probe task panicked: {e}"))?;
        assert!(probe.topic_joined && probe.probe_sent && probe.probe_received);
        ok += 1;
    }
    assert_eq!(ok, 2);

    // The store must have absorbed the probe events.
    {
        let store = service_a.connectivity_store();
        let guard = store.lock().unwrap();
        let entry = guard.get(&pk_b).expect("A has B entry");
        assert!(entry.last_outbound_direct.is_some(), "A recorded outbound");
        assert!(
            entry.last_inbound_direct.is_some(),
            "A recorded inbound from B"
        );
    }

    shutdown_service(service_a).await;
    shutdown_service(service_b).await;
    Ok(())
}
