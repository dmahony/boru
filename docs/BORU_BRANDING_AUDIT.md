# Boru Chat Branding Audit Report

> **Generated:** 2026-07-22  
> **Scope:** Entire repository at `/home/dan/iroh-gossip-chat`  
> **Task:** t_303d9549

## Summary

This audit catalogs every occurrence of old branding names across the codebase. **Total: ~240+ unique matches** across all search terms, concentrated in documentation, Rust source code comments/module docs, test files, and the `examples/iced_chat/` frontend.

## Categorisation Legend

| # | Category | Description |
|---|----------|-------------|
| 1 | **User-facing branding** | UI text, window titles, error messages, about dialogs |
| 2 | **Rust crate/module identifier** | Cargo.toml, `mod.rs`, `use` statements, struct/variable names |
| 3 | **Package metadata** | Cargo.toml name/description, crate metadata |
| 4 | **Storage path** | Data directories, XDG paths, config files, DB filenames |
| 5 | **Environment variable** | `BORU_CHAT_*`, `BORU_*` env vars |
| 6 | **Documentation** | README, markdown files, inline comments, doc comments |
| 7 | **CI/build/packaging/release** | GitHub Actions, build scripts, Docker, justfile, cliff.toml |
| 8 | **Legacy compatibility identifier** | Wire protocol ALPNs, database keys, network namespaces, domain separators |
| 9 | **Other** | Test temp dirs, utility scripts, etc. |

---

## 1. "Boru Chat" (user-facing text)

| # | File | Line | Match | Category | User-facing? | Safe to rename? |
|---|------|------|-------|----------|-------------|-----------------|
| 1 | DESIGN_SYSTEM.md | 1 | `# Boru Chat — Design System` | 6 (Documentation) | Yes | Yes |
| 2 | DESIGN_SYSTEM.md | 8 | `...every visual token...in the Boru Chat UI` | 6 | Yes | Yes |
| 3 | DESIGN_SYSTEM.md | 96 | `Sidebar header (Boru Chat title)` | 6 | Yes | Yes |
| 4 | DESIGN_SYSTEM.md | 466 | `Boru Chat + ⚙` | 6 | Yes | Yes |
| 5 | docs/resource-exhaustion-mitigations.md | 3 | `every resource-exhaustion attack scenario that Boru Chat` | 6 | Yes | Yes |
| 6 | docs/resource-exhaustion-mitigations.md | 7 | `configured Boru Chat node` | 6 | Yes | Yes |
| 7 | examples/iced_chat/app.rs | 11298 | `.push(text("Boru Chat").size(TYPO_LG).width(Length::Fill))` | 1 (UI heading) | **Yes** | Yes |
| 8 | examples/iced_chat/app.rs | 12314 | `let heading = text("BORU CHAT")` | 1 (UI heading) | **Yes** | Yes |
| 9 | examples/iced_chat/log_viewer.rs | 40 | `text("Boru Chat logs").size(22)` | 1 (UI text) | **Yes** | Yes |
| 10 | examples/iced_chat/log_viewer.rs | 98 | `"Boru Chat logs {} — {}"` | 1 (UI title) | **Yes** | Yes |
| 11 | examples/iced_chat/main.rs | 1040 | `.title(\|_\| format!("Boru Chat {}", app::version_tag()))` | 1 (window title) | **Yes** | Yes |
| 12 | UX_AUDIT.md | 1 | `# UX Audit: Boru Chat (Iced GUI)` | 6 | Yes | Yes |
| 13 | UX_AUDIT.md | 12 | `Boru Chat's Iced GUI is a...` | 6 | Yes | Yes |
| 14 | UX_AUDIT.md | 26 | `See "BORU CHAT" heading + "Private..." tagline` | 6 | Yes | Yes |
| 15 | UX_AUDIT.md | 269 | `Boru Chat has a solid visual foundation...` | 6 | Yes | Yes |

---

## 2. "boru-chat" (crate names, paths, identifiers)

### Package / Crate metadata

| # | File | Line | Match | Category | User-facing? | Safe to rename? |
|---|------|------|-------|----------|-------------|-----------------|
| 1 | Cargo.toml | 2 | `name = "boru-chat"` | 3 (package name) | No | **Preserve** — would change crate name; requires downstream update |
| 2 | Cargo.toml | 23 | `repository = "https://github.com/dmahony/boru-chat"` | 3 | Yes | Yes (update repo URL) |
| 3 | Cargo.lock | 828 | `name = "boru-chat"` | 3 | No | Generated — auto-updates on build |
| 4 | cliff.toml | 5 | `All notable changes to boru-chat will be documented` | 7 (release config) | Yes | Yes |

### CLI / Example metadata

| # | File | Line | Match | Category | Safe to rename? |
|---|------|------|-------|----------|-----------------|
| 5 | examples/doctor.rs | 1 | `//! boru-chat install doctor / sanity-check.` | 6 | Yes |
| 6 | examples/doctor.rs | 33 | `#[command(name = "doctor", about = "Check boru-chat install health")]` | 9 | Yes |
| 7 | examples/doctor.rs | 579 | `println!("═══ boru-chat doctor ══╕");` | 1 (CLI output) | Yes |
| 8 | examples/iced_chat/main.rs | 1 | `//! Iced desktop frontend for boru-chat.` | 6 | Yes |
| 9 | examples/iced_chat/mcp_server.rs | 1 | `//! MCP diagnostic server for boru-chat.` | 6 | Yes |
| 10 | examples/dht_harness.rs | 4 | `//! boru-chat public-room discovery system...` | 6 | Yes |
| 11 | examples/dht_harness.rs | 86 | `about = "Manual live Mainline DHT test harness for boru-chat"` | 9 | Yes |
| 12 | examples/dht_harness.rs | 160 | `println!("─── boru-chat DHT Test Harness ──────");` | 1 | Yes |

### Documentation (markdown)

| # | File | Line | Match | Category | Safe to rename? |
|---|------|------|-------|----------|-----------------|
| 13 | ARCHITECTURE.md | 1 | `# boru-chat Architecture` | 6 | Yes |
| 14 | ARCHITECTURE.md | 5 | `boru-chat is a peer-to-peer chat application...` | 6 | Yes |
| 15 | ARCHITECTURE.md | 111 | `boru-chat uses **two independent DHT systems**` | 6 | Yes |
| 16 | DESIGN_SYSTEM.md | 5 | `...the iced desktop GUI for boru-chat` | 6 | Yes |
| 17 | docs/configuration.md | 3 | `boru-chat is configured through CLI flags...` | 6 | Yes |
| 18 | docs/discovery-architecture.md | 3 | `boru-chat uses **two independent discovery layers**` | 6 | Yes |
| 19 | docs/discovery-architecture.md | 118 | `` (`public-room-topic` vs `boru-chat discovery-key v1`) `` | 6 | Yes |
| 20 | docs/discovery-architecture.md | 471 | `When running two instances of boru-chat on the same machine` | 6 | Yes |
| 21 | docs/protocol-layers.md | 3 | `boru-chat uses multiple distinct QUIC-based protocols` | 6 | Yes |
| 22 | docs/protocol-layers.md | 203 | `boru-chat uses two independent DHT systems` | 6 | Yes |
| 23 | docs/testing.md | 3 | `boru-chat has a comprehensive test suite...` | 6 | Yes |
| 24 | docs/networking-audit.md | 4 | `Codebase: iroh-gossip-chat (boru-chat), commit 9ed4f23` | 6 | Yes |
| 25 | justfile | 1 | `# ── boru-chat development justfile ──` | 7 | Yes |
| 26 | README.md | 1 | `# boru-chat` | 6 | Yes |
| 27 | README.md | 8 | `boru-chat is a Rust library (boru_chat) and example GUI` | 6 | Yes |
| 28 | scripts/flamegraph.sh | 2 | `# ── CPU flamegraph for boru-chat GUI example ──` | 7 | Yes |

### Source code doc comments

| # | File | Line | Match | Category | Safe to rename? |
|---|------|------|-------|----------|-----------------|
| 29 | src/api.rs | 1 | `//! Public API for using boru-chat` | 6 | Yes |
| 30 | src/chat_history.rs | 1 | `//! Durable chat history storage for boru-chat.` | 6 | Yes |
| 31 | src/conversations.rs | 1 | `//! Durable conversation records for boru-chat.` | 6 | Yes |
| 32 | src/diagnostics.rs | 1 | `//! Core diagnostics — bounded event and probe storage for boru-chat.` | 6 | Yes |
| 33 | src/friend_request.rs | 1 | `//! Durable friend request store and API for boru-chat.` | 6 | Yes |
| 34 | src/friends.rs | 1 | `//! Durable friends list storage for boru-chat.` | 6 | Yes |
| 35 | src/metrics.rs | 1 | `//! Metrics for boru-chat` | 6 | Yes |
| 36 | src/net.rs | 1 | `//! Networking for the boru-chat protocol` | 6 | Yes |
| 37 | src/net/util.rs | 1 | `//! Utilities for boru-chat networking` | 6 | Yes |
| 38 | src/outbox.rs | 1 | `//! Durable encrypted outbox storage for boru-chat.` | 6 | Yes |
| 39 | src/perf.rs | 1 | `//! Performance instrumentation for boru-chat.` | 6 | Yes |
| 40 | src/proto.rs | 1, 3, 6 | `//! Implementation of the boru-chat protocol...` | 6 | Yes |
| 41 | src/proto/state.rs | 1, 34, 145 | `/// The state of the boru-chat protocol.` | 6 | Yes |
| 42 | src/room.rs | 1 | `//! Durable room metadata for boru-chat.` | 6 | Yes |
| 43 | src/room_history.rs | 1 | `//! Transient multi-room state for boru-chat.` | 6 | Yes |
| 44 | src/user_profile.rs | 1 | `//! User profile and shared file data models for boru-chat.` | 6 | Yes |
| 45 | src/lib.rs | 221 | `/// Opt-in boru-chat debug tracing — append-only event log` | 6 | Yes |

### Storage paths

| # | File | Line | Match | Category | Safe to rename? |
|---|------|------|-------|----------|-----------------|
| 46 | docs/configuration.md | 64 | `$XDG_DATA_HOME/boru-chat` | 4 | **Preserve** — runtime data path; would break existing installs |
| 47 | docs/configuration.md | 65 | `$HOME/.local/share/boru-chat/` | 4 | **Preserve** |
| 48 | docs/configuration.md | 66 | `$LOCALAPPDATA/boru-chat` | 4 | **Preserve** |
| 49 | docs/configuration.md | 67 | `$PWD/.boru-chat` | 4 | **Preserve** |
| 50 | docs/message-storage-design.md | 12, 13 | Same paths as above | 4 | **Preserve** |
| 51 | examples/doctor.rs | 156-169 | Various `join("boru-chat")` and `join(".boru-chat")` | 4 | **Preserve** — runtime paths |
| 52 | examples/iced_chat/main.rs | 103, 156-169 | Same pattern | 4 | **Preserve** |
| 53 | examples/iced_chat/log_viewer.rs | 133 | `Path::new("/tmp/boru-chat")` | 4 (test temp) | Yes — test temporary path |
| 54 | examples/setup.rs | 16-30 | Various `join("boru-chat")` and `join(".boru-chat")` | 4 | **Preserve** |
| 55 | src/gossip_debug.rs | 148, 156 | `~/.local/share/boru-chat/gossip-debug.log` | 4 | **Preserve** |
| 56 | README.md | 27, 28 | `$XDG_DATA_HOME/boru-chat` and `$PWD/.boru-chat` | 4 | **Preserve** |
| 57 | README.md | 192 | `BORU_CHAT_DATA_DIR=~/.boru-chat cargo run...` | 4 | **Preserve** |

### Test file doc comments

| # | File | Line | Match | Category | Safe to rename? |
|---|------|------|-------|----------|-----------------|
| 58 | tests/test_local_address_lookup.rs | 4 | `//! boru-chat endpoints, enabling LAN peer discovery` | 6 | Yes |
| 59 | tests/test_offline_delivery_integration.rs | 4 | `//! of the boru-chat storage layer — outbox persistence` | 6 | Yes |
| 60 | tests/test_performance_baseline.rs | 1 | `//! Performance baseline measurement for boru-chat.` | 6 | Yes |
| 61 | tests/test_performance_regression.rs | 1 | `//! Performance regression tests for boru-chat.` | 6 | Yes |
| 62 | tests/test_security.rs | 1 | `//! Security integration tests for boru-chat.` | 6 | Yes |
| 63 | tests/test_storage_integration.rs | 3 | `//! These tests exercise boru_chat::storage::Storage...` | 6 | Yes |

### Wire protocol / domain separators (legacy compat)

| # | File | Line | Match | Category | Safe to rename? |
|---|------|------|-------|----------|-----------------|
| 64 | src/discovery_backend.rs | 21 | `b"boru-chat/public-lobby/v1"` | 8 | **Preserve** — DHT key domain; renaming would break network discovery |
| 65 | src/discovery_secret.rs | 47-49, 72, 77, 82 | `b"boru-chat private-room v2 namespace/encryption/signing"` | 8 | **Preserve** — cryptographic domain separators |
| 66 | src/private_room_tracker.rs | 78 | `b"boru-chat private-room v1"` | 8 | **Preserve** |
| 67 | src/public_room.rs | 39 | `b"boru-chat discovery-key v1"` | 8 | **Preserve** |
| 68 | src/public_room.rs | 44 | `"boru-chat"` as APPLICATION_NAMESPACE | 8 | **Preserve** |
| 69 | src/storage.rs | 1014 | `b"boru-chat/dm/request/v1"` | 8 | **Preserve** DB key |
| 70 | src/topic_derivation.rs | 16 | `b"boru-chat public-room v1"` | 8 | **Preserve** |
| 71 | src/topic_derivation.rs | 72 | `b"boru-chat room discovery v1"` | 8 | **Preserve** |
| 72 | src/topic_derivation.rs | 93 | `other boru-chat domain separators` (comment) | 6 | Yes |

### Test temporary directories using `boru-*` prefix

| # | File | Line | Match | Category | Safe to rename? |
|---|------|------|-------|----------|-----------------|
| 73 | src/chat_history.rs | 510 | `boru-chat-history-{name}-{suffix}` | 9 | Yes |
| 74 | test_interop.sh | 4, 7 | `cd /home/dan/boru-chat` | 7 | Yes |

### `iced_chat` app `name: Some("boru-chat")`

| # | File | Line | Match | Category | Safe to rename? |
|---|------|------|-------|----------|-----------------|
| 75 | examples/iced_chat/app.rs | 4939 | `name: Some("boru-chat".to_string())` | 2 (struct field) | Yes — changes the gossip node name used for identification |
| 76 | examples/iced_chat/app.rs | 5261 | `name: Some("boru-chat".to_string())` | 2 | Yes |
| 77 | tests/test_image_iced_gui_flow.rs | 168 | `name: Some("boru-chat".to_string())` | 2 (test fixture) | Yes |
| 78 | tests/test_image_iced_gui_flow.rs | 216 | `name: Some("boru-chat".to_string())` | 2 | Yes |

---

## 3. "boru_chat" (Rust identifiers, module paths)

### `use boru_chat::*` imports (massive in `app.rs` and tests)

The crate name itself is `boru_chat` (the Rust library). Every file in `src/` that is part of the library has implicit `boru_chat::` access internally. The external usages are in:

**examples/iced_chat/**: ~150+ imports using `boru_chat::...` path prefixes  
**tests/**: ~300+ imports using `boru_chat::...` path prefixes  
**src/bin/sim.rs**: `use boru_chat::proto::sim`  
**examples/dht_harness.rs**, **examples/doctor.rs**, **examples/catalogue_browser.rs**: various `use boru_chat::*` imports

| # | File Count | Category | User-facing? | Safe to rename? |
|---|-----------|----------|-------------|-----------------|
| 1 | All `use` statements (~450+) across ~40 files | 2 (Rust module identifier) | No | **Rename or preserve?** — These are Rust `use` paths referencing the crate name. If the crate is renamed from `boru_chat` to something else, ALL of these must change. This is the biggest mechanical rename task. |

### Documentation references to the library name

| # | File | Line | Match | Category | Safe to rename? |
|---|------|------|-------|----------|-----------------|
| 2 | ARCHITECTURE.md | 7 | `Rust library (boru_chat)` | 6 | Yes |
| 3 | ARCHITECTURE.md | 25 | `Core Library (boru_chat)` | 6 | Yes |
| 4 | tests/test_storage_integration.rs | 3 | `boru_chat::storage::Storage` | 6 | Yes |
| 5 | tests/test_fixture.rs | 29 | `//! use boru_chat::discovery_backend::InMemoryDiscoveryBackend;` | 6 | Yes |
| 6 | src/compression.rs | 22 | `//! let result = boru_chat::compression::compress_image(raw, 1280, 80);` | 6 | Yes |
| 7 | src/diagnostics.rs | 2005, 3034 | `/// use boru_chat::diagnostics::ExpectedState;` | 6 | Yes |
| 8 | src/public_room_config.rs | 39 | `//! use boru_chat::public_room_config::PublicRoomConfig;` | 6 | Yes |
| 9 | README.md | 8 | `Rust library (boru_chat)` | 6 | Yes |

### Test doc comments referencing `boru_chat::`

| # | File | Line | Match | Category | Safe to rename? |
|---|------|------|-------|----------|-----------------|
| 10 | tests/test_file_library_integration.rs | 4 | `boru_chat::storage::Storage API` | 6 | Yes |
| 11 | tests/test_interrupted_transfer_harness.rs | 395 | `/// Returns the Download (boru_chat::storage::Download)` | 6 | Yes |

---

## 4. "BORU_CHAT" (environment variables, constants)

| # | File | Line | Match | Category | Safe to rename? |
|---|------|------|-------|----------|-----------------|
| 1 | docs/configuration.md | 48 | `BORU_CHAT_DATA_DIR` | 5 (env var) | **Preserve** — would break existing configs |
| 2 | docs/configuration.md | 49 | `BORU_CHAT_FILES_DIR` | 5 | **Preserve** |
| 3 | docs/configuration.md | 63 | `BORU_CHAT_DATA_DIR` environment variable | 5 | **Preserve** |
| 4 | docs/configuration.md | 115-119 | `BORU_CHAT_MAX_CONCURRENT_DOWNLOADS`, `BORU_CHAT_MAX_STARTUP_DOWNLOADS`, `BORU_CHAT_MAX_DOWNLOADS_PER_PEER`, `BORU_CHAT_MAX_QUEUED_DOWNLOADS`, `BORU_CHAT_PROGRESS_DB_UPDATE_INTERVAL_MS` | 5 | **Preserve** |
| 5 | docs/message-storage-design.md | 11 | `BORU_CHAT_DATA_DIR` | 5 | **Preserve** |
| 6 | docs/message-storage-design.md | 285 | `BORU_CHAT_FILES_DIR` env var | 5 | **Preserve** |
| 7 | examples/doctor.rs | 152, 182, 499, 500 | `BORU_CHAT_DATA_DIR` | 5 | **Preserve** |
| 8 | examples/iced_chat/app.rs | 1703 | `BORU_CHAT_FILES_DIR` | 5 | **Preserve** |
| 9 | examples/iced_chat/app.rs | 3095, 3096 | `BORU_CHAT_FILES_DIR` | 5 | **Preserve** |
| 10 | examples/iced_chat/log_viewer.rs | 89, 144 | `BORU_CHAT_DATA_DIR` | 5 | **Preserve** |
| 11 | examples/iced_chat/main.rs | 103, 152 | `BORU_CHAT_DATA_DIR` | 5 | **Preserve** |
| 12 | examples/iced_chat/mcp_server.rs | 5171, 5192 | `BORU_CHAT_DATA_DIR` in env passthrough | 5 | **Preserve** |
| 13 | examples/setup.rs | 12 | `BORU_CHAT_DATA_DIR` | 5 | **Preserve** |
| 14 | src/download_limits.rs | 93, 97, 101, 105, 121, 125 | `BORU_CHAT_MAX_CONCURRENT_DOWNLOADS`, `BORU_CHAT_MAX_STARTUP_DOWNLOADS`, `BORU_CHAT_MAX_DOWNLOADS_PER_PEER`, `BORU_CHAT_MAX_QUEUED_DOWNLOADS`, `BORU_CHAT_PROGRESS_DB_UPDATE_INTERVAL_MS` | 5 | **Preserve** |
| 15 | src/file_indexer.rs | 366 | `BORU_CHAT_DATA_DIR` | 5 | **Preserve** |
| 16 | src/image_store.rs | 34 | `BORU_CHAT_DATA_DIR` (doc comment) | 5 | **Preserve** |
| 17 | README.md | 26, 192 | `BORU_CHAT_DATA_DIR` | 5 | **Preserve** |

---

## 5. "BORU_" (non-CHAT environment variables)

| # | File | Line | Match | Category | Safe to rename? |
|---|------|------|-------|----------|-----------------|
| 1 | docs/configuration.md | 50 | `BORU_PERF` | 5 | **Preserve** |
| 2 | docs/configuration.md | 51 | `BORU_PERF_PRINT` | 5 | **Preserve** |
| 3 | docs/configuration.md | 52 | `BORU_PERF_SLOW_MS` | 5 | **Preserve** |
| 4 | docs/configuration.md | 53 | `BORU_DEBUG` | 5 | **Preserve** |
| 5 | docs/configuration.md | 54 | `BORU_DEBUG_PATH` | 5 | **Preserve** |
| 6 | build.rs | 133 | `BORU_APP_VERSION` | 5 (build-time env) | Yes — compile-time constant |
| 7 | examples/iced_chat/app.rs | 227 | `option_env!("BORU_APP_VERSION")` | 5 | Yes — now reads CARGO_PKG_VERSION as fallback |
| 8 | justfile | 26, 34, 38 | `BORU_PERF=1` | 7 | Yes |
| 9 | src/gossip_debug.rs | 4, 9, 54, 137, 139, 144, 349 | `BORU_DEBUG`, `BORU_DEBUG_PATH` | 5 | **Preserve** |
| 10 | src/lib.rs | 224, 241 | `BORU_DEBUG=1`, `BORU_PERF=1` | 5 | **Preserve** |
| 11 | src/net.rs | 227 | `BORU_DEBUG` env var | 5 | **Preserve** |
| 12 | src/perf.rs | 9-18, 85, 251-266 | `BORU_PERF`, `BORU_PERF_PRINT`, `BORU_PERF_SLOW_MS` | 5 | **Preserve** |
| 13 | docs/offline-direct-messaging.md | 119 | `iroh-gossip-chat/direct/v1` (documentation reference, not env var) | 6 | Yes |

---

## 6. ".boru-chat" (data/config directories)

| # | File | Line | Match | Category | Safe to rename? |
|---|------|------|-------|----------|-----------------|
| 1 | docs/configuration.md | 67 | `$PWD/.boru-chat` | 4 | **Preserve** |
| 2 | docs/message-storage-design.md | 13 | `$PWD/.boru-chat` | 4 | **Preserve** |
| 3 | examples/doctor.rs | 169 | `.join(".boru-chat")` | 4 | **Preserve** |
| 4 | examples/iced_chat/main.rs | 169 | `.join(".boru-chat")` | 4 | **Preserve** |
| 5 | examples/setup.rs | 30 | `.join(".boru-chat")` | 4 | **Preserve** |
| 6 | README.md | 28, 192 | `$PWD/.boru-chat` and `~/.boru-chat` | 4 | **Preserve** |

---

## 7. "iroh-gossip-chat" (repository name)

### Wire protocol ALPN constants

| # | File | Line | Match | Category | Safe to rename? |
|---|------|------|-------|----------|-----------------|
| 1 | src/backfill.rs | 62 | `b"/iroh-gossip-chat/backfill/1"` (BACKFILL_ALPN) | 8 (ALPN) | **Preserve** — wire protocol; changing would break compat with existing peers |
| 2 | src/chat_core/friend_ping.rs | 36 | `b"/iroh-gossip-chat/friend-ping/1"` (FRIEND_PING_ALPN) | 8 | **Preserve** |
| 3 | src/contact.rs | 138 | `b"iroh-gossip-chat/direct/v1"` (domain for direct topic) | 8 | **Preserve** |
| 4 | src/mailbox.rs | 208 | `b"iroh-gossip-chat/mailbox/v1"` | 8 | **Preserve** |
| 5 | src/whisper/mod.rs | 42 | `b"/iroh-gossip-chat/whisper/1"` (WHISPER_ALPN) | 8 | **Preserve** |

Note: `docs/networking-audit.md` documents a migration plan from `/iroh-gossip-chat/*/1` to `/iroh-chat-*/1` ALPNs. This is already partially done for inbox (`/iroh-chat-inbox/1`), backfill (`/iroh-chat-backfill/1`), whisper (`/iroh-chat-whisper/1`), and friend ping (`/iroh-chat-ping/1`).

### Lobby topic derivation

| # | File | Line | Match | Category | Safe to rename? |
|---|------|------|-------|----------|-----------------|
| 6 | examples/iced_chat/app.rs | 3364 | `b"iroh-gossip-chat/default-lobby/v1"` | 8 | **Preserve** — changes the default lobby topic hash; all peers must re-derive |
| 7 | examples/iced_chat/app.rs | 3370 | `b"iroh-gossip-chat/personal-room/v1"` | 8 | **Preserve** |
| 8 | examples/iced_chat/mcp_server.rs | 1932 | `b"iroh-gossip-chat/default-lobby/v1"` | 8 | **Preserve** |

### Documentation references

| # | File | Line | Match | Category | Safe to rename? |
|---|------|------|-------|----------|-----------------|
| 9 | CATALOGUE_AUDIT.md | 5 | `Repo: /home/dan/iroh-gossip-chat` | 6 | Yes |
| 10 | docs/networking-audit.md | 4 | `Codebase: iroh-gossip-chat (boru-chat), commit 9ed4f23` | 6 | Yes |
| 11 | docs/networking-audit.md | 14-16, 36-38 | Various ALPN path references | 6 | Yes |
| 12 | docs/offline-direct-messaging.md | 119 | `iroh-gossip-chat/direct/v1` domain | 6 | Yes |
| 13 | docs/protocol-layers.md | 12-14, 138, 158, 188, 199 | Various ALPN references | 6 | Yes |
| 14 | run_all_tests.sh | 3 | `cd /home/dan/iroh-gossip-chat` | 7 | Yes |
| 15 | run_flaky_check.sh | 16 | `/home/dan/iroh-gossip-chat/"$binary"` | 7 | Yes |
| 16 | tests/stress_test_comprehensive.rs | 1 | `//! Comprehensive stress test for iroh-gossip-chat` | 6 | Yes |

---

## 8. "Iced Chat" / "Gossip Chat" / "chat example"

### "Iced Chat" references

| # | File | Line | Match | Category | Safe to rename? |
|---|------|------|-------|----------|-----------------|
| 1 | examples/iced_chat/app.rs | 1 | `//! The iced Application for the gossip chat frontend.` | 6 | Yes |
| 2 | examples/iced_chat/main.rs | 359 | `info!(data_dir = %data_dir.display(), "starting iced chat");` | 1 (log message) | Yes |
| 3 | docs/gui-architecture.md | 1 | `# GUI Architecture — iced_chat` | 6 | Yes |
| 4 | docs/gui-architecture.md | 3 | `The iced_chat GUI is a Iced desktop application` | 6 | Yes |
| 5 | docs/gui-architecture.md | 70 | `` `iced_chat logs` `` | 6 | Yes |
| 6 | src/diagnostics.rs | 2837 | `/// Only commands that map to existing GUI behaviour in the Iced chat` | 6 | Yes |

### "Gossip Chat" references

| # | File | Line | Match | Category | Safe to rename? |
|---|------|------|-------|----------|-----------------|
| 7 | examples/iced_chat/app.rs | 1 | `//! The iced Application for the gossip chat frontend.` | 6 | Yes |
| 8 | docs/networking-audit.md | 4 | `Codebase: iroh-gossip-chat (boru-chat)` | 6 | Yes |

### "chat example" references

No matches found.

---

## 9. `/boru-*` ALPN protocol identifiers

These are the **current** wire protocols — some already migrated, some in transition.

| # | File | Line | ALPN | Category | Safe to rename? |
|---|------|------|------|----------|-----------------|
| 1 | src/protocol_version.rs | 22 | `/boru-file-catalog/1` (CATALOGUE_ALPN) | 8 | **Preserve** — wire protocol |
| 2 | src/net.rs | 56 | `/boru-file-access/1` (FILE_ACCESS_ALPN) | 8 | **Preserve** — wire protocol |
| 3 | src/file_access_handler.rs | 594-595 | `/boru-file-access/1` (FILE_ACCESS_ALPN) | 8 | **Preserve** |

---

## 10. Other "boru" name usages

### Relay URL / domain references

| # | File | Line | Match | Category | Safe to rename? |
|---|------|------|-------|----------|-----------------|
| 1 | examples/iced_chat/main.rs | 58 | `const VPS_RELAY_URL: &str = "https://boru.chat:8443"` | 1/9 | **Preserve** — actual network endpoint |
| 2 | examples/catalogue_browser.rs | 22 | `"https://boru.chat:8443/"` | 9 | **Preserve** |
| 3 | docs/configuration.md | 15 | `--relay URL | https://boru.chat:8443` | 6 | **Preserve** |
| 4 | DHT_AUDIT.md | 169 | `https://boru.chat:8443` | 6 | **Preserve** |

### `boru.db` (SQLite database filename)

| # | File | Line | Match | Category | Safe to rename? |
|---|------|------|-------|----------|-----------------|
| 1 | src/storage.rs | 58 | `pub const DB_FILE_NAME: &str = "boru.db"` | 4 | **Preserve** — would break existing installs |
| 2 | src/storage.rs | 4878, 4920, 5023 | `dir.path().join("boru.db")` | 4 | **Preserve** |
| 3 | ARCHITECTURE.md | 50, 178 | `boru.db` references | 4 | **Preserve** |
| 4 | docs/configuration.md | 140 | `boru.db` | 4 | **Preserve** |
| 5 | docs/message-storage-design.md | 24, 51, 66, 291 | `boru.db` | 4 | **Preserve** |
| 6 | docs/offline-direct-messaging.md | 148 | `boru.db` | 4 | **Preserve** |
| 7 | docs/gui-architecture.md | 136 | `boru.db` | 4 | **Preserve** |
| 8 | examples/iced_chat/main.rs | 575, 577 | `boru.db` | 4 | **Preserve** |
| 9 | README.md | 34, 54 | `boru.db` | 4 | **Preserve** |
| 10 | tests/test_storage_integration.rs | 686 | `boru.db` | 4 | **Preserve** |

### Script names / paths

| # | File | Line | Match | Category | Safe to rename? |
|---|------|------|-------|----------|-----------------|
| 1 | scripts/boru-test-instance.sh | 3, 6, 25, 47, 51, 55, 56, 62, 105-108 | `boru-test`, `iced_chat-*` binary names | 7 | Yes (internal scripts) |
| 2 | docs/testing.md | 368 | `scripts/boru-test-instance.sh` | 6 | Yes |
| 3 | add-test-file.py | 26, 35 | `/tmp/boru-live-54/` test paths | 9 | Yes |

### Test temp directory names using `boru-` prefix

| # | File | Line | Match | Category | Safe to rename? |
|---|------|------|-------|----------|-----------------|
| 1 | src/conversations.rs | 508 | `boru-conversations-{name}-{suffix}` | 9 | Yes |
| 2 | src/friend_request.rs | 679 | `boru-friend-requests-{name}-{suffix}` | 9 | Yes |
| 3 | src/friends.rs | 543 | `boru-friends-{name}-{suffix}` | 9 | Yes |
| 4 | src/outbox.rs | 510 | `boru-outbox-{name}-{suffix}` | 9 | Yes |
| 5 | src/room.rs | 233 | `boru-room-{name}-{suffix}` | 9 | Yes |
| 6 | src/room_cleanup.rs | 101 | `boru-room-cleanup-{name}-{suffix}` | 9 | Yes |
| 7 | src/room_history.rs | 237 | `boru-room-history-{name}-{suffix}` | 9 | Yes |
| 8 | src/file_indexer.rs | 147 | `"boru-shared-folder-watch"` (tokio task name) | 9 | Yes |
| 9 | src/file_indexer.rs | 372 | `.join("boru")` | 9 | Yes |
| 10 | examples/iced_chat/app.rs | 15172, 15195, 15214, 15252 | `boru-confirmed-invite-*`, `boru-gui-dark-mode-test-*` temp dirs | 9 | Yes |
| 11 | examples/iced_chat/app.rs | 16963 | `boru-iced-chat-join-request-{suffix}` | 9 | Yes |
| 12 | tests/test_conversation_integration.rs | 49 | `boru-conv-int-{name}-{suffix}` | 9 | Yes |
| 13 | tests/test_crash_recovery.rs | 47 | `boru-crash-recovery-{name}-{pid}-{nanos}` | 9 | Yes |
| 14 | tests/test_friend_request_e2e.rs | 42 | `boru-fr-e2e-{name}-{suffix}` | 9 | Yes |
| 15 | tests/test_interrupted_transfer_harness.rs | 89 | `boru-transfer-harness-{name}-{pid}-{nanos}` | 9 | Yes |
| 16 | tests/test_message_lifecycle.rs | 30 | `boru-lifecycle-{name}-{suffix}` | 9 | Yes |
| 17 | tests/test_offline_delivery_integration.rs | 63 | `boru-offline-{name}-{suffix}-{nanos}` | 9 | Yes |
| 18 | tests/test_storage_integration.rs | 48 | `boru-storage-int-{name}-{suffix}-{nanos}` | 9 | Yes |

---

## Summary Counts by Category

| Category | Count (approx) | Must Preserve | Safe to Rename |
|----------|---------------|---------------|----------------|
| 1. User-facing branding | ~15 | 0 | ~15 |
| 2. Rust crate/module identifier | ~450 (`use` statements) + ~75 doc refs | Crate name itself | `use` paths if crate renamed |
| 3. Package metadata | ~6 | Crate name in Cargo.toml | Repository URL |
| 4. Storage path | ~40 | **~30** (runtime data paths, DB filename) | ~10 (test temp paths) |
| 5. Environment variable | ~40 | **~38** (all `BORU_CHAT_*` and `BORU_*`) | ~2 (build.rs compile-time) |
| 6. Documentation | ~80 | 0 | ~80 |
| 7. CI/build/packaging | ~15 | 0 | ~15 |
| 8. Legacy compatibility | ~25 | **~25** (ALPNs, domain separators, DB keys) | 0 |
| 9. Other (test temp dirs) | ~25 | 0 | ~25 |

## Key Decisions Required

Before renaming, the following must be decided:

1. **What is the new crate name?** `boru-chat` → `?`
   - This is the single biggest change — ~450 `use boru_chat::*` lines in tests and the GUI example
   - The Rust module/import path changes everywhere

2. **Storage paths**: Must either migrate existing data or maintain backward compat
   - `~/.local/share/boru-chat/` → `~/.local/share/<new-name>/`
   - `boru.db` filename
   - All `BORU_CHAT_DATA_DIR` env var handling

3. **Wire protocol ALPNs**: These are **permanent** compatibility concerns
   - `/boru-file-catalog/1`, `/boru-file-access/1` 
   - Domain separators like `b"boru-chat private-room v1"` are cryptographic — renaming them changes namespace derivation
   - `/iroh-gossip-chat/*/1` ALPNs — these are already partially migrated to `/iroh-chat-*/1` per `docs/networking-audit.md`

4. **Environment variables**: Old values like `BORU_CHAT_DATA_DIR` need graceful fallback
   - Can accept both old and new names during a deprecation period

5. **Relay URL**: `https://boru.chat:8443` — this is a DNS/network endpoint; separate from code renaming

6. **GitHub repository**: `dmahony/iroh-gossip-chat` → `dmahony/<new-repo-name>`
   - Update `Cargo.toml` repository field, `cliff.toml` templates, scripts with hardcoded paths
