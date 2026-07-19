//! Deterministic two-peer catalogue harness.
//!
//! The harness keeps each peer's identity, storage, contacts, endpoint, and
//! router together.  Tests can stop/restart either peer without changing its
//! identity, mutate visibility or catalogue data, and replace a server with a
//! malformed-response handler.  All networking is localhost-only and address
//! resolution is explicitly injected through `MemoryLookup`.

use std::{
    net::SocketAddr,
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc,
    },
    time::{SystemTime, UNIX_EPOCH},
};

use boru_chat::{
    catalogue_client::{
        fetch_paginated_remote_catalogue, fetch_remote_catalogue, RemoteCatalogueFetchError,
    },
    catalogue_handler::CatalogueHandler,
    catalogue_model::{RemoteSharedFile, SignedFileCatalogue},
    catalogue_protocol::{CatalogResponse, CatalogWireResponse, CataloguePage},
    friends::{FriendId, FriendRecord, FriendRelationship, FriendsStore},
    protocol_version::{
        read_frame, write_frame, CATALOGUE_ALPN, CATALOGUE_RETRIEVAL_V1,
        SUPPORTED_CATALOGUE_RETRIEVAL,
    },
    storage::Storage,
};
use iroh::{
    address_lookup::memory::MemoryLookup,
    endpoint::{presets, Connection},
    protocol::{AcceptError, ProtocolHandler, Router},
    Endpoint, PublicKey, RelayMode, SecretKey,
};
use n0_error::Result;
use tempfile::TempDir;

const LOCAL_ADDR: &str = "127.0.0.1:0";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PeerId {
    Alice,
    Bob,
}

impl PeerId {
    #[expect(dead_code)]
    fn other(self) -> Self {
        match self {
            Self::Alice => Self::Bob,
            Self::Bob => Self::Alice,
        }
    }
}

/// A persistent temporary profile and its optional running catalogue server.
#[derive(Debug)]
pub struct CataloguePeer {
    pub id: PeerId,
    pub secret_key: SecretKey,
    pub public_key: PublicKey,
    pub data_dir: TempDir,
    pub storage: Arc<Storage>,
    pub friends: FriendsStore,
    pub endpoint: Option<Endpoint>,
    pub router: Option<Router>,
    pub lookup: MemoryLookup,
    malformed_response: Option<Vec<u8>>,
    custom_handler: Option<AlternatingPageHandler>,
}

impl CataloguePeer {
    fn new(id: PeerId, key_byte: u8) -> Self {
        let mut bytes = [0u8; 32];
        bytes.fill(key_byte);
        let secret_key = SecretKey::from_bytes(&bytes);
        let data_dir = tempfile::tempdir().expect("temporary profile");
        let friends = FriendsStore::empty_at(data_dir.path());
        Self {
            id,
            public_key: secret_key.public(),
            secret_key,
            data_dir,
            storage: Arc::new(Storage::memory().expect("in-memory catalogue storage")),
            friends,
            endpoint: None,
            router: None,
            lookup: MemoryLookup::new(),
            malformed_response: None,
            custom_handler: None,
        }
    }

    pub fn is_running(&self) -> bool {
        self.endpoint.is_some()
    }

    pub fn profile_id(&self) -> String {
        self.public_key.to_string()
    }

    pub fn set_custom_handler(&mut self, handler: AlternatingPageHandler) {
        self.custom_handler = Some(handler);
    }
}

/// A two-peer, localhost-only catalogue test environment.
#[derive(Debug)]
pub struct CatalogueHarness {
    pub alice: CataloguePeer,
    pub bob: CataloguePeer,
}

impl Default for CatalogueHarness {
    fn default() -> Self {
        Self::new()
    }
}

impl CatalogueHarness {
    pub fn new() -> Self {
        Self {
            alice: CataloguePeer::new(PeerId::Alice, 0x11),
            bob: CataloguePeer::new(PeerId::Bob, 0x22),
        }
    }

    pub fn peer(&self, id: PeerId) -> &CataloguePeer {
        match id {
            PeerId::Alice => &self.alice,
            PeerId::Bob => &self.bob,
        }
    }

    fn peer_mut(&mut self, id: PeerId) -> &mut CataloguePeer {
        match id {
            PeerId::Alice => &mut self.alice,
            PeerId::Bob => &mut self.bob,
        }
    }

    /// Start both catalogue servers.  Calling this repeatedly is harmless.
    pub async fn start(&mut self) -> Result<()> {
        self.start_peer(PeerId::Alice).await?;
        self.start_peer(PeerId::Bob).await?;
        self.seed_lookups();
        Ok(())
    }

    pub async fn start_peer(&mut self, id: PeerId) -> Result<()> {
        if self.peer(id).is_running() {
            return Ok(());
        }
        let (storage, key, profile_id, friends, malformed, custom_handler, lookup) = {
            let peer = self.peer(id);
            (
                peer.storage.clone(),
                peer.secret_key.clone(),
                peer.profile_id(),
                peer.friends.clone(),
                peer.malformed_response.clone(),
                peer.custom_handler.clone(),
                peer.lookup.clone(),
            )
        };
        let endpoint = Endpoint::builder(presets::N0DisableRelay)
            .secret_key(key.clone())
            .address_lookup(lookup)
            .relay_mode(RelayMode::Disabled)
            .bind_addr(LOCAL_ADDR.parse::<SocketAddr>().unwrap())?
            .bind()
            .await?;
        let router = if let Some(handler) = custom_handler {
            Router::builder(endpoint.clone())
                .accept(CATALOGUE_ALPN, handler)
                .spawn()
        } else if let Some(payload) = malformed {
            Router::builder(endpoint.clone())
                .accept(CATALOGUE_ALPN, MalformedCatalogueHandler { payload })
                .spawn()
        } else {
            Router::builder(endpoint.clone())
                .accept(
                    CATALOGUE_ALPN,
                    CatalogueHandler::new(storage, key, profile_id, friends),
                )
                .spawn()
        };
        let peer = self.peer_mut(id);
        peer.endpoint = Some(endpoint);
        peer.router = Some(router);
        Ok(())
    }

    pub async fn stop_peer(&mut self, id: PeerId) {
        let (router, endpoint) = {
            let peer = self.peer_mut(id);
            (peer.router.take(), peer.endpoint.take())
        };
        if let Some(router) = router {
            let _ = router.shutdown().await;
        }
        if let Some(endpoint) = endpoint {
            endpoint.close().await;
        }
    }

    pub async fn restart_peer(&mut self, id: PeerId) -> Result<()> {
        self.stop_peer(id).await;
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        self.start_peer(id).await?;
        self.seed_lookups();
        Ok(())
    }

    fn seed_lookups(&self) {
        if let (Some(a), Some(b)) = (&self.alice.endpoint, &self.bob.endpoint) {
            self.alice.lookup.set_endpoint_info(b.addr());
            self.bob.lookup.set_endpoint_info(a.addr());
        }
    }

    /// Set the owner's view of the other peer, then restart its server so the
    /// handler receives the new immutable contact snapshot.
    pub async fn set_relationship(
        &mut self,
        owner: PeerId,
        other: PeerId,
        relationship: FriendRelationship,
    ) -> Result<()> {
        let other_pk = self.peer(other).public_key;
        let owner_peer = self.peer_mut(owner);
        let record = FriendRecord {
            relationship,
            ..Default::default()
        };
        owner_peer
            .friends
            .upsert(FriendId::from_public_key(other_pk), record);
        if owner_peer.is_running() {
            self.restart_peer(owner).await?;
        }
        Ok(())
    }

    pub fn add_file(&self, owner: PeerId, hash: &str, filename: &str) -> Result<u64> {
        let peer = self.peer(owner);
        peer.storage
            .put_file_object(hash, 1024, "text/plain", filename, b"catalogue-test")?;
        peer.storage
            .upsert_shared_file(hash, &peer.profile_id(), hash, filename, None, true)?;
        peer.storage
            .bump_manifest_revision(&peer.profile_id(), "harness update")
    }

    /// Add an explicit permission for one peer on a shared file.
    pub fn grant_permission(
        &self,
        owner: PeerId,
        grantee: PeerId,
        hash: &str,
        permission: &str,
    ) -> Result<()> {
        let owner_peer = self.peer(owner);
        owner_peer.storage.grant_permission(
            hash,
            &owner_peer.profile_id(),
            &self.peer(grantee).profile_id(),
            permission,
            None,
        )
    }

    pub async fn fetch(
        &self,
        from: PeerId,
        owner: PeerId,
    ) -> std::result::Result<
        boru_chat::catalogue_model::SignedFileCatalogue,
        RemoteCatalogueFetchError,
    > {
        let client = self.peer(from);
        let server = self.peer(owner);
        let server_endpoint = server.endpoint.as_ref().expect("owner is running");
        // Use a fresh transport for every fetch.  This deliberately avoids
        // stale QUIC address/connection state after the owner is restarted,
        // while retaining the caller's stable cryptographic identity.
        let lookup = MemoryLookup::new();
        lookup.set_endpoint_info(server_endpoint.addr());
        let client_endpoint = Endpoint::builder(presets::N0DisableRelay)
            .secret_key(client.secret_key.clone())
            .address_lookup(lookup)
            .relay_mode(RelayMode::Disabled)
            .bind_addr(LOCAL_ADDR.parse::<SocketAddr>().unwrap())
            .expect("bind catalogue client")
            .bind()
            .await
            .map_err(|e| RemoteCatalogueFetchError::ConnectionFailed {
                details: format!("bind client: {e}"),
            })?;
        let result = fetch_remote_catalogue(&client_endpoint, server.public_key, None).await;
        client_endpoint.close().await;
        result
    }

    pub async fn fetch_known_revision(
        &self,
        from: PeerId,
        owner: PeerId,
        known_revision: Option<u64>,
    ) -> std::result::Result<SignedFileCatalogue, RemoteCatalogueFetchError> {
        let client = self.peer(from);
        let server = self.peer(owner);
        let lookup = MemoryLookup::new();
        lookup.set_endpoint_info(server.endpoint.as_ref().expect("owner is running").addr());
        let client_endpoint = Endpoint::builder(presets::N0DisableRelay)
            .secret_key(client.secret_key.clone())
            .address_lookup(lookup)
            .relay_mode(RelayMode::Disabled)
            .bind_addr(LOCAL_ADDR.parse::<SocketAddr>().unwrap())
            .expect("bind catalogue client")
            .bind()
            .await
            .map_err(|e| RemoteCatalogueFetchError::ConnectionFailed {
                details: format!("bind client: {e}"),
            })?;
        let result =
            fetch_remote_catalogue(&client_endpoint, server.public_key, known_revision).await;
        client_endpoint.close().await;
        result
    }

    pub async fn fetch_paginated(
        &self,
        from: PeerId,
        owner: PeerId,
        page_size: u32,
    ) -> std::result::Result<SignedFileCatalogue, RemoteCatalogueFetchError> {
        let client = self.peer(from);
        let server = self.peer(owner);
        let lookup = MemoryLookup::new();
        lookup.set_endpoint_info(server.endpoint.as_ref().expect("owner is running").addr());
        let client_endpoint = Endpoint::builder(presets::N0DisableRelay)
            .secret_key(client.secret_key.clone())
            .address_lookup(lookup)
            .relay_mode(RelayMode::Disabled)
            .bind_addr(LOCAL_ADDR.parse::<SocketAddr>().unwrap())
            .expect("bind catalogue client")
            .bind()
            .await
            .map_err(|e| RemoteCatalogueFetchError::ConnectionFailed {
                details: format!("bind client: {e}"),
            })?;
        let result =
            fetch_paginated_remote_catalogue(&client_endpoint, server.public_key, page_size).await;
        client_endpoint.close().await;
        result
    }

    pub fn set_malformed_response(&mut self, owner: PeerId, payload: Vec<u8>) {
        self.peer_mut(owner).malformed_response = Some(payload);
    }

    pub fn clear_malformed_response(&mut self, owner: PeerId) {
        self.peer_mut(owner).malformed_response = None;
    }

    pub async fn shutdown(&mut self) {
        self.stop_peer(PeerId::Bob).await;
        self.stop_peer(PeerId::Alice).await;
    }
}

#[derive(Clone)]
struct MalformedCatalogueHandler {
    payload: Vec<u8>,
}

impl std::fmt::Debug for MalformedCatalogueHandler {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MalformedCatalogueHandler")
            .field("payload_len", &self.payload.len())
            .finish()
    }
}

impl ProtocolHandler for MalformedCatalogueHandler {
    async fn accept(&self, connection: Connection) -> std::result::Result<(), AcceptError> {
        let (mut send, mut recv) = connection.accept_bi().await?;
        let _ = read_frame(&mut recv, SUPPORTED_CATALOGUE_RETRIEVAL, "catalogue").await;
        write_frame(&mut send, CATALOGUE_RETRIEVAL_V1, &self.payload)
            .await
            .map_err(|e| AcceptError::from(n0_error::anyerr!("write malformed response: {e}")))?;
        send.finish()?;
        Ok(())
    }
}

#[tokio::test]
async fn catalogue_harness_is_deterministic_and_supports_visibility_changes() -> Result<()> {
    let mut harness = CatalogueHarness::new();
    let alice_key = harness.alice.public_key;
    let bob_key = harness.bob.public_key;
    harness.start().await?;
    assert_eq!(harness.alice.public_key, alice_key);
    assert_eq!(harness.bob.public_key, bob_key);

    harness
        .set_relationship(PeerId::Alice, PeerId::Bob, FriendRelationship::Friends)
        .await?;
    harness.add_file(PeerId::Alice, "hash-visible", "visible.txt")?;
    let visible = harness
        .fetch(PeerId::Bob, PeerId::Alice)
        .await
        .map_err(|e| n0_error::anyerr!("fetch visible catalogue: {e}"))?;
    assert_eq!(visible.files.len(), 1);

    harness
        .set_relationship(PeerId::Alice, PeerId::Bob, FriendRelationship::NotFriend)
        .await?;
    let hidden = harness
        .fetch(PeerId::Bob, PeerId::Alice)
        .await
        .map_err(|e| n0_error::anyerr!("fetch hidden catalogue: {e}"))?;
    assert!(hidden.files.is_empty());
    harness.shutdown().await;
    Ok(())
}

#[tokio::test]
async fn catalogue_harness_supports_stop_restart_and_updates() -> Result<()> {
    let mut harness = CatalogueHarness::new();
    harness.start().await?;
    harness
        .set_relationship(PeerId::Bob, PeerId::Alice, FriendRelationship::Friends)
        .await?;
    harness.add_file(PeerId::Bob, "before", "before.txt")?;
    let first = harness
        .fetch(PeerId::Alice, PeerId::Bob)
        .await
        .map_err(|e| n0_error::anyerr!("fetch initial catalogue: {e}"))?;
    let revision = first.revision;
    harness.stop_peer(PeerId::Bob).await;
    assert!(!harness.peer(PeerId::Bob).is_running());
    harness.restart_peer(PeerId::Bob).await?;
    assert_eq!(
        harness.bob.public_key,
        SecretKey::from_bytes(&[0x22; 32]).public()
    );
    harness.add_file(PeerId::Bob, "after", "after.txt")?;
    let second = harness
        .fetch(PeerId::Alice, PeerId::Bob)
        .await
        .map_err(|e| n0_error::anyerr!("fetch updated catalogue: {e}"))?;
    assert!(second.revision > revision);
    assert_eq!(second.files.len(), 2);
    harness.shutdown().await;
    Ok(())
}

#[tokio::test]
async fn catalogue_harness_covers_every_permission_visibility_rule() -> Result<()> {
    let mut harness = CatalogueHarness::new();
    harness.start().await?;

    // Accepted contacts see ordinary offered files.
    harness
        .set_relationship(PeerId::Alice, PeerId::Bob, FriendRelationship::Friends)
        .await?;
    harness.add_file(PeerId::Alice, "contact", "contact.txt")?;
    let contact = harness
        .fetch(PeerId::Bob, PeerId::Alice)
        .await
        .map_err(|e| n0_error::anyerr!("fetch accepted-contact catalogue: {e}"))?;
    assert!(contact.files.iter().any(|f| f.content_hash == "contact"));

    // Non-contacts do not see files covered only by the contacts-only default.
    harness
        .set_relationship(PeerId::Alice, PeerId::Bob, FriendRelationship::NotFriend)
        .await?;
    let non_contact = harness
        .fetch(PeerId::Bob, PeerId::Alice)
        .await
        .map_err(|e| n0_error::anyerr!("fetch non-contact catalogue: {e}"))?;
    assert!(non_contact.files.is_empty());

    // An explicit grant selects a peer even when that peer is not a contact.
    harness.add_file(PeerId::Alice, "selected", "selected.txt")?;
    harness.grant_permission(PeerId::Alice, PeerId::Bob, "selected", "read")?;
    let selected = harness
        .fetch(PeerId::Bob, PeerId::Alice)
        .await
        .map_err(|e| n0_error::anyerr!("fetch explicitly granted catalogue: {e}"))?;
    assert_eq!(selected.files.len(), 1);
    assert_eq!(selected.files[0].content_hash, "selected");

    // Explicit denial overrides the accepted-contact default.
    harness
        .set_relationship(PeerId::Alice, PeerId::Bob, FriendRelationship::Friends)
        .await?;
    harness.add_file(PeerId::Alice, "denied", "denied.txt")?;
    harness.grant_permission(PeerId::Alice, PeerId::Bob, "denied", "deny")?;
    let denied = harness
        .fetch(PeerId::Bob, PeerId::Alice)
        .await
        .map_err(|e| n0_error::anyerr!("fetch explicitly denied catalogue: {e}"))?;
    assert!(!denied.files.iter().any(|f| f.content_hash == "denied"));

    // Disabled offers and missing backing objects are both hidden.
    let alice_profile = harness.peer(PeerId::Alice).profile_id();
    harness.add_file(PeerId::Alice, "disabled", "disabled.txt")?;
    harness.peer(PeerId::Alice).storage.upsert_shared_file(
        "disabled",
        &alice_profile,
        "disabled",
        "disabled.txt",
        None,
        false,
    )?;
    // The storage foreign key rejects a dangling reference before it can
    // enter the catalogue; verify the end-to-end view remains clean.
    let missing_result = harness.peer(PeerId::Alice).storage.upsert_shared_file(
        "missing",
        &alice_profile,
        "missing",
        "missing.txt",
        None,
        true,
    );
    assert!(
        missing_result.is_err(),
        "dangling reference must be rejected"
    );
    let unavailable = harness
        .fetch(PeerId::Bob, PeerId::Alice)
        .await
        .map_err(|e| n0_error::anyerr!("fetch unavailable catalogue: {e}"))?;
    assert!(!unavailable
        .files
        .iter()
        .any(|f| matches!(f.content_hash.as_str(), "disabled" | "missing")));

    // Blocked peers are denied at the protocol boundary, rather than getting
    // an indistinguishable empty catalogue.
    harness
        .set_relationship(PeerId::Alice, PeerId::Bob, FriendRelationship::Blocked)
        .await?;
    let blocked = harness.fetch(PeerId::Bob, PeerId::Alice).await;
    assert!(matches!(
        blocked,
        Err(RemoteCatalogueFetchError::PermissionDenied)
    ));

    harness.shutdown().await;
    Ok(())
}

#[expect(dead_code)]
fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock after epoch")
        .as_millis() as u64
}

#[tokio::test]
async fn catalogue_cache_first_fetch_not_modified_and_revision_replacement() -> Result<()> {
    let mut harness = CatalogueHarness::new();
    harness.start().await?;
    harness
        .set_relationship(PeerId::Alice, PeerId::Bob, FriendRelationship::Friends)
        .await?;
    harness.add_file(PeerId::Alice, "removed", "removed.txt")?;
    let first = harness
        .fetch(PeerId::Bob, PeerId::Alice)
        .await
        .map_err(|e| n0_error::anyerr!("first fetch: {e}"))?;
    let receiver = Storage::memory()?;
    receiver.replace_remote_catalogue(&first)?;
    let unchanged = harness
        .fetch_known_revision(PeerId::Bob, PeerId::Alice, Some(first.revision))
        .await;
    assert!(matches!(
        unchanged,
        Err(RemoteCatalogueFetchError::NotModified)
    ));
    harness.peer(PeerId::Alice).storage.upsert_shared_file(
        "removed",
        &harness.peer(PeerId::Alice).profile_id(),
        "removed",
        "removed.txt",
        None,
        false,
    )?;
    harness.add_file(PeerId::Alice, "kept", "kept.txt")?;
    let second = harness
        .fetch_known_revision(PeerId::Bob, PeerId::Alice, Some(first.revision))
        .await
        .map_err(|e| n0_error::anyerr!("refresh fetch: {e}"))?;
    assert!(second.revision > first.revision);
    receiver.replace_remote_catalogue(&second)?;
    let owner = harness.alice.public_key;
    let remote_files = receiver.get_remote_shared_files(&owner)?;
    assert!(
        !remote_files.iter().any(|f| f.content_hash == "removed"),
        "removed file should not appear in remote files"
    );
    let kept_file = remote_files
        .iter()
        .find(|f| f.content_hash == "kept")
        .expect("kept file should be in remote files");
    assert_eq!(kept_file.content_hash, "kept");
    let meta = receiver.get_remote_catalogue_meta(&owner)?.unwrap();
    assert_eq!(meta.revision, second.revision);
    harness.shutdown().await;
    Ok(())
}

/// Store a remote peer's catalogue and verify it can be read back.
#[tokio::test]
async fn catalogue_cache_stale_and_offline_states_retain_cached_profile() -> Result<()> {
    let mut harness = CatalogueHarness::new();
    harness.start().await?;
    harness
        .set_relationship(PeerId::Alice, PeerId::Bob, FriendRelationship::Friends)
        .await?;
    harness.add_file(PeerId::Alice, "cached", "cached.txt")?;
    let catalogue = harness
        .fetch(PeerId::Bob, PeerId::Alice)
        .await
        .map_err(|e| n0_error::anyerr!("cache fetch: {e}"))?;
    let receiver = Storage::memory()?;
    receiver.replace_remote_catalogue(&catalogue)?;
    let owner = harness.alice.public_key;
    let remote_files = receiver.get_remote_shared_files(&owner)?;
    assert!(
        remote_files.iter().any(|f| f.content_hash == "cached"),
        "cached file should appear in remote files"
    );
    let meta = receiver.get_remote_catalogue_meta(&owner)?.unwrap();
    assert_eq!(meta.revision, catalogue.revision);
    harness.shutdown().await;
    Ok(())
}

/// Store a remote peer's catalogue and verify that calling
/// `replace_remote_catalogue` again with a newer revision updates the
/// `fetched_at_ms` timestamp.
#[tokio::test]
async fn catalogue_cache_timestamp_updates_on_replacement() -> Result<()> {
    let mut harness = CatalogueHarness::new();
    harness.start().await?;
    harness
        .set_relationship(PeerId::Alice, PeerId::Bob, FriendRelationship::Friends)
        .await?;
    harness.add_file(PeerId::Alice, "v1", "v1.txt")?;
    let first = harness
        .fetch(PeerId::Bob, PeerId::Alice)
        .await
        .map_err(|e| n0_error::anyerr!("first fetch: {e}"))?;
    let owner = harness.alice.public_key;

    // --- initial store ---
    let receiver = Storage::memory()?;
    receiver.replace_remote_catalogue(&first)?;
    let meta_before = receiver
        .get_remote_catalogue_meta(&owner)?
        .expect("meta after first store");
    assert_eq!(
        meta_before.revision, first.revision,
        "cache revision matches first catalogue"
    );

    // --- second store with newer revision ---
    tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    harness.add_file(PeerId::Alice, "v2", "v2.txt")?;
    let second = harness
        .fetch(PeerId::Bob, PeerId::Alice)
        .await
        .map_err(|e| n0_error::anyerr!("second fetch: {e}"))?;
    assert!(
        second.revision > first.revision,
        "second catalogue has higher revision"
    );
    receiver.replace_remote_catalogue(&second)?;

    let meta_after = receiver
        .get_remote_catalogue_meta(&owner)?
        .expect("meta after second store");
    assert_eq!(
        meta_after.revision, second.revision,
        "cache revision bumped to second catalogue"
    );
    assert!(
        meta_after.fetched_at_ms > meta_before.fetched_at_ms,
        "fetched_at_ms increased after replacement"
    );

    // Both files should appear — the snapshots include all currently
    // shared files at fetch time.
    let files = receiver.get_remote_shared_files(&owner)?;
    assert!(
        files.iter().any(|f| f.content_hash == "v1"),
        "v1 file present"
    );
    assert!(
        files.iter().any(|f| f.content_hash == "v2"),
        "v2 file present"
    );

    harness.shutdown().await;
    Ok(())
}

/// Store a cached remote catalogue, then stop the owning peer (simulating
/// offline).  Verify the cached data remains intact and readable, and
/// that a failed live fetch does not modify the cache.
#[tokio::test]
async fn catalogue_cache_survives_offline_peer() -> Result<()> {
    let mut harness = CatalogueHarness::new();
    harness.start().await?;
    harness
        .set_relationship(PeerId::Alice, PeerId::Bob, FriendRelationship::Friends)
        .await?;
    harness.add_file(PeerId::Alice, "offline-test", "offline.txt")?;
    let catalogue = harness
        .fetch(PeerId::Bob, PeerId::Alice)
        .await
        .map_err(|e| n0_error::anyerr!("initial fetch: {e}"))?;
    let owner = harness.alice.public_key;

    let receiver = Storage::memory()?;
    receiver.replace_remote_catalogue(&catalogue)?;

    // Snapshot the cached revision and file count before going offline.
    let meta_before = receiver
        .get_remote_catalogue_meta(&owner)?
        .expect("meta before offline");
    let files_before = receiver.get_remote_shared_files(&owner)?;
    assert_eq!(files_before.len(), 1);

    // Stop the owner peer — this makes the remote endpoint unreachable.
    harness.stop_peer(PeerId::Alice).await;
    assert!(!harness.peer(PeerId::Alice).is_running());

    // The cached data must still be readable without corruption.
    let meta_after = receiver
        .get_remote_catalogue_meta(&owner)?
        .expect("meta after offline");
    assert_eq!(
        meta_after.revision, meta_before.revision,
        "revision unchanged while offline"
    );
    assert_eq!(
        meta_after.fetched_at_ms, meta_before.fetched_at_ms,
        "fetched_at_ms unchanged while offline"
    );

    let files_after = receiver.get_remote_shared_files(&owner)?;
    assert_eq!(
        files_after.len(),
        files_before.len(),
        "file count unchanged while offline"
    );
    assert!(
        files_after.iter().any(|f| f.content_hash == "offline-test"),
        "offline file still present"
    );

    // Attempt a live fetch via a fresh endpoint -> should fail because
    // Alice is offline.  Build the client ourselves so we don't hit the
    // harness fetch's "owner is running" expectation.
    let client_pk = harness.bob.secret_key.clone();
    let server_pk = harness.alice.public_key;
    let client_ep = Endpoint::builder(presets::N0DisableRelay)
        .secret_key(client_pk)
        .address_lookup(MemoryLookup::new()) // no address info -> cannot connect
        .relay_mode(RelayMode::Disabled)
        .bind_addr(LOCAL_ADDR.parse::<SocketAddr>().unwrap())
        .unwrap()
        .bind()
        .await
        .unwrap();
    let fetch_result = fetch_remote_catalogue(&client_ep, server_pk, None).await;
    client_ep.close().await;
    // Without relay and with no lookup info, the connection must fail.
    // The exact error depends on how iroh's connect times out — it may
    // be ConnectionFailed (DNS/transport) or Timeout (no address found).
    assert!(
        matches!(
            fetch_result,
            Err(RemoteCatalogueFetchError::ConnectionFailed { .. }
                | RemoteCatalogueFetchError::Timeout)
        ),
        "live fetch from offline peer must fail, got: {fetch_result:?}"
    );

    // Verify cached data was NOT modified by the failed fetch attempt.
    let meta_after_fail = receiver
        .get_remote_catalogue_meta(&owner)?
        .expect("meta after failed fetch");
    assert_eq!(
        meta_after_fail.revision, meta_before.revision,
        "revision unchanged after failed fetch"
    );
    assert_eq!(
        meta_after_fail.fetched_at_ms, meta_before.fetched_at_ms,
        "fetched_at_ms unchanged after failed fetch"
    );

    let files_after_fail = receiver.get_remote_shared_files(&owner)?;
    assert_eq!(
        files_after_fail.len(),
        files_before.len(),
        "file count unchanged after failed fetch"
    );

    harness.shutdown().await;
    Ok(())
}

/// Store the same peer's catalogue twice and verify that offline reads
/// do not corrupt, delete, or silently rewrite the cached revision.
#[tokio::test]
async fn catalogue_cache_multiple_revisions_then_offline_read() -> Result<()> {
    let mut harness = CatalogueHarness::new();
    harness.start().await?;
    harness
        .set_relationship(PeerId::Alice, PeerId::Bob, FriendRelationship::Friends)
        .await?;
    harness.add_file(PeerId::Alice, "persist", "persist.txt")?;
    let v1 = harness
        .fetch(PeerId::Bob, PeerId::Alice)
        .await
        .map_err(|e| n0_error::anyerr!("v1 fetch: {e}"))?;
    let owner = harness.alice.public_key;

    let receiver = Storage::memory()?;
    receiver.replace_remote_catalogue(&v1)?;

    // Second revision adds another file (both remain).
    harness.add_file(PeerId::Alice, "evolved", "evolved.txt")?;
    let v2 = harness
        .fetch(PeerId::Bob, PeerId::Alice)
        .await
        .map_err(|e| n0_error::anyerr!("v2 fetch: {e}"))?;
    receiver.replace_remote_catalogue(&v2)?;

    let meta = receiver
        .get_remote_catalogue_meta(&owner)?
        .expect("meta after v2");
    assert_eq!(meta.revision, v2.revision, "cache holds latest revision");

    // Both files appear because the server included them at v2.
    let files = receiver.get_remote_shared_files(&owner)?;
    let hashes: Vec<&str> = files.iter().map(|f| f.content_hash.as_str()).collect();
    assert!(hashes.contains(&"persist"), "v1 file still present");
    assert!(hashes.contains(&"evolved"), "v2 file present");

    // Stop Alice — cached data must remain intact.
    harness.stop_peer(PeerId::Alice).await;

    let meta_offline = receiver
        .get_remote_catalogue_meta(&owner)?
        .expect("meta offline");
    assert_eq!(
        meta_offline.revision, v2.revision,
        "revision preserved through offline"
    );

    let files_offline = receiver.get_remote_shared_files(&owner)?;
    assert_eq!(
        files_offline.len(),
        files.len(),
        "file count preserved through offline"
    );

    harness.shutdown().await;
    Ok(())
}

#[tokio::test]
async fn catalogue_revision_change_during_paginated_fetch_is_not_cached() -> Result<()> {
    let mut harness = CatalogueHarness::new();
    harness.start().await?;
    let payload = postcard::to_stdvec(&CatalogWireResponse::new(
        CatalogResponse::RevisionChanged { new_revision: 77 },
    ))
    .map_err(|e| n0_error::anyerr!("encode revision response: {e}"))?;
    harness.set_malformed_response(PeerId::Alice, payload);
    harness.restart_peer(PeerId::Alice).await?;
    let result = harness.fetch_paginated(PeerId::Bob, PeerId::Alice, 1).await;
    assert!(matches!(
        result,
        Err(RemoteCatalogueFetchError::ProtocolError { .. })
    ));
    harness.shutdown().await;
    Ok(())
}

#[tokio::test]
async fn catalogue_invalid_signature_is_rejected_before_cache_replacement() -> Result<()> {
    let mut harness = CatalogueHarness::new();
    harness.start().await?;
    harness
        .set_relationship(PeerId::Alice, PeerId::Bob, FriendRelationship::Friends)
        .await?;
    harness.add_file(PeerId::Alice, "signed", "signed.txt")?;
    let valid = harness
        .fetch(PeerId::Bob, PeerId::Alice)
        .await
        .map_err(|e| n0_error::anyerr!("valid fetch: {e}"))?;
    let mut tampered = valid.clone();
    tampered.files[0].display_name = "tampered.txt".to_string();
    assert!(tampered.verify().is_err());
    let payload = postcard::to_stdvec(&CatalogWireResponse::new(CatalogResponse::SignedCatalogue(
        tampered,
    )))
    .map_err(|e| n0_error::anyerr!("encode signature response: {e}"))?;
    harness.set_malformed_response(PeerId::Alice, payload);
    harness.restart_peer(PeerId::Alice).await?;
    let result = harness.fetch(PeerId::Bob, PeerId::Alice).await;
    assert!(matches!(
        result,
        Err(RemoteCatalogueFetchError::ProtocolError { .. })
    ));
    harness.shutdown().await;
    Ok(())
}

/// A handler that alternates between two pre-built wire-format responses
/// on successive connections.  Used to simulate a revision change across
/// paginated catalogue page fetches.
#[derive(Clone)]
struct AlternatingPageHandler {
    payload_a: Vec<u8>,
    payload_b: Vec<u8>,
    counter: Arc<AtomicU64>,
}

impl std::fmt::Debug for AlternatingPageHandler {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AlternatingPageHandler")
            .field("payload_a_len", &self.payload_a.len())
            .field("payload_b_len", &self.payload_b.len())
            .field("counter", &self.counter.load(Ordering::SeqCst))
            .finish()
    }
}

impl ProtocolHandler for AlternatingPageHandler {
    async fn accept(&self, connection: Connection) -> std::result::Result<(), AcceptError> {
        let (mut send, mut recv) = connection.accept_bi().await
            .map_err(|e| AcceptError::from(n0_error::anyerr!("accept_bi: {e}")))?;
        let request = read_frame(&mut recv, SUPPORTED_CATALOGUE_RETRIEVAL, "catalogue").await;
        match request {
            Ok(_) => {
                let count = self.counter.fetch_add(1, Ordering::SeqCst);
                let payload = if count % 2 == 0 {
                    &self.payload_a
                } else {
                    &self.payload_b
                };
                write_frame(&mut send, CATALOGUE_RETRIEVAL_V1, payload)
                    .await
                    .map_err(|e| AcceptError::from(n0_error::anyerr!("write: {e}")))?;
                let _ = send.finish();
            }
            Err(e) => {
                eprintln!("AlternatingPageHandler: read_frame error: {e}");
            }
        }
        Ok(())
    }
}

/// Simulate a revision change mid-pagination.
///
/// The handler returns the first page with revision=1 and a cursor pointing
/// to a second page, then the second page with revision=2.  The client's
/// pagination logic must detect the revision mismatch and refuse to assemble
/// a mixed-revision catalogue.
#[tokio::test]
async fn pagination_detects_revision_change_across_pages() -> Result<()> {
    // Build page data.
    let file1 = RemoteSharedFile::new("hash-a", "file-a", None, 100, "text/plain", None, 1);
    let file2 = RemoteSharedFile::new("hash-b", "file-b", None, 100, "text/plain", None, 1);

    // Page 1: revision=1, cursor to page 2.
    let page1 = CataloguePage {
        revision: 1,
        items: vec![file1.clone()],
        next_cursor: Some("page2-cursor".to_string()),
    };
    let page2 = CataloguePage {
        revision: 2, // ← different revision!
        items: vec![file2],
        next_cursor: None,
    };

    let payload_a = postcard::to_stdvec(&CatalogWireResponse::new(CatalogResponse::CataloguePage(
        page1,
    )))
    .map_err(|e| n0_error::anyerr!("encode page1: {e}"))?;

    let payload_b = postcard::to_stdvec(&CatalogWireResponse::new(CatalogResponse::CataloguePage(
        page2,
    )))
    .map_err(|e| n0_error::anyerr!("encode page2: {e}"))?;

    // Build a harness where Alice uses the alternating handler.
    let mut harness = CatalogueHarness::new();
    let handler = AlternatingPageHandler {
        payload_a,
        payload_b,
        counter: Arc::new(AtomicU64::new(0)),
    };
    harness.alice.set_custom_handler(handler);

    // Start both peers through the harness — this picks up Alice's custom handler.
    harness.start().await?;

    // Fetch paginated with page_size=1 so we get 2 pages →
    // page 1 (rev 1) → page 2 (rev 2 → mismatch).
    let result = harness.fetch_paginated(PeerId::Bob, PeerId::Alice, 1).await;

    assert!(
        matches!(
            result,
            Err(RemoteCatalogueFetchError::RevisionChanged { new_revision: 2 })
        ),
        "expected RevisionChanged(2), got {result:?}"
    );

    harness.shutdown().await;
    Ok(())
}

/// Verify that an invalid catalogue signature preserves an existing valid cache.
///
/// 1. Fetch a valid catalogue and cache it via `replace_remote_catalogue`.
/// 2. Replace the server with a handler returning a tampered (invalid-signature)
///    signed catalogue.
/// 3. Attempt a fetch (must fail with a signature-related error).
/// 4. Verify the original valid cache is still intact — revision, file count,
///    and file content are unchanged.
#[tokio::test]
async fn invalid_signature_preserves_existing_valid_cache() -> Result<()> {
    let mut harness = CatalogueHarness::new();
    harness.start().await?;
    harness
        .set_relationship(PeerId::Alice, PeerId::Bob, FriendRelationship::Friends)
        .await?;
    harness.add_file(PeerId::Alice, "cache-me", "cache-me.txt")?;

    // Step 1: fetch valid catalogue and cache it.
    let valid = harness
        .fetch(PeerId::Bob, PeerId::Alice)
        .await
        .map_err(|e| n0_error::anyerr!("valid fetch: {e}"))?;
    let owner = harness.alice.public_key;
    let receiver = Storage::memory()?;
    receiver.replace_remote_catalogue(&valid)?;

    let meta_before = receiver
        .get_remote_catalogue_meta(&owner)?
        .expect("meta before tampered fetch");
    let files_before = receiver.get_remote_shared_files(&owner)?;
    assert_eq!(
        files_before.len(),
        1,
        "exactly one cached file before tampered fetch"
    );
    assert!(
        files_before.iter().any(|f| f.content_hash == "cache-me"),
        "cached file content before tampered fetch"
    );

    // Step 2: replace Alice's handler with a tampered catalogue response.
    let valid_clone = valid.clone();
    let mut tampered = valid_clone;
    tampered.files[0].display_name = "tampered.txt".to_string();
    assert!(
        tampered.verify().is_err(),
        "tampered catalogue must fail verify"
    );
    let payload = postcard::to_stdvec(&CatalogWireResponse::new(CatalogResponse::SignedCatalogue(
        tampered,
    )))
    .map_err(|e| n0_error::anyerr!("encode tampered: {e}"))?;
    harness.set_malformed_response(PeerId::Alice, payload);
    harness.restart_peer(PeerId::Alice).await?;

    // Step 3: attempt fetch — must fail with signature/protocol error.
    let result = harness.fetch(PeerId::Bob, PeerId::Alice).await;
    assert!(result.is_err(), "tampered catalogue fetch must fail");
    assert!(
        matches!(
            result,
            Err(RemoteCatalogueFetchError::ProtocolError { .. })
                | Err(RemoteCatalogueFetchError::SignatureInvalid { .. })
        ),
        "expected ProtocolError or SignatureInvalid, got {result:?}"
    );

    // Step 4: verify the original valid cache is entirely intact.
    let meta_after = receiver
        .get_remote_catalogue_meta(&owner)?
        .expect("meta after failed fetch");
    assert_eq!(
        meta_after.revision, meta_before.revision,
        "revision unchanged after failed fetch"
    );
    assert_eq!(
        meta_after.fetched_at_ms, meta_before.fetched_at_ms,
        "fetched_at_ms unchanged after failed fetch"
    );

    let files_after = receiver.get_remote_shared_files(&owner)?;
    assert_eq!(
        files_after.len(),
        files_before.len(),
        "file count unchanged after failed fetch"
    );
    assert!(
        files_after.iter().any(|f| f.content_hash == "cache-me"),
        "cached file content unchanged after failed fetch"
    );
    assert!(
        !files_after
            .iter()
            .any(|f| f.display_filename == "tampered.txt"),
        "tampered file data must NOT appear in cache"
    );

    harness.shutdown().await;
    Ok(())
}
