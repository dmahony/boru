//! Regression test: public-room blob size cap is enforced at the download
//! boundary.
//!
//! Verifies that `download_blob_with_safety` with a safety instance whose
//! `max_blob_size_bytes` is small rejects oversized blobs *before* returning
//! the full payload to the caller, while passing `None` (private-room path)
//! allows any size.

use std::sync::Arc;
use std::time::Duration;

use boru_chat::chat_callbacks::TransferKind;
use boru_chat::chat_core::download_blob_with_safety;
use boru_chat::public_room_config::PublicRoomConfig;
use boru_chat::public_room_safety::PublicRoomSafety;
use iroh::{
    address_lookup::memory::MemoryLookup, endpoint::presets, protocol::Router, PublicKey,
    RelayMode, SecretKey,
};
use iroh_blobs::{store::mem::MemStore, BlobsProtocol};
use n0_error::Result;
use n0_future::time::sleep;

/// Helper: create a test peer using a shared memory lookup for direct
/// peer-to-peer connectivity (no relay, no pkarr — Minimal preset).
async fn make_peer(
    seed: u8,
    lookup: MemoryLookup,
) -> Result<(iroh::protocol::Router, iroh::Endpoint, PublicKey, MemStore)> {
    let sk = SecretKey::from_bytes(&[seed; 32]);
    let ep = iroh::Endpoint::builder(presets::Minimal)
        .secret_key(sk.clone())
        .relay_mode(RelayMode::Disabled)
        .bind_addr("127.0.0.1:0".parse::<std::net::SocketAddr>().unwrap())?
        .bind()
        .await?;
    // Wire the MemoryLookup after bind (Minimal preset has no built-in lookup).
    ep.address_lookup()
        .expect("endpoint is not closed")
        .add(lookup);
    let pk = ep.secret_key().public();
    let blob_store = MemStore::new();
    let blobs_protocol = BlobsProtocol::new(&blob_store, None);
    let router = Router::builder(ep.clone())
        .accept(iroh_blobs::ALPN, blobs_protocol)
        .spawn();
    Ok((router, ep, pk, blob_store))
}

/// Seed the receiver's memory lookup with the provider's endpoint address
/// so the blob downloader can find the provider directly.
fn seed_lookup(lookup: &MemoryLookup, ep: &iroh::Endpoint) {
    lookup.set_endpoint_info(ep.addr());
}

#[tokio::test]
async fn safety_rejects_oversized_blob() -> Result<()> {
    let _ = tracing_subscriber::fmt::try_init();

    // ── Shared lookup so peers find each other without a relay ──
    let lookup = MemoryLookup::new();

    let (router_a, ep_a, pk_a, blob_store_a) = make_peer(1, lookup.clone()).await?;
    let (router_b, ep_b, _pk_b, blob_store_b) = make_peer(2, lookup.clone()).await?;

    // Seed B's lookup with A's address so B can download from A.
    seed_lookup(&lookup, &ep_a);

    // Give iroh time to propagate address information.
    sleep(Duration::from_millis(200)).await;

    // ── Peer A stores a blob that exceeds the public-room cap ──
    let oversized = vec![0u8; 100_000]; // 100 KiB — well past default 10 MiB
    let tag = blob_store_a.blobs().add_bytes(oversized).await?;
    let blob_hash = tag.hash;
    let blob_name = "oversized-test-blob".to_string();

    // Candidates: the sender is peer A.
    let candidates = vec![pk_a];

    // ── Peer B: safety with tiny blob cap ──────────────────────
    let mut config = PublicRoomConfig::default();
    config.max_blob_size_bytes = 50_000; // 50 KiB cap — blob is 100 KiB
    let safety = Some(Arc::new(PublicRoomSafety::new(config)));

    let result = download_blob_with_safety(
        &blob_store_b,
        &ep_b,
        blob_hash,
        candidates.clone(),
        blob_name.clone(),
        TransferKind::Image,
        |_| {},
        safety.as_deref(),
        pk_a,
    )
    .await;

    let err = result.expect_err("expected oversized blob to be rejected");
    let err_msg = format!("{err:#}");
    assert!(
        err_msg.contains("exceeds size limit") || err_msg.contains("blob too large"),
        "error message should mention the size limit: {err_msg}",
    );

    // ── Cleanup ────────────────────────────────────────────────
    let _ = router_a.shutdown().await;
    let _ = router_b.shutdown().await;
    Ok(())
}

#[tokio::test]
async fn safety_allows_small_blob() -> Result<()> {
    let _ = tracing_subscriber::fmt::try_init();

    let lookup = MemoryLookup::new();
    let (router_a, ep_a, pk_a, blob_store_a) = make_peer(3, lookup.clone()).await?;
    let (router_b, ep_b, _pk_b, blob_store_b) = make_peer(4, lookup.clone()).await?;

    seed_lookup(&lookup, &ep_a);
    sleep(Duration::from_millis(200)).await;

    // Small blob — well under the cap.
    let small = vec![0xABu8; 1_000];
    let tag = blob_store_a.blobs().add_bytes(small.clone()).await?;
    let blob_hash = tag.hash;
    let candidates = vec![pk_a];

    let mut config = PublicRoomConfig::default();
    config.max_blob_size_bytes = 50_000; // 1 KiB blob < 50 KiB cap
    let safety = Some(Arc::new(PublicRoomSafety::new(config)));

    let bytes = download_blob_with_safety(
        &blob_store_b,
        &ep_b,
        blob_hash,
        candidates.clone(),
        "small-blob".into(),
        TransferKind::Image,
        |_| {},
        safety.as_deref(),
        pk_a,
    )
    .await
    .expect("small blob should be allowed");

    assert_eq!(bytes.len(), 1_000, "should return the full blob contents");

    let _ = router_a.shutdown().await;
    let _ = router_b.shutdown().await;
    Ok(())
}

#[tokio::test]
async fn no_safety_allows_oversized_blob() -> Result<()> {
    let _ = tracing_subscriber::fmt::try_init();

    let lookup = MemoryLookup::new();
    let (router_a, ep_a, pk_a, blob_store_a) = make_peer(5, lookup.clone()).await?;
    let (router_b, ep_b, _pk_b, blob_store_b) = make_peer(6, lookup.clone()).await?;

    seed_lookup(&lookup, &ep_a);
    sleep(Duration::from_millis(200)).await;

    // Oversized blob — but safety is None (private-room path).
    let oversized = vec![0xFFu8; 100_000];
    let tag = blob_store_a.blobs().add_bytes(oversized.clone()).await?;
    let blob_hash = tag.hash;
    let candidates = vec![pk_a];

    let bytes = download_blob_with_safety(
        &blob_store_b,
        &ep_b,
        blob_hash,
        candidates.clone(),
        "private-oversized".into(),
        TransferKind::Image,
        |_| {},
        None, // private room — no size enforcement
        pk_a,
    )
    .await
    .expect("private-room path should allow oversized blobs");

    assert_eq!(bytes.len(), 100_000, "should return full oversized blob");

    let _ = router_a.shutdown().await;
    let _ = router_b.shutdown().await;
    Ok(())
}
