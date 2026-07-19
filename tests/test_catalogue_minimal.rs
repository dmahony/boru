// Minimal test to check if CatalogueHandler works
use boru_chat::{
    catalogue_client::fetch_remote_catalogue,
    catalogue_handler::CatalogueHandler,
    friends::{FriendId, FriendRecord, FriendRelationship, FriendsStore},
    protocol_version::CATALOGUE_ALPN,
    storage::Storage,
};
use iroh::{
    address_lookup::memory::MemoryLookup, endpoint::presets, protocol::Router, Endpoint, RelayMode,
    SecretKey,
};
use std::sync::Arc;

#[tokio::main]
async fn main() {
    let server_sk = SecretKey::generate();
    let server_pk = server_sk.public();
    let friend_sk = SecretKey::generate();
    let friend_pk = friend_sk.public();

    let profile_user_id = server_pk.to_string();
    let storage = Arc::new(Storage::memory().expect("storage"));

    // Bump manifest revision + add a file
    storage
        .bump_manifest_revision(&profile_user_id, "initial")
        .expect("bump");
    storage
        .put_file_object(
            "abcdef01",
            1024,
            "application/octet-stream",
            "test.txt",
            b"data",
        )
        .expect("put");
    storage
        .upsert_shared_file(
            "abcdef01",
            &profile_user_id,
            "meta_abcdef01",
            "test.txt",
            None,
            true,
        )
        .expect("upsert");

    let friends_dir = tempfile::TempDir::new().expect("tmpdir");
    let mut friends = FriendsStore::empty_at(friends_dir.path());
    let fid = FriendId::from_public_key(friend_pk);
    let rec = FriendRecord {
        relationship: FriendRelationship::Friends,
        ..FriendRecord::default()
    };
    friends.upsert(fid, rec);

    let handler = CatalogueHandler::new(storage, server_sk, profile_user_id, friends);
    let ep = Endpoint::builder(presets::N0DisableRelay)
        .secret_key(SecretKey::generate())
        .relay_mode(RelayMode::Disabled)
        .bind_addr("127.0.0.1:0".parse::<std::net::SocketAddr>().expect("addr"))
        .expect("bind addr")
        .bind()
        .await
        .expect("bind");
    let router = Router::builder(ep.clone())
        .accept(CATALOGUE_ALPN, handler)
        .spawn();

    let (cli_ep, lookup) = {
        let lookup = MemoryLookup::new();
        let ce = Endpoint::builder(presets::N0DisableRelay)
            .secret_key(friend_sk)
            .address_lookup(lookup.clone())
            .relay_mode(RelayMode::Disabled)
            .bind_addr("127.0.0.1:0".parse::<std::net::SocketAddr>().expect("addr"))
            .expect("bind addr")
            .bind()
            .await
            .expect("bind");
        (ce, lookup)
    };
    lookup.set_endpoint_info(ep.addr());

    println!("connecting...");
    match fetch_remote_catalogue(&cli_ep, server_pk, None).await {
        Ok(cat) => println!("OK: {} files, revision {}", cat.files.len(), cat.revision),
        Err(e) => println!("ERROR: {:?}", e),
    }

    drop(cli_ep);
    drop(router);
    drop(ep);
}
