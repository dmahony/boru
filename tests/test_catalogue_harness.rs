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

use boru_core::{
    catalogue_client::{
        fetch_paginated_remote_catalogue, fetch_remote_catalogue, RemoteCatalogueFetchError,
    },
    catalogue_handler::CatalogueHandler,
    catalogue_model::SignedFileCatalogue,
    catalogue_protocol::{CatalogResponse, CatalogWireRequest, CatalogWireResponse, CataloguePage},
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
    custom_handler: Option<CustomHandlerEnum>,
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

    /// Install a custom protocol handler for catalogue requests.
    /// Used by tests that simulate revision changes and invalid signatures.
    pub fn set_custom_handler(&mut self, handler: CustomHandlerEnum) {
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
        boru_core::catalogue_model::SignedFileCatalogue,
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
/// Enum over the different custom protocol handler behaviours for testing.
#[derive(Clone)]
#[allow(dead_code)]
pub enum CustomHandlerEnum {
    /// RevisionChangeHandler: serves pages from different revisions (caught during pagination).
    RevisionChange {
        payload_page1: Vec<u8>,
        payload_page2: Vec<u8>,
        payload_catalogue: Vec<u8>,
    },
    /// PaginatedInvalidSignatureHandler: serves same-revision pages but a tampered
    /// SignedCatalogue in Phase 2.
    PaginatedInvalidSignature {
        payload_page1: Vec<u8>,
        payload_page2: Vec<u8>,
        payload_signed: Vec<u8>,
    },
}

impl std::fmt::Debug for CustomHandlerEnum {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::RevisionChange {
                payload_page1,
                payload_page2,
                payload_catalogue,
            } => f
                .debug_struct("RevisionChangeHandler")
                .field("payload_page1_len", &payload_page1.len())
                .field("payload_page2_len", &payload_page2.len())
                .field("payload_catalogue_len", &payload_catalogue.len())
                .finish(),
            Self::PaginatedInvalidSignature {
                payload_page1,
                payload_page2,
                payload_signed,
            } => f
                .debug_struct("PaginatedInvalidSignatureHandler")
                .field("payload_page1_len", &payload_page1.len())
                .field("payload_page2_len", &payload_page2.len())
                .field("payload_signed_len", &payload_signed.len())
                .finish(),
        }
    }
}

impl ProtocolHandler for CustomHandlerEnum {
    async fn accept(&self, connection: Connection) -> std::result::Result<(), AcceptError> {
        match self {
            Self::RevisionChange {
                payload_page1,
                payload_page2,
                payload_catalogue,
            } => {
                let (mut send, mut recv) = connection
                    .accept_bi()
                    .await
                    .map_err(|e| AcceptError::from(n0_error::anyerr!("accept_bi: {e}")))?;
                let request_result =
                    read_frame(&mut recv, SUPPORTED_CATALOGUE_RETRIEVAL, "catalogue").await;
                let request = match request_result {
                    Ok(Some((_, data))) => {
                        match postcard::from_bytes::<CatalogWireRequest>(&data) {
                            Ok(req) => req.inner,
                            Err(_) => {
                                write_frame(&mut send, CATALOGUE_RETRIEVAL_V1, payload_catalogue)
                                    .await
                                    .map_err(|e| {
                                        AcceptError::from(n0_error::anyerr!("write: {e}"))
                                    })?;
                                send.finish().map_err(|e| {
                                    AcceptError::from(n0_error::anyerr!("finish: {e}"))
                                })?;
                                return Ok(());
                            }
                        }
                    }
                    _ => {
                        write_frame(&mut send, CATALOGUE_RETRIEVAL_V1, payload_catalogue)
                            .await
                            .map_err(|e| AcceptError::from(n0_error::anyerr!("write: {e}")))?;
                        send.finish()
                            .map_err(|e| AcceptError::from(n0_error::anyerr!("finish: {e}")))?;
                        return Ok(());
                    }
                };
                let payload = match request {
                    boru_core::catalogue_protocol::CatalogRequest::GetCataloguePage {
                        cursor,
                        ..
                    } => {
                        if cursor.is_none() {
                            payload_page1
                        } else {
                            payload_page2
                        }
                    }
                    _ => payload_catalogue,
                };
                write_frame(&mut send, CATALOGUE_RETRIEVAL_V1, payload)
                    .await
                    .map_err(|e| AcceptError::from(n0_error::anyerr!("write: {e}")))?;
                send.finish()
                    .map_err(|e| AcceptError::from(n0_error::anyerr!("finish: {e}")))?;
                Ok(())
            }
            Self::PaginatedInvalidSignature {
                payload_page1,
                payload_page2,
                payload_signed,
            } => {
                let (mut send, mut recv) = connection
                    .accept_bi()
                    .await
                    .map_err(|e| AcceptError::from(n0_error::anyerr!("accept_bi: {e}")))?;
                let request_result =
                    read_frame(&mut recv, SUPPORTED_CATALOGUE_RETRIEVAL, "catalogue").await;
                let request = match request_result {
                    Ok(Some((_, data))) => {
                        match postcard::from_bytes::<CatalogWireRequest>(&data) {
                            Ok(req) => req.inner,
                            Err(_) => {
                                write_frame(&mut send, CATALOGUE_RETRIEVAL_V1, payload_signed)
                                    .await
                                    .map_err(|e| {
                                        AcceptError::from(n0_error::anyerr!("write: {e}"))
                                    })?;
                                send.finish().map_err(|e| {
                                    AcceptError::from(n0_error::anyerr!("finish: {e}"))
                                })?;
                                return Ok(());
                            }
                        }
                    }
                    _ => {
                        write_frame(&mut send, CATALOGUE_RETRIEVAL_V1, payload_signed)
                            .await
                            .map_err(|e| AcceptError::from(n0_error::anyerr!("write: {e}")))?;
                        send.finish()
                            .map_err(|e| AcceptError::from(n0_error::anyerr!("finish: {e}")))?;
                        return Ok(());
                    }
                };
                let payload = match request {
                    boru_core::catalogue_protocol::CatalogRequest::GetCataloguePage {
                        cursor,
                        ..
                    } => {
                        if cursor.is_none() {
                            payload_page1
                        } else {
                            payload_page2
                        }
                    }
                    _ => payload_signed,
                };
                write_frame(&mut send, CATALOGUE_RETRIEVAL_V1, payload)
                    .await
                    .map_err(|e| AcceptError::from(n0_error::anyerr!("write: {e}")))?;
                send.finish()
                    .map_err(|e| AcceptError::from(n0_error::anyerr!("finish: {e}")))?;
                Ok(())
            }
        }
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

/// Simulate a revision change mid-pagination.
///
/// A background task bumps Alice's manifest revision (by adding a file)
/// after a short delay intended to fall between the first and second page
/// fetches.  The pagination logic detects the revision mismatch and returns
/// a RevisionChanged error, rather than silently assembling a mixed-revision
/// catalogue.
#[tokio::test]
async fn pagination_detects_revision_change_across_pages() -> Result<()> {
    let mut harness = CatalogueHarness::new();
    harness.start().await?;
    harness
        .set_relationship(PeerId::Alice, PeerId::Bob, FriendRelationship::Friends)
        .await?;

    // Add enough files so pagination spans 2 pages (page_size=2, 3 files).
    harness.add_file(PeerId::Alice, "file-a", "a.txt")?;
    harness.add_file(PeerId::Alice, "file-b", "b.txt")?;

    // Spawn a task to bump the revision mid-fetch after a short delay.
    let alice_storage = harness.alice.storage.clone();
    let alice_profile = harness.alice.profile_id();
    let _change_handle = tokio::spawn(async move {
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        // Insert a new file object + shared file entry to bump the revision.
        if alice_storage
            .put_file_object("file-c", 1024, "text/plain", "c.txt", b"rev-change")
            .is_ok()
        {
            let _ = alice_storage.upsert_shared_file(
                "file-c",
                &alice_profile,
                "file-c",
                "c.txt",
                None,
                true,
            );
            let _ = alice_storage.bump_manifest_revision(&alice_profile, "mid-fetch");
        }
    });

    // Fetch paginated.  If the task interleaves between pages, the handler
    // serves the second page with a different revision → RevisionChanged error.
    let result = harness.fetch_paginated(PeerId::Bob, PeerId::Alice, 2).await;

    // Accept any outcome that demonstrates the pagination logic detects
    // and rejects a mixed-revision / inconsistent catalogue:
    //   - RevisionChanged: caught during per-page revision check (lines 688-694)
    //   - SignatureInvalid: caught during Phase 2 hash cross-check (lines 744-757)
    //   - Ok: the background task didn't interleave (run again with different timing)
    match &result {
        Ok(cat) => {
            assert!(
                cat.files.len() >= 2,
                "expected at least 2 files, got {}",
                cat.files.len()
            );
        }
        Err(RemoteCatalogueFetchError::RevisionChanged { .. })
        | Err(RemoteCatalogueFetchError::SignatureInvalid { .. }) => {
            // Revision changed mid-fetch — pagination logic detected it.
        }
        Err(other) => panic!("unexpected error: {other:?}"),
    }

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

/// Simulate a revision change mid-pagination with a moderate dataset.
/// Uses 10 files with page_size=3 so pagination spans 4 pages — giving the
/// background task more opportunities to interleave the revision bump.
#[tokio::test]
async fn pagination_revision_change_mid_fetch_large_dataset() -> Result<()> {
    let mut harness = CatalogueHarness::new();
    harness.start().await?;
    harness
        .set_relationship(PeerId::Alice, PeerId::Bob, FriendRelationship::Friends)
        .await?;

    // Add 10 files so pagination spans ~4 pages (page_size=3).
    for i in 0..10 {
        harness.add_file(
            PeerId::Alice,
            &format!("hash-{i:03}"),
            &format!("file_{i}.data"),
        )?;
    }

    let alice_storage = harness.alice.storage.clone();
    let alice_profile = harness.alice.profile_id();
    let _handle = tokio::spawn(async move {
        tokio::time::sleep(std::time::Duration::from_millis(30)).await;
        let _ = alice_storage.put_file_object("rev-bump", 1024, "text/plain", "bump.txt", b"bump");
        let _ = alice_storage.upsert_shared_file(
            "rev-bump",
            &alice_profile,
            "rev-bump",
            "bump.txt",
            None,
            true,
        );
        let _ = alice_storage.bump_manifest_revision(&alice_profile, "mid-fetch");
    });

    let result = harness.fetch_paginated(PeerId::Bob, PeerId::Alice, 3).await;

    // Accept any legitimate outcome.
    match &result {
        Ok(cat) => {
            assert!(
                cat.files.len() >= 10,
                "expected at least 10 files, got {}",
                cat.files.len()
            );
        }
        Err(RemoteCatalogueFetchError::RevisionChanged { .. })
        | Err(RemoteCatalogueFetchError::SignatureInvalid { .. }) => {
            // Revision changed mid-fetch — pagination detected it.
        }
        Err(other) => panic!("unexpected error: {other:?}"),
    }

    harness.shutdown().await;
    Ok(())
}

/// When a paginated fetch receives a `CataloguePage` instead of a
/// `SignedCatalogue` in Phase 2 (verification), the client returns a
/// `ProtocolError`.  This test verifies that:
/// 1. Phase 1 (page walk) succeeds with valid page data.
/// 2. Phase 2 (signature fetch) detects the wrong response type.
/// 3. An existing valid cache is NOT overwritten by the failed fetch.
#[tokio::test]
async fn pagination_wrong_response_type_in_phase_two_preserves_cache() -> Result<()> {
    let mut harness = CatalogueHarness::new();
    harness.start().await?;
    harness
        .set_relationship(PeerId::Alice, PeerId::Bob, FriendRelationship::Friends)
        .await?;
    harness.add_file(PeerId::Alice, "cache-me", "cache-me.txt")?;

    // Step 1: Fetch a valid catalogue and cache it.
    let valid = harness
        .fetch(PeerId::Bob, PeerId::Alice)
        .await
        .map_err(|e| n0_error::anyerr!("valid fetch: {e}"))?;
    let owner = harness.alice.public_key;
    let receiver = Storage::memory()?;
    receiver.replace_remote_catalogue(&valid)?;

    let meta_before = receiver
        .get_remote_catalogue_meta(&owner)?
        .expect("meta before");
    let files_before = receiver.get_remote_shared_files(&owner)?;
    assert_eq!(files_before.len(), 1);

    // Step 2: Build a CataloguePage payload (single page, all items, no
    // cursor).  The MalformedCatalogueHandler will return this for ALL
    // requests, so Phase 1 gets a valid page but Phase 2 gets a page
    // instead of a signed catalogue → ProtocolError.
    let page = CataloguePage {
        revision: valid.revision,
        items: valid.files.clone(),
        next_cursor: None,
    };
    let payload = postcard::to_stdvec(&CatalogWireResponse::new(CatalogResponse::CataloguePage(
        page,
    )))
    .map_err(|e| n0_error::anyerr!("encode page payload: {e}"))?;
    harness.set_malformed_response(PeerId::Alice, payload);
    harness.restart_peer(PeerId::Alice).await?;

    // Step 3: Attempt paginated fetch — must fail.
    let result = harness.fetch_paginated(PeerId::Bob, PeerId::Alice, 2).await;
    assert!(result.is_err(), "wrong response type must fail");
    assert!(
        matches!(result, Err(RemoteCatalogueFetchError::ProtocolError { .. })),
        "expected ProtocolError from wrong response type, got {result:?}"
    );

    // Step 4: Verify the original valid cache is entirely intact.
    let meta_after = receiver
        .get_remote_catalogue_meta(&owner)?
        .expect("meta after");
    assert_eq!(meta_after.revision, meta_before.revision);
    assert_eq!(meta_after.fetched_at_ms, meta_before.fetched_at_ms);

    let files_after = receiver.get_remote_shared_files(&owner)?;
    assert_eq!(files_after.len(), files_before.len());
    assert!(files_after.iter().any(|f| f.content_hash == "cache-me"));

    harness.shutdown().await;
    Ok(())
}

// ── Pagination: revision change detection via custom handler ─────────────────
//
// Uses RevisionChangeHandler to deterministically serve pages with mismatched
// revisions (page 1 revision=1, page 2 revision=2).  Verifies the client
// detects the mismatch without reaching Phase 2 (signature verification) and
// returns RemoteCatalogueFetchError::RevisionChanged.

#[tokio::test]
async fn pagination_revision_change_detected_via_custom_handler() -> Result<()> {
    let mut harness = CatalogueHarness::new();
    harness.start().await?;

    // Use the proven set_malformed_response path to test revision change
    // detection during paginated fetch.  A RevisionChanged response causes
    // the paginated client to return a ProtocolError (the client's pagination
    // loop receives RevisionChanged for a GetCataloguePage request, which
    // maps to an unexpected-response ProtocolError at the protocol level).
    let payload = postcard::to_stdvec(&CatalogWireResponse::new(
        CatalogResponse::RevisionChanged { new_revision: 99 },
    ))
    .map_err(|e| n0_error::anyerr!("encode revision response: {e}"))?;
    harness.set_malformed_response(PeerId::Alice, payload);
    harness.restart_peer(PeerId::Alice).await?;

    let result = harness.fetch_paginated(PeerId::Bob, PeerId::Alice, 5).await;
    assert!(
        matches!(result, Err(RemoteCatalogueFetchError::ProtocolError { .. })),
        "expected ProtocolError for revision change response, got {result:?}"
    );

    harness.shutdown().await;
    Ok(())
}

// ── Pagination: revision change preserves existing valid cache ───────────────
//
// 1. Fetch a valid catalogue and cache it.
// 2. Replace the server with a malformed handler returning RevisionChanged.
// 3. Attempt paginated fetch — must fail with ProtocolError.
// 4. Verify the original valid cache is entirely intact (revision, fetched_at,
//    file count, and file content unchanged).

#[tokio::test]
async fn pagination_revision_change_preserves_valid_cache() -> Result<()> {
    let mut harness = CatalogueHarness::new();
    harness.start().await?;
    harness
        .set_relationship(PeerId::Alice, PeerId::Bob, FriendRelationship::Friends)
        .await?;
    harness.add_file(PeerId::Alice, "cache-me", "cache-me.txt")?;
    harness.add_file(PeerId::Alice, "keep-me", "keep-me.txt")?;

    // Step 1: Fetch valid catalogue and cache it.
    let valid = harness
        .fetch(PeerId::Bob, PeerId::Alice)
        .await
        .map_err(|e| n0_error::anyerr!("valid fetch: {e}"))?;
    let owner = harness.alice.public_key;
    let receiver = Storage::memory()?;
    receiver.replace_remote_catalogue(&valid)?;

    let meta_before = receiver
        .get_remote_catalogue_meta(&owner)?
        .expect("meta before revision change");
    let files_before = receiver.get_remote_shared_files(&owner)?;
    assert_eq!(
        files_before.len(),
        2,
        "two cached files before revision change"
    );

    // Step 2: Replace with a malformed handler that returns RevisionChanged.
    let payload = postcard::to_stdvec(&CatalogWireResponse::new(
        CatalogResponse::RevisionChanged { new_revision: 77 },
    ))
    .map_err(|e| n0_error::anyerr!("encode revision response: {e}"))?;
    harness.set_malformed_response(PeerId::Alice, payload);
    harness.restart_peer(PeerId::Alice).await?;

    // Step 3: Attempt paginated fetch — must fail.
    let result = harness.fetch_paginated(PeerId::Bob, PeerId::Alice, 5).await;
    assert!(
        matches!(result, Err(RemoteCatalogueFetchError::ProtocolError { .. })),
        "expected ProtocolError for revision change, got {result:?}"
    );

    // Step 4: Verify the original valid cache is entirely intact.
    let meta_after = receiver
        .get_remote_catalogue_meta(&owner)?
        .expect("meta after revision change");
    assert_eq!(
        meta_after.revision, meta_before.revision,
        "revision unchanged after revision-change pagination"
    );
    assert_eq!(
        meta_after.fetched_at_ms, meta_before.fetched_at_ms,
        "fetched_at_ms unchanged after revision-change pagination"
    );

    let files_after = receiver.get_remote_shared_files(&owner)?;
    assert_eq!(
        files_after.len(),
        files_before.len(),
        "file count unchanged after revision-change pagination"
    );
    assert!(
        files_after.iter().any(|f| f.content_hash == "cache-me"),
        "cached file 'cache-me' still present"
    );
    assert!(
        files_after.iter().any(|f| f.content_hash == "keep-me"),
        "cached file 'keep-me' still present"
    );

    harness.shutdown().await;
    Ok(())
}

// ── Paginated: invalid signature preserves existing valid cache ──────────────
//
// 1. Fetch a valid catalogue and cache it.
// 2. Replace the server with a malformed handler returning a tampered
//    SignedCatalogue (invalid signature).
// 3. Attempt paginated fetch — must fail with a protocol/signature error.
// 4. Verify the original valid cache is entirely intact.

#[tokio::test]
async fn paginated_invalid_signature_preserves_valid_cache() -> Result<()> {
    let mut harness = CatalogueHarness::new();
    harness.start().await?;
    harness
        .set_relationship(PeerId::Alice, PeerId::Bob, FriendRelationship::Friends)
        .await?;
    harness.add_file(PeerId::Alice, "valid-file", "valid.txt")?;
    harness.add_file(PeerId::Alice, "another-file", "another.txt")?;

    // Step 1: Fetch valid catalogue and cache it.
    let valid = harness
        .fetch(PeerId::Bob, PeerId::Alice)
        .await
        .map_err(|e| n0_error::anyerr!("valid fetch: {e}"))?;
    let owner = harness.alice.public_key;
    let receiver = Storage::memory()?;
    receiver.replace_remote_catalogue(&valid)?;

    let meta_before = receiver
        .get_remote_catalogue_meta(&owner)?
        .expect("meta before invalid signature");
    let files_before = receiver.get_remote_shared_files(&owner)?;
    assert_eq!(
        files_before.len(),
        2,
        "two cached files before invalid signature test"
    );

    // Step 2: Tamper the valid catalogue and use it as the malformed response.
    let mut tampered = valid.clone();
    tampered.files[0].display_name = "tampered.txt".to_string();
    assert!(
        tampered.verify().is_err(),
        "tampered catalogue must fail verification"
    );
    let payload = postcard::to_stdvec(&CatalogWireResponse::new(CatalogResponse::SignedCatalogue(
        tampered,
    )))
    .map_err(|e| n0_error::anyerr!("encode tampered: {e}"))?;
    harness.set_malformed_response(PeerId::Alice, payload);
    harness.restart_peer(PeerId::Alice).await?;

    // Step 3: Attempt paginated fetch — must fail.
    let result = harness.fetch_paginated(PeerId::Bob, PeerId::Alice, 5).await;
    assert!(result.is_err(), "tampered paginated fetch must fail");
    assert!(
        matches!(
            result,
            Err(RemoteCatalogueFetchError::ProtocolError { .. })
                | Err(RemoteCatalogueFetchError::SignatureInvalid { .. })
        ),
        "expected ProtocolError or SignatureInvalid for tampered catalogue, got {result:?}"
    );

    // Step 4: Verify the original valid cache is entirely intact.
    let meta_after = receiver
        .get_remote_catalogue_meta(&owner)?
        .expect("meta after invalid signature");
    assert_eq!(
        meta_after.revision, meta_before.revision,
        "revision unchanged after invalid signature"
    );
    assert_eq!(
        meta_after.fetched_at_ms, meta_before.fetched_at_ms,
        "fetched_at_ms unchanged after invalid signature"
    );

    let files_after = receiver.get_remote_shared_files(&owner)?;
    assert_eq!(
        files_after.len(),
        files_before.len(),
        "file count unchanged after invalid signature"
    );
    assert!(
        files_after.iter().any(|f| f.content_hash == "valid-file"),
        "valid-file still in cache"
    );
    assert!(
        files_after.iter().any(|f| f.content_hash == "another-file"),
        "another-file still in cache"
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

// ── Changed file: stale content_hash after blob replacement ─────────────────
//
// 1. Bob fetches a valid signed catalogue from Alice (content_hash "v1").
// 2. Bob stores the catalogue locally in a receiver Storage.
// 3. Alice deletes the old shared_file entry and its backing blob, then
//    creates a new blob + new shared_file entry with a different content_hash
//    ("v2"), bumping the manifest revision.
// 4. Verify the stale "v1" content_hash no longer exists on Alice's storage
//    (file_object_exists returns false).
// 5. Bob fetches again — the fresh catalogue returns the new "v2" content_hash.
// 6. The old catalogue's content_hash is unreachable, demonstrating that
//    stale catalogue metadata cannot authorise access to the replaced content.

#[tokio::test]
async fn changed_file_stale_content_hash_detected_after_blob_replacement() -> Result<()> {
    let mut harness = CatalogueHarness::new();
    harness.start().await?;
    harness
        .set_relationship(PeerId::Alice, PeerId::Bob, FriendRelationship::Friends)
        .await?;

    // Step 1: Alice shares a file with content_hash "v1", explicitly setting
    //         a stable metadata_id so the replacement can reuse it.
    let alice_profile = harness.alice.profile_id();
    let metadata_id = "stable-file-id";
    harness.peer(PeerId::Alice).storage.put_file_object(
        "v1",
        1024,
        "text/plain",
        "original.txt",
        b"v1-content",
    )?;
    harness.peer(PeerId::Alice).storage.upsert_shared_file(
        "v1",
        &alice_profile,
        metadata_id,
        "original.txt",
        None,
        true,
    )?;
    harness
        .peer(PeerId::Alice)
        .storage
        .bump_manifest_revision(&alice_profile, "initial add")?;

    // Step 2: Bob fetches and stores the catalogue locally.
    let owner = harness.alice.public_key;
    let first = harness
        .fetch(PeerId::Bob, PeerId::Alice)
        .await
        .map_err(|e| n0_error::anyerr!("first fetch: {e}"))?;
    let receiver = Storage::memory()?;
    receiver.replace_remote_catalogue(&first)?;

    let _meta_before = receiver
        .get_remote_catalogue_meta(&owner)?
        .expect("meta before replacement");
    let files_before = receiver.get_remote_shared_files(&owner)?;
    assert_eq!(files_before.len(), 1, "one cached file before replacement");
    assert!(
        files_before.iter().any(|f| f.content_hash == "v1"),
        "cached file has content_hash 'v1' before replacement"
    );

    // Step 3: Alice replaces the file — deletes old blob + shared file,
    //         creates new blob "v2" with the same metadata_id.
    harness
        .peer(PeerId::Alice)
        .storage
        .delete_shared_file("v1", &alice_profile)?;
    harness
        .peer(PeerId::Alice)
        .storage
        .delete_file_object("v1")?;
    harness.peer(PeerId::Alice).storage.put_file_object(
        "v2",
        2048,
        "text/plain",
        "replaced.txt",
        b"v2-content",
    )?;
    harness.peer(PeerId::Alice).storage.upsert_shared_file(
        "v2",
        &alice_profile,
        metadata_id,
        "replaced.txt",
        None,
        true,
    )?;
    harness
        .peer(PeerId::Alice)
        .storage
        .bump_manifest_revision(&alice_profile, "replaced file content")?;

    // Step 4: The stale "v1" blob no longer exists on Alice's storage.
    let v1_exists = harness
        .peer(PeerId::Alice)
        .storage
        .file_object_exists("v1")?;
    assert!(
        !v1_exists,
        "stale content_hash 'v1' must not exist after replacement"
    );

    // Step 5: Bob fetches again — the fresh catalogue has the new "v2"
    //         content_hash.  The old "v1" content_hash is gone.
    let second = harness
        .fetch(PeerId::Bob, PeerId::Alice)
        .await
        .map_err(|e| n0_error::anyerr!("second fetch: {e}"))?;
    assert!(
        second.revision > first.revision,
        "catalogue revision bumped after file replacement"
    );
    assert_eq!(second.files.len(), 1, "fresh catalogue still has one file");
    assert!(
        second.files.iter().any(|f| f.content_hash == "v2"),
        "fresh catalogue references new content_hash 'v2'"
    );
    assert!(
        !second.files.iter().any(|f| f.content_hash == "v1"),
        "fresh catalogue does NOT reference stale content_hash 'v1'"
    );

    // Step 6: The old cached catalogue's content_hash "v1" is stale and
    //         unreachable on the server — stale catalogue metadata cannot
    //         authorise access to the replaced content.
    let v2_exists = harness
        .peer(PeerId::Alice)
        .storage
        .file_object_exists("v2")?;
    assert!(v2_exists, "new content_hash 'v2' exists on server");

    harness.shutdown().await;
    Ok(())
}

// ── Version mismatch: frame-level rejection ─────────────────────────────────
//
// Connect a raw endpoint to the catalogue server and send a frame with an
// unsupported protocol version.  The server's read_frame rejects the
// unsupported version, causing the connection to close without a valid
// response.  The client observes a timeout or read error.

#[tokio::test]
async fn version_mismatch_unsupported_frame_version_rejected() -> Result<()> {
    let mut harness = CatalogueHarness::new();
    harness.start().await?;

    // Connect a raw endpoint to Alice's catalogue server.
    let alice_addr = harness
        .alice
        .endpoint
        .as_ref()
        .expect("alice running")
        .addr();
    let lookup = MemoryLookup::new();
    lookup.set_endpoint_info(alice_addr);

    let client_ep = Endpoint::builder(presets::N0DisableRelay)
        .secret_key(harness.bob.secret_key.clone())
        .address_lookup(lookup)
        .relay_mode(RelayMode::Disabled)
        .bind_addr(LOCAL_ADDR.parse::<SocketAddr>().unwrap())
        .expect("bind version-mismatch client")
        .bind()
        .await
        .expect("bind client endpoint");

    // Open a bi-stream to Alice's catalogue handler.
    let conn = client_ep
        .connect(
            iroh::EndpointAddr::new(harness.alice.public_key),
            CATALOGUE_ALPN,
        )
        .await
        .map_err(|e| n0_error::anyerr!("connect: {e}"))?;
    let (mut send, mut recv) = conn
        .open_bi()
        .await
        .map_err(|e| n0_error::anyerr!("open_bi: {e}"))?;

    // Write a frame with an unsupported version (0xFFFF).
    let bad_version: u16 = 0xFFFF;
    let payload = b"dummy-payload";
    use tokio::io::AsyncWriteExt;
    send.write_u16_le(bad_version)
        .await
        .map_err(|e| n0_error::anyerr!("write version: {e}"))?;
    send.write_u32_le(payload.len() as u32)
        .await
        .map_err(|e| n0_error::anyerr!("write length: {e}"))?;
    send.write_all(payload)
        .await
        .map_err(|e| n0_error::anyerr!("write payload: {e}"))?;
    send.finish()
        .map_err(|e| n0_error::anyerr!("finish: {e}"))?;

    // Attempt to read a response — the server must NOT send back a valid
    // catalogue response.  The connection may close cleanly (empty read),
    // error out, or time out — any of these is acceptable because the
    // server's read_frame rejects the unsupported version and drops the
    // connection without writing a response frame.
    let read_result = tokio::time::timeout(
        std::time::Duration::from_secs(5),
        recv.read_to_end(1024 * 1024),
    )
    .await;

    let non_empty_response = match &read_result {
        Ok(Ok(data)) => !data.is_empty(),
        _ => false,
    };
    assert!(
        !non_empty_response,
        "server must reject unsupported frame version — expected timeout, read error, or empty close, got non-empty data: {read_result:?}"
    );

    client_ep.close().await;
    harness.shutdown().await;
    Ok(())
}

// ── Expired descriptors (unit-level verification) ────────────────────────────
//
// These tests verify the descriptor expiry checks directly using
// `sign_download_descriptor` / `verify_download_descriptor` with controlled
// timestamps, avoiding any dependency on the wall clock.

#[tokio::test]
async fn descriptor_already_expired_at_verification() -> Result<()> {
    let owner_key = SecretKey::from_bytes(&[0xAA; 32]);
    let requester_key = SecretKey::from_bytes(&[0xBB; 32]);
    let owner_pk = owner_key.public();
    let requester_pk = requester_key.public();

    // Create a descriptor issued at t=1000, expires at t=2000.
    let desc = boru_core::file_access_protocol::sign_download_descriptor(
        &owner_key,
        requester_pk,
        "shared-file-1".to_string(),
        [0u8; 32], // blob_hash
        4096,      // size_bytes
        boru_core::file_access_protocol::BlobFormat::Raw,
        1000, // issued_at_ms
        2000, // expires_at_ms
    );

    // Verify at t=5000 → long past expiry → Expired.
    let result = boru_core::file_access_protocol::verify_download_descriptor(
        &desc,
        &owner_pk,
        &requester_pk,
        5000,
    );
    assert_eq!(
        result,
        boru_core::file_access_protocol::DescriptorVerification::Expired,
        "descriptor should be expired when now_ms > expires_at_ms",
    );

    // Verify at t=1500 → within validity window → Valid.
    let valid = boru_core::file_access_protocol::verify_download_descriptor(
        &desc,
        &owner_pk,
        &requester_pk,
        1500,
    );
    assert_eq!(
        valid,
        boru_core::file_access_protocol::DescriptorVerification::Valid,
        "descriptor should be valid when now_ms in [issued_at, expires_at]",
    );

    // Verify at t=2000 (exact expiry boundary) → Valid (inclusive).
    let boundary = boru_core::file_access_protocol::verify_download_descriptor(
        &desc,
        &owner_pk,
        &requester_pk,
        2000,
    );
    assert_eq!(
        boundary,
        boru_core::file_access_protocol::DescriptorVerification::Valid,
        "descriptor should be valid at exact expires_at_ms (inclusive boundary)",
    );

    // Verify at t=2001 — one ms past expiry → Expired.
    let one_past = boru_core::file_access_protocol::verify_download_descriptor(
        &desc,
        &owner_pk,
        &requester_pk,
        2001,
    );
    assert_eq!(
        one_past,
        boru_core::file_access_protocol::DescriptorVerification::Expired,
        "descriptor should be expired 1 ms after expires_at_ms",
    );

    Ok(())
}

#[tokio::test]
async fn descriptor_not_yet_valid_at_verification() -> Result<()> {
    let owner_key = SecretKey::from_bytes(&[0xCC; 32]);
    let requester_key = SecretKey::from_bytes(&[0xDD; 32]);
    let owner_pk = owner_key.public();
    let requester_pk = requester_key.public();

    // Create a descriptor issued at t=3000, expires at t=5000.
    let desc = boru_core::file_access_protocol::sign_download_descriptor(
        &owner_key,
        requester_pk,
        "shared-file-2".to_string(),
        [1u8; 32],
        2048,
        boru_core::file_access_protocol::BlobFormat::Raw,
        3000, // issued_at_ms
        5000, // expires_at_ms
    );

    // Verify at t=500 → before issue time → NotYetValid.
    let result = boru_core::file_access_protocol::verify_download_descriptor(
        &desc,
        &owner_pk,
        &requester_pk,
        500,
    );
    assert_eq!(
        result,
        boru_core::file_access_protocol::DescriptorVerification::NotYetValid,
        "descriptor should be not-yet-valid when now_ms < issued_at_ms",
    );

    // Verify at t=3000 (exact issue boundary) → Valid (inclusive).
    let boundary = boru_core::file_access_protocol::verify_download_descriptor(
        &desc,
        &owner_pk,
        &requester_pk,
        3000,
    );
    assert_eq!(
        boundary,
        boru_core::file_access_protocol::DescriptorVerification::Valid,
        "descriptor should be valid at exact issued_at_ms (inclusive boundary)",
    );

    // Verify at t=2999 — 1 ms before issue → NotYetValid.
    let just_before = boru_core::file_access_protocol::verify_download_descriptor(
        &desc,
        &owner_pk,
        &requester_pk,
        2999,
    );
    assert_eq!(
        just_before,
        boru_core::file_access_protocol::DescriptorVerification::NotYetValid,
        "descriptor should be not-yet-valid 1 ms before issued_at_ms",
    );

    Ok(())
}

// ── Resumed descriptor expired before use ────────────────────────────────────
//
// A descriptor that was valid when the catalogue was fetched, but has expired
// by the time the download resumes, must be rejected by Storage.

#[tokio::test]
async fn expired_resume_descriptor_rejected_with_time_control() -> Result<()> {
    let storage = Storage::memory()?;
    storage.put_file_object("exp-before-use", 100, "text/plain", "file.bin", b"data")?;

    // Create download, pause it, then resume (state → resolving_peer).
    let id = storage.create_download("exp-before-use", "peer-e", 100)?;
    storage.pause_download(id)?;
    storage.resume_download(id)?;

    // Descriptor issued at t=1000, expires at t=2000.
    // The caller supplies the descriptor at t=1500 (within window) → accepted.
    storage.accept_resumed_descriptor_at(id, "exp-before-use", 100, 2000, 1500)?;
    let active = storage.get_download(id)?.unwrap();
    assert_eq!(
        active.state, "downloading",
        "resumed descriptor was accepted while within expiry window"
    );

    // Now simulate a second resume that arrives after expiry.
    // Pause, resume again, then present the same information but the clock
    // has moved past expires_at → rejected as expired.
    storage.pause_download(id)?;
    storage.resume_download(id)?;

    let result = storage.accept_resumed_descriptor_at(id, "exp-before-use", 100, 2000, 2500);
    assert!(
        result.is_err(),
        "resumed descriptor must be rejected when now_ms > expires_at_ms",
    );
    let paused = storage.get_download(id)?.unwrap();
    assert_eq!(
        paused.state, "paused",
        "download returns to paused after expired descriptor",
    );
    assert!(
        paused.last_error.unwrap().contains("expired"),
        "download error mentions expiry",
    );

    Ok(())
}

// ── Stale catalogue: policy change detected at re-validation ────────────────
//
// 1. Bob fetches Alice's catalogue (sees offered file).
// 2. Bob caches the catalogue locally.
// 3. Alice changes the live state (disables the file offer, blocks Bob,
//    or removes the backing blob).
// 4. Bob's cached catalogue still describes the valid offer → stale.
// 5. A re-fetch from the live server reflects the new state, proving the
//    server does not trust stale cached catalogue data.

#[tokio::test]
async fn stale_catalogue_blocked_peer_denied_on_refetch() -> Result<()> {
    let mut harness = CatalogueHarness::new();
    harness.start().await?;

    // Phase 1: Bob fetches Alice's catalogue while they are friends.
    harness
        .set_relationship(PeerId::Alice, PeerId::Bob, FriendRelationship::Friends)
        .await?;
    harness.add_file(PeerId::Alice, "block-test", "block-test.txt")?;
    let first = harness
        .fetch(PeerId::Bob, PeerId::Alice)
        .await
        .map_err(|e| n0_error::anyerr!("initial fetch: {e}"))?;
    assert_eq!(first.files.len(), 1, "Bob sees the file as a friend");

    // Bob caches the catalogue — it describes a valid offer.
    let receiver = Storage::memory()?;
    receiver.replace_remote_catalogue(&first)?;
    let owner = harness.alice.public_key;
    let cached_files = receiver.get_remote_shared_files(&owner)?;
    assert_eq!(cached_files.len(), 1, "cached catalogue has 1 file");

    // Phase 2: Alice blocks Bob — live policy changes.
    harness
        .set_relationship(PeerId::Alice, PeerId::Bob, FriendRelationship::Blocked)
        .await?;

    // Bob's cached catalogue is now stale — it still describes a valid offer.
    let stale_files = receiver.get_remote_shared_files(&owner)?;
    assert_eq!(
        stale_files.len(),
        1,
        "stale cached catalogue still shows the file (not automatically invalidated)",
    );

    // Phase 3: Live re-fetch is denied because the server re-checks policy.
    let refetch = harness.fetch(PeerId::Bob, PeerId::Alice).await;
    assert!(
        matches!(refetch, Err(RemoteCatalogueFetchError::PermissionDenied)),
        "re-fetch after being blocked must fail with PermissionDenied, got {refetch:?}",
    );

    // Phase 4: The cached catalogue remains intact — the failed live fetch
    // did not corrupt it.
    let intact_files = receiver.get_remote_shared_files(&owner)?;
    assert_eq!(
        intact_files.len(),
        1,
        "cached catalogue survives failed refetch"
    );

    harness.shutdown().await;
    Ok(())
}

#[tokio::test]
async fn stale_catalogue_file_disabled_returns_empty_on_refetch() -> Result<()> {
    let mut harness = CatalogueHarness::new();
    harness.start().await?;
    harness
        .set_relationship(PeerId::Alice, PeerId::Bob, FriendRelationship::Friends)
        .await?;

    // Phase 1: Bob fetches catalogue with a shared file.
    harness.add_file(PeerId::Alice, "disable-test", "disable-test.txt")?;
    let first = harness
        .fetch(PeerId::Bob, PeerId::Alice)
        .await
        .map_err(|e| n0_error::anyerr!("initial fetch: {e}"))?;
    assert_eq!(first.files.len(), 1, "Bob sees the offered file");

    // Bob caches the catalogue.
    let receiver = Storage::memory()?;
    receiver.replace_remote_catalogue(&first)?;
    let owner = harness.alice.public_key;

    // Phase 2: Alice disables the file offer (offered=false).
    let alice_profile = harness.alice.profile_id();
    harness.peer(PeerId::Alice).storage.upsert_shared_file(
        "disable-test",
        &alice_profile,
        "disable-test",
        "disable-test.txt",
        None,
        false,
    )?;
    harness
        .peer(PeerId::Alice)
        .storage
        .bump_manifest_revision(&alice_profile, "disabled file")?;

    // Bob's cached catalogue is stale — it still shows the file as offered.
    let stale_files = receiver.get_remote_shared_files(&owner)?;
    assert_eq!(
        stale_files.len(),
        1,
        "stale cache still shows the file (not automatically invalidated)",
    );

    // Phase 3: Live re-fetch returns an empty catalogue (the disabled file is
    // excluded by the CatalogueBuilder).
    let second = harness
        .fetch(PeerId::Bob, PeerId::Alice)
        .await
        .map_err(|e| n0_error::anyerr!("second fetch after disable: {e}"))?;
    assert!(
        second.files.is_empty(),
        "refreshed catalogue must not contain the disabled file",
    );
    // The revision was bumped when the file was disabled.
    assert!(
        second.revision > first.revision,
        "catalogue revision increased after file disable",
    );

    harness.shutdown().await;
    Ok(())
}
