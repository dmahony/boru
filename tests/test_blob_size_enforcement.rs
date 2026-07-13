//! Regression test: public-room blob size cap is enforced at the download
//! boundary.
//!
//! Verifies that `download_blob_with_safety` with a safety instance whose
//! `max_blob_size_bytes` is small rejects oversized blobs, while passing
//! `None` (private-room path) allows any size.

use std::sync::Arc;
use std::time::Duration;

use boru_chat::chat_callbacks::TransferKind;
use boru_chat::chat_core::download_blob_with_safety;
use boru_chat::public_room_config::PublicRoomConfig;
use boru_chat::public_room_safety::PublicRoomSafety;
use iroh::{address_lookup::memory::MemoryLookup, endpoint::presets, protocol::Router, SecretKey};
use iroh_blobs::{store::mem::MemStore, BlobsProtocol};
use n0_error::Result;
use n0_future::time::sleep;
use rand::{RngExt, SeedableRng};

/// Helper: bind an iroh endpoint with blobs protocol,
/// optionally sharing a memory address lookup.
async fn make_peer(
    rng: &mut impl rand::Rng,
    lookup: Option<MemoryLookup>,
) -> Result<(Router, iroh::Endpoint, MemStore)> {
    let sk = SecretKey::from_bytes(&rng.random());
    let mut builder = iroh::Endpoint::builder(presets::N0DisableRelay)
        .secret_key(sk)
        .bind_addr("127.0.0.1:0".parse::<std::net::SocketAddr>().unwrap())?;
    if let Some(l) = lookup {
        builder = builder.address_lookup(l);
    }
    let ep = builder.bind().await?;
    ep.online().await;
    let blob_store = MemStore::new();
    let blobs_protocol = BlobsProtocol::new(&blob_store, None);
    let router = Router::builder(ep.clone())
        .accept(iroh_blobs::ALPN, blobs_protocol)
        .spawn();
    Ok((router, ep, blob_store))
}

#[tokio::test]
async fn safety_rejects_oversized_blob() -> Result<()> {
    let _ = tracing_subscriber::fmt::try_init();
    let mut rng = rand::rngs::StdRng::seed_from_u64(42);
    let lookup = MemoryLookup::new();
    let lookup_b = lookup.clone();

    let (_router_a, ep_a, blob_store_a) = make_peer(&mut rng, Some(lookup.clone())).await?;
    let (_router_b, ep_b, blob_store_b) = make_peer(&mut rng, Some(lookup_b)).await?;

    lookup.set_endpoint_info(ep_a.addr());
    lookup.set_endpoint_info(ep_b.addr());
    sleep(Duration::from_millis(500)).await;

    // Peer A stores an oversized blob (100 KiB).
    let oversized = vec![0u8; 100_000];
    let tag = blob_store_a.blobs().add_bytes(oversized).await?;
    let blob_hash = tag.hash;
    let peer_a_pk = ep_a.secret_key().public();
    let candidates = vec![peer_a_pk];

    // Peer B: safety with 50 KiB cap.
    let mut config = PublicRoomConfig::default();
    config.max_blob_size_bytes = 50_000;
    let safety = Arc::new(PublicRoomSafety::new(config));

    let result = download_blob_with_safety(
        &blob_store_b,
        &ep_b,
        blob_hash,
        candidates,
        "oversized-blob".into(),
        TransferKind::Image,
        |_| {},
        Some(&*safety),
        peer_a_pk,
    )
    .await;

    let err = result.expect_err("safety should reject oversized blob");
    let msg = err.to_string();
    assert!(
        msg.contains("exceeds size limit") || msg.contains("exceeds max_blob_size_bytes"),
        "error message should mention size limit, got: {msg}",
    );

    sleep(Duration::from_millis(200)).await;
    Ok(())
}

#[tokio::test]
async fn safety_allows_small_blob() -> Result<()> {
    let _ = tracing_subscriber::fmt::try_init();
    let mut rng = rand::rngs::StdRng::seed_from_u64(99);
    let lookup = MemoryLookup::new();
    let lookup_b = lookup.clone();

    let (_router_a, ep_a, blob_store_a) = make_peer(&mut rng, Some(lookup.clone())).await?;
    let (_router_b, ep_b, blob_store_b) = make_peer(&mut rng, Some(lookup_b)).await?;

    lookup.set_endpoint_info(ep_a.addr());
    lookup.set_endpoint_info(ep_b.addr());
    sleep(Duration::from_millis(500)).await;

    // Small blob — well under the cap.
    let small = vec![0xABu8; 1_000];
    let tag = blob_store_a.blobs().add_bytes(small.clone()).await?;
    let blob_hash = tag.hash;
    let peer_a_pk = ep_a.secret_key().public();
    let candidates = vec![peer_a_pk];

    let mut config = PublicRoomConfig::default();
    config.max_blob_size_bytes = 50_000;
    let safety = Arc::new(PublicRoomSafety::new(config));

    let bytes = download_blob_with_safety(
        &blob_store_b,
        &ep_b,
        blob_hash,
        candidates,
        "small-blob".into(),
        TransferKind::Image,
        |_| {},
        Some(&*safety),
        peer_a_pk,
    )
    .await
    .expect("small blob should be allowed");

    assert_eq!(bytes.len(), 1_000, "should return full blob contents");

    sleep(Duration::from_millis(200)).await;
    Ok(())
}

#[tokio::test]
async fn no_safety_allows_oversized_blob() -> Result<()> {
    let _ = tracing_subscriber::fmt::try_init();
    let mut rng = rand::rngs::StdRng::seed_from_u64(123);
    let lookup = MemoryLookup::new();
    let lookup_b = lookup.clone();

    let (_router_a, ep_a, blob_store_a) = make_peer(&mut rng, Some(lookup.clone())).await?;
    let (_router_b, ep_b, blob_store_b) = make_peer(&mut rng, Some(lookup_b)).await?;

    lookup.set_endpoint_info(ep_a.addr());
    lookup.set_endpoint_info(ep_b.addr());
    sleep(Duration::from_millis(500)).await;

    // Oversized blob — but safety is None (private-room path).
    let oversized = vec![0xFFu8; 100_000];
    let tag = blob_store_a.blobs().add_bytes(oversized.clone()).await?;
    let blob_hash = tag.hash;
    let peer_a_pk = ep_a.secret_key().public();
    let candidates = vec![peer_a_pk];

    let bytes = download_blob_with_safety(
        &blob_store_b,
        &ep_b,
        blob_hash,
        candidates,
        "private-blob".into(),
        TransferKind::Image,
        |_| {},
        None,
        peer_a_pk,
    )
    .await
    .expect("private-room path (None) should allow oversized blobs");

    assert_eq!(bytes.len(), 100_000, "should return full oversized blob");

    sleep(Duration::from_millis(200)).await;
    Ok(())
}
