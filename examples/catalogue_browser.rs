//! Browse a remote peer's file catalogue.
//!
//! Connects to the iroh relay, fetches the catalogue from a peer,
//! and prints the files found.
//!
//! Usage:
//!   cargo run --example catalogue_browser --features net -- <PEER_PUBLIC_KEY_HEX>
use std::time::Duration;

use boru_core::catalogue_client::fetch_paginated_remote_catalogue;
use iroh::{endpoint::presets, Endpoint, PublicKey, RelayMap, RelayMode, RelayUrl, SecretKey};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let peer_hex = std::env::args().nth(1).unwrap_or_else(|| {
        eprintln!("Usage: catalogue_browser <peer_public_key_hex> [relay_url]");
        std::process::exit(1);
    });

    let relay_url_str = std::env::args()
        .nth(2)
        .unwrap_or_else(|| "https://boru.chat:8443/".to_string());
    let _relay_url: RelayUrl = relay_url_str.parse()?;
    let relay_map = RelayMap::try_from_iter([relay_url_str.as_str()])?;

    let secret_key = SecretKey::generate();
    let ep = Endpoint::builder(presets::N0)
        .secret_key(secret_key)
        .alpns(vec![boru_core::protocol_version::CATALOGUE_ALPN.to_vec()])
        .relay_mode(RelayMode::Custom(relay_map))
        .bind()
        .await?;

    let peer_pk: PublicKey = peer_hex.parse()?;
    eprintln!("Local node: {}", ep.id().fmt_short());
    eprintln!("Fetching catalogue from: {peer_pk}");

    // Wait for relay connection
    tokio::time::sleep(Duration::from_secs(3)).await;

    match fetch_paginated_remote_catalogue(&ep, peer_pk, 500).await {
        Ok(catalogue) => {
            println!("CATALOGUE_OK");
            println!("files: {}", catalogue.files.len());
            for f in &catalogue.files {
                println!(
                    "  file|{}|{}|{}|{}",
                    f.content_hash, f.size_bytes, f.mime_type, f.display_name
                );
            }
        }
        Err(e) => {
            eprintln!("CATALOGUE_ERROR: {e:?}");
            std::process::exit(1);
        }
    }

    Ok(())
}
