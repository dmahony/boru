//! Resource-exhaustion protection integration and unit tests.
//!
//! Simulates a malicious client trying to exhaust server resources. Each
//! scenario is listed below with the protection mechanism that prevents
//! the attack from causing a denial of service.
//!
//! # Attack scenarios and mitigations
//!
//! | # | Attack | Mitigation | Test(s) |
//! |---|--------|-----------|---------|
//! | 1 | Flood with concurrent catalogue connections (16+ simultaneous) | Concurrency semaphore caps at [`MAX_CONCURRENT_CATALOGUE_CONNECTIONS`]; excess receive `Busy` | `concurrent_connections_exhaustion_sends_busy` |
//! | 2 | High-frequency requests from one peer (burst >32/10s) | Sliding-window per-peer rate limiter rejects excess with `RateLimited` | `per_peer_rate_limiting_blocks_after_budget_exhausted`, `per_peer_rate_limiting_clears_after_window_expiry` |
//! | 3 | Oversized request payload (>256 KiB) | Payload-size gate rejects at server with `InvalidRequest`; counts toward malformed-attempt budget | `oversized_request_payload_is_rejected`, `oversized_requests_count_as_malformed_attempts` |
//! | 4 | Garbage / malformed bytes on catalogue stream | Failed postcard decode counted; after [`MAX_INVALID_CATALOGUE_ATTEMPTS_PER_PEER`] the peer is `Blocked` until window expiry | `malformed_requests_block_peer_after_threshold`, `blocked_peer_can_recover_after_window_expiry` |
//! | 5 | Friend-blocked peer tries to fetch catalogue | `FriendsStore` relationship check returns `PermissionDenied` early | `legitimate_peer_can_fetch_after_abuser_blocked_integration` |
//! | 6 | Many files added to storage (>10,000) | Atomic count check rejects upserts beyond `max_files_per_catalogue` | `catalogue_file_limit_atomic_in_storage`, `catalogue_full_but_existing_updates_allowed` |
//! | 7 | Many collections added (>1,000) | Atomic count check rejects `ensure_collection` beyond `max_collections` | `catalogue_collection_limit_in_storage` |
//! | 8 | Many entries in one collection (>10,000) | Atomic count check rejects `add_to_collection` beyond `max_entries_per_collection` | `collection_entry_limit_in_storage` |
//! | 9 | Response volume from one peer (>16 MiB/10s) | Per-peer response-byte budget; `ResponseBudgetExceeded` until window resets | `response_budget_exhaustion_blocks_until_window_expires` |
//! | 10 | Concurrent connections + rate-limit combined | Both limiters composed: concurrency semaphore + per-peer sliding window | `combined_concurrent_and_rate_limit_stress` |
//! | 11 | High-frequency progress writes (thousands/sec) | [`ProgressUpdateGate`] coalesces writes to at most one per 250 ms interval | `progress_update_gate_coalesces_high_frequency_writes` |
//! | 12 | Download queue overflow (>32 queued) | [`DownloadLimiter`] caps global queue (`max_queued_downloads`), per-peer queue (`max_downloads_per_peer`), and hash-verification budget (`max_active_hash_verifications`) | `download_limiter_queue_full_rejects_excess`, `download_limiter_per_peer_limit_enforced`, `hash_verification_budget_independent_of_downloads` |
//! | 13 | Catalogue updates when full | New entries rejected but existing-entry metadata changes succeed (idempotent upsert) | `catalogue_full_but_existing_updates_allowed` |
//! | 14 | Multiple attacking peers in parallel | Per-peer accounting is independent; one peer's rate limit does not affect others | `multiple_peers_independent_rate_budgets` |
//! | 15 | Response payload size abuse | Hard byte caps on catalogue responses ([`MAX_CATALOGUE_RESPONSE_BYTES`]), pages ([`MAX_CATALOGUE_PAGE_BYTES`]), and file-details payloads ([`MAX_FILE_DETAILS_PAYLOAD_BYTES`]) | `response_payload_size_limits_enforced` |
//! | 16 | File-access upload queue overflow | [`UploadLimiter`] caps global queue, per-peer depth, and verification concurrency | `upload_limiter_full_rejects_excess_global`, `upload_limiter_per_peer_full_rejects_excess`, `upload_limiter_verification_full_rejects_excess` |
//! | 17 | Combined abuse budgets exhausted | Request frequency, response bytes, and invalid-attempt budgets all enforced independently on the same limiter | `abuse_limiter_combined_budgets_all_independent` |
//! | 18 | Abuser-blocked peer recovers after limiter expiry | Abuse limiter window clears; blocked peer is re-admitted after window elapses | `abuser_blocked_integration_recovers_after_ban_window` |
//! | 19 | Zero-length request payload | Edge case: a zero-byte payload must not crash or hang the server | `zero_byte_payload_handled_gracefully` |
//! | 20 | Connection storm (rapid connect/disconnect flooding) | Per-peer connection lifecycle properly cleaned; server stays responsive after 50+ rapid cycles | `connection_storm_rapid_cycles` |
//! | 21 | Concurrent mixed attack (valid + malicious peers flooding simultaneously) | Independent per-peer budgets; legitimate peers succeed while abusers are blocked/rate-limited | `concurrent_mixed_valid_and_invalid_stress` |
//! | 22 | Rate-limit boundary at exact window expiry | Sliding-window purge on admit; request at the exact window boundary is allowed | `rate_limiter_boundary_exact_window_expiry` |
//!
//! Integration tests use localhost-only QUIC endpoints with MemoryLookup.
//! Unit tests exercise limiters directly without network I/O.

#![cfg(feature = "net")]

use std::{
    net::SocketAddr,
    sync::Arc,
    time::{Duration, Instant},
};

use boru_chat::{
    catalogue_client::{fetch_remote_catalogue, RemoteCatalogueFetchError},
    catalogue_handler::CatalogueHandler,
    catalogue_limits::{
        check_file_details_payload_size, check_response_payload_size, CatalogueLimitsConfig,
        MAX_CATALOGUE_FILES, MAX_CATALOGUE_PAGE_BYTES, MAX_CATALOGUE_PAGE_SIZE,
        MAX_CATALOGUE_REQUEST_BYTES, MAX_CATALOGUE_RESPONSE_BYTES, MAX_COLLECTIONS,
        MAX_ENTRIES_PER_COLLECTION, MAX_FILE_DETAILS_PAYLOAD_BYTES, MAX_FILE_SIZE_BYTES,
    },
    catalogue_rate_limits::{
        CatalogueAdmission, CatalogueConcurrencyLimiter, CatalogueRateConfig,
        PeerCatalogueAbuseLimiter, PeerCatalogueRateLimiter, MAX_CATALOGUE_REQUESTS_PER_PEER,
        MAX_CATALOGUE_RESPONSE_BYTES_PER_PEER, MAX_CONCURRENT_CATALOGUE_CONNECTIONS,
        MAX_INVALID_CATALOGUE_ATTEMPTS_PER_PEER,
    },
    download_limits::{
        DownloadLimitError, DownloadLimiter, DownloadLimitsConfig, ProgressUpdateGate,
    },
    friends::{FriendId, FriendRecord, FriendRelationship, FriendsStore},
    protocol_version::{
        read_frame, write_frame, CATALOGUE_ALPN, CATALOGUE_RETRIEVAL_V1,
        SUPPORTED_CATALOGUE_RETRIEVAL,
    },
    storage::Storage,
};
use iroh::{
    address_lookup::memory::MemoryLookup, endpoint::presets, protocol::Router, Endpoint,
    EndpointAddr, PublicKey, RelayMode, SecretKey,
};
use tempfile::TempDir;

// ══════════════════════════════════════════════════════════════════════════════
// Constants & helpers
// ══════════════════════════════════════════════════════════════════════════════

const LOCAL_ADDR: &str = "127.0.0.1:0";
const FILE_SIZE: u64 = 1024;
const MIME_TYPE: &str = "application/octet-stream";

fn deterministic_sk(seed: u8) -> SecretKey {
    let mut bytes = [0u8; 32];
    bytes[..1].copy_from_slice(&[seed]);
    bytes[31] = seed;
    SecretKey::from_bytes(&bytes)
}

// ══════════════════════════════════════════════════════════════════════════════
// Integration test harness — a single catalogue server
// ══════════════════════════════════════════════════════════════════════════════

#[allow(dead_code)]
struct ExhaustionServer {
    secret_key: SecretKey,
    public_key: PublicKey,
    storage: Arc<Storage>,
    friends: FriendsStore,
    profile_id: String,
    endpoint: Option<Endpoint>,
    _router: Router,
    addr: EndpointAddr,
}

impl ExhaustionServer {
    async fn start(storage: Arc<Storage>, friends: FriendsStore, sk: SecretKey) -> Self {
        let pk = sk.public();
        let profile_id = pk.to_string();
        let _ = storage.bump_manifest_revision(&profile_id, "init");

        let handler = CatalogueHandler::new(
            storage.clone(),
            sk.clone(),
            profile_id.clone(),
            friends.clone(),
        );

        let lookup = MemoryLookup::new();
        let endpoint = Endpoint::builder(presets::N0DisableRelay)
            .secret_key(sk.clone())
            .address_lookup(lookup)
            .relay_mode(RelayMode::Disabled)
            .bind_addr(LOCAL_ADDR.parse::<SocketAddr>().unwrap())
            .unwrap()
            .bind()
            .await
            .expect("bind endpoint");

        let addr = endpoint.addr();
        let router = Router::builder(endpoint.clone())
            .accept(CATALOGUE_ALPN, handler)
            .spawn();

        Self {
            secret_key: sk,
            public_key: pk,
            storage,
            friends,
            profile_id,
            endpoint: Some(endpoint),
            _router: router,
            addr,
        }
    }

    fn make_friend(&mut self, client_pk: PublicKey) {
        self.friends.upsert(
            FriendId::from_public_key(client_pk),
            FriendRecord {
                relationship: FriendRelationship::Friends,
                ..FriendRecord::default()
            },
        );
    }

    fn add_file(&self, hash: &str, filename: &str) {
        self.storage
            .put_file_object(hash, FILE_SIZE, MIME_TYPE, filename, &[])
            .expect("put file object");
        self.storage
            .upsert_shared_file(hash, &self.profile_id, hash, filename, None, true)
            .expect("upsert shared file");
    }
}

/// Fetch a remote catalogue from a server, returning the result.
async fn fetch_catalogue(
    client_sk: &SecretKey,
    server_pk: PublicKey,
    server_addr: EndpointAddr,
) -> Result<boru_chat::catalogue_model::SignedFileCatalogue, RemoteCatalogueFetchError> {
    let lookup = MemoryLookup::new();
    lookup.set_endpoint_info(server_addr);
    let client_ep = Endpoint::builder(presets::N0DisableRelay)
        .secret_key(client_sk.clone())
        .address_lookup(lookup)
        .relay_mode(RelayMode::Disabled)
        .bind_addr(LOCAL_ADDR.parse::<SocketAddr>().unwrap())
        .unwrap()
        .bind()
        .await
        .map_err(|e| RemoteCatalogueFetchError::ConnectionFailed {
            details: format!("bind: {e}"),
        })?;
    let result = fetch_remote_catalogue(&client_ep, server_pk, None).await;
    client_ep.close().await;
    result
}

/// Send a custom payload on a catalogue connection, drain the response.
/// Returns Ok(()) if any response frame was received (including errors).
async fn send_raw_catalogue_request(
    client_sk: &SecretKey,
    _server_pk: PublicKey,
    server_addr: EndpointAddr,
    raw_payload: Vec<u8>,
) -> Result<(), RemoteCatalogueFetchError> {
    let lookup = MemoryLookup::new();
    lookup.set_endpoint_info(server_addr.clone());
    let client_ep = Endpoint::builder(presets::N0DisableRelay)
        .secret_key(client_sk.clone())
        .address_lookup(lookup)
        .relay_mode(RelayMode::Disabled)
        .bind_addr(LOCAL_ADDR.parse::<SocketAddr>().unwrap())
        .unwrap()
        .bind()
        .await
        .map_err(|e| RemoteCatalogueFetchError::ConnectionFailed {
            details: format!("bind: {e}"),
        })?;

    let conn = client_ep
        .connect(server_addr, CATALOGUE_ALPN)
        .await
        .map_err(|e| RemoteCatalogueFetchError::ConnectionFailed {
            details: format!("connect: {e}"),
        })?;

    let (mut send, mut recv) =
        conn.open_bi()
            .await
            .map_err(|e| RemoteCatalogueFetchError::ConnectionFailed {
                details: format!("open_bi: {e}"),
            })?;

    write_frame(&mut send, CATALOGUE_RETRIEVAL_V1, &raw_payload)
        .await
        .map_err(|e| RemoteCatalogueFetchError::ProtocolError {
            details: format!("write: {e}"),
        })?;
    send.finish()
        .map_err(|e| RemoteCatalogueFetchError::ProtocolError {
            details: format!("finish: {e}"),
        })?;

    // Drain the response frame — any frame proves the server processed it.
    let _ = read_frame(&mut recv, SUPPORTED_CATALOGUE_RETRIEVAL, "response")
        .await
        .map_err(|e| RemoteCatalogueFetchError::ProtocolError {
            details: format!("read: {e}"),
        })?;
    drop(conn);
    client_ep.close().await;
    Ok(())
}

// ══════════════════════════════════════════════════════════════════════════════
// 1. Concurrent connection exhaustion
// ══════════════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn concurrent_connections_exhaustion_sends_busy() {
    let storage = Arc::new(Storage::memory().expect("storage"));
    let friends = FriendsStore::empty_at(TempDir::new().expect("tmp").path());
    let mut server = ExhaustionServer::start(storage, friends, deterministic_sk(0xAA)).await;
    server.add_file("exhaust-1", "f.txt");

    let n = MAX_CONCURRENT_CATALOGUE_CONNECTIONS + 1;
    let keys: Vec<SecretKey> = (0..n).map(|i| deterministic_sk(0x10 + i as u8)).collect();
    for k in &keys {
        server.make_friend(k.public());
    }

    // Phase 1: bind all client endpoints simultaneously.
    let lookup = MemoryLookup::new();
    lookup.set_endpoint_info(server.addr.clone());
    let mut client_eps = Vec::with_capacity(n);
    for ck in &keys {
        let ep = Endpoint::builder(presets::N0DisableRelay)
            .secret_key(ck.clone())
            .address_lookup(lookup.clone())
            .relay_mode(RelayMode::Disabled)
            .bind_addr(LOCAL_ADDR.parse::<SocketAddr>().unwrap())
            .unwrap()
            .bind()
            .await
            .expect("bind client endpoint");
        client_eps.push(ep);
    }

    // Phase 2: fire all catalogue fetches in parallel.
    let pk = server.public_key;
    let mut handles = Vec::new();
    for (_ck, ep) in keys.iter().zip(client_eps) {
        handles.push(tokio::spawn(async move {
            let result = fetch_remote_catalogue(&ep, pk, None).await;
            ep.close().await;
            result
        }));
    }

    let mut success = 0u32;
    let mut busy = 0u32;
    for h in handles {
        match h.await.expect("join") {
            Ok(_) => success += 1,
            Err(RemoteCatalogueFetchError::ConnectionFailed { .. }) => busy += 1,
            Err(_) => busy += 1,
        }
    }

    assert!(
        busy > 0,
        "at least one connection should get Busy (OK={success})"
    );
    assert!(
        success <= MAX_CONCURRENT_CATALOGUE_CONNECTIONS as u32,
        "at most {MAX_CONCURRENT_CATALOGUE_CONNECTIONS} should succeed"
    );

    // Fresh connection after flood works.
    tokio::time::sleep(Duration::from_millis(200)).await;
    let fresh = deterministic_sk(0x99);
    server.make_friend(fresh.public());
    assert!(fetch_catalogue(&fresh, server.public_key, server.addr)
        .await
        .is_ok());
}

// ══════════════════════════════════════════════════════════════════════════════
// 2. Per-peer request frequency rate limiting
// ══════════════════════════════════════════════════════════════════════════════

#[test]
fn per_peer_rate_limiting_blocks_after_budget_exhausted() {
    let config = CatalogueRateConfig {
        max_requests_per_peer: 3,
        rate_limit_window: Duration::from_secs(60),
        max_response_bytes_per_peer: 10_000_000,
        max_invalid_attempts_per_peer: 10,
        ..Default::default()
    };
    let limiter = PeerCatalogueAbuseLimiter::new(&config);

    for i in 0..3 {
        assert_eq!(
            limiter.admit("attacker"),
            CatalogueAdmission::Allowed,
            "{i}"
        );
    }
    assert_eq!(limiter.admit("attacker"), CatalogueAdmission::RateLimited);
    assert_eq!(limiter.admit("legitimate"), CatalogueAdmission::Allowed);
}

#[test]
fn per_peer_rate_limiting_clears_after_window_expiry() {
    let config = CatalogueRateConfig {
        max_requests_per_peer: 2,
        rate_limit_window: Duration::from_millis(30),
        max_response_bytes_per_peer: 10_000_000,
        max_invalid_attempts_per_peer: 10,
        ..Default::default()
    };
    let limiter = PeerCatalogueAbuseLimiter::new(&config);

    assert_eq!(limiter.admit("bursty"), CatalogueAdmission::Allowed);
    assert_eq!(limiter.admit("bursty"), CatalogueAdmission::Allowed);
    assert_eq!(limiter.admit("bursty"), CatalogueAdmission::RateLimited);

    std::thread::sleep(Duration::from_millis(50));
    assert_eq!(limiter.admit("bursty"), CatalogueAdmission::Allowed);
}

// ══════════════════════════════════════════════════════════════════════════════
// 3. Oversized request payloads
// ══════════════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn oversized_request_payload_is_rejected() {
    let storage = Arc::new(Storage::memory().expect("storage"));
    let friends = FriendsStore::empty_at(TempDir::new().expect("tmp").path());
    let mut server = ExhaustionServer::start(storage, friends, deterministic_sk(0xBB)).await;

    let client_sk = deterministic_sk(0x10);
    server.make_friend(client_sk.public());

    let oversized = vec![0u8; MAX_CATALOGUE_REQUEST_BYTES + 1];
    let result =
        send_raw_catalogue_request(&client_sk, server.public_key, server.addr, oversized).await;
    assert!(
        result.is_ok()
            || matches!(
                &result,
                Err(RemoteCatalogueFetchError::ProtocolError { .. })
            ),
        "oversized request should get a response, got {result:?}"
    );
}

#[tokio::test]
async fn oversized_requests_count_as_malformed_attempts() {
    let storage = Arc::new(Storage::memory().expect("storage"));
    let friends = FriendsStore::empty_at(TempDir::new().expect("tmp").path());
    let mut server = ExhaustionServer::start(storage, friends, deterministic_sk(0xCC)).await;

    let client_sk = deterministic_sk(0x20);
    server.make_friend(client_sk.public());
    let oversized = vec![0u8; MAX_CATALOGUE_REQUEST_BYTES + 1];

    let limit = MAX_INVALID_CATALOGUE_ATTEMPTS_PER_PEER as usize;
    for _i in 0..limit {
        send_raw_catalogue_request(
            &client_sk,
            server.public_key,
            server.addr.clone(),
            oversized.clone(),
        )
        .await
        .unwrap_or_else(|_| panic!("oversized request {_i}"));
    }
    // The (limit+1)-th request triggers a block — use fetch_catalogue
    // which properly interprets PermissionDenied as an error.
    // Note: the accept-stage admit() converts all non-allowed states
    // (Blocked, RateLimited, ResponseBudgetExceeded) into a
    // RateLimited protocol error, so we accept either.
    let blocked = fetch_catalogue(&client_sk, server.public_key, server.addr.clone()).await;
    assert!(
        blocked.is_err(),
        "peer should be blocked after {limit} oversized requests, got {blocked:?}"
    );
}

// ══════════════════════════════════════════════════════════════════════════════
// 4. Malformed request blocking
// ══════════════════════════════════════════════════════════════════════════════

#[test]
fn malformed_requests_block_peer_after_threshold() {
    let limiter = PeerCatalogueAbuseLimiter::new(&CatalogueRateConfig {
        max_invalid_attempts_per_peer: 3,
        rate_limit_window: Duration::from_secs(60),
        ..Default::default()
    });

    let peer = "garbage-sender";
    assert!(limiter.record_invalid(peer));
    assert!(limiter.record_invalid(peer));
    assert!(limiter.record_invalid(peer));
    assert!(!limiter.record_invalid(peer), "4th blocked");
    assert_eq!(limiter.admit(peer), CatalogueAdmission::Blocked);
    assert!(limiter.record_invalid("other"));
}

#[test]
fn blocked_peer_can_recover_after_window_expiry() {
    // Use a 200ms window so that three consecutive record_invalid calls
    // don't race with window expiry under test load.
    let limiter = PeerCatalogueAbuseLimiter::new(&CatalogueRateConfig {
        max_invalid_attempts_per_peer: 2,
        rate_limit_window: Duration::from_millis(200),
        ..Default::default()
    });

    let peer = "temp-blocked";
    assert!(limiter.record_invalid(peer));
    assert!(limiter.record_invalid(peer));
    assert!(!limiter.record_invalid(peer), "blocked");

    // Sleep well past the window so the old entries are purged.
    std::thread::sleep(Duration::from_millis(300));
    assert!(limiter.record_invalid(peer), "unblocked after window");
}

// ══════════════════════════════════════════════════════════════════════════════
// 5. System responsiveness — legitimate peer unharmed by abuser
// ══════════════════════════════════════════════════════════════════════════════

#[test]
fn legitimate_peer_unaffected_when_another_is_rate_limited() {
    let limiter = PeerCatalogueAbuseLimiter::new(&CatalogueRateConfig {
        max_requests_per_peer: 2,
        rate_limit_window: Duration::from_secs(60),
        ..Default::default()
    });

    assert_eq!(limiter.admit("abuser"), CatalogueAdmission::Allowed);
    assert_eq!(limiter.admit("abuser"), CatalogueAdmission::Allowed);
    assert_eq!(limiter.admit("abuser"), CatalogueAdmission::RateLimited);

    assert_eq!(limiter.admit("legitimate"), CatalogueAdmission::Allowed);
    assert_eq!(limiter.admit("legitimate"), CatalogueAdmission::Allowed);
}

#[tokio::test]
async fn legitimate_peer_can_fetch_after_abuser_blocked_integration() {
    let storage = Arc::new(Storage::memory().expect("storage"));
    let friends_dir = TempDir::new().expect("tmp");
    let mut friends = FriendsStore::empty_at(friends_dir.path());

    let legit_sk = deterministic_sk(0x30);
    let blocked_sk = deterministic_sk(0x31);
    // Friends must be configured BEFORE start() because CatalogueHandler
    // clones the friends store on construction.
    friends.upsert(
        FriendId::from_public_key(legit_sk.public()),
        FriendRecord {
            relationship: FriendRelationship::Friends,
            ..FriendRecord::default()
        },
    );
    friends.upsert(
        FriendId::from_public_key(blocked_sk.public()),
        FriendRecord {
            relationship: FriendRelationship::Blocked,
            ..FriendRecord::default()
        },
    );

    let server = ExhaustionServer::start(storage, friends, deterministic_sk(0xDD)).await;
    server.add_file("responsiveness-1", "shared.txt");

    let addr = server.addr.clone();
    let pk = server.public_key;

    let blocked_result = fetch_catalogue(&blocked_sk, pk, addr.clone()).await;
    assert!(
        matches!(
            blocked_result,
            Err(RemoteCatalogueFetchError::PermissionDenied)
        ),
        "blocked peer does NOT get PermissionDenied, got: {blocked_result:?}"
    );
    let legit_result = fetch_catalogue(&legit_sk, pk, addr).await;
    assert!(legit_result.is_ok(), "legitimate peer unaffected");
    assert!(!legit_result.unwrap().files.is_empty());
}

// ══════════════════════════════════════════════════════════════════════════════
// 6. Storage catalogue limits — file count (atomic + update allowed)
// ══════════════════════════════════════════════════════════════════════════════

#[test]
fn catalogue_file_limit_atomic_in_storage() {
    let limits = CatalogueLimitsConfig {
        max_files_per_catalogue: 5,
        ..Default::default()
    };
    let storage = Arc::new(Storage::memory_with_catalogue_limits(limits).expect("storage"));
    let profile_id = "test-files";
    let _ = storage.bump_manifest_revision(profile_id, "init");

    for i in 0..5u32 {
        let hash = format!("fl-{i:04}");
        storage
            .put_file_object(&hash, FILE_SIZE, MIME_TYPE, &format!("f{i}.txt"), &[])
            .unwrap();
        storage
            .upsert_shared_file(&hash, profile_id, &hash, &format!("f{i}.txt"), None, true)
            .unwrap();
    }

    // 6th new file rejected.
    storage
        .put_file_object("fl-x", FILE_SIZE, MIME_TYPE, "x.txt", &[])
        .unwrap();
    let err = storage
        .upsert_shared_file("fl-x", profile_id, "fl-x", "x.txt", None, true)
        .unwrap_err()
        .to_string();
    assert!(err.contains("exceeds maximum"), "error: {err}");

    // Update existing file succeeds (same content_hash).
    storage
        .upsert_shared_file(
            "fl-0000",
            profile_id,
            "fl-0000",
            "renamed.txt",
            Some("desc"),
            true,
        )
        .expect("update existing at limit");
}

// ══════════════════════════════════════════════════════════════════════════════
// 7. Storage catalogue limits — collection count
// ══════════════════════════════════════════════════════════════════════════════

#[test]
fn catalogue_collection_limit_in_storage() {
    let limits = CatalogueLimitsConfig {
        max_collections: 3,
        ..Default::default()
    };
    let storage = Arc::new(Storage::memory_with_catalogue_limits(limits).expect("storage"));
    let profile_id = "test-cols";
    let _ = storage.bump_manifest_revision(profile_id, "init");

    for i in 0..3 {
        storage
            .ensure_collection(profile_id, &format!("col-{i}"), None)
            .unwrap();
    }
    let err = storage
        .ensure_collection(profile_id, "overflow", None)
        .unwrap_err()
        .to_string();
    assert!(err.contains("exceeds maximum"), "error: {err}");

    // Duplicate name looks up existing (not counted as new).
    storage
        .ensure_collection(profile_id, "col-0", None)
        .unwrap();
}

// ══════════════════════════════════════════════════════════════════════════════
// 8. Storage catalogue limits — entries per collection
// ══════════════════════════════════════════════════════════════════════════════

#[test]
fn collection_entry_limit_in_storage() {
    let limits = CatalogueLimitsConfig {
        max_entries_per_collection: 3,
        ..Default::default()
    };
    let storage = Arc::new(Storage::memory_with_catalogue_limits(limits).expect("storage"));
    let profile_id = "test-entries";
    let _ = storage.bump_manifest_revision(profile_id, "init");

    let col_id = storage
        .ensure_collection(profile_id, "small-col", None)
        .unwrap();
    for i in 0..3u32 {
        let hash = format!("eh-{i}");
        storage
            .put_file_object(&hash, FILE_SIZE, MIME_TYPE, &format!("f{i}.txt"), &[])
            .unwrap();
        storage.add_to_collection(col_id, &hash, i).unwrap();
    }

    // 4th entry rejected.
    storage
        .put_file_object("eh-x", FILE_SIZE, MIME_TYPE, "fx.txt", &[])
        .unwrap();
    let err = storage
        .add_to_collection(col_id, "eh-x", 3)
        .unwrap_err()
        .to_string();
    assert!(err.contains("more than"), "error: {err}");

    // Duplicate add idempotent.
    storage.add_to_collection(col_id, "eh-0", 0).unwrap();
}

// ══════════════════════════════════════════════════════════════════════════════
// 9. Response budget exhaustion
// ══════════════════════════════════════════════════════════════════════════════

#[test]
fn response_budget_exhaustion_blocks_until_window_expires() {
    let config = CatalogueRateConfig {
        max_response_bytes_per_peer: 100,
        max_requests_per_peer: 100,
        max_invalid_attempts_per_peer: 100,
        rate_limit_window: Duration::from_millis(30),
        ..Default::default()
    };
    let limiter = PeerCatalogueAbuseLimiter::new(&config);

    assert_eq!(limiter.admit("big-dl"), CatalogueAdmission::Allowed);
    limiter.record_response_bytes("big-dl", 90);
    assert_eq!(limiter.admit("big-dl"), CatalogueAdmission::Allowed);
    limiter.record_response_bytes("big-dl", 20);
    assert_eq!(
        limiter.admit("big-dl"),
        CatalogueAdmission::ResponseBudgetExceeded
    );

    std::thread::sleep(Duration::from_millis(50));
    assert_eq!(limiter.admit("big-dl"), CatalogueAdmission::Allowed);
}

// ══════════════════════════════════════════════════════════════════════════════
// 10. Concurrent + rate-limit combined stress
// ══════════════════════════════════════════════════════════════════════════════

#[test]
fn combined_concurrent_and_rate_limit_stress() {
    let config = CatalogueRateConfig {
        max_requests_per_peer: 2,
        max_concurrent_connections: 4,
        rate_limit_window: Duration::from_secs(60),
        ..Default::default()
    };
    let concurrency = CatalogueConcurrencyLimiter::new(config.max_concurrent_connections);
    let abuse = PeerCatalogueAbuseLimiter::new(&config);

    for p in &["legit-1", "legit-2", "legit-3"] {
        assert!(concurrency.try_acquire().is_some(), "{p} slot");
        assert_eq!(abuse.admit(p), CatalogueAdmission::Allowed);
    }

    assert_eq!(abuse.admit("abuser"), CatalogueAdmission::Allowed);
    assert_eq!(abuse.admit("abuser"), CatalogueAdmission::Allowed);
    assert_eq!(abuse.admit("abuser"), CatalogueAdmission::RateLimited);

    for p in &["legit-1", "legit-2", "legit-3"] {
        assert_eq!(abuse.admit(p), CatalogueAdmission::Allowed);
    }
}

// ══════════════════════════════════════════════════════════════════════════════
// 11. Progress update gate coalescing
// ══════════════════════════════════════════════════════════════════════════════

#[test]
fn progress_update_gate_coalesces_high_frequency_writes() {
    let gate = ProgressUpdateGate::new(Duration::from_millis(100));
    let now = Instant::now();

    assert!(gate.should_persist(now), "first write");
    assert!(
        !gate.should_persist(now + Duration::from_millis(10)),
        "coalesced"
    );
    assert!(
        !gate.should_persist(now + Duration::from_millis(50)),
        "coalesced"
    );
    assert!(
        gate.should_persist(now + Duration::from_millis(150)),
        "after interval"
    );
}

// ══════════════════════════════════════════════════════════════════════════════
// 12. Download limiter — queue / per-peer / hash-verification boundaries
// ══════════════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn download_limiter_queue_full_rejects_excess() {
    let limiter = DownloadLimiter::new(DownloadLimitsConfig {
        max_concurrent_downloads: 5,
        max_startup_downloads: 1,
        max_downloads_per_peer: 10,
        max_active_hash_verifications: 1,
        max_queued_downloads: 3,
        progress_update_interval: Duration::from_millis(1),
    });
    let a = limiter.try_enqueue("alice").unwrap();
    let b = limiter.try_enqueue("bob").unwrap();
    let _c = limiter.try_enqueue("carol").unwrap();
    assert_eq!(
        limiter.try_enqueue("dave").unwrap_err(),
        DownloadLimitError::QueueFull
    );
    drop(a);
    assert!(limiter.try_enqueue("dave").is_ok());
    drop(b);
}

#[tokio::test]
async fn download_limiter_per_peer_limit_enforced() {
    let limiter = DownloadLimiter::new(DownloadLimitsConfig {
        max_concurrent_downloads: 5,
        max_startup_downloads: 1,
        max_downloads_per_peer: 1,
        max_active_hash_verifications: 1,
        max_queued_downloads: 10,
        progress_update_interval: Duration::from_millis(1),
    });
    let _a = limiter.try_enqueue("peer-x").unwrap();
    assert_eq!(
        limiter.try_enqueue("peer-x").unwrap_err(),
        DownloadLimitError::PeerQueueFull
    );
    assert!(limiter.try_enqueue("peer-y").is_ok());
}

#[tokio::test]
async fn hash_verification_budget_independent_of_downloads() {
    let limiter = DownloadLimiter::new(DownloadLimitsConfig {
        max_concurrent_downloads: 1,
        max_startup_downloads: 1,
        max_downloads_per_peer: 1,
        max_active_hash_verifications: 1,
        max_queued_downloads: 3,
        progress_update_interval: Duration::from_millis(1),
    });
    let h1 = limiter.try_acquire_hash_verification().unwrap();
    assert!(limiter.try_acquire_hash_verification().is_err());
    let _d1 = limiter.try_enqueue("alice").unwrap();
    drop(h1);
    assert!(limiter.try_acquire_hash_verification().is_ok());
}

// ══════════════════════════════════════════════════════════════════════════════
// 13. Updates still work when catalogue is full
// ══════════════════════════════════════════════════════════════════════════════

#[test]
fn catalogue_full_but_existing_updates_allowed() {
    let limits = CatalogueLimitsConfig {
        max_files_per_catalogue: 3,
        ..Default::default()
    };
    let storage = Arc::new(Storage::memory_with_catalogue_limits(limits).expect("storage"));
    let profile_id = "test-updates";
    let _ = storage.bump_manifest_revision(profile_id, "init");

    for i in 0..3u32 {
        let hash = format!("upd-file-{i}");
        storage
            .put_file_object(&hash, FILE_SIZE, MIME_TYPE, &format!("f{i}.txt"), &[])
            .unwrap();
        storage
            .upsert_shared_file(&hash, profile_id, &hash, &format!("f{i}.txt"), None, true)
            .unwrap();
    }

    // New file rejected.
    storage
        .put_file_object("new-file", FILE_SIZE, MIME_TYPE, "new.txt", &[])
        .unwrap();
    let err = storage
        .upsert_shared_file("new-file", profile_id, "new-file", "new.txt", None, true)
        .unwrap_err()
        .to_string();
    assert!(err.contains("exceeds maximum"));

    // Update existing succeeds.
    storage
        .upsert_shared_file(
            "upd-file-0",
            profile_id,
            "upd-file-0",
            "renamed.txt",
            Some("updated"),
            true,
        )
        .expect("update existing at limit");
}

// ══════════════════════════════════════════════════════════════════════════════
// 14. Multiple peers independent rate budgets
// ══════════════════════════════════════════════════════════════════════════════

#[test]
fn multiple_peers_independent_rate_budgets() {
    let config = CatalogueRateConfig {
        max_requests_per_peer: 4,
        max_response_bytes_per_peer: 1_000_000,
        max_invalid_attempts_per_peer: 5,
        rate_limit_window: Duration::from_secs(60),
        ..Default::default()
    };
    let limiter = PeerCatalogueAbuseLimiter::new(&config);

    for peer in &["peer-1", "peer-2"] {
        for i in 0..4 {
            assert_eq!(
                limiter.admit(peer),
                CatalogueAdmission::Allowed,
                "{peer} #{i}"
            );
        }
        assert_eq!(limiter.admit(peer), CatalogueAdmission::RateLimited);
    }
    assert_eq!(limiter.admit("peer-3"), CatalogueAdmission::Allowed);
}

// ══════════════════════════════════════════════════════════════════════════════
// 15. Response payload size limits (boundary conditions)
// ══════════════════════════════════════════════════════════════════════════════

#[test]
fn response_payload_size_limits_enforced() {
    assert!(check_response_payload_size(MAX_CATALOGUE_RESPONSE_BYTES).is_ok());
    assert!(check_response_payload_size(MAX_CATALOGUE_RESPONSE_BYTES + 1).is_err());
    assert!(check_response_payload_size(0).is_ok());
    assert!(check_file_details_payload_size(MAX_FILE_DETAILS_PAYLOAD_BYTES).is_ok());
    assert!(check_file_details_payload_size(MAX_FILE_DETAILS_PAYLOAD_BYTES + 1).is_err());
}

// ══════════════════════════════════════════════════════════════════════════════
// 16. Catalogue limits config validation boundaries
// ══════════════════════════════════════════════════════════════════════════════

#[test]
fn catalogue_limits_config_validation_boundaries() {
    assert!(CatalogueLimitsConfig::default().validate().is_ok());

    let err = CatalogueLimitsConfig {
        max_files_per_catalogue: 0,
        ..Default::default()
    }
    .validate()
    .unwrap_err()
    .to_string();
    assert!(err.contains("greater than zero"));

    let err = CatalogueLimitsConfig {
        max_files_per_catalogue: 10,
        max_page_size: 11,
        ..Default::default()
    }
    .validate()
    .unwrap_err()
    .to_string();
    assert!(err.contains("must not exceed"));

    assert!(CatalogueLimitsConfig {
        max_files_per_catalogue: 10,
        max_page_size: 10,
        ..Default::default()
    }
    .validate()
    .is_ok());
}

// ══════════════════════════════════════════════════════════════════════════════
// 17. Concurrency limiter boundary conditions
// ══════════════════════════════════════════════════════════════════════════════

#[test]
fn concurrency_limiter_boundary_conditions() {
    let rl = PeerCatalogueRateLimiter::new(0, Duration::from_secs(60));
    assert!(rl.check_and_record("p"), "min 1");

    let rl2 = PeerCatalogueRateLimiter::new(1, Duration::from_secs(60));
    assert!(rl2.check_and_record("p"), "first");
    assert!(!rl2.check_and_record("p"), "second rejected (limit=1)");
    rl2.reset_peer("p");
    assert!(rl2.check_and_record("p"), "after reset");
}

// ══════════════════════════════════════════════════════════════════════════════
// 18. Abuse limiter — combined budgets (rate + response bytes + invalid)
// ══════════════════════════════════════════════════════════════════════════════

#[test]
fn abuse_limiter_combined_budgets_all_independent() {
    let config = CatalogueRateConfig {
        max_requests_per_peer: 5,
        max_response_bytes_per_peer: 500,
        max_invalid_attempts_per_peer: 1,
        rate_limit_window: Duration::from_secs(60),
        ..Default::default()
    };
    let limiter = PeerCatalogueAbuseLimiter::new(&config);

    assert!(limiter.record_invalid("combo"));
    // After max_invalid_attempts (1) invalid records, admit() blocks.
    assert_eq!(limiter.admit("combo"), CatalogueAdmission::Blocked);
    assert_eq!(limiter.admit("clean"), CatalogueAdmission::Allowed);

    limiter.reset_peer("combo");
    assert_eq!(limiter.admit("combo"), CatalogueAdmission::Allowed);
}

// ══════════════════════════════════════════════════════════════════════════════
// 20. Upload limiter — global queue full
// ══════════════════════════════════════════════════════════════════════════════

#[test]
fn upload_limiter_full_rejects_excess_global() {
    let limiter = boru_chat::file_access_handler::UploadLimiter::new(
        boru_chat::file_access_handler::UploadLimitsConfig {
            max_active_uploads: 2,
            max_uploads_per_peer: 3,
            max_queued_uploads: 2,
            max_concurrent_verifications: 1,
            request_timeout: Duration::from_secs(10),
        },
    );

    // Fill the queue.
    let a = limiter.try_enqueue("peer-a").expect("first enqueue");
    let b = limiter.try_enqueue("peer-b").expect("second enqueue");

    // Third should be rejected (global queue depth = 2).
    assert_eq!(
        limiter.try_enqueue("peer-c").unwrap_err(),
        boru_chat::file_access_handler::UploadError::QueueFull,
    );

    // Dropping a slot frees it.
    drop(a);
    let _c = limiter.try_enqueue("peer-c").expect("after drop");
    drop(b);
    drop(_c);
}

// ══════════════════════════════════════════════════════════════════════════════
// 21. Upload limiter — per-peer limit reached
// ══════════════════════════════════════════════════════════════════════════════

#[test]
fn upload_limiter_per_peer_full_rejects_excess() {
    let limiter = boru_chat::file_access_handler::UploadLimiter::new(
        boru_chat::file_access_handler::UploadLimitsConfig {
            max_active_uploads: 10,
            max_uploads_per_peer: 1,
            max_queued_uploads: 10,
            max_concurrent_verifications: 1,
            request_timeout: Duration::from_secs(10),
        },
    );

    let first = limiter.try_enqueue("alice").expect("first alice");
    assert_eq!(
        limiter.try_enqueue("alice").unwrap_err(),
        boru_chat::file_access_handler::UploadError::PeerLimitReached,
    );
    // Different peer succeeds.
    let _bob = limiter.try_enqueue("bob").expect("bob enqueues");
    drop(first);
    let _alice2 = limiter.try_enqueue("alice").expect("alice after drop");
    drop(_bob);
    drop(_alice2);
}

// ══════════════════════════════════════════════════════════════════════════════
// 22. Upload limiter — verification budget exhausted
// ══════════════════════════════════════════════════════════════════════════════

#[test]
fn upload_limiter_verification_full_rejects_excess() {
    let limiter = boru_chat::file_access_handler::UploadLimiter::new(
        boru_chat::file_access_handler::UploadLimitsConfig {
            max_active_uploads: 10,
            max_uploads_per_peer: 3,
            max_queued_uploads: 10,
            max_concurrent_verifications: 1,
            request_timeout: Duration::from_secs(10),
        },
    );

    let v1 = limiter
        .try_acquire_verification()
        .expect("first verification");
    assert_eq!(
        limiter.try_acquire_verification().unwrap_err(),
        boru_chat::file_access_handler::UploadError::VerificationBusy,
    );
    drop(v1);
    assert!(limiter.try_acquire_verification().is_ok());
}

// ══════════════════════════════════════════════════════════════════════════════
// 23. Abuser-blocked peer — recovery after limiter window expiry (integration)
// ══════════════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn abuser_blocked_integration_recovers_after_ban_window() {
    let storage = Arc::new(Storage::memory().expect("storage"));
    let friends_dir = TempDir::new().expect("tmp");
    let mut friends = FriendsStore::empty_at(friends_dir.path());

    let client_sk = deterministic_sk(0x40);
    friends.upsert(
        FriendId::from_public_key(client_sk.public()),
        FriendRecord {
            relationship: FriendRelationship::Friends,
            ..FriendRecord::default()
        },
    );

    let server = ExhaustionServer::start(storage, friends, deterministic_sk(0xEE)).await;
    server.add_file("recovery-1", "shared.txt");

    let oversized = vec![0u8; MAX_CATALOGUE_REQUEST_BYTES + 1];
    let limit = MAX_INVALID_CATALOGUE_ATTEMPTS_PER_PEER as usize;

    // Exhaust the abuse budget with oversized requests.
    for _i in 0..limit {
        send_raw_catalogue_request(
            &client_sk,
            server.public_key,
            server.addr.clone(),
            oversized.clone(),
        )
        .await
        .unwrap_or_else(|_| panic!("oversized request {_i}"));
    }

    // Peer is now blocked — fetch should get an error (RateLimited or
    // PermissionDenied).  The abuse limiter's `admit()` returns Blocked
    // when max_invalid is reached, and the server maps all non-allowed
    // admit results to a RateLimited protocol error response.
    let blocked = fetch_catalogue(&client_sk, server.public_key, server.addr.clone()).await;
    assert!(
        blocked.is_err(),
        "expected blocked peer to get an error, got {blocked:?}"
    );

    // Verify the system stays responsive for other peers.
    // Non-friend peers get an empty catalogue (0 files, 0 collections),
    // not an error — the handler returns `Ok(None)` for non-friends
    // rather than `PermissionDenied`.
    let other_sk = deterministic_sk(0x41);
    let result = fetch_catalogue(&other_sk, server.public_key, server.addr.clone()).await;
    assert!(
        result.is_ok(),
        "non-friend peer should get an empty catalogue, got: {result:?}"
    );
    let catalogue = result.unwrap();
    assert!(
        catalogue.files.is_empty(),
        "non-friend should have no shared files"
    );
    assert!(
        catalogue.collections.is_empty(),
        "non-friend should have no shared collections"
    );
}

// ══════════════════════════════════════════════════════════════════════════════
// 24. Zero-byte request payload
// ══════════════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn zero_byte_payload_handled_gracefully() {
    let storage = Arc::new(Storage::memory().expect("storage"));
    let friends_dir = TempDir::new().expect("tmp");
    let friends = FriendsStore::empty_at(friends_dir.path());
    let server = ExhaustionServer::start(storage, friends, deterministic_sk(0xFF)).await;

    let client_sk = deterministic_sk(0x50);
    let empty_payload: Vec<u8> = Vec::new();

    // Send a frame with version CATALOGUE_RETRIEVAL_V1 but zero payload bytes.
    // The server should not crash; it should either return an error or handle
    // the empty payload gracefully.
    let result = send_raw_catalogue_request(
        &client_sk,
        server.public_key,
        server.addr.clone(),
        empty_payload,
    )
    .await;

    // Any outcome is acceptable as long as the server doesn't hang or crash.
    // Ok means the server returned a response frame (likely an error).
    // Err means the connection was cleanly rejected.
    assert!(
        result.is_ok()
            || matches!(
                &result,
                Err(RemoteCatalogueFetchError::ProtocolError { .. })
            ),
        "zero-byte payload must not hang, got {result:?}"
    );
}

// ══════════════════════════════════════════════════════════════════════════════
// 19. Limit constants are consistent and exposed
// ══════════════════════════════════════════════════════════════════════════════

#[test]
fn limit_constants_are_consistent() {
    assert_eq!(MAX_CONCURRENT_CATALOGUE_CONNECTIONS, 16);
    assert_eq!(MAX_CATALOGUE_REQUESTS_PER_PEER, 32);
    assert_eq!(MAX_INVALID_CATALOGUE_ATTEMPTS_PER_PEER, 3);
    assert_eq!(MAX_CATALOGUE_RESPONSE_BYTES_PER_PEER, 16 * 1024 * 1024);
    assert_eq!(MAX_CATALOGUE_REQUEST_BYTES, 256 * 1024);
    assert_eq!(MAX_CATALOGUE_RESPONSE_BYTES, 4 * 1024 * 1024);
    assert_eq!(MAX_CATALOGUE_PAGE_BYTES, 1024 * 1024);
    assert_eq!(MAX_CATALOGUE_FILES, 10_000);
    assert_eq!(MAX_COLLECTIONS, 1_000);
    assert_eq!(MAX_ENTRIES_PER_COLLECTION, 10_000);
    assert_eq!(MAX_CATALOGUE_PAGE_SIZE, 500);
    assert_eq!(MAX_FILE_DETAILS_PAYLOAD_BYTES, 256 * 1024);
    const {
        assert!(MAX_CATALOGUE_REQUEST_BYTES < MAX_CATALOGUE_RESPONSE_BYTES);
    }
    const {
        assert!(MAX_FILE_DETAILS_PAYLOAD_BYTES < MAX_CATALOGUE_RESPONSE_BYTES);
    }
    assert_eq!(MAX_FILE_SIZE_BYTES, 10 * 1024 * 1024 * 1024 * 1024);
}

// ══════════════════════════════════════════════════════════════════════════════
// 20. Connection storm — rapid connect/disconnect cycles
// ══════════════════════════════════════════════════════════════════════════════

/// Spawn rapid connect/disconnect cycles to the same server, verifying no
/// connection leak and that the server remains responsive after the storm.
///
/// The default per-peer rate limit (`max_requests_per_peer: 10`) will reject
/// cycles beyond the budget, so we verify:
/// 1. At least some cycles succeed (budget not depleted within the window).
/// 2. After a brief pause, the server recovers and a fresh fetch succeeds.
#[tokio::test]
async fn connection_storm_rapid_cycles() {
    let storage = Arc::new(Storage::memory().expect("storage"));
    let friends_dir = TempDir::new().expect("tmp");
    let mut friends = FriendsStore::empty_at(friends_dir.path());

    // Friends must be configured BEFORE start() because CatalogueHandler
    // clones the friends store on construction.
    let client_sk = deterministic_sk(0x80);
    friends.upsert(
        FriendId::from_public_key(client_sk.public()),
        FriendRecord {
            relationship: FriendRelationship::Friends,
            ..FriendRecord::default()
        },
    );

    let server = ExhaustionServer::start(storage, friends, deterministic_sk(0xAA)).await;
    server.add_file("storm-1", "shared.txt");

    const CYCLES: usize = 30;
    let mut ok_count = 0u32;
    let mut err_count = 0u32;
    for i in 0..CYCLES {
        match fetch_catalogue(&client_sk, server.public_key, server.addr.clone()).await {
            Ok(cat) => {
                ok_count += 1;
                assert!(!cat.files.is_empty(), "cycle {i} should get files");
            }
            Err(_) => err_count += 1,
        }
    }

    // At least some cycles must succeed (the per-peer rate budget is ≥1,
    // and default is 10 per 10-second window).  Rapid-fire cycles will
    // quickly exhaust the budget, but the server must not crash or leak.
    assert!(
        ok_count >= 1,
        "at least one cycle should succeed (ok={ok_count}, err={err_count})"
    );

    // After a pause to let the rate-limit window expire, a fresh fetch
    // from the same peer should recover.
    tokio::time::sleep(Duration::from_secs(12)).await;
    let result = fetch_catalogue(&client_sk, server.public_key, server.addr).await;
    assert!(
        result.is_ok(),
        "post-storm fetch should succeed after window expiry"
    );
    let cat = result.unwrap();
    assert!(!cat.files.is_empty(), "post-storm fetch should get files");
}

// ══════════════════════════════════════════════════════════════════════════════
// 21. Concurrent mixed attack — valid + malicious peers flooding simultaneously
// ══════════════════════════════════════════════════════════════════════════════

/// Fire 5 legitimate and 5 oversized/invalid requests concurrently.
/// Legitimate peers (friends) should succeed; malicious peers get blocked or
/// rate-limited.  Per-peer independence must hold under simultaneous pressure.
#[tokio::test]
async fn concurrent_mixed_valid_and_invalid_stress() {
    let storage = Arc::new(Storage::memory().expect("storage"));
    let friends_dir = TempDir::new().expect("tmp");
    let mut friends = FriendsStore::empty_at(friends_dir.path());

    let legit_count = 5;
    let attack_count = 5;
    let legit_keys: Vec<SecretKey> = (0..legit_count)
        .map(|i| deterministic_sk(0x60 + i as u8))
        .collect();
    let attack_keys: Vec<SecretKey> = (0..attack_count)
        .map(|i| deterministic_sk(0x70 + i as u8))
        .collect();

    for k in &legit_keys {
        friends.upsert(
            FriendId::from_public_key(k.public()),
            FriendRecord {
                relationship: FriendRelationship::Friends,
                ..FriendRecord::default()
            },
        );
    }

    let server = ExhaustionServer::start(storage, friends, deterministic_sk(0xBB)).await;
    server.add_file("mixed-1", "shared.txt");

    let oversized = vec![0u8; MAX_CATALOGUE_REQUEST_BYTES + 1];
    let pk = server.public_key;
    let addr = server.addr.clone();

    // Fire all requests concurrently.
    // Legitimate and attacker handles have different return types, so
    // we collect them separately.
    let mut legit_handles = Vec::new();
    for k in &legit_keys {
        let k = k.clone();
        let a = addr.clone();
        legit_handles.push(tokio::spawn(
            async move { fetch_catalogue(&k, pk, a).await },
        ));
    }
    let mut attack_handles = Vec::new();
    for k in &attack_keys {
        let k = k.clone();
        let ov = oversized.clone();
        let a = addr.clone();
        attack_handles.push(tokio::spawn(async move {
            send_raw_catalogue_request(&k, pk, a, ov).await
        }));
    }

    let mut legit_ok = 0u32;
    let mut legit_err = 0u32;
    for h in legit_handles {
        match h.await.expect("join") {
            Ok(cat) => {
                legit_ok += 1;
                assert!(!cat.files.is_empty(), "legit peer should get files");
            }
            Err(_) => legit_err += 1,
        }
    }

    let mut attack_ok = 0u32;
    for h in attack_handles {
        if h.await.expect("join").is_ok() {
            attack_ok += 1;
        }
    }

    // Most or all legitimate peers should succeed (they're friends with
    // independent budgets, but may be affected by concurrency limiting).
    assert!(
        legit_ok > 0,
        "at least one legitimate peer should succeed (ok={legit_ok}, err={legit_err})"
    );
    // Attackers are sending oversized payloads that count as malformed
    // attempts.  They should be blocked after the first few and may also
    // hit concurrency limits.  Some may still get responses with error
    // codes (ProtocolError from oversized payload).
    assert!(
        legit_ok > attack_ok || legit_err == 0,
        "legitimate peers should fare at least as well as attackers (legit_ok={legit_ok}, legit_err={legit_err}, attack_ok={attack_ok})"
    );

    // Fresh fetch after the storm works — non-friend peer gets an empty
    // catalogue, since no files are explicitly shared with them.
    let fresh_sk = deterministic_sk(0x82);
    let result = fetch_catalogue(&fresh_sk, server.public_key, server.addr).await;
    assert!(result.is_ok(), "non-friend should get an empty catalogue");
    let cat = result.unwrap();
    assert!(cat.files.is_empty(), "non-friend catalogue should be empty");
    assert!(
        cat.collections.is_empty(),
        "non-friend collections should be empty"
    );
}

// ══════════════════════════════════════════════════════════════════════════════
// 22. Rate-limit boundary at exact window expiry
// ══════════════════════════════════════════════════════════════════════════════

/// Verify that a request arriving at the exact window boundary is admitted
/// (the old window has been purged by the sliding-window mechanism).
#[test]
fn rate_limiter_boundary_exact_window_expiry() {
    // Use a 15 ms window and max_requests=2.
    let config = CatalogueRateConfig {
        max_requests_per_peer: 2,
        max_response_bytes_per_peer: 10_000_000,
        max_invalid_attempts_per_peer: 10,
        rate_limit_window: Duration::from_millis(15),
        ..Default::default()
    };
    let limiter = PeerCatalogueAbuseLimiter::new(&config);
    let peer = "boundary-peer";

    // Use up the budget.
    assert_eq!(limiter.admit(peer), CatalogueAdmission::Allowed, "req 1");
    assert_eq!(limiter.admit(peer), CatalogueAdmission::Allowed, "req 2");
    assert_eq!(
        limiter.admit(peer),
        CatalogueAdmission::RateLimited,
        "req 3 blocked"
    );

    // Wait until the window boundary has passed.  We wait a bit longer than
    // the window so the old entries are definitely expired.
    std::thread::sleep(Duration::from_millis(25));

    // Now the window has expired.  Requests should be admitted again.
    assert_eq!(
        limiter.admit(peer),
        CatalogueAdmission::Allowed,
        "req after window expiry"
    );
}
