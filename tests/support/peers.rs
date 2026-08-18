//! Peer / node fixture creation (BORU-TEST-010).
//!
//! Lifts the endpoint + gossip + router scaffolding that was byte-identical
//! across several integration-test files into a single, reusable constructor.

use boru_core::net::{Gossip, GOSSIP_ALPN};
use iroh::{
    address_lookup::memory::MemoryLookup, endpoint::presets, protocol::Router, Endpoint, RelayMode,
    SecretKey,
};
use n0_error::Result;
use rand::RngExt;

/// A spawned gossip peer: its router, endpoint, secret key and gossip actor.
///
/// Keeping the router alive (and dropping it after the endpoint) is required
/// for clean teardown; `PeerFixture` owns all four so tests can `drop(fx)`
/// in one place.
#[derive(Debug)]
pub struct PeerFixture {
    pub router: Router,
    pub endpoint: Endpoint,
    pub secret_key: SecretKey,
    pub gossip: Gossip,
}

impl PeerFixture {
    /// The peer's public endpoint id (`fmt().fmt_short()` for a compact id).
    pub fn id(&self) -> iroh::EndpointId {
        self.endpoint.id()
    }
}

/// Generate a fresh secret key from a caller-supplied RNG.
///
/// Deterministic tests pass a seeded `rand::rngs::ChaCha12Rng`; random tests
/// pass `&mut rand::rng()`.
pub fn make_sk(rng: &mut impl rand::Rng) -> SecretKey {
    SecretKey::from_bytes(&rng.random())
}

/// Spawn a local, relay-disabled gossip peer bound to `127.0.0.1:0`.
///
/// This mirrors the historical `spawn_peer_relay` helper that was duplicated
/// (verbatim) in `test_two_peers_exchange.rs` and `test_two_peers_relay.rs`.
/// It uses `RelayMode::Disabled` + `MemoryLookup` (no public relay, no mDNS)
/// so two such peers only discover each other through an explicit
/// `address_lookup` wiring step in the caller.
pub async fn spawn_peer_relay(rng: &mut impl rand::Rng) -> Result<PeerFixture> {
    let ep = Endpoint::builder(presets::N0DisableRelay)
        .secret_key(make_sk(rng))
        .address_lookup(MemoryLookup::new())
        .relay_mode(RelayMode::Disabled)
        .bind_addr("127.0.0.1:0".parse::<std::net::SocketAddr>().unwrap())?
        .bind()
        .await?;
    let gossip = Gossip::builder().spawn(ep.clone());
    let router = Router::builder(ep.clone())
        .accept(GOSSIP_ALPN, gossip.clone())
        .spawn();
    Ok(PeerFixture {
        router,
        endpoint: ep.clone(),
        secret_key: ep.secret_key().clone(),
        gossip,
    })
}

/// Register gossip on an endpoint's router and spawn it.
///
/// Convenience wrapper for tests that build their own endpoint (e.g. via
/// [`crate::net::create_endpoint`]) and just need the gossip router.
pub fn spawn_gossip_router(ep: Endpoint, gossip: Gossip) -> Router {
    Router::builder(ep).accept(GOSSIP_ALPN, gossip).spawn()
}
