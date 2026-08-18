#![cfg(feature = "net")]

//! # BORU-CP-16: two-node Phase 6 extensions round trip + metadata-only proof
//!
//! Proves the PDF Phase 6 acceptance criteria in-process:
//!
//! 1. **Every extension is metadata-only.** A fully populated
//!    [`ExtensionsPayload`] carries no file bytes, tunnel data, media,
//!    session keys, credentials, or LAN topology — the wire encoding stays
//!    tiny, contains no data-plane markers, and the typed payload has no
//!    field capable of carrying them (structural by construction, tested at
//!    the wire level here).
//! 2. **Advertisements flow over the control plane.** Two in-process nodes
//!    (A and B) form a gossip mesh on the internal discovery topic; A
//!    advertises a full Phase 6 payload via `update_local_extensions`, and B
//!    reads the cached advertisement via `peer_extensions`.
//! 3. **Extensions traffic never registers peers.** The EXTENSIONS envelope
//!    is routed as a control-plane event only; announcing/receiving
//!    extensions adds nothing to the legacy peer registry (only the legacy
//!    HELLO path ever registers a peer).

use std::sync::Arc;
use std::time::{Duration, Instant};

use boru_core::control_plane::extensions::{
    CallAvailability, CallCapability, ExtensionsPayload, FileReadiness, GroupHints,
    MultiDeviceIdentity, PathPreference, RelayHealthHint, ScreenShareCapability, TunnelCapability,
};
use boru_core::discovery_service::DiscoveryService;
use boru_core::discovery_topic::discovery_topic;
use boru_core::net::{Gossip, GOSSIP_ALPN};
use boru_core::public_room::PublicNetwork;
use iroh::{
    address_lookup::memory::MemoryLookup, endpoint::presets, protocol::Router, Endpoint, PublicKey,
    RelayMode, SecretKey,
};
use n0_error::{bail_any, Result};
use rand::{RngExt, SeedableRng};

const MESH_TIMEOUT: Duration = Duration::from_secs(20);
const POLL_TICK: Duration = Duration::from_millis(100);

/// A fully populated Phase 6 extensions payload (all eight sections).
fn full_extensions() -> ExtensionsPayload {
    ExtensionsPayload {
        group: Some(GroupHints { available: true }),
        file: Some(FileReadiness {
            protocol_versions: vec!["v2".into()],
            can_receive: true,
        }),
        tunnel: Some(TunnelCapability {
            protocol_versions: vec!["v1".into()],
        }),
        call: Some(CallCapability {
            protocol_versions: vec!["v1".into()],
            availability: Some(CallAvailability::Available),
        }),
        screen_share: Some(ScreenShareCapability {
            protocol_versions: vec!["v1".into()],
        }),
        identity: Some(MultiDeviceIdentity {
            identity_id: "user-alice".into(),
            device_id: "dev-phone".into(),
            active_device: true,
        }),
        path_preference: Some(PathPreference::DirectPreferred),
        relay_health: Some(RelayHealthHint::Healthy),
    }
}

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

async fn shutdown_service(service: Arc<DiscoveryService>) {
    if let Ok(service) = Arc::try_unwrap(service) {
        service.shutdown().await;
    }
}

/// Build a discovery service on `gossip`/`topic` with zero announce
/// throttles so the join-time HELLO burst and any test-driven announcements
/// are never suppressed by the default min-interval.
async fn join_service(
    gossip: &Gossip,
    topic: boru_core::proto::TopicId,
    peers: Vec<PublicKey>,
    node: PublicKey,
    secret: SecretKey,
) -> Result<Arc<DiscoveryService>> {
    Ok(Arc::new(
        DiscoveryService::join(gossip, topic, peers, node, secret)
            .await?
            .with_announce_min_interval(Duration::ZERO)
            .with_control_announce_min_interval(Duration::ZERO)
            .with_extensions_announce_min_interval(Duration::ZERO),
    ))
}

/// Wait until `service` has a legacy-registry entry for `peer` (i.e. the
/// gossip mesh has formed and the legacy HELLO re-announcement landed).
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

/// Wait until `service` has a control-plane presence entry for `peer`
/// (i.e. the peer's EXTENSIONS advertisement has been received and cached).
async fn wait_for_peer_extensions(
    service: &DiscoveryService,
    peer: PublicKey,
    what: &str,
) -> Result<()> {
    let deadline = Instant::now() + MESH_TIMEOUT;
    while Instant::now() < deadline {
        if service.peer_extensions(&peer).is_some() {
            return Ok(());
        }
        tokio::time::sleep(POLL_TICK).await;
    }
    bail_any!("timed out waiting for {what}")
}

/// A's full Phase 6 extensions advertisement reaches B through the internal
/// discovery topic: B reads it back via `peer_extensions`.
#[tokio::test]
async fn two_node_extensions_round_trip() -> Result<()> {
    let mut rng = rand::rngs::StdRng::seed_from_u64(0xC016);
    let memory = MemoryLookup::new();
    let (_router_a, ep_a, sk_a, gossip_a) = spawn_node(&mut rng, memory.clone()).await?;
    let (_router_b, ep_b, sk_b, gossip_b) = spawn_node(&mut rng, memory.clone()).await?;
    memory.add_endpoint_info(ep_a.addr());
    memory.add_endpoint_info(ep_b.addr());

    let pk_a = sk_a.public();
    let pk_b = sk_b.public();
    let topic = discovery_topic(PublicNetwork::Mainnet);

    let service_a = join_service(&gossip_a, topic, Vec::new(), pk_a, sk_a.clone()).await?;
    let service_b = join_service(&gossip_b, topic, vec![ep_a.id()], pk_b, sk_b.clone()).await?;

    // Wait for the gossip mesh to form (B sees A's legacy HELLO). Join-time
    // control announcements are sent before the mesh exists, so the
    // extensions advertisement must be announced AFTER mesh formation.
    wait_for_peer(&service_b, pk_a, "A's legacy hello at B").await?;

    // A advertises its full extensions payload (metadata only).
    let full = full_extensions();
    service_a.update_local_extensions(full.clone()).await?;

    // B discovers A and reads the cached advertisement.
    wait_for_peer_extensions(&service_b, pk_a, "A's extensions at B").await?;
    let seen = service_b.peer_extensions(&pk_a).expect("cached");
    assert_eq!(seen, full, "B must see exactly what A advertised");
    // The cached advertisement is metadata only: a coarse group-availability
    // flag, protocol versions, and an identity/device pair — never content.
    assert!(seen.group.is_some());
    assert!(seen.file.is_some());
    assert!(seen.tunnel.is_some());
    assert!(seen.call.is_some());
    assert!(seen.screen_share.is_some());
    assert!(seen.identity.is_some());
    assert!(seen.path_preference.is_some());
    assert!(seen.relay_health.is_some());

    shutdown_service(service_a).await;
    shutdown_service(service_b).await;
    Ok(())
}

/// The wire form of a fully populated extensions payload is metadata only:
/// tiny, bounded, and free of data-plane markers (no file bytes, tunnel
/// data, media, session keys, credentials, or LAN topology).
#[test]
fn extensions_wire_is_metadata_only() {
    let payload = full_extensions();

    // Postcard-encode the payload the way the envelope would carry it.
    let encoded = postcard::to_stdvec(&payload).expect("encode");
    assert!(
        encoded.len() < 512,
        "fully populated extensions payload must stay tiny, got {} bytes",
        encoded.len()
    );

    // None of the data-plane markers may appear on the discovery wire.
    let text = String::from_utf8_lossy(&encoded).to_lowercase();
    for marker in [
        "0.0.0.0",     // LAN topology / direct addresses
        "192.168.",    // LAN topology
        "10.0.",       // LAN topology
        ":8080",       // tunnel/call transport detail
        ":443",        // relay transport detail
        "session_key", // call session key
        "password",    // credentials
        "token",       // credentials
        "file_bytes",  // file content
        "video_frame", // media
        "audio_frame", // media
        "vnc",         // screen-share session data
    ] {
        assert!(!text.contains(marker), "wire must not contain {marker:?}");
    }

    // The typed payload structurally cannot carry raw bytes: every field is
    // a bounded string, a bool, or a coarse enum. A malicious peer that
    // tries to stuff 4 KiB of "file bytes" into a protocol-version string is
    // rejected by the privacy layer's bounds.
    let bounds = boru_core::control_plane::extensions::ExtensionsBounds::default();
    let smuggled = ExtensionsPayload {
        file: Some(FileReadiness {
            protocol_versions: vec!["x".repeat(4096)],
            can_receive: true,
        }),
        ..Default::default()
    };
    assert!(
        smuggled.validate(&bounds).is_err(),
        "oversized version strings must be rejected by the bounds"
    );
}

/// Receiving an EXTENSIONS envelope caches metadata but never registers the
/// peer: the legacy registry on B contains only what the legacy HELLO path
/// registered, and the extensions announcement adds nothing to it.
#[tokio::test]
async fn extensions_envelope_never_touches_peer_registry() -> Result<()> {
    let mut rng = rand::rngs::StdRng::seed_from_u64(0xC017);
    let memory = MemoryLookup::new();
    let (_router_a, ep_a, sk_a, gossip_a) = spawn_node(&mut rng, memory.clone()).await?;
    let (_router_b, ep_b, sk_b, gossip_b) = spawn_node(&mut rng, memory.clone()).await?;
    memory.add_endpoint_info(ep_a.addr());
    memory.add_endpoint_info(ep_b.addr());

    let pk_a = sk_a.public();
    let pk_b = sk_b.public();
    let topic = discovery_topic(PublicNetwork::Mainnet);

    let service_a = join_service(&gossip_a, topic, Vec::new(), pk_a, sk_a.clone()).await?;
    let service_b = join_service(&gossip_b, topic, vec![ep_a.id()], pk_b, sk_b.clone()).await?;

    // Mesh formed: the legacy HELLO registered A (that is the ONLY registry
    // write in this whole flow).
    wait_for_peer(&service_b, pk_a, "A's legacy hello at B").await?;
    let registered_before = service_b.peer_count();

    // A advertises extensions; B caches them.
    service_a.update_local_extensions(full_extensions()).await?;
    wait_for_peer_extensions(&service_b, pk_a, "A's extensions at B").await?;
    assert!(
        service_b.peer_extensions(&pk_a).is_some(),
        "extensions must be cached"
    );

    // The EXTENSIONS envelope itself registered nobody: the registry size is
    // unchanged by the extensions round trip.
    assert_eq!(
        service_b.peer_count(),
        registered_before,
        "extensions traffic must never register peers in the legacy registry"
    );

    shutdown_service(service_a).await;
    shutdown_service(service_b).await;
    Ok(())
}
