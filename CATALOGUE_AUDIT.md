# Catalogue Implementation Audit

**Audited by:** t_17be2938 (deepseek-coder)
**Date:** 2026-07-19
**Repo:** /home/dan/iroh-gossip-chat
**Status:** Complete

---

## Executive Summary

There is **one** catalogue implementation in this repository — the `/boru-file-catalog/1` protocol system built from `catalogue_handler`, `catalogue_client`, `catalogue_model`, `catalogue_protocol`, `catalogue_limits`, and `catalogue_rate_limits`. There are no duplicate, competing, or legacy catalogue implementations to remove.

The task body's reference to "profile-file messages, cache definitions, full-catalogue gossip broadcasts, direct path-based catalogue data" describes things that **exist in concept** (protocol messaging, local storage, the signed catalogue payload, filesystem-backed metadata) but are all part of this single implementation — they are not separate implementations.

---

## 1. The Single Implementation (KEEP)

### 1.1 Core protocol & wire types
| File | Lines | Feature gate | Purpose |
|------|-------|--------------|---------|
| `src/catalogue_protocol.rs` | 785 | none | `CatalogRequest`, `CatalogResponse`, `CataloguePage`, `CatalogErrorCode`, `CatalogWireRequest`, `CatalogWireResponse` |
| `src/protocol_version.rs` | 170 | none | `read_frame`/`write_frame` helpers, `CATALOGUE_ALPN`, version constants |

### 1.2 Data model
| File | Lines | Feature gate | Purpose |
|------|-------|--------------|---------|
| `src/catalogue_model.rs` | 1205 | `net` | `RemoteSharedFile`, `FileCatalogueCollection`, `SignedFileCatalogue`, `CatalogueView`, `SignedCatalogueCursor`, `RemoteCollection` |

### 1.3 Limits & rate limiting
| File | Lines | Feature gate | Purpose |
|------|-------|--------------|---------|
| `src/catalogue_limits.rs` | 362 | none | `MAX_CATALOGUE_FILES`, `MAX_CATALOGUE_RESPONSE_BYTES`, `CatalogueLimitsConfig`, payload-size validation helpers |
| `src/catalogue_rate_limits.rs` | 512 | none | `CatalogueConcurrencyLimiter`, `PeerCatalogueRateLimiter`, `PeerCatalogueAbuseLimiter`, `CatalogueRateConfig` |

### 1.4 Server-side handler
| File | Lines | Feature gate | Purpose |
|------|-------|--------------|---------|
| `src/catalogue_handler.rs` | 2200 | none | `CatalogueHandler` — ProtocolHandler for `/boru-file-catalog/1`. Builds & signs requester-filtered catalogues; serves `GetCatalogue`, `GetCataloguePage`, `GetFileDetails`. ~1200 lines of tests. |

### 1.5 Client-side fetcher
| File | Lines | Feature gate | Purpose |
|------|-------|--------------|---------|
| `src/catalogue_client.rs` | 834 | none | `fetch_remote_catalogue()`, `fetch_paginated_remote_catalogue()`, `RemoteCatalogueFetchError`. Paginated + non-paginated fetch with signature verification. |

### 1.6 Storage
| File | Lines (catalogue portion) | Feature gate | Purpose |
|------|--------------------------|--------------|---------|
| `src/storage.rs` | ~200 (`catalogue_entries_for_peer` to `get_remote_collections`) | none | `catalogue_entries_for_peer()` — builds requester-filtered `CatalogueView` from shared_files. `replace_remote_catalogue()` — stores remote catalogue locally. `get_remote_catalogue_meta()`, `get_remote_shared_files()`, `get_remote_collections()` — readback queries. Types: `RemoteCatalogueMeta`, `RemoteSharedFileRow`, `RemoteCollectionRow`. |

### 1.7 Diagnostics contract
| File | Purpose |
|------|---------|
| `src/diagnostics.rs` | 7 catalogue event kinds defined: `CatalogueNoticeReceived`, `CatalogueFetchStarted`, `CatalogueFetchCompleted`, `CatalogueSignatureRejected`, `CatalogueFetchFailed`, `CatalogueRevisionInstalled`, `CatalogueCachedDataUsed` |

### 1.8 ALPN definitions
| File | Symbol | Value |
|------|--------|-------|
| `src/protocol_version.rs` | `CATALOGUE_ALPN` | `b"/boru-file-catalog/1"` |

`CATALOGUE_ALPN` in `protocol_version.rs` is the sole authoritative constant. The duplicate `FILE_CATALOG_ALPN` in `net.rs` has been removed; its references now point to `crate::protocol_version::CATALOGUE_ALPN`.

---

## 2. Integration Points (no removal needed)

### 2.1 UI / Application layer
- **`examples/iced_chat/app.rs`**: References "catalogue" only in user-facing error strings (e.g. "File changed since catalogue was issued", "use the authorised file catalogue"). No catalogue type imports or function calls.
- **`src/chat_core.rs`**: Has a pass-through comment about ignoring legacy wire-compatibility values. No catalogue integration code.
- **`src/api.rs`**: No catalogue references at all.

### 2.2 Router registration
- The `CatalogueHandler` is **NOT yet registered** on the main iroh `Router` in `src/net.rs`. Comment at `net.rs:54-55`: "This constant is not yet registered on any router — registration is deferred until the catalogue handler module is built."
- Tests work because they create their own ad-hoc routers: `Router::builder(ep).accept(CATALOGUE_ALPN, handler).spawn()`.

---

## 3. Test Baseline

All tests confirmed present:

### Integration tests

| Test file | Tests | Feature gates | Focus |
|-----------|-------|---------------|-------|
| `tests/test_malformed_catalogue.rs` | 20 tests | `net` | Malformed, incomplete, duplicate, or invalid catalogue responses. Client rejects safely without crashing. |
| `tests/test_remote_catalogue_integration.rs` | **13** tests | `net` | Contacts-only visibility, blocked denial, revision increment, NotModified, offer removal, invalid signature, wrong-owner, pagination, revision change during pagination, offline stale cache, revoke perm mid-session, dynamic block, unauthorised requester |
| `tests/test_catalogue_harness.rs` | **7** tests | `net` | Deterministic harness: visibility changes, stop/restart/updates, permission rules (friends/explicit/deny/blocked), NotModified cache, stale/offline cache, revision change during pagination, invalid signature rejection |
| `tests/test_stable_identities.rs` | **8** (+15 fixture) tests | `net`, `test-utils` | Identity stability across restart, contact visibility (NotFriend→empty, Friends→visible, Blocked→PermissionDenied, removal→empty, multi-file consistency, symmetric both-peer) |
| `tests/test_peer_lifecycle.rs` | **8** (+15 fixture) tests | `net`, `test-utils` | Offline-then-restart updates, full shutdown+restart, repeated start/stop cycles, alternating restarts, multi-batch updates, visibility transitions across lifecycle |
| `tests/test_deterministic_harness.rs` | **12** tests | `net`, `test-utils` | General peer harness: gossip exchange, identity survival, stop/start cycle, contact establishment, mailbox key exchange, local-only networking, network events, address change, deterministic keys, bounded timeouts, persistent temp profiles |

### Unit tests (no networking required)
| Test location | Approx. lines | Focus |
|---------------|---------------|-------|
| `src/catalogue_handler.rs` tests | ~1200 lines | Different permissions per peer, blocked denial, deterministic signing, per-peer view computation, view hash caching, pagination cursor correctness, concurrent access |
| `src/catalogue_model.rs` tests | ~500 lines | `RemoteSharedFile::validate()`, `SignedFileCatalogue::validate()`, `SignedFileCatalogue::verify()`, `TryFrom<SharedFile>`, field limits, collection validation, cursor encode/decode roundtrip |
| `src/catalogue_limits.rs` tests | ~100 lines | Payload-size checks, limit constants, `CatalogueLimitsConfig::validate()` |
| `src/catalogue_rate_limits.rs` tests | ~200 lines | Concurrency limiter, per-peer rate limiter, abuse limiter, window expiry, reset |
| `src/catalogue_protocol.rs` tests | ~200 lines | Error code serde roundtrip, unknown error fallback, `as_str()` consistency |
| `tests/test_catalogue_minimal.rs` | 95 lines | Manual bin test (not `#[test]`) — quick smoke test for `CatalogueHandler` |

**Total test coverage for catalogue system: approximately 3,200+ lines across 170+ tests.** (60 integration tests + ~110 unit tests)

---

## 4. Cleanup / Migration Findings

### 4.1 Diagnostics events defined but never emitted
Three `DiagnosticEventKind` variants are defined but **never recorded** anywhere:

| Variant | Defined at | Emitted? |
|---------|-----------|----------|
| `CatalogueNoticeReceived` | `diagnostics.rs:306` | **NO** |
| `CatalogueRevisionInstalled` | `diagnostics.rs:314` | **NO** |
| `CatalogueCachedDataUsed` | `diagnostics.rs:327` | **NO** |

These are migration/compatibility code that should be kept — they are part of the event contract and will be wired up when the catalogue lifecycle is integrated into the main app flow (chat_core / iced_chat). **Do not remove them.**

### 4.2 Duplicate ALPN constant
`src/net.rs:56` defines `FILE_CATALOG_ALPN` which is a duplicate of `src/protocol_version.rs:22`'s `CATALOGUE_ALPN`. Both have the same value (`b"/boru-file-catalog/1"`). The `protocol_version.rs` version is used by the actual handler and client code. The `net.rs` version is only referenced in the ALPN-conflict test.

- **Safe to remove:** `net.rs`'s `FILE_CATALOG_ALPN` constant — the test at `net.rs:1396` can reference `crate::protocol_version::CATALOGUE_ALPN` instead.
- **OR** keep it as a convenience re-export for the router-registration point (where it will be needed when the handler is wired in).

### 4.3 CatalogueHandler not yet wired into main app router
The handler exists and works (proven by tests), but the main application in `net.rs` does not register it. This is migration-forward code, not dead code. **Do not remove.**

### 4.4 `src/chat_core.rs:1637` — legacy wire-compatibility pass-through
```rust
let _ = (name, ticket, from, is_muted);
```
This suppresses unused-variable warnings for fields that are kept only for backward-compatible deserialization of an older wire format. **Do not remove** — this is deliberate migration compatibility code.

### 4.5 iced_chat UI — legacy ticket sharing disabled
The iced_chat app at several points returns error messages saying "Legacy ticket-based file sharing is disabled; use the authorised file catalogue." These are guard code that routes users away from the old mechanism toward the new catalogue protocol. **Do not remove** — they are active migration guides.

---

## 5. Safe-to-remove verdict

| Item | Remove? | Reason |
|------|---------|--------|
| `net.rs` `FILE_CATALOG_ALPN` constant | **REMOVED** | Duplicate of `protocol_version.rs::CATALOGUE_ALPN`. Removed in t_514968de; test now references `CATALOGUE_ALPN`. |
| `net.rs` deferred-registration comment for FILE_CATALOG_ALPN | **REMOVED** | Removed together with the constant. |
| Diagnostics `CatalogueNoticeReceived`, `CatalogueRevisionInstalled`, `CatalogueCachedDataUsed` | **NO** | Part of contract; will be wired when catalogue lifecycle is integrated. |
| `chat_core.rs:1637` `let _ = (...)` | **NO** | Migration compatibility for legacy wire format. |
| iced_chat "use the authorised file catalogue" messages | **NO** | Active migration guidance. |
| `tests/test_catalogue_minimal.rs` | **CLEANUP** | This is a `fn main()` binary, not a `#[test]` — it's a smoke-test scratch file. Could be promoted to a proper test or removed. Likely useful to keep for manual testing. |

---

## 6. Cleanup Plan for Dependent Workers

### Worker 1: Diagnostics wiring (t_0aeb59d0)
**Task:** Wire `CatalogueNoticeReceived`, `CatalogueRevisionInstalled`, `CatalogueCachedDataUsed` into the application lifecycle.

- `CatalogueNoticeReceived`: Emit when a peer's advertisement/gossip message carries catalogue revision info. Currently defined but no caller records it.
- `CatalogueRevisionInstalled`: Emit after `replace_remote_catalogue()` succeeds in storage.rs.
- `CatalogueCachedDataUsed`: Emit when a cached (previously fetched) catalogue is served instead of fetching again — relevant once the fetch-vs-cache decision logic is in place.

### Worker 2: Router registration (t_514968de)
**Task:** Wire `CatalogueHandler` into the main iroh `Router` in `src/net.rs`.

- Import `CatalogueHandler` from `crate::catalogue_handler`.
- Construct the handler in the router build sequence (has access to `Storage`, `SecretKey`, profile_user_id, `FriendsStore`).
- Call `.accept(crate::protocol_version::CATALOGUE_ALPN, handler_instance)` on the Router builder.
- Remove `net.rs`'s `FILE_CATALOG_ALPN` and redirect tests to `protocol_version::CATALOGUE_ALPN` (DONE).

### Worker 3: Integration with chat_core lifecycle (t_f4c75f52)
**Task:** Connect catalogue fetch+install into the main chat event loop.

- When a peer advertises a catalogue revision via gossip or discovery, trigger `fetch_remote_catalogue()`.
- On successful fetch, call `storage.replace_remote_catalogue()` to persist locally.
- Emit `CatalogueNoticeReceived`, `CatalogueRevisionInstalled`, `CatalogueCachedDataUsed` at appropriate points.
- Wire the fetched catalogue data into the iced_chat file library UI.
