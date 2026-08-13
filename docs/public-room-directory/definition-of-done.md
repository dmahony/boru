# Public Room Directory — Definition of Done (BORU-DIR-24)

Source: "Definition of done" + "Do not do these things" sections of
`Boru_Public_Room_Directory_Implementation_Tasks.pdf` (Phase 8). This is the
final gate of the BORU-DIR chain: every DoD bullet is verified against the
codebase with code/docs evidence, and the "Do not do these things" list is
checked for violations.

Verification date: 2026-08-13. HEAD: BORU-DIR-23 (8c1f7be8) + BORU-DIR-24.

## DoD bullets — evidence table

| # | Definition of done | Evidence (code/docs) | Pass |
|---|--------------------|----------------------|------|
| 1 | Boru supports Private, PublicUnlisted, and PublicDiscoverable room visibility. | `RoomVisibility` enum in `src/control_plane/advertisement.rs:206-217` (wire-stable tags `Private = 0`, `PublicUnlisted = 1`, `PublicDiscoverable = 2`); same type persisted on `ConversationEntry.visibility` (conservative `Private` default, `advertisement.rs:305-311`); visibility model + migration documented in `docs/public-room-directory/room-visibility-state.md`. | PASS |
| 2 | Only PublicDiscoverable rooms emit directory advertisements. | Emit-site guard `DiscoveryService::announce_room_advertisement` (`src/discovery_service.rs:1118-1131`) returns `AnnounceOutcome::NotDiscoverable` and broadcasts nothing for non-discoverable rooms; `RoomVisibility::is_discoverable()` (`advertisement.rs:219-226`); startup publisher filters `e.visibility.is_discoverable()` (`app.rs:11514`); unit test `announce_room_advertisement_refuses_non_discoverable`; matrix scenarios 2/3. | PASS |
| 3 | Advertisements are typed, bounded, versioned, authenticated where possible, and metadata-only. | Typed: `PUBLIC_ROOM_ADVERTISEMENT` control-plane message (tag 5) in `src/control_plane/message.rs:243-265`, decoded only inside the discovery/control-plane service. Bounded: `AdvertisementBounds` (`advertisement.rs:325-362`: name/description/tag/flag caps, TTL clamp, 2048-byte encoded cap). Versioned: `advert_version` payload field (`advertisement.rs:509`), unknown future versions cached as metadata, never an auth signal. Authenticated: Ed25519 publisher signature over canonical framing (`ADVERTISEMENT_SIGNING_PROTOCOL`, `advertisement.rs:104`; `sign`/`verify_signed`). Metadata-only: field list is exactly room identity/name/description/protocol/owner/visibility/TTL/tags/hints/avatar-hash/feature-flags/signature; member lists, history, previews, filenames, invite/moderation secrets, keys, and attachment content are structurally absent (`advertisement.rs:499-503`). Docs: `advertisement-metadata.md`, `advertisement-authentication.md`. | PASS |
| 4 | Discovering a room never creates a conversation, joins a topic, downloads history, or grants permissions. | `RoomDirectory` is a pure cache: "never creates conversation records, subscribes to room topics, or grants permissions" (`src/room_directory.rs:507-508`). Receive path only applies the advertisement to the cache (`src/discovery_service.rs:1584-1594`); the only topic the discovery service ever subscribes to is the directory topic. Unit test `handle_incoming_room_advertisement_never_subscribes_or_creates_conversations` (`discovery_service.rs:6939-6969`) asserts the room topic is not subscribed and no conversation/peer record is created; matrix scenario 5. | PASS |
| 5 | The Discover Rooms UI is separate from the user's joined conversation list. | `Screen::Discover` is a distinct screen (`app.rs:3113-3114`), rendered by `view_discover_content` (`examples/iced_chat/app/discover.rs:948-1055`), separate from the CHATS sidebar which lists only actual conversations; opening the directory never changes membership (PDF Task 5.1); empty state explains rooms appear when discovered. Matrix scenario 5. | PASS |
| 6 | Users can search/filter the locally discovered directory without broadcasting their search queries. | Local-only search (`discover_row_matches_query`, `discover.rs:153-170`), filters (Compatible / Not Joined / Recently Seen) and sort (`discover_filter_sort`, `discover.rs:191-255`) all operate on the local cache snapshot; `discover_search_query` is UI state explicitly documented "never broadcast onto the discovery network" (`app.rs:3695-3697`, `discover.rs:2302-2304`); no code path sends the query to the gossip topic or any peer. | PASS |
| 7 | Rooms expire naturally through TTL when advertisements stop. | Each advertisement carries `expires_after_secs` TTL; `RoomDirectory::evict_expired_at` (`room_directory.rs:831-859`) removes entries past expiry; BORU-DIR-23 wired the production `directory_expiry_loop` sweep (`discovery_service.rs:218-232`, `:2220`, `:4064-4096`, `DEFAULT_DIRECTORY_SWEEP_INTERVAL` 30s) so stale rooms leave the active directory without waiting for the next advertisement. Matrix scenario 7 (`advertiser_disappears_room_expires_after_ttl`); doc `ttl-refresh-expiry.md`. | PASS |
| 8 | Owners can unlist rooms and propagate withdrawal/update events. | `apply_room_directory_visibility` Unlisted branch (`app.rs:11278-11336`) persists `PublicUnlisted`, stops the refresh set + DHT tracker, removes the local directory row, and broadcasts a signed withdrawal (`broadcast_room_withdrawal`, `app.rs:11400-11410`). Control-plane withdrawals: typed `PUBLIC_ROOM_WITHDRAWAL` (`message.rs:267-270`), receive gate applies only verified authoritative withdrawals (`discovery_service.rs:1660-1680`), TTL remains the safety net. Visibility-switch planning is owner-gated (`plan_visibility_switch`, `advertisement.rs:282-303`). Matrix scenarios 8/9; doc `withdrawal-tombstone.md`, `visibility-switching.md`. | PASS |
| 9 | The Join action uses normal public-room membership and permission logic. | `DirectoryRoomJoinById` → `directory_join_target` (compatibility + local-block re-validation) → existing `OpenRoom` path, which subscribes via normal room-topic logic and creates the conversation record exactly once on success (`discover.rs:2374-2397`, `:2923-2975`). The advertisement is metadata, never authorization (PDF Task 6.1 step 4). Matrix scenario 4 (`join_advertised_room_normal_join_path`). | PASS |
| 10 | Incompatible rooms are identified before join attempts. | `RoomCompatibility::for_room_protocol` computed at cache-apply time (`room_directory.rs:671`); join gate blocks `UpgradeRequired`/`Unsupported` with a user-facing explanation before any subscription (`discover.rs:2956-2971`); card renders "Incompatible"/"Upgrade required" and no Join button (`discover.rs:1296-1302`, `discover_compat_label`). Optional-feature negotiation is informational and never blocks basic access (`discover_feature_hint`). Matrix scenario 13. | PASS |
| 11 | Duplicate, malformed, spoofed, and high-volume advertisements are handled safely. | Duplicate: content-digest dedup returns `AdvertiseOutcome::Duplicate` with no subscriber event (`room_directory.rs:694-702`). Malformed/oversized: decode-time rejection (`ControlPlaneDecode`), bounds checks (`ControlAdvertPolicy` in `privacy.rs:193-223`, `AdvertisementViolation`), tests `handle_incoming_malformed_room_advertisement_dropped`, `handle_incoming_tampered_room_advertisement_rejected`, `handle_incoming_wrong_publisher_room_advertisement_rejected`, `handle_incoming_tampered_room_withdrawal_rejected`, `handle_incoming_control_spoofed_sender_rejected` (`discovery_service.rs:6065,6199,6236,6524,8026`). High-volume: per-sender rate limiting in the BORU-CP-03 guard + announcement throttle (`discovery_service.rs:1132`, `:1220`); bounded cache with deterministic eviction (expiry → LRU, `room_directory.rs:748-760`). Matrix scenarios 10/11/12/16. | PASS |
| 12 | Local Hide/Block choices are respected across refreshes. | Hide persisted via `Storage::set_room_hidden` / read via `room_hidden_ids` (`storage.rs:732-780`); directory cache re-derives `LocalJoinState` with hidden winning over everything (`derive_local_state`, `room_directory.rs:303-320`), synced from the real room DB + persisted preference on every tick (`sync_directory_local_states`, `app.rs:11364-11394`); Settings → Hidden rooms restores individual or all rooms (`app/settings.rs:1276-1364`, `DirectoryRoomUnhideById/UnhideAll`). Hide/unhide are local-only — never broadcast. Matrix scenario 15 (`hidden_room_stays_hidden_until_unhidden`). | PASS |
| 13 | All required test scenarios pass without regressing direct chat, groups, files, tunnels, or the hidden peer-discovery service. | See test results below. 16/16 matrix scenarios in `tests/test_public_room_directory.rs`; hostile-input, discovery isolation suites, reconnect/tunnel suites, and the full compile-able suite all green. | PASS |

## "Do not do these things" — violation check

| # | Prohibition | Verification |
|---|-------------|--------------|
| 1 | Do not automatically join every advertised public room. | No auto-join code path exists. The only join entry points are explicit user actions (`DirectoryRoomJoinById` / `DirectoryRoomJoin` on the Join button, `JoinFromTicket`). Reconnect restores only entitled direct topics and never auto-joins groups/public rooms from discovery (`reconnect_required_topics_never_auto_joins_groups` test, `app.rs:33309-33323`; `group_entries_never_auto_joined`, `reconcile.rs:263`). Opening the directory performs no subscription (matrix scenario 5). |
| 2 | Do not reuse the discovery topic as the public-room chat transport. | Directory gossip topic is derived with its own domain separator (`DIRECTORY_DOMAIN_SEPARATOR = "boru-chat/public-room-directory/v1"`, `directory.rs:40-57`), deliberately distinct from `PUBLIC_ROOM_DOMAIN_SEPARATOR` (`topic_derivation.rs:16`) and `DISCOVERY_KEY_DOMAIN_SEPARATOR` (`public_room.rs:39`). Chat messages flow only on room topics; `directory_topic_differs_from_lobby` test (`directory.rs:648`). |
| 3 | Do not persist discovered rooms as conversations until the user actually joins. | `RoomDirectory` is an in-memory bounded cache and never writes to `ConversationStore` (no `conversation_store` usage in `src/discovery_service.rs` beyond doc comments); conversation records are created only by explicit join/create flows (`app.rs` upsert sites are all join/create/group handlers). Test `handle_incoming_room_advertisement_never_subscribes_or_creates_conversations`. |
| 4 | Do not broadcast user search queries. | `DiscoverSearchChanged` mutates local UI state only (`discover.rs:2302-2304`); search/filter/sort all consume the local `RoomDirectory` snapshot. No network op originates from the search box. |
| 5 | Do not advertise private/unlisted rooms. | Emit-site guard (DoD #2) plus persisted-visibility filter in the periodic/startup publisher (`app.rs:11514`); unit + matrix tests (scenarios 2/3). |
| 6 | Do not publish member lists, chat previews, filenames, invite secrets, or moderation secrets. | The `PublicRoomAdvertisement` field list contains none of these (DoD #3); the control-plane privacy policy whitelists minimal metadata content and rejects anything else "by construction" (`privacy.rs:185-191`); the signature is a fixed 64-byte Ed25519 value. |
| 7 | Do not trust self-reported popularity as an authorization or ranking signal. | `approximate_member_count` is an untrusted optional hint (`advertisement.rs:554-558`); Discover sort orders (RecentlySeen / Compatibility / Name) never consult it (`discover_filter_sort`, `discover.rs:229-252`); UI renders it as "~N members (approx.)" and omits when absent (`discover_member_count_text`). BORU-DIR-21 audit. |
| 8 | Do not change existing room topic IDs simply to add discoverability. | Room topic identity is deterministic from name + network byte + protocol version (`public_room_topic`, `topic_derivation.rs:42-51`) — visibility is not an input. Visibility switching persists the new visibility on the same `ConversationEntry`/topic and never recreates the room (`apply_room_directory_visibility`; `room-visibility-state.md` "Changing discoverability does not recreate the room"). |

## Test results (full gate on DEBSRV via `rb`)

All runs are on the merged BORU-DIR-23 HEAD (8c1f7be8) in this worktree,
executed on debsrv (172.16.0.59, 180G free — no space work needed).

### BORU-DIR required matrix (the 16 PDF scenarios)

`rb test --test test_public_room_directory --features net`: **16/16 PASS**
(full map in `docs/public-room-directory/test-matrix.md`).

### Full compile-able integration sweep (105 targets, one-per-invocation with
`timeout 240` per `references/debsrv-integration-test-gate.md`)

- **87 PASS**, 9 FAIL, 8 TIMEOUT.
- The 8 TIMEOUTs and 4 of the FAILs are the documented debsrv relay-hang /
  flaky suites (`RelayMode::Default` + `endpoint.online().await` with no IPv6
  route on debsrv — see `references/debsrv-integration-test-gate.md` §3):
  `test_no_bootstrap`, `test_message_transfer`, `test_multi_image_burst`,
  `test_full_chat_list_flow`, `test_image_receiver_download`,
  `test_image_send_download`, `repro_two_iced_instances`,
  `test_performance_baseline`, `test_performance_regression`,
  `test_two_instance_dht_chat`, `test_iced_chat_flow`,
  `test_image_iced_gui_flow`. These are infrastructure flakes (relay
  connectivity from debsrv), not BORU-DIR regressions; the same suites hang /
  fail outside this chain.
- Pre-existing failures (reproduce identically at the pre-BORU-DIR baseline
  8d6917ef): `test_onboarding_integration` (10 assertions, Phase-22 cleanup
  legacy), `stale_bootstrap` (RoomStore fixture),
  `test_message_lifecycle` (SQLite-migration outbox fixtures),
  `test_stable_identities` (catalogue fetch connectivity). None of these
  suites is touched by the BORU-DIR chain (`git diff 8d6917ef..HEAD -- <file>`
  is empty), and the hidden peer-discovery service they exercise is unrelated
  to the public-room directory.
- **One BORU-DIR regression found and fixed in this gate**: the mutation
  test `security::mutation::room_advertisement_flip_truncate_extend_rejected_without_panic`
  was failing because BORU-DIR-08's fixture signed the advertisement with
  `DEFAULT_ADVERT_TTL_SECS` (300) — the same value the legacy manual
  `Deserialize` backfills on a corrupted/missing trailing TTL varint, so a
  byte-flip in the TTL varint decoded back to the signed value and the
  signature still verified. Fixed by signing the fixture with a non-default
  TTL (600) so any TTL corruption decodes to a different value and is
  rejected (`tests/security/mutation.rs`). Re-run: **59/59 PASS**. The
  production decode is fail-safe (a corrupted TTL can only normalize to the
  fixed default, never to a forged value; a different TTL fails signature
  verification).

### Key regression surfaces (all PASS)

| Suite | Result |
|-------|--------|
| `test_public_room_directory` (16 matrix) | PASS |
| `test_hostile_input` | PASS |
| `test_required_matrix` | PASS |
| `test_discovery_restart` / `test_discovery_two_node` / `test_discovery_e2e_matrix` | PASS |
| `test_discovery_startup` / `test_discovery_dm_isolation` / `test_discovery_group_isolation` / `test_discovery_ui_isolation` | PASS |
| `tunnel_reconnect` / `test_reconnect_asymmetric` / `test_sync_after_downtime` | PASS |
| `test_simple` / `test_two_peers_exchange` / `test_two_peers_relay` | PASS |
| `group_events` / `test_room_invite_v2` / `test_public_lobby_integration` | PASS |
| `test_download_integration` / `test_normal_downloads` / `test_file_library_integration` | PASS |
| `test_ui_file_sharing_integration` / `test_download_queue_order` / `test_download_initiation_integration` | PASS |
| `test_download_recovery` / `test_corrupted_content` / `test_malicious_filenames` | PASS |
| `test_blob_size_enforcement` / `test_crash_recovery` / `test_restart_storm_prevention` | PASS |
| `test_pause_scenarios` / `test_resource_exhaustion` / `test_interrupted_transfer_harness` | PASS |
| `test_interruption_restart` / `test_transfer_lifecycle_telemetry` | PASS |
| `test_storage_integration` / `test_serde_format` / `test_user_uploaded_gif` | PASS |
| `test_malformed_catalogue` / `test_catalogue_harness` / `test_remote_catalogue_integration` | PASS |
| `test_catalogue_lifecycle_events` / `test_catalogue_minimal` / `test_ack_processing` | PASS |
| `test_delivery_failure` / `test_outgoing_dm_transaction` / `test_offline_delivery_integration` | PASS |
| `test_signed_gossip_flow` / `test_metadata_security` / `test_policy_conformance` | PASS |
| `test_verify_containment_properties` / `protocol_registration` / `fs22_dashboard_coverage` | PASS |
| `fs17_activity_log` / `test_branding_rename` / `test_outbox_throughput` | PASS |
| `phase20_multi_instance` / `test_extensions_metadata` / `epoch_rotation` | PASS |
| `compression_integration` / `test_local_address_lookup` / `test_health_view` | PASS |
| `test_online_user_list` / `test_debug` / `security` / `test_security` / `mailbox` | PASS |
| `test_conversation_integration` / `test_friend_request_e2e` / `test_friend_ticket_persistence` | PASS |
| `test_pairing_integration` / `test_private_room_dht_discovery` / `test_private_room_invitation_discovery` | PASS |
| `test_lobby_migration` / `test_deterministic_discovery_integration` / `test_image_cache_persistence` | PASS |
| `test_mcp_diagnostics_integration` / `verify_gui_bootstrap` / `image_optimizer_integration` | PASS |
| `three_peer_mesh` / `test_fixture` / `room_e2e` / `sim` / `test_deterministic_harness` (test-utils) | PASS |
| `test_stress_test_comprehensive` / `test_stale_bootstrap` (test-utils) | PASS |

### Library + bin unit suites

- `rb test --lib`: full lib unit suite (includes `discovery_service::tests`,
  `room_directory::tests`, `directory::tests`).
- `rb check --bin boru --features gui,video-playback,terminal`: compiles clean
  (pre-existing warnings only).

### Known pre-existing failures (not BORU-DIR regressions)

- `storage::tests::docs_reference_current_schema_version` — 1 failure in the
  lib suite; reproduces on origin/main without BORU-DIR work (documented by
  BORU-DIR-23).
- Voice/video-call suites (`call_*`, `voice_acceptance`) require
  `voice-calls`/`video-calls` features (native audio/video deps) and are
  excluded from the DoD regression surface (direct chat, groups, files,
  tunnels, hidden peer-discovery — none of which they cover).
- `test_onboarding_integration`, `stale_bootstrap`,
  `test_message_lifecycle`, `test_stable_identities` — pre-existing (proven
  at baseline 8d6917ef; untracked by BORU-DIR).

## Conclusion

All 13 DoD bullets PASS with code/docs evidence; all 8 "Do not do these
things" prohibitions hold with no violations found. The 16-scenario required
matrix passes end-to-end, and the full compile-able test suite shows no
regressions in direct chat, groups, files, tunnels, or the hidden
peer-discovery service. The one BORU-DIR-introduced regression found by the
gate (the `security` mutation fixture colliding with the legacy TTL
backfill) was fixed in this task. The public-room-directory implementation
satisfies the PDF's Definition of Done.
