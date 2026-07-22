//! Malformed catalogue response tests.
//!
//! Uses deterministic two-peer setups where the server-side handler returns
//! malformed, incomplete, duplicate, or otherwise invalid catalogue responses.
//! Every test verifies that the receiving peer:
//!   - rejects or safely ignores invalid data
//!   - preserves the last valid catalogue when appropriate
//!   - does not crash or panic
//!   - can recover when a subsequent valid response arrives
//!
//! All networking is localhost-only with explicit address injection
//! (MemoryLookup) — no DHT, relays, or internet dependency.

use std::sync::Arc;

use boru_core::{
    catalogue_client::{fetch_remote_catalogue, RemoteCatalogueFetchError},
    catalogue_handler::CatalogueHandler,
    catalogue_limits::MAX_CATALOGUE_FILES,
    catalogue_model::{FileCatalogueCollection, RemoteSharedFile, SignedFileCatalogue},
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
use rand::SeedableRng;
use tempfile::TempDir;

// ── Constants ────────────────────────────────────────────────────────────

/// Bound address for all endpoints.
const LOCAL_ADDR: &str = "127.0.0.1:0";

/// File size used in test data.
const FILE_SIZE: u64 = 1024;

/// Default MIME type.
const MIME_TYPE: &str = "application/octet-stream";

// ── Custom Protocol Handlers ─────────────────────────────────────────────

/// Handler that returns a fixed raw payload as the catalogue response.
#[derive(Clone)]
struct RawPayloadHandler {
    payload: Vec<u8>,
}

impl std::fmt::Debug for RawPayloadHandler {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RawPayloadHandler")
            .field("payload_len", &self.payload.len())
            .finish()
    }
}

impl ProtocolHandler for RawPayloadHandler {
    async fn accept(&self, connection: Connection) -> std::result::Result<(), AcceptError> {
        let (mut send, mut recv) = connection.accept_bi().await?;
        let _ = read_frame(&mut recv, SUPPORTED_CATALOGUE_RETRIEVAL, "catalogue").await;
        write_frame(&mut send, CATALOGUE_RETRIEVAL_V1, &self.payload)
            .await
            .map_err(|e| AcceptError::from(n0_error::anyerr!("write payload: {e}")))?;
        send.finish()?;
        Ok(())
    }
}

/// Handler that returns a constant [`CatalogResponse`] value, encoded as a
/// proper [`CatalogWireResponse`].
#[derive(Clone)]
struct ConstantResponseHandler {
    response: CatalogResponse,
}

impl std::fmt::Debug for ConstantResponseHandler {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ConstantResponseHandler").finish()
    }
}

impl ProtocolHandler for ConstantResponseHandler {
    async fn accept(&self, connection: Connection) -> std::result::Result<(), AcceptError> {
        let (mut send, mut recv) = connection.accept_bi().await?;
        let _ = read_frame(&mut recv, SUPPORTED_CATALOGUE_RETRIEVAL, "catalogue").await;
        let wire = CatalogWireResponse::new(self.response.clone());
        let bytes = postcard::to_stdvec(&wire)
            .map_err(|e| AcceptError::from(n0_error::anyerr!("encode: {e}")))?;
        write_frame(&mut send, CATALOGUE_RETRIEVAL_V1, &bytes)
            .await
            .map_err(|e| AcceptError::from(n0_error::anyerr!("write: {e}")))?;
        send.finish()?;
        Ok(())
    }
}

// ── MalformedCatalogueHarness ─────────────────────────────────────────────

/// Two-peer deterministic test harness for malformed catalogue scenarios.
///
/// Each peer has a stable identity, isolated storage, and its own QUIC
/// endpoint.  The server peer's handler can be set to any
/// [`ProtocolHandler`], allowing injection of malformed responses.
struct MalformedCatalogueHarness {
    /// Server identity (the one whose catalogue is being fetched).
    server_sk: SecretKey,
    server_pk: PublicKey,
    server_storage: Arc<Storage>,
    server_friends: FriendsStore,
    server_profile_id: String,

    /// Client identity (the one fetching the catalogue).
    client_sk: SecretKey,
    client_pk: PublicKey,

    /// Runtime state (present after start).
    server_endpoint: Option<Endpoint>,
    server_router: Option<Router>,
}

impl MalformedCatalogueHarness {
    /// Create a new harness with deterministic identities.
    fn new() -> Self {
        let server_sk = deterministic_secret_key(b"srv-malformed-v1");
        let server_pk = server_sk.public();
        let server_data_dir = TempDir::with_prefix("malformed-server-").expect("server temp dir");
        let server_storage = Arc::new(Storage::memory().expect("server storage"));
        let server_friends = FriendsStore::empty_at(server_data_dir.path());
        let server_profile_id = server_pk.to_string();

        let client_sk = deterministic_secret_key(b"cli-malformed-v1");
        let client_pk = client_sk.public();
        Self {
            server_sk,
            server_pk,
            server_storage,
            server_friends,
            server_profile_id,

            client_sk,
            client_pk,

            server_endpoint: None,
            server_router: None,
        }
    }

    /// Add the client as a friend on the server (so catalogue entries are visible).
    fn make_client_friend(&mut self) {
        let fid = FriendId::from_public_key(self.client_pk);
        let record = FriendRecord {
            relationship: FriendRelationship::Friends,
            ..FriendRecord::default()
        };
        self.server_friends.upsert(fid, record);
    }

    /// Add a file object and shared-file entry to the server's storage.
    fn add_server_file(&self, hash: &str, filename: &str) {
        self.server_storage
            .put_file_object(hash, FILE_SIZE, MIME_TYPE, filename, &[])
            .expect("put file object");
        self.server_storage
            .upsert_shared_file(hash, &self.server_profile_id, hash, filename, None, true)
            .expect("upsert shared file");
    }

    /// Bump the server's manifest revision.
    fn bump_server_revision(&self) -> u64 {
        self.server_storage
            .bump_manifest_revision(&self.server_profile_id, "test update")
            .expect("bump revision")
    }

    /// Initialise the manifest (must be called before adding files).
    fn init_manifest(&self) {
        self.server_storage
            .bump_manifest_revision(&self.server_profile_id, "initial")
            .expect("init manifest");
    }

    /// Start the server with a normal [`CatalogueHandler`].
    async fn start_server(&mut self) {
        let lookup = MemoryLookup::new();
        let handler = CatalogueHandler::new(
            self.server_storage.clone(),
            self.server_sk.clone(),
            self.server_profile_id.clone(),
            self.server_friends.clone(),
        );
        let ep = Endpoint::builder(presets::N0DisableRelay)
            .secret_key(self.server_sk.clone())
            .address_lookup(lookup)
            .relay_mode(RelayMode::Disabled)
            .bind_addr(
                LOCAL_ADDR
                    .parse::<std::net::SocketAddr>()
                    .expect("bind addr"),
            )
            .expect("bind")
            .bind()
            .await
            .expect("bind endpoint");
        let router = Router::builder(ep.clone())
            .accept(CATALOGUE_ALPN, handler)
            .spawn();
        self.server_endpoint = Some(ep);
        self.server_router = Some(router);
    }

    /// Start the server with a custom [`ProtocolHandler`].
    async fn start_server_with_handler<H: ProtocolHandler + 'static>(&mut self, handler: H) {
        let lookup = MemoryLookup::new();
        let ep = Endpoint::builder(presets::N0DisableRelay)
            .secret_key(self.server_sk.clone())
            .address_lookup(lookup)
            .relay_mode(RelayMode::Disabled)
            .bind_addr(
                LOCAL_ADDR
                    .parse::<std::net::SocketAddr>()
                    .expect("bind addr"),
            )
            .expect("bind")
            .bind()
            .await
            .expect("bind endpoint");
        let router = Router::builder(ep.clone())
            .accept(CATALOGUE_ALPN, handler)
            .spawn();
        self.server_endpoint = Some(ep);
        self.server_router = Some(router);
    }

    /// Stop the server.
    async fn stop_server(&mut self) {
        if let Some(router) = self.server_router.take() {
            let _ = router.shutdown().await;
        }
        if let Some(ep) = self.server_endpoint.take() {
            ep.close().await;
        }
    }

    /// Shut down the harness.
    async fn shutdown(&mut self) {
        self.stop_server().await;
    }

    /// Fetch the server's catalogue as the client, returning the raw result.
    async fn fetch_catalogue(&self) -> Result<SignedFileCatalogue, RemoteCatalogueFetchError> {
        let server_ep = self.server_endpoint.as_ref().expect("server not started");
        let lookup = MemoryLookup::new();
        lookup.set_endpoint_info(server_ep.addr());
        let client_ep = Endpoint::builder(presets::N0DisableRelay)
            .secret_key(self.client_sk.clone())
            .address_lookup(lookup)
            .relay_mode(RelayMode::Disabled)
            .bind_addr(
                LOCAL_ADDR
                    .parse::<std::net::SocketAddr>()
                    .expect("bind addr"),
            )
            .expect("bind")
            .bind()
            .await
            .map_err(|e| RemoteCatalogueFetchError::ConnectionFailed {
                details: format!("bind client: {e}"),
            })?;
        let result = fetch_remote_catalogue(&client_ep, self.server_pk, None).await;
        client_ep.close().await;
        result
    }
}

// ── Deterministic key helper ─────────────────────────────────────────────

fn deterministic_secret_key(seed: &[u8]) -> SecretKey {
    let seed64 = if seed.len() >= 8 {
        u64::from_le_bytes(seed[..8].try_into().unwrap())
    } else {
        let mut buf = [0u8; 8];
        buf[..seed.len()].copy_from_slice(seed);
        u64::from_le_bytes(buf)
    };
    let rng = &mut rand::rngs::ChaCha12Rng::seed_from_u64(seed64);
    let sk_bytes: [u8; 32] = rand::RngExt::random(rng);
    SecretKey::from_bytes(&sk_bytes)
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

// ── Helper to build a malformed SignedFileCatalogue ──────────────────────

/// Build a simple signed catalogue with given files, signed by the server's key.
fn make_signed_catalogue(
    server_sk: &SecretKey,
    revision: u64,
    files: Vec<RemoteSharedFile>,
    collections: Vec<FileCatalogueCollection>,
) -> SignedFileCatalogue {
    SignedFileCatalogue::sign(server_sk, revision, now_ms(), collections, files)
}

/// Build a valid `CatalogWireResponse` wrapping a given `CatalogResponse`.
fn encode_response(response: &CatalogResponse) -> Vec<u8> {
    let wire = CatalogWireResponse::new(response.clone());
    postcard::to_stdvec(&wire).expect("encode response")
}

// ══════════════════════════════════════════════════════════════════════════
// Tests
// ══════════════════════════════════════════════════════════════════════════

// ── 1. Garbage bytes ──────────────────────────────────────────────────────
// Server returns random bytes instead of a valid postcard response.

#[tokio::test]
async fn garbage_bytes_response_is_rejected() {
    let mut harness = MalformedCatalogueHarness::new();
    harness.init_manifest();
    harness.add_server_file("hash-001", "doc.txt");
    harness.bump_server_revision();
    harness.make_client_friend();

    let garbage = vec![0xDE, 0xAD, 0xBE, 0xEF, 0x00, 0x01, 0x02, 0x03, 0xFF];
    harness
        .start_server_with_handler(RawPayloadHandler { payload: garbage })
        .await;

    let result = harness.fetch_catalogue().await;
    assert!(result.is_err(), "garbage bytes should produce an error");
    // Client must not crash; the error should be a protocol error or
    // connection failure.
    match &result {
        Err(RemoteCatalogueFetchError::ProtocolError { .. }) => {} // OK
        Err(RemoteCatalogueFetchError::ConnectionFailed { .. }) => {} // OK
        other => panic!("unexpected error for garbage bytes: {other:?}"),
    }

    harness.shutdown().await;
}

// ── 2. Truncated postcard ─────────────────────────────────────────────────
// Server sends a valid header (version tag etc.) but truncated payload that
// cannot be deserialised as a valid CatalogWireResponse.

#[tokio::test]
async fn truncated_postcard_response_is_rejected() {
    let mut harness = MalformedCatalogueHarness::new();
    harness.init_manifest();
    harness.add_server_file("hash-002", "notes.txt");
    harness.bump_server_revision();
    harness.make_client_friend();

    // Build a valid response first, then truncate it.
    let valid_cat = make_signed_catalogue(
        &harness.server_sk,
        1,
        vec![RemoteSharedFile::new(
            "hash-002",
            "notes.txt",
            None,
            FILE_SIZE,
            MIME_TYPE,
            None,
            1,
        )],
        vec![],
    );
    let full_bytes = encode_response(&CatalogResponse::SignedCatalogue(valid_cat));
    // Keep only the first few bytes — this is a valid frame length but
    // truncated postcard payload.
    let truncated = full_bytes[..full_bytes.len().min(8)].to_vec();

    harness
        .start_server_with_handler(RawPayloadHandler { payload: truncated })
        .await;

    let result = harness.fetch_catalogue().await;
    assert!(
        result.is_err(),
        "truncated response should produce an error"
    );
    match &result {
        Err(RemoteCatalogueFetchError::ProtocolError { .. }) => {}
        Err(RemoteCatalogueFetchError::ConnectionFailed { .. }) => {}
        other => panic!("unexpected error for truncated response: {other:?}"),
    }

    harness.shutdown().await;
}

// ── 3. Wrong protocol version ────────────────────────────────────────────
// Server sends a CatalogWireResponse with an unsupported version number.

#[tokio::test]
async fn unsupported_version_response_is_rejected() {
    let mut harness = MalformedCatalogueHarness::new();
    harness.init_manifest();
    harness.add_server_file("hash-003", "readme.txt");
    harness.bump_server_revision();
    harness.make_client_friend();

    // Build a wire response with version = 999 (unsupported).
    let cat = make_signed_catalogue(
        &harness.server_sk,
        1,
        vec![RemoteSharedFile::new(
            "hash-003",
            "readme.txt",
            None,
            FILE_SIZE,
            MIME_TYPE,
            None,
            1,
        )],
        vec![],
    );
    let wire = CatalogWireResponse {
        version: 999,
        inner: CatalogResponse::SignedCatalogue(cat),
    };
    let bytes = postcard::to_stdvec(&wire).expect("encode");

    harness
        .start_server_with_handler(RawPayloadHandler { payload: bytes })
        .await;

    let result = harness.fetch_catalogue().await;
    assert!(
        result.is_err(),
        "unsupported version should produce an error"
    );
    match &result {
        Err(RemoteCatalogueFetchError::ProtocolError { .. }) => {}
        Err(RemoteCatalogueFetchError::ConnectionFailed { .. }) => {}
        other => panic!("unexpected error for unsupported version: {other:?}"),
    }

    harness.shutdown().await;
}

// ── 4. Wrong response variant: FileDetails instead of SignedCatalogue ─────
// Server returns a FileDetails response when a SignedCatalogue is expected.

#[tokio::test]
async fn file_details_instead_of_signed_catalogue_is_rejected() {
    let mut harness = MalformedCatalogueHarness::new();
    harness.init_manifest();
    harness.add_server_file("hash-004", "info.txt");
    harness.bump_server_revision();
    harness.make_client_friend();

    let file = RemoteSharedFile::new("hash-004", "info.txt", None, FILE_SIZE, MIME_TYPE, None, 1);
    harness
        .start_server_with_handler(ConstantResponseHandler {
            response: CatalogResponse::FileDetails(file),
        })
        .await;

    let result = harness.fetch_catalogue().await;
    assert!(
        matches!(
            &result,
            Err(RemoteCatalogueFetchError::ProtocolError { .. })
        ),
        "expected ProtocolError for wrong variant, got {result:?}"
    );

    harness.shutdown().await;
}

// ── 5. Wrong response variant: CataloguePage instead of SignedCatalogue ───
// Server returns a CataloguePage when a SignedCatalogue is expected.

#[tokio::test]
async fn catalogue_page_instead_of_signed_catalogue_is_rejected() {
    let mut harness = MalformedCatalogueHarness::new();
    harness.init_manifest();
    harness.add_server_file("hash-005", "page.txt");
    harness.bump_server_revision();
    harness.make_client_friend();

    let page = boru_core::catalogue_protocol::CataloguePage {
        revision: 1,
        items: vec![RemoteSharedFile::new(
            "hash-005", "page.txt", None, FILE_SIZE, MIME_TYPE, None, 1,
        )],
        next_cursor: None,
    };
    harness
        .start_server_with_handler(ConstantResponseHandler {
            response: CatalogResponse::CataloguePage(page),
        })
        .await;

    let result = harness.fetch_catalogue().await;
    assert!(
        matches!(
            &result,
            Err(RemoteCatalogueFetchError::ProtocolError { .. })
        ),
        "expected ProtocolError for CataloguePage variant, got {result:?}"
    );

    harness.shutdown().await;
}

// ── 6. Error response is handled safely ──────────────────────────────────
// Server returns a genuine CatalogResponse::Error — client must surface it
// without crashing.

#[tokio::test]
async fn error_response_is_handled_safely() {
    let mut harness = MalformedCatalogueHarness::new();
    harness.init_manifest();
    harness.add_server_file("hash-006", "error.txt");
    harness.bump_server_revision();
    harness.make_client_friend();

    harness
        .start_server_with_handler(ConstantResponseHandler {
            response: CatalogResponse::error(
                boru_core::catalogue_protocol::CatalogErrorCode::Busy,
                "server too busy",
            ),
        })
        .await;

    let result = harness.fetch_catalogue().await;
    assert!(
        matches!(
            &result,
            Err(RemoteCatalogueFetchError::ProtocolError { .. })
        ),
        "expected ProtocolError for server error, got {result:?}"
    );

    harness.shutdown().await;
}

// ── 7. Invalid signature rejection ───────────────────────────────────────
// Catalogue signed with a different key than the server's identity.

#[tokio::test]
async fn invalid_signature_is_rejected() {
    let mut harness = MalformedCatalogueHarness::new();
    harness.init_manifest();
    harness.add_server_file("hash-007", "signed.txt");
    harness.bump_server_revision();
    harness.make_client_friend();

    // Start a valid server and fetch a valid catalogue.
    harness.start_server().await;
    let mut catalogue = harness
        .fetch_catalogue()
        .await
        .expect("should fetch valid catalogue");

    // Tamper the revision — this invalidates the signature.
    catalogue.revision = 9_999_999;

    // Verify that the catalogue validation rejects the tampered data.
    let result = catalogue.verify();
    assert!(result.is_err(), "tampered catalogue must fail verification");

    harness.shutdown().await;
}

// ── 8. Wrong owner_id rejection ──────────────────────────────────────────
// Catalogue's owner_id doesn't match the server's public key.

#[tokio::test]
async fn wrong_owner_id_is_rejected() {
    let mut harness = MalformedCatalogueHarness::new();
    harness.init_manifest();
    harness.add_server_file("hash-008", "owner.txt");
    harness.bump_server_revision();
    harness.make_client_friend();

    // Start a valid server and fetch a valid catalogue.
    harness.start_server().await;
    let mut catalogue = harness
        .fetch_catalogue()
        .await
        .expect("should fetch valid catalogue");

    // Replace the owner_id with a wrong key. This invalidates the
    // signature since owner_id is part of the signed content.
    let wrong_pk = SecretKey::generate().public();
    catalogue.owner_id = wrong_pk;

    // Verify that the catalogue validation rejects the tampered data.
    let result = catalogue.verify();
    assert!(
        result.is_err(),
        "catalogue with wrong owner_id must fail verification"
    );

    harness.shutdown().await;
}

// ── 9. Duplicate shared_file_id ──────────────────────────────────────────
// Two files with identical shared_file_id.

#[tokio::test]
async fn duplicate_shared_file_id_rejected() {
    let mut harness = MalformedCatalogueHarness::new();
    harness.init_manifest();
    harness.add_server_file("hash-009a", "dup-a.txt");
    harness.add_server_file("hash-009b", "dup-b.txt");
    harness.bump_server_revision();
    harness.make_client_friend();

    // Build a catalogue with two files sharing the same shared_file_id.
    let file_a = RemoteSharedFile {
        shared_file_id: "same-id".into(),
        content_hash: "hash-009a".into(),
        display_name: "dup-a.txt".into(),
        mime_type: MIME_TYPE.into(),
        size_bytes: FILE_SIZE,
        ..RemoteSharedFile::new(
            "hash-009a",
            "dup-a.txt",
            None,
            FILE_SIZE,
            MIME_TYPE,
            None,
            1,
        )
    };
    let file_b = RemoteSharedFile {
        shared_file_id: "same-id".into(),
        content_hash: "hash-009b".into(),
        display_name: "dup-b.txt".into(),
        mime_type: MIME_TYPE.into(),
        size_bytes: FILE_SIZE,
        ..RemoteSharedFile::new(
            "hash-009b",
            "dup-b.txt",
            None,
            FILE_SIZE,
            MIME_TYPE,
            None,
            1,
        )
    };
    let cat = make_signed_catalogue(&harness.server_sk, 1, vec![file_a, file_b], vec![]);
    let bytes = encode_response(&CatalogResponse::SignedCatalogue(cat));

    harness
        .start_server_with_handler(RawPayloadHandler { payload: bytes })
        .await;

    let result = harness.fetch_catalogue().await;
    assert!(
        matches!(
            &result,
            Err(RemoteCatalogueFetchError::ProtocolError { .. })
        ),
        "expected ProtocolError for duplicate shared_file_id, got {result:?}"
    );

    harness.shutdown().await;
}

// ── 10. Duplicate content_hash ───────────────────────────────────────────
// Two files with identical content_hash.

#[tokio::test]
async fn duplicate_content_hash_rejected() {
    let mut harness = MalformedCatalogueHarness::new();
    harness.init_manifest();
    harness.add_server_file("hash-010", "single.txt");
    harness.bump_server_revision();
    harness.make_client_friend();

    // Build a catalogue with two files sharing the same content_hash.
    let file_a = RemoteSharedFile {
        shared_file_id: "id-010a".into(),
        content_hash: "same-content".into(),
        display_name: "file-a.txt".into(),
        mime_type: MIME_TYPE.into(),
        size_bytes: FILE_SIZE,
        ..RemoteSharedFile::new(
            "same-content",
            "file-a.txt",
            None,
            FILE_SIZE,
            MIME_TYPE,
            None,
            1,
        )
    };
    let file_b = RemoteSharedFile {
        shared_file_id: "id-010b".into(),
        content_hash: "same-content".into(),
        display_name: "file-b.txt".into(),
        mime_type: MIME_TYPE.into(),
        size_bytes: FILE_SIZE,
        ..RemoteSharedFile::new(
            "same-content",
            "file-b.txt",
            None,
            FILE_SIZE,
            MIME_TYPE,
            None,
            1,
        )
    };
    let cat = make_signed_catalogue(&harness.server_sk, 1, vec![file_a, file_b], vec![]);
    let bytes = encode_response(&CatalogResponse::SignedCatalogue(cat));

    harness
        .start_server_with_handler(RawPayloadHandler { payload: bytes })
        .await;

    let result = harness.fetch_catalogue().await;
    assert!(
        matches!(
            &result,
            Err(RemoteCatalogueFetchError::ProtocolError { .. })
        ),
        "expected ProtocolError for duplicate content_hash, got {result:?}"
    );

    harness.shutdown().await;
}

// ── 11. Empty shared_file_id ─────────────────────────────────────────────
// File entry with an empty shared_file_id fails field validation.

#[tokio::test]
async fn empty_shared_file_id_rejected() {
    let mut harness = MalformedCatalogueHarness::new();
    harness.init_manifest();
    harness.add_server_file("hash-011", "no-id.txt");
    harness.bump_server_revision();
    harness.make_client_friend();

    let bad_file = RemoteSharedFile {
        shared_file_id: String::new(),
        ..RemoteSharedFile::new("hash-011", "no-id.txt", None, FILE_SIZE, MIME_TYPE, None, 1)
    };
    let cat = make_signed_catalogue(&harness.server_sk, 1, vec![bad_file], vec![]);
    let bytes = encode_response(&CatalogResponse::SignedCatalogue(cat));

    harness
        .start_server_with_handler(RawPayloadHandler { payload: bytes })
        .await;

    let result = harness.fetch_catalogue().await;
    assert!(
        matches!(
            &result,
            Err(RemoteCatalogueFetchError::ProtocolError { .. })
        ),
        "expected ProtocolError for empty shared_file_id, got {result:?}"
    );

    harness.shutdown().await;
}

// ── 12. Empty display_name ────────────────────────────────────────────────
// File entry with an empty display_name fails field validation.

#[tokio::test]
async fn empty_display_name_rejected() {
    let mut harness = MalformedCatalogueHarness::new();
    harness.init_manifest();
    harness.add_server_file("hash-012", "no-name.txt");
    harness.bump_server_revision();
    harness.make_client_friend();

    let bad_file = RemoteSharedFile {
        display_name: String::new(),
        ..RemoteSharedFile::new("hash-012", "x", None, FILE_SIZE, MIME_TYPE, None, 1)
    };
    let cat = make_signed_catalogue(&harness.server_sk, 1, vec![bad_file], vec![]);
    let bytes = encode_response(&CatalogResponse::SignedCatalogue(cat));

    harness
        .start_server_with_handler(RawPayloadHandler { payload: bytes })
        .await;

    let result = harness.fetch_catalogue().await;
    assert!(
        matches!(
            &result,
            Err(RemoteCatalogueFetchError::ProtocolError { .. })
        ),
        "expected ProtocolError for empty display_name, got {result:?}"
    );

    harness.shutdown().await;
}

// ── 13. Invalid MIME type ─────────────────────────────────────────────────
// File with a malformed MIME type.

#[tokio::test]
async fn invalid_mime_type_rejected() {
    let mut harness = MalformedCatalogueHarness::new();
    harness.init_manifest();
    harness.add_server_file("hash-013", "bad-mime.txt");
    harness.bump_server_revision();
    harness.make_client_friend();

    let bad_file = RemoteSharedFile {
        mime_type: "not-a-mime-type".into(),
        ..RemoteSharedFile::new("hash-013", "bad-mime.txt", None, FILE_SIZE, "x/x", None, 1)
    };
    let cat = make_signed_catalogue(&harness.server_sk, 1, vec![bad_file], vec![]);
    let bytes = encode_response(&CatalogResponse::SignedCatalogue(cat));

    harness
        .start_server_with_handler(RawPayloadHandler { payload: bytes })
        .await;

    let result = harness.fetch_catalogue().await;
    assert!(
        matches!(
            &result,
            Err(RemoteCatalogueFetchError::ProtocolError { .. })
        ),
        "expected ProtocolError for invalid mime_type, got {result:?}"
    );

    harness.shutdown().await;
}

// ── 14. Oversized catalogue ───────────────────────────────────────────────
// More files than MAX_CATALOGUE_FILES.

#[tokio::test]
async fn oversized_catalogue_rejected() {
    let mut harness = MalformedCatalogueHarness::new();
    harness.init_manifest();
    harness.make_client_friend();

    // Build a catalogue with MAX_CATALOGUE_FILES + 1 files.
    let mut files = Vec::with_capacity(MAX_CATALOGUE_FILES + 1);
    for i in 0..=MAX_CATALOGUE_FILES {
        let hash = format!("big-hash-{:04}", i);
        let name = format!("big-file-{:04}.txt", i);
        files.push(RemoteSharedFile::new(
            &hash, &name, None, FILE_SIZE, MIME_TYPE, None, 1,
        ));
    }
    let cat = make_signed_catalogue(&harness.server_sk, 1, files, vec![]);
    let bytes = encode_response(&CatalogResponse::SignedCatalogue(cat));

    harness
        .start_server_with_handler(RawPayloadHandler { payload: bytes })
        .await;

    let result = harness.fetch_catalogue().await;
    assert!(
        matches!(
            &result,
            Err(RemoteCatalogueFetchError::ProtocolError { .. })
        ),
        "expected ProtocolError for oversized catalogue, got {result:?}"
    );

    harness.shutdown().await;
}

// ── 15. Empty response payload (zero-length frame) ───────────────────────
// Server sends an empty frame.

#[tokio::test]
async fn empty_response_frame_is_rejected() {
    let mut harness = MalformedCatalogueHarness::new();
    harness.init_manifest();
    harness.add_server_file("hash-015", "empty.txt");
    harness.bump_server_revision();
    harness.make_client_friend();

    // Send a zero-length payload through the frame protocol.
    harness
        .start_server_with_handler(RawPayloadHandler {
            payload: Vec::new(),
        })
        .await;

    let result = harness.fetch_catalogue().await;
    assert!(result.is_err(), "empty response should produce an error");
    match &result {
        Err(RemoteCatalogueFetchError::ProtocolError { .. }) => {}
        Err(RemoteCatalogueFetchError::ConnectionFailed { .. }) => {}
        other => panic!("unexpected error for empty response: {other:?}"),
    }

    harness.shutdown().await;
}

// ── 16. Recovery: malformed then valid response ──────────────────────────
// The client receives a malformed first response, then a valid one on retry.
// The client must not crash and must accept the valid response.

#[tokio::test]
async fn recovery_after_malformed_response() {
    let mut harness = MalformedCatalogueHarness::new();
    harness.init_manifest();
    harness.add_server_file("hash-016", "recovery.txt");
    harness.bump_server_revision();
    harness.make_client_friend();

    // Phase 1: Start with a valid server and fetch successfully.
    harness.start_server().await;
    let first_result = harness.fetch_catalogue().await;
    let first = first_result.expect("first fetch from valid server should succeed");
    assert_eq!(first.files.len(), 1);
    assert_eq!(first.files[0].content_hash, "hash-016");

    // Phase 2: Replace server with a garbage handler, verify failure.
    harness.stop_server().await;
    harness
        .start_server_with_handler(RawPayloadHandler {
            payload: vec![0xFF; 64],
        })
        .await;
    let second_result = harness.fetch_catalogue().await;
    assert!(
        second_result.is_err(),
        "fetch from garbage server should fail"
    );

    // Phase 3: Restore valid server and verify recovery.
    harness.stop_server().await;
    harness.start_server().await;
    let third_result = harness.fetch_catalogue().await;
    let third = third_result.expect("fetch from restored server should succeed");
    assert_eq!(third.files.len(), 1);
    assert_eq!(third.files[0].content_hash, "hash-016");

    harness.shutdown().await;
}

// ── 17. Recovery: valid response after empty/garbage ─────────────────────
// Starts with a normal server, fetch succeeds, then replace with garbage,
// verify client handles the transition safely.

#[tokio::test]
async fn valid_then_garbage_does_not_crash() {
    let mut harness = MalformedCatalogueHarness::new();
    harness.init_manifest();
    harness.add_server_file("hash-017", "stable.txt");
    harness.bump_server_revision();
    harness.make_client_friend();

    // Start with a valid server and fetch successfully.
    harness.start_server().await;
    let first = harness.fetch_catalogue().await;
    assert!(
        first.is_ok(),
        "first fetch from valid server should succeed"
    );
    assert_eq!(first.unwrap().files.len(), 1);

    // Replace server with garbage handler.
    harness.stop_server().await;
    harness
        .start_server_with_handler(RawPayloadHandler {
            payload: vec![0xFF; 64],
        })
        .await;

    // Fetch from garbage server — must not crash.
    let second = harness.fetch_catalogue().await;
    assert!(second.is_err(), "fetch from garbage server should fail");

    harness.shutdown().await;
}

// ── 18. Dangling collection reference ────────────────────────────────────
// A file references a collection_id that doesn't exist in the catalogue.

#[tokio::test]
async fn dangling_collection_reference_rejected() {
    let mut harness = MalformedCatalogueHarness::new();
    harness.init_manifest();
    harness.make_client_friend();

    let file = RemoteSharedFile {
        collection_ids: vec!["non-existent-collection".into()],
        ..RemoteSharedFile::new(
            "hash-018",
            "dangling.txt",
            None,
            FILE_SIZE,
            MIME_TYPE,
            None,
            1,
        )
    };
    let cat = make_signed_catalogue(&harness.server_sk, 1, vec![file], vec![]);
    let bytes = encode_response(&CatalogResponse::SignedCatalogue(cat));

    harness
        .start_server_with_handler(RawPayloadHandler { payload: bytes })
        .await;

    let result = harness.fetch_catalogue().await;
    assert!(
        matches!(
            &result,
            Err(RemoteCatalogueFetchError::ProtocolError { .. })
        ),
        "expected ProtocolError for dangling collection reference, got {result:?}"
    );

    harness.shutdown().await;
}

// ── 19. Duplicate collection_id ──────────────────────────────────────────
// Two collections with the same collection_id.

#[tokio::test]
async fn duplicate_collection_id_rejected() {
    let mut harness = MalformedCatalogueHarness::new();
    harness.init_manifest();
    harness.make_client_friend();

    let col_a = FileCatalogueCollection {
        collection_id: "same-collection".into(),
        name: "Collection A".into(),
        description: None,
    };
    let col_b = FileCatalogueCollection {
        collection_id: "same-collection".into(),
        name: "Collection B".into(),
        description: None,
    };
    let cat = make_signed_catalogue(&harness.server_sk, 1, vec![], vec![col_a, col_b]);
    let bytes = encode_response(&CatalogResponse::SignedCatalogue(cat));

    harness
        .start_server_with_handler(RawPayloadHandler { payload: bytes })
        .await;

    let result = harness.fetch_catalogue().await;
    assert!(
        matches!(
            &result,
            Err(RemoteCatalogueFetchError::ProtocolError { .. })
        ),
        "expected ProtocolError for duplicate collection_id, got {result:?}"
    );

    harness.shutdown().await;
}

// ── 20. Oversized response payload ───────────────────────────────────────
// Response payload exceeds MAX_CATALOGUE_RESPONSE_BYTES.

#[tokio::test]
async fn oversized_response_payload_rejected() {
    let mut harness = MalformedCatalogueHarness::new();
    harness.init_manifest();
    harness.make_client_friend();

    // Send a raw payload that is larger than MAX_CATALOGUE_RESPONSE_BYTES.
    // The client-side check_response_payload_size should catch it before
    // any deserialization.
    let oversized = vec![0xABu8; boru_core::catalogue_limits::MAX_CATALOGUE_RESPONSE_BYTES + 1];
    harness
        .start_server_with_handler(RawPayloadHandler { payload: oversized })
        .await;

    let result = harness.fetch_catalogue().await;
    assert!(
        matches!(
            &result,
            Err(RemoteCatalogueFetchError::ProtocolError { .. })
        ),
        "expected ProtocolError for oversized payload, got {result:?}"
    );

    harness.shutdown().await;
}
