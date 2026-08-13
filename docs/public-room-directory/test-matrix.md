# Public Room Directory — Required test matrix (BORU-DIR-23)

Source: "Required test matrix" section of
`Boru_Public_Room_Directory_Implementation_Tasks.pdf` (Phase 8 — Diagnostics
and tests). This document records how each of the 16 matrix scenarios is
proven by automated tests.

## Dedicated test suite

`tests/test_public_room_directory.rs` (registered as `[[test]]`
`test_public_room_directory`, `required-features = ["net"]`) is the
dedicated integration suite: two real Boru nodes (advertiser/owner `A` and
viewer `B`) joined to the internal discovery gossip topic over a loopback
mesh, with advertisements/withdrawals crossing the real control-plane
receive gate into the bounded `RoomDirectory` cache. A third node `C`
(attacker) is spawned on demand for the spoofing scenario.

The suite complements the module-level coverage in `src/room_directory.rs`
(cache semantics: dedupe, bounds, replacement, local-state derivation) and
`src/discovery_service.rs` (receive-gate behaviour: auth, malformed input,
withdrawals, diagnostics counters, and the directory TTL sweep).

## Scenario → test map

| # | Scenario | Expected result | Test | Status |
|---|----------|-----------------|------|--------|
| 1 | Create discoverable room | Other client sees it in Discover Rooms without joining | `create_discoverable_room_visible_without_join` | PASS |
| 2 | Create unlisted room | Other client does not see it via directory discovery | `create_unlisted_room_not_visible` | PASS |
| 3 | Create private room | No directory advertisement is emitted | `create_private_room_no_advertisement` | PASS |
| 4 | Join advertised room | Explicit Join invokes normal join path; room then appears in conversations | `join_advertised_room_normal_join_path` | PASS |
| 5 | Open directory only | No room topic subscription or membership change occurs | `open_directory_only_no_subscription_or_membership` | PASS |
| 6 | Advertiser restarts | Room advertisement returns after discovery startup | `advertiser_restart_republication_returns_room` | PASS |
| 7 | Advertiser disappears | Room becomes stale and expires after TTL | `advertiser_disappears_room_expires_after_ttl` | PASS |
| 8 | Room becomes unlisted | Withdrawal removes it quickly, with TTL as fallback | `room_becomes_unlisted_withdrawal_removes_quickly` | PASS |
| 9 | Room metadata changes | Directory card updates without creating duplicate room entry | `room_metadata_change_updates_without_duplicate` | PASS |
| 10 | Duplicate advertisements | One directory entry; no UI churn | `duplicate_advertisements_one_entry_no_churn` | PASS |
| 11 | Malformed advertisement | Rejected safely; no chat/UI corruption | `malformed_advertisement_rejected_safely` | PASS |
| 12 | Oversized advertisement | Rejected before large allocation/UI rendering | `oversized_advertisement_rejected_before_rendering` | PASS |
| 13 | Unsupported room protocol | Room is marked incompatible; Join is blocked or explained | `unsupported_room_protocol_marked_incompatible` | PASS |
| 14 | Already joined room | Directory shows Open instead of Join | `already_joined_room_shows_open` | PASS |
| 15 | Hidden room | Does not reappear on refresh until user unhides it | `hidden_room_stays_hidden_until_unhidden` | PASS |
| 16 | Spoofed withdrawal | Cannot remove a room unless authority rules validate it | `spoofed_withdrawal_cannot_remove_room` | PASS |

## Production gap fixed by this task

Writing the matrix exposed two small production gaps, both fixed in
`src/discovery_service.rs`:

1. **Stale rooms never expired on their own (scenario 7).** The bounded
   control-plane `RoomDirectory` cache was only ever evicted as a side effect
   of the *next* advertisement arriving (`apply_advertisement` evicts expired
   entries before inserting a new room). If an advertiser disappeared and no
   other advertisement arrived, an expired room stayed in the browse surface
   indefinitely.

   Fix (small, targeted — BORU-DIR-23): the discovery service now runs a
   periodic room-directory TTL sweep, `directory_expiry_loop`, on every
   `DEFAULT_DIRECTORY_SWEEP_INTERVAL` (30 s), calling
   `RoomDirectory::evict_expired()` so stale advertisements leave the active
   directory naturally (PDF Task 3.2 step 4; TTL remains the final cleanup
   mechanism). The sweep interval is tunable via
   `DiscoveryService::with_directory_sweep_interval` (tests use short
   intervals). Unit proof: `directory_expiry_sweep_evicts_expired_entries` in
   `src/discovery_service.rs`; end-to-end proof: scenarios 6/7.

2. **Restarted advertiser's re-announcement was silently dropped (scenario
   6).** The control-plane sequence counter was seeded with a random value, so
   after a restart (same identity, fresh random seed) the re-announcement's
   sequence collided with the pre-restart sequence space at the receive gate
   (`PeerControlStateStore::record` rejects any sequence `<=` the last seen
   for that sender) ~50% of the time. The seed is now wall-clock seconds:
   monotonic per identity across restarts, so the post-restart sequence is
   strictly newer than anything the same identity broadcast before (it also
   preserves the original random-seed rationale — avoiding the gossip actor's
   blake3 content dedup for byte-identical frames).

The suite also gained `DiscoveryService::with_advert_min_interval` (test
knob mirroring the existing control-announce throttle builder) so
re-announcements in the restart scenario are not throttled — the production
periodic refresh cadence is longer than the default throttle interval, so
real re-announcements are never throttled either.

## Supporting module-level coverage

- `src/room_directory.rs` tests: cache bounds, deterministic replacement,
  duplicate/conflict handling, withdrawal authority guard, TTL expiry,
  local-state derivation (Joined/Hidden/Incompatible), diagnostics.
- `src/discovery_service.rs` tests: receive-gate auth (signed/tampered/
  wrong-publisher/unsigned), malformed + oversized advertisement rejection,
  verified/duplicate/conflicting advertisement handling, withdrawal auth
  and authority rules, directory counters (received/accepted/rejected/
  deduplicated/withdrawn/rate-limited/expired), the directory TTL sweep.
- `examples/iced_chat/app.rs` tests: join gate (compatibility, local
  hidden/blocked), one-record join, no privilege grant from advertised
  metadata, legacy join path, hide/unhide persistence semantics.
