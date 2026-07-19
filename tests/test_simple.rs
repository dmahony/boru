use boru_chat::{
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
use std::time::Duration;
use tempfile::TempDir;

#[tokio::test]
async fn simple_test() {
    let server_sk = SecretKey::generate();
    let server_pk = server_sk.public();
    let friend_sk = SecretKey::generate();
    let friend_pk = friend_sk.public();

    let profile_user_id = server_pk.to_string();
    let storage = Arc::new(Storage::memory().unwrap());
    storage
        .bump_manifest_revision(&profile_user_id, "initial")
        .unwrap();
    storage
        .put_file_object("abcdef01", 1024, "text/plain", "test.txt", b"data")
        .unwrap();
    storage
        .upsert_shared_file(
            "abcdef01",
            &profile_user_id,
            "meta_abcdef01",
            "test.txt",
            None,
            true,
        )
        .unwrap();

    let friends_dir = TempDir::new().unwrap();
    let mut friends = FriendsStore::empty_at(friends_dir.path());
    let fid = FriendId::from_public_key(friend_pk);
    let rec = FriendRecord {
        relationship: FriendRelationship::Friends,
        ..Default::default()
    };
    friends.upsert(fid, rec);

    let handler = CatalogueHandler::new(storage, server_sk, profile_user_id, friends);
    let ep = Endpoint::builder(presets::N0DisableRelay)
        .secret_key(SecretKey::generate())
        .relay_mode(RelayMode::Disabled)
        .bind_addr("127.0.0.1:0".parse::<std::net::SocketAddrV4>().unwrap())
        .unwrap()
        .bind()
        .await
        .unwrap();

    let router = Router::builder(ep.clone())
        .accept(CATALOGUE_ALPN, handler)
        .spawn();

    let lookup = MemoryLookup::new();
    let cli_ep = Endpoint::builder(presets::N0DisableRelay)
        .secret_key(friend_sk)
        .address_lookup(lookup.clone())
        .relay_mode(RelayMode::Disabled)
        .bind_addr("127.0.0.1:0".parse::<std::net::SocketAddrV4>().unwrap())
        .unwrap()
        .bind()
        .await
        .unwrap();
    lookup.set_endpoint_info(ep.addr());

    // Try connecting directly without the fetch_remote_catalogue
    eprintln!("--- Connecting ---");
    let conn = match tokio::time::timeout(
        Duration::from_secs(5),
        cli_ep.connect(ep.addr(), CATALOGUE_ALPN),
    )
    .await
    {
        Ok(Ok(c)) => c,
        Ok(Err(e)) => {
            eprintln!("connect error: {e}");
            return;
        }
        Err(_) => {
            eprintln!("connect timeout");
            return;
        }
    };
    eprintln!("--- Connected ---");

    match tokio::time::timeout(Duration::from_secs(5), conn.accept_bi()).await {
        Ok(Ok((mut send, mut recv))) => {
            eprintln!("--- open_bi OK ---");
            let wire_req = boru_chat::catalogue_protocol::CatalogWireRequest::new(
                boru_chat::catalogue_protocol::CatalogRequest::GetCatalogue {
                    known_revision: None,
                },
            );
            let payload = postcard::to_stdvec(&wire_req).unwrap();
            boru_chat::protocol_version::write_frame(&mut send, 1, &payload)
                .await
                .unwrap();
            send.finish().unwrap();

            let result = tokio::time::timeout(
                Duration::from_secs(5),
                boru_chat::protocol_version::read_frame(&mut recv, &[1], "catalogue"),
            )
            .await;
            match result {
                Ok(Ok(Some((_v, data)))) => eprintln!("--- Response: {} bytes ---", data.len()),
                Ok(Ok(None)) => eprintln!("--- No response (clean close) ---"),
                Ok(Err(e)) => eprintln!("--- read error: {e} ---"),
                Err(_) => eprintln!("--- read timeout ---"),
            }
        }
        Ok(Err(e)) => eprintln!("--- open_bi error: {e} ---"),
        Err(_) => eprintln!("--- open_bi timeout ---"),
    }

    drop(cli_ep);
    drop(router);
    drop(ep);
}
