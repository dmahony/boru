# Testing Infrastructure

Boru has a comprehensive test suite spanning unit tests, integration tests,
deterministic simulation, and GUI tests.

## Test Layout

```
tests/
├── sim.rs                              # Deterministic protocol simulation
├── room_e2e.rs                         # End-to-end room messaging
├── test_three_peer_mesh.rs             # Three-peer mesh connectivity
├── test_message_lifecycle.rs           # Full message send/receive/ack lifecycle
├── test_message_transfer.rs            # Message transfer between two peers
├── test_storage_integration.rs         # SQLite storage layer tests
├── test_file_library_integration.rs    # File library operations tests
├── test_conversation_integration.rs    # Conversation management tests
├── test_mcp_diagnostics_integration.rs # MCP diagnostic server tests
├── test_friend_request_e2e.rs          # Friend request end-to-end flow
├── test_friend_ticket_persistence.rs   # Friend ticket persistence
├── test_public_lobby_integration.rs    # Public lobby discovery tests
├── test_private_room_dht_discovery.rs  # Private room DHT tests
├── test_private_room_invitation_discovery.rs
├── test_performance_baseline.rs        # Performance baseline measurement
├── test_performance_regression.rs      # Performance regression detection
├── stress_test_comprehensive.rs        # Stress test suite
├── test_blob_size_enforcement.rs       # Blob transfer size limits
├── test_image_iced_gui_flow.rs         # Image send/download through GUI
├── test_image_receiver_download.rs     # Image download flow
├── test_image_send_download.rs         # Image send → download round trip
├── test_image_cache_persistence.rs     # Image cache durability
├── test_iced_chat_flow.rs              # GUI chat flow
├── test_full_chat_list_flow.rs         # Full chat list navigation
├── test_multi_image_burst.rs           # Burst image message test
├── test_online_user_list.rs            # Online user list test
├── test_signed_gossip_flow.rs          # Signed gossip message flow
├── test_serde_format.rs                # Serialization format stability
├── test_security.rs                    # Security edge cases
├── test_no_bootstrap.rs                # Bootstrap-free operation
├── test_stale_bootstrap.rs             # Stale bootstrap handling
├── test_local_address_lookup.rs        # mDNS address lookup tests
├── test_room_invite_v2.rs              # Room invite V2 flow
├── test_two_peers_exchange.rs          # Two-peer message exchange
├── test_two_peers_relay.rs             # Two-peer relay connectivity
├── stale_bootstrap.rs                  # Stale bootstrap edge cases
├── repro_two_iced_instances.rs         # Two concurrent GUI instances
├── mailbox.rs                          # Mailbox protocol tests (encryption, signatures, idempotent acceptance)
├── image_optimizer_integration.rs      # Image optimization tests
├── compression_integration.rs          # Compression integration tests
├── verify_gui_bootstrap.rs             # GUI bootstrap verification
├── gen_stress_data.rs                  # Stress test data generator
├── generate_test_images.py             # Script: generate test images
│                                       #
│   ── Fixture / Harness ──            #
├── test_fixture.rs                     # Reusable TwoPeerFixture (deterministic identities, MemoryLookup, gossip)
├── test_catalogue_harness.rs           # CatalogueHarness (two-peer localhost catalogue handler + client)
├── test_deterministic_harness.rs       # DeterministicHarness (identity, contact, mailbox, gossip lifecycle)
│                                       #
│   ── Catalogue Tests ──              #
├── test_malformed_catalogue.rs         # 20 malformed-catalogue rejection scenarios
├── test_stable_identities.rs           # 31 identity stability + visibility tests
├── test_peer_lifecycle.rs              # 26 peer lifecycle + catalogue update tests
├── test_catalogue_harness.rs           # 7 catalogue harness coverage tests
│                                       #
│   ── Download Tests ──               #
├── test_download_integration.rs        # Download lifecycle (init, progress, completion)
├── test_download_queue_order.rs        # Download queue FIFO ordering
├── test_download_initiation_integration.rs  # Download initiation state machine
├── test_download_recovery.rs           # Download resumption after interruption
├── test_normal_downloads.rs            # Normal download flow
├── test_corrupted_content.rs           # Corrupted content handling
│                                       #
│   ── Resilience Tests ──             #
├── test_crash_recovery.rs              # Recovery from abrupt crashes
├── test_sync_after_downtime.rs         # Sync after peer downtime
├── test_interruption_restart.rs        # Interruption and restart cycles
├── test_pause_scenarios.rs             # Pause/resume scenarios
├── test_restart_storm_prevention.rs    # Restart storm prevention
├── test_resource_exhaustion.rs         # Resource exhaustion limits
│                                       #
│   ── Security Tests ──               #
├── test_hostile_input.rs               # 61 hostile-input rejection scenarios
├── test_security.rs                    # Security edge cases (overwrite, replay, flood)
├── test_malicious_filenames.rs         # Path traversal, control chars, injection in filenames
├── test_metadata_security.rs           # Metadata tampering detection
│                                       #
│   ── Other ──                        #
├── test_outgoing_dm_transaction.rs     # DM transaction states
├── test_ack_processing.rs              # ACK lifecycle and retry
├── test_delivery_failure.rs            # Delivery failure handling
├── test_offline_delivery_integration.rs  # Offline message delivery
├── test_resume_integration.rs          # Download resume integration
├── test_performance_regression.rs      # Performance regression detection
└── test_performance_baseline.rs        # Performance baseline
```

## Running Tests

```sh
# Run all tests (requires net + test-utils features)
cargo test --features net,test-utils

# Run GUI tests (requires gui feature)
cargo test --features gui

# Run a specific test
cargo test --test test_message_lifecycle --features net,test-utils

# Run with full output
cargo test -- --nocapture

# Run storage integration tests
cargo test --test test_storage_integration --features net
```

## Deterministic Simulation

The `sim` test at `tests/sim.rs` and the `sim` binary at `src/bin/sim.rs`
provide a deterministic simulation framework for the gossip protocol.

### Feature: `simulator`

The `simulator` feature enables the `sim` binary with:
- `tracing-subscriber` for structured logging
- `toml` for simulation configuration
- `clap` for CLI argument parsing
- `serde_json` for output serialization
- `rayon` for parallel simulation execution
- `comfy-table` for formatted output

### Running the Simulation

```sh
# Run the sim test
cargo test --test sim --features simulator

# Run the sim binary
cargo run --bin sim --features simulator -- --help

# With a config file
cargo run --bin sim --features simulator -- --config simulations/all.toml
```

## Test Patterns

### Delivery State Machine Tests

The delivery state machine in `src/delivery_state.rs` has exhaustive tests
covering:

- All happy-path transitions (Queued → Sending → SentAwaitingAck → Acknowledged)
- Retry loop (Sending → RetryScheduled → Sending)
- Failure / expiry / cancellation from every non-terminal state
- Identity (no-op) transitions for every state
- Invalid backwards transitions from every state
- Invalid skip transitions (e.g. Queued → Acknowledged)
- Terminal-state immutability (Acknowledged, FailedPermanent, Expired, Cancelled)

These tests use the FSM's `can_transition_to()` function directly without IO
or networking, making them fast and deterministic.

### Integration Tests (two peers)

Most integration tests follow a pattern:
1. Spawn two (or more) iroh endpoints with in-memory storage
2. Subscribe both to the same gossip topic
3. Send a message from peer A
4. Assert peer B receives it
5. Verify delivery state (ACKs, storage)

Example from `test_message_transfer.rs`:
```rust
#[tokio::test]
async fn test_basic_message_transfer() -> Result<()> {
    let (ep1, gossip1, _) = make_endpoint_and_gossip(1).await?;
    let (ep2, gossip2, _) = make_endpoint_and_gossip(2).await?;
    // ... subscribe, send, verify
}
```

### Catalogue Tests

The catalogue suite validates signed file catalogue retrieval, caching, and
visibility enforcement. Tests use two purpose-built harnesses:

**`CatalogueHarness`** (`tests/test_catalogue_harness.rs`)
A lightweight two-peer localhost harness with direct QUIC connections (no relay).
Covers 7 scenarios:

| Test | Coverage |
|------|----------|
| `catalogue_harness_is_deterministic` | Deterministic identities, identical keys across runs |
| `catalogue_harness_covers_every_permission_visibility_rule` | All visibility rules (friend, non-friend, blocked, explicit grant, deny, disabled offer) |
| `catalogue_harness_supports_stop_restart_and_updates` | Peer restart preserves catalogue content; updates visible after restart |
| `catalogue_cache_stale_and_offline_states` | Stale/not-modified/offline states retain cached profile |
| `catalogue_cache_first_fetch_not_modified_and_revision_replacement` | NotModified on known revision, replacement on change |
| `catalogue_invalid_signature_is_rejected_before_cache_replacement` | Invalid signature never replaces cached entry |
| `catalogue_revision_change_during_paginated_fetch_is_not_cached` | In-flight pagination with revision change is not cached |

**`TwoPeerFixture`** (`tests/test_fixture.rs`)
A comprehensive fixture with deterministic identities, in-memory discovery, local
relay, gossip subscription, and controllable contact visibility. Shared across
three test modules below (15 fixture baseline tests run in each).

**Malformed Catalogue** (`tests/test_malformed_catalogue.rs` — 20 tests)

| Scenario | What It Verifies |
|----------|-----------------|
| `garbage_bytes` | Random bytes are rejected without crash |
| `truncated_postcard` | Incomplete postcard frame is rejected |
| `unsupported_version` | Unsupported protocol version returns error |
| `file_details_variant` | Wrong variant (FileDetails) is rejected |
| `catalogue_page_variant` | Wrong variant (CataloguePage) is rejected |
| `error_response` | Wire error response is handled safely |
| `invalid_signature` | Valid structure, bad signature is rejected |
| `wrong_owner_id` | Catalogue signed by wrong key is rejected |
| `duplicate_shared_file_id` | Duplicate `shared_file_id` in one catalogue |
| `duplicate_content_hash` | Duplicate content hashes in one catalogue |
| `duplicate_collection_ids` | Duplicate collection ids in one catalogue |
| `empty_shared_file_id` | Empty `shared_file_id` field rejected |
| `empty_display_name` | Empty display name rejected |
| `invalid_mime_type` | Malformed MIME type rejected |
| `oversized_catalogue` | Payload over size limit rejected |
| `empty_response_frame` | Zero-length frame rejected |
| `oversized_response_payload` | Oversized raw response payload rejected |
| `dangling_collection_reference` | Collection referencing non-existent file |
| `recovery_after_malformed` | Valid → garbage → valid cycle recovers cleanly |
| `valid_then_garbage` | Valid then garbage doesn't crash client |

**Stable Identities** (`tests/test_stable_identities.rs` — 31 tests including 15 fixture baseline)

| Test | Coverage |
|------|----------|
| `identity_preserved_across_peer_restart` | Key identity unchanged after restart |
| `identity_stable_across_repeated_observations` | Same key observed over multiple fetches |
| `non_contact_sees_empty_catalogue` | Non-contact peer sees empty catalogue |
| `non_contact_sees_only_explicitly_granted_file` | Non-contact sees only explicitly `read`-granted file |
| `selected_peer_sees_explicitly_granted_file` | Selected peer sees their granted file |
| `non_friend_sees_empty_catalogue_after_friendship_sees_files` | Friends see files; non-friends after removal see empty |
| `removed_contact_has_empty_catalogue` | Removed contact has empty catalogue |
| `removing_friendship_hides_entries` | Changing friend back to non-friend hides files |
| `blocked_peer_gets_permission_denied` | Blocked peer receives PermissionDenied |
| `blocked_peer_gets_permission_denied_at_boundary` | Blocked at visibility boundary |
| `multiple_files_visibility_consistent_across_changes` | Multiple files consistent across friend/block/remove |
| `both_peers_verify_symmetric_visibility` | Both peers verify opposite visibility state |
| `explicit_grant_works_within_contact_relationship` | Explicit grant within a contact relationship |
| `explicit_deny_hides_file_from_friend` | Explicit deny overrides friendship for that file |
| `disabled_offer_hidden_from_catalogue` | Disabled offer is hidden from catalogue |
| `missing_file_object_hidden_from_catalogue` | File without storage object is hidden |

**Peer Lifecycle** (`tests/test_peer_lifecycle.rs` — 26 tests including 15 fixture baseline)

| Test | Coverage |
|------|----------|
| `offline_updates_seen_after_restart` | Updates made while peer offline visible after restart |
| `both_peers_shutdown_and_restart` | Both peers shutdown and restart with persisted state |
| `peer_goes_offline_adds_happen_peer_restarts` | Offline → adds → restart → sees updates |
| `repeated_start_stop_cycles_no_duplicates` | Repeated start/stop produces no duplicate entries |
| `both_peers_update_after_alternating_restarts` | Both peers update across alternating restarts |
| `repeated_updates_are_deterministic` | Repeated multi-batch updates deterministic |
| `visibility_transitions_across_restarts` | Friend/block/restart visibility preserved |
| `symmetric_restart_preserves_catalogue_access` | Symmetric restart preserves access for both |
| `accepted_contact_sees_contacts_only_files` | Accepted contact sees their files |
| `explicit_grant_makes_file_visible_with_and_without_friendship` | Explicit grant works with and without friendship |
| `selected_peer_sees_granted_file_without_friendship` | Non-friend sees explicitly granted file |

**Deterministic Harness** (`tests/test_deterministic_harness.rs` — 12 tests)

| Test | Coverage |
|------|----------|
| `test_two_peers_connect_and_exchange_gossip` | Gossip connectivity between two peers |
| `test_identities_survive_restart` | Identities persist across peer restart |
| `test_stop_start_restart_cycle` | Repeated stop/start cycles work |
| `test_contact_establishment` | Contact establishment between peers |
| `test_mailbox_key_exchange` | Mailbox key exchange works |
| `test_no_public_infrastructure` | No external discovery service required |
| `test_observe_network_events` | Network events observed correctly |
| `test_address_change` | Address change events propagated |
| `test_deterministic_keys` | Keys are deterministic across runs |
| `test_event_log` | Event log captures lifecycle events |
| `test_bounded_timeouts` | Timeouts are bounded (no indefinite waits) |
| `test_persistent_temp_profiles` | Profiles persist across stop/start |

All 95+ catalogue-related tests pass deterministically with `--test-threads=1`
and require zero external network infrastructure.  They use `MemoryLookup` for
address resolution and run entirely on localhost.

### Storage Tests

Storage tests use in-memory or temp-directory SQLite databases to verify:
- CRUD operations on inbox, outbox, file objects
- Migration correctness (V1 → V2 → V3 → V4)
- Crash recovery (simulated crashes via incomplete operations)
- Tombstone semantics (local/remote deletion, re-insertion prevention)
- File library lifecycle (import, reference, change detection, cleanup)

### MCP Tests

The MCP diagnostic server is tested via TCP connections in
`test_mcp_diagnostics_integration.rs`:
1. Start the application with `--mcp` enabled
2. Connect to the MCP port
3. Send JSON-RPC 2.0 requests
4. Assert responses match expected shapes

### Performance Tests

- `test_performance_baseline.rs` — measures message throughput, latency, and
  memory usage on reference hardware
- `test_performance_regression.rs` — compares current results against stored
  baseline, failing if degradation exceeds configured thresholds
- `stress_test_comprehensive.rs` — multi-peer message burst tests

## Test Features

| Feature | Required For |
|---------|--------------|
| `net` | All networking tests (gossip, inbox, backfill, whisper) |
| `test-utils` | Deterministic RNG (`chacha`), `humantime-serde` for config |
| `gui` | GUI tests (image optimization, iced integration) |
| `simulator` | Deterministic simulation binary |

## Remote file-sharing tests

The remote catalogue and transfer tests should verify the stages independently:

1. **Catalogue signing** — a generated catalogue verifies with the owner's key;
   changing any signed field, owner, revision, or file metadata is rejected.
2. **Requester filtering** — friends see enabled available offers, explicit
   `read` grants expose only granted files to non-friends, blocked peers receive
   access denied, and disabled/unavailable files are omitted. Build catalogues
   for two requesters and assert their projections are isolated.
3. **Revision/cache behaviour** — `known_revision` returns `NotModified` for an
   unchanged requester-specific view, a changed revision returns fresh data,
   and stale/manual/missing-file refresh is initiated by the caller. There is
   no separate push-notification or continuous polling worker.
4. **Authorisation** — access is re-evaluated after catalogue creation;
   changed hashes/versions, disabled offers, missing data, blocked peers, and
   expired descriptors are denied. A descriptor must be bound to the transport
   requester and fail signature/expiry verification when tampered with.
5. **Blob transfer** — exercise successful iroh-blobs download, wrong hash,
   wrong size, provider failure, cancellation, pause/resume, atomic destination
   installation, and cleanup of temporary output. Verify that only verified
   BLAKE3 content is installed. Resume must re-resolve the peer and obtain a
   fresh descriptor; retained iroh-blobs chunks may be reused, but the
   destination file is not resumed by byte offset.
6. **Abuse limits** — test request/response/catalogue size caps, pagination
   bounds, per-peer/global concurrency, rate limits, timeouts, and upload
   preparation limits. Assertions should use structured protocol errors rather
   than log text.

A complete end-to-end scenario is: publish an enabled local file, expose its
signed requester-filtered catalogue, fetch and verify that catalogue, request
fresh authorization, fetch through iroh-blobs, and assert size/hash plus
durable download state. A revision change can be exercised by sending
`known_revision` and checking `NotModified`, then changing the manifest or
permissions and fetching again. Run the same scenario from both peers where
applicable; a successful catalogue fetch is not evidence that transfer
authorization or blob delivery succeeded. There is no continuous catalogue
notification/polling worker to test; refresh is triggered by the caller.

## Scripts

| Script | Purpose |
|--------|---------|
| `scripts/lint.sh` | Wrapper for clippy that handles edition correctly |
| `scripts/flamegraph.sh` | Generate CPU flamegraphs (requires `cargo-flamegraph`) |
| `scripts/boru-test-instance.sh` | Launch a test GUI instance for manual testing |
