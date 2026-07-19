//! End-to-end coverage for ordinary file downloads.
//!
//! Each case exercises the real iroh-blobs downloader between two localhost
//! peers and verifies the final BLAKE3 hash and byte count.  Imported and
//! referenced files are prepared through the same file-access preparation
//! helpers used by the transfer authorisation path.

use std::sync::Arc;
use std::time::Duration;

use boru_chat::chat_callbacks::TransferKind;
use boru_chat::chat_core::download_blob_with_safety;
use boru_chat::file_access_handler::{prepare_imported_file, prepare_referenced_file};
use boru_chat::storage::Storage;
use iroh::{
    address_lookup::memory::MemoryLookup, endpoint::presets, protocol::Router, PublicKey,
    RelayMode, SecretKey,
};
use iroh_blobs::{api::Store, store::mem::MemStore, BlobsProtocol};
use n0_error::Result;
use tempfile::TempDir;

struct Peer {
    router: Router,
    endpoint: iroh::Endpoint,
    public_key: PublicKey,
    blobs: Arc<Store>,
}

async fn make_peer(seed: u8, lookup: MemoryLookup) -> Result<Peer> {
    let endpoint = iroh::Endpoint::builder(presets::Minimal)
        .secret_key(SecretKey::from_bytes(&[seed; 32]))
        .address_lookup(lookup)
        .relay_mode(RelayMode::Disabled)
        .bind_addr("127.0.0.1:0".parse::<std::net::SocketAddr>().unwrap())?
        .bind()
        .await?;
    let blobs: Arc<Store> = Arc::new(MemStore::new().into());
    let router = Router::builder(endpoint.clone())
        .accept(iroh_blobs::ALPN, BlobsProtocol::new(&blobs, None))
        .spawn();
    Ok(Peer {
        public_key: endpoint.secret_key().public(),
        router,
        endpoint,
        blobs,
    })
}

async fn download_and_verify(
    receiver: &Peer,
    sender: &Peer,
    hash: iroh_blobs::Hash,
    name: &str,
    expected: &[u8],
) -> Result<()> {
    let downloaded = download_blob_with_safety(
        &receiver.blobs,
        &receiver.endpoint,
        hash,
        vec![sender.public_key],
        name.to_string(),
        TransferKind::File,
        |_| {},
        None,
        sender.public_key,
    )
    .await?;

    assert_eq!(downloaded.len(), expected.len(), "size mismatch for {name}");
    let expected_hash = blake3::hash(expected).to_hex().to_string();
    let actual_hash = blake3::hash(&downloaded).to_hex().to_string();
    assert_eq!(actual_hash, expected_hash, "hash mismatch for {name}");
    assert_eq!(downloaded, expected, "content mismatch for {name}");
    Ok(())
}

#[tokio::test]
async fn normal_downloads_cover_empty_small_large_imported_referenced_and_duplicate_files(
) -> Result<()> {
    let lookup = MemoryLookup::new();
    let sender = make_peer(0x31, lookup.clone()).await?;
    let receiver = make_peer(0x32, lookup.clone()).await?;
    lookup.set_endpoint_info(sender.endpoint.addr());
    tokio::time::sleep(Duration::from_millis(100)).await;

    // Zero-byte files are valid blobs and must not be treated as missing.
    let empty: Vec<u8> = Vec::new();
    let empty_tag = sender.blobs.blobs().add_bytes(empty.clone()).await?;
    download_and_verify(&receiver, &sender, empty_tag.hash, "empty.bin", &empty).await?;

    // Small text and common binary signatures exercise ordinary file payloads.
    for (name, data) in [
        ("notes.txt", b"hello from Boru\n".to_vec()),
        ("image.png", b"\x89PNG\r\n\x1a\nboru-test".to_vec()),
        ("document.pdf", b"%PDF-1.7\nboru-test\n%%EOF\n".to_vec()),
    ] {
        let tag = sender.blobs.blobs().add_bytes(data.clone()).await?;
        download_and_verify(&receiver, &sender, tag.hash, name, &data).await?;
    }

    // A generated multi-megabyte payload verifies streaming rather than only
    // a tiny inline transfer.
    let large: Vec<u8> = (0..(3 * 1024 * 1024))
        .map(|i| ((i * 31 + i / 97) % 251) as u8)
        .collect();
    let large_tag = sender.blobs.blobs().add_bytes(large.clone()).await?;
    download_and_verify(&receiver, &sender, large_tag.hash, "large.bin", &large).await?;

    // Imported object: the file is represented by a blob reference in storage.
    let imported = b"imported file bytes\n".to_vec();
    let imported_content_hash = blake3::hash(&imported).to_hex().to_string();
    let imported_tag = sender.blobs.blobs().add_bytes(imported.clone()).await?;
    let imported_storage = Storage::memory()?;
    imported_storage.put_imported_file_object(
        &imported_content_hash,
        imported.len() as u64,
        "text/plain",
        "imported.txt",
        &imported_tag.hash.to_string(),
        "test-importer",
    )?;
    let imported_prepared = prepare_imported_file(
        &imported_storage,
        &sender.blobs,
        &imported_content_hash,
        Some(&imported_content_hash),
        Some(imported.len() as u64),
    )
    .await?;
    assert_eq!(imported_prepared.content_hash, imported_content_hash);
    download_and_verify(
        &receiver,
        &sender,
        imported_tag.hash,
        "imported.txt",
        &imported,
    )
    .await?;

    // Referenced object: preparation re-reads the source path, verifies it,
    // and imports the bytes into the serving blob store before download.
    let source_dir = TempDir::new()?;
    let referenced = b"referenced source bytes\n".to_vec();
    let source_path = source_dir.path().join("source.txt");
    std::fs::write(&source_path, &referenced)?;
    let referenced_content_hash = blake3::hash(&referenced).to_hex().to_string();
    let referenced_storage = Storage::memory()?;
    referenced_storage.put_file_object(
        &referenced_content_hash,
        referenced.len() as u64,
        "text/plain",
        "source.txt",
        &[],
    )?;
    let referenced_prepared = prepare_referenced_file(
        &referenced_storage,
        &sender.blobs,
        &referenced_content_hash,
        Some(&referenced_content_hash),
        Some(referenced.len() as u64),
    )
    .await?;
    assert_eq!(referenced_prepared.content_hash, referenced_content_hash);
    let referenced_hash: iroh_blobs::Hash = blake3::hash(&referenced).into();
    download_and_verify(
        &receiver,
        &sender,
        referenced_hash,
        "source.txt",
        &referenced,
    )
    .await?;

    // Duplicate content has one content hash and remains downloadable through
    // either logical file identity.
    let duplicate = b"same bytes, two logical files".to_vec();
    let first = sender.blobs.blobs().add_bytes(duplicate.clone()).await?;
    let second = sender.blobs.blobs().add_bytes(duplicate.clone()).await?;
    assert_eq!(
        first.hash, second.hash,
        "duplicate content must deduplicate"
    );
    download_and_verify(
        &receiver,
        &sender,
        first.hash,
        "duplicate-a.txt",
        &duplicate,
    )
    .await?;
    download_and_verify(
        &receiver,
        &sender,
        second.hash,
        "duplicate-b.txt",
        &duplicate,
    )
    .await?;

    sender.router.shutdown().await.unwrap();
    receiver.router.shutdown().await.unwrap();
    Ok(())
}
