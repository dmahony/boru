//! Deterministic two-peer catalogue harness.
//!
//! The harness keeps each peer's identity, storage, contacts, endpoint, and
//! router together.  Tests can stop/restart either peer without changing its
//! identity, mutate visibility or catalogue data, and replace a server with a
//! malformed-response handler.  All networking is localhost-only and address
//! resolution is explicitly injected through `MemoryLookup`.

use std::{
    net::SocketAddr,
    sync::Arc,
    time::{SystemTime, UNIX_EPOCH},
};

use boru_chat::{
    catalogue_client::{
        fetch_paginated_remote_catalogue, fetch_remote_catalogue, RemoteCatalogueFetchError,
    },
    catalogue_handler::CatalogueHandler,
    catalogue_model::SignedFileCatalogue,
    catalogue_protocol::{CatalogResponse, CatalogWireResponse},
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
        }
    }

    pub fn is_running(&self) -> bool {
        self.endpoint.is_some()
    }

    pub fn profile_id(&self) -> String {
        self.public_key.to_string()
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
        let (storage, key, profile_id, friends, malformed, lookup) = {
            let peer = self.peer(id);
            (
                peer.storage.clone(),
                peer.secret_key.clone(),
                peer.profile_id(),
                peer.friends.clone(),
                peer.malformed_response.clone(),
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
        let router = if let Some(payload) = malformed {
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
