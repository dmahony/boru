//! Relay / network setup helpers (BORU-TEST-010).

use boru_core::net::GOSSIP_ALPN;
use iroh::{
    address_lookup::memory::MemoryLookup, endpoint::presets, tls::CaTlsConfig, Endpoint, RelayMode,
    SecretKey,
};
use iroh_mdns_address_lookup::MdnsAddressLookup;
use n0_error::Result;
use rand::RngExt;

/// Build a gossip-enabled endpoint from a caller-supplied relay map and mode.
///
/// Mirrors the historical `create_endpoint` helper from `test_three_peer_mesh.rs`.
///
/// * `relay_mode` — pass `RelayMode::Custom(relay_map.clone())` to force relay,
///   or `RelayMode::Disabled` for pure direct/mDNS connectivity.
/// * `memory` — an optional `MemoryLookup` used to register peers at
///   `127.0.0.1` addresses so they can find each other without mDNS.
///
/// Waits for the endpoint to come online (`Endpoint::online()`) before
/// returning, matching the original three-peer test's behaviour. Note that
/// `online()` can block indefinitely on `RelayMode::Default` when no relay
/// route exists — prefer an explicit `RelayMode::Custom`/`Disabled`.
pub async fn create_endpoint(
    rng: &mut rand::rngs::ChaCha12Rng,
    _relay_map: iroh::RelayMap,
    relay_mode: RelayMode,
    memory: Option<MemoryLookup>,
) -> Result<Endpoint> {
    let builder = Endpoint::builder(presets::Minimal)
        .relay_mode(relay_mode)
        .secret_key(SecretKey::from_bytes(&rng.random()))
        .alpns(vec![GOSSIP_ALPN.to_vec()])
        .ca_tls_config(CaTlsConfig::insecure_skip_verify())
        .address_lookup(MdnsAddressLookup::builder());
    let ep = builder.bind().await?;
    if let Some(m) = memory {
        ep.address_lookup()?.add(m);
    }
    ep.online().await;
    Ok(ep)
}

/// Start a local in-process relay server for network-scoped tests.
///
/// Returns `(relay_map, relay_url)` which is wired into endpoints via
/// `RelayMode::Custom`. The returned guard must stay alive for the test's
/// lifetime or the relay shuts down. Prefer this over public infra so tests
/// are hermetic and run in parallel.
pub async fn run_relay() -> Result<(iroh::RelayMap, iroh::RelayUrl)> {
    let (relay_map, relay_url, _guard) = iroh::test_utils::run_relay_server().await.unwrap();
    Ok((relay_map, relay_url))
}
