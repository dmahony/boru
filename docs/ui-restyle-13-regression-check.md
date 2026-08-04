# UI-RESTYLE-13: Regression Check Report

Date: 2026-08-05
Worktree: wt/t_8c4e8ebe @ e8c8676a (merged main lineage)
Scope: UI-RESTYLE-02..07 merged into main (the restyle chain). UI-RESTYLE-08/09/10
       exist on their own task branches but are NOT in the merged main lineage —
       see Finding F2.

## Summary

PASS with findings. All six regression areas listed in the task are confirmed
working in the merged main state. Two pre-existing issues were discovered that
are NOT caused by the UI restyle workstream (see Findings). One process-level
merge-state observation was confirmed (see F2).

## Test execution

### 1. GUI example suite — the actual regression surface

The UI restyle touched ONLY `examples/iced_chat/**` + docs + Cargo.toml/lock
(verified: `git diff --name-only 911a05fd..0569061a` shows zero `src/` changes).
The GUI example suite is therefore the authoritative regression surface for
this work.

```
cargo test --example boru --features gui
→ test result: ok. 822 passed; 0 failed; 0 ignored (57.08s)
```

This is an increase over the 816 tests reported by UI-RESTYLE-11 (a few tests
were added since). It includes all dialog/keyboard/focus regression tests
listed in section 3.

### 2. Library suite (boru_core lib tests)

```
target/debug/deps/boru_core-<hash> --test-threads=8 --skip gossip_net_smoke ...
→ 1808 passed before hitting pre-existing net-layer hangs (below)
```

- ALL compression tests pass (they are merely slow in debug builds — pure-Rust
  JPEG encoding of large synthetic images).
- ALL app/dialog/form/ui-component tests pass.
- 3 pre-existing tests in the net/outbox layer hang (busy-spin / deadlock),
  unrelated to the restyle (see Finding F1):
  - `net::tests::gossip_net_smoke`
  - `outbox_delivery::tests::test_different_peers_deliver_concurrently`
  - `outbox_delivery::tests::test_same_peer_serialized`

### 3. Full `cargo test`

Attempted with `CARGO_TARGET_DIR` shared against the primary tree's 387G cache.
The compile phase alone exceeded a 25-minute window under heavy machine load
(load average 11–20 on 6 cores; concurrent deflate-chain builds in
/tmp/verify-perf2 plus three stale lib-test processes were already consuming
CPU). The lib test binary and ~50 integration test binaries did compile; the
remaining targets were still building when the window closed. This is an
environment/toolchain-time limitation, not a code failure.

## Per-item regression results

### Chat (public room) creation logic — PASS

- `view_create_room_dialog` (app.rs:21556) builds via `BoruDialog` with
  Cancel/backdrop/primary wiring; name validation gates the primary button;
  Enter submits only when valid (`on_submit` at app.rs:21581).
- `ConfirmCreateNewRoom` (app.rs:9746) preserves the advertise branch
  (directory store upsert, archived conversation entry, periodic re-advertise
  tick) and the auto-join branch.
- Entry points wired: sidebar + quick actions + home CTA all dispatch
  `CreateNewRoom` (app.rs:22057/22106/22146/22453); `Shortcut(NewChat)` and
  GUI test command `CreateNewRoom` both route to it.
- Covered by tests: `confirm_group_with_empty_name_*`, `cancel_handlers_*`,
  `room_join_failed_while_room_submitting_*`, GUI open-room action tests.

### Public room creation logic — PASS

- `view_create_room_dialog` renders "Create Public Room" with Visibility /
  Discovery / Access / Info sections (app.rs:21556-21632).
- DHT + advertise toggles still dispatch `CreateNewRoomAdvertiseToggled` /
  `CreateNewRoomDhtToggled`; GUI test commands `SetCreateRoomAdvertise`
  present (app.rs:16799).
- Existing behaviour preserved: empty name falls back to topic-id display name
  (matching pre-restyle behaviour, per UI-RESTYLE-11).

### Tunnel creation logic — PASS

- Two-stage flow intact: friend picker (`view_create_tunnel_dialog`,
  app.rs:21789) → `CreateTunnel(peer)` hands off to the share-local-service
  form (`view_share_local_service_dialog`, app.rs:33683) which registers the
  tunnel with `TunnelService` on `ConfirmShareLocalService` (app.rs:15390).
- Port validation: invalid / zero port → inline error + toast, dialog stays
  open; valid port → tunnel created, dialog closes, state recorded.
- Entry point: Tunnels card "View all" → `ShowCreateTunnelDialog`
  (app.rs:24079). Escape handling for both stages present.

### Keyboard interaction — PASS

- Escape closes overlays in priority order with mid-submit guards
  (app.rs:12602-12668): create-room, tunnel picker, share form,
  connection-details, invite menu, create-group, help, settings; Escape must
  NOT dismiss any dialog while its submit flag is set.
- Tab / Shift+Tab map to focus-next / focus-previous
  (`shortcut_from_key`, app.rs:29005; tests at app.rs:34116).
- Enter submits forms via `on_submit` (room name, group name, share name/port,
  join-ticket, composer, friend rename).
- Tests: `escape_closes_tunnel_and_share_dialogs_when_idle`,
  `escape_does_not_close_dialog_while_submitting`,
  `cancel_handlers_are_ignored_while_submitting`,
  `tab_moves_focus_to_next_input_and_shift_tab_previous`,
  `escape_new_chat_and_back_to_chat_list_shortcuts_still_map`.

### Focus handling — PASS

- Auto-focus first meaningful field on dialog open: `CreateNewRoom` →
  `focus(CREATE_ROOM_NAME_INPUT)` (app.rs:9361), `ShowCreateGroupDialog` →
  `focus(CREATE_GROUP_NAME_INPUT)` (app.rs:9385), `CreateTunnel` →
  `focus(SHARE_SERVICE_NAME_INPUT)` (app.rs:9438).
- Inputs carry stable focus `Id`s (`CREATE_ROOM_NAME_INPUT` at 21574,
  `CREATE_GROUP_NAME_INPUT` at 21744, `SHARE_SERVICE_NAME_INPUT` at 33621,
  composer at 27719, connection-details trigger at 28513).
- Connection-details dialog stores + restores focus target
  (`connection_details_focus_target`, tests at 34169/34506/34529).
- `Shortcut(FocusNext/Previous)` return iced focus tasks (app.rs:12686-12689).
- Shared `TextInput::id()` / `on_submit` plumbing verified in
  form_components.rs:229-238 and ui_components.rs:643-648.

### Other dialogs still using shared components — PASS (with merge-state note)

- `connection_details` dialog: own pre-existing module, not converted to
  BoruDialog — untouched, still builds and its focus/close tests pass.
- `view_invite_member_dialog` (app.rs:21841): still uses raw iced scaffolding
  in the merged main — it was NOT converted because UI-RESTYLE-09's shared
  `SelectablePeerList` refactor was never merged (see F2). This is a
  merge-state observation, not a code regression: the dialog is unchanged from
  the pre-restyle baseline.
- Shared components (`BoruDialog`, `form_components::*`,
  `ui_components::text_input_field_opts`) are consumed by the three creation
  dialogs + component gallery; all covered by build tests
  (boru_dialog.rs tests, form_components.rs tests) which pass.

## Findings

### F1 — Pre-existing lib-test hangs in net/outbox layer (NOT caused by restyle)

- Tests: `net::tests::gossip_net_smoke`,
  `outbox_delivery::tests::test_different_peers_deliver_concurrently`,
  `outbox_delivery::tests::test_same_peer_serialized`.
- Evidence they are pre-existing:
  - `git diff --name-only 911a05fd..0569061a` (the entire restyle chain) shows
    ZERO `src/` changes; the restyle is confined to `examples/iced_chat`.
  - `src/outbox_delivery.rs` last touched 2026-07-28 (commit 3a0ea8c7,
    Phase 22 cleanup); `src/net.rs` last touched 2026-08-02 (bf970d19) — both
    before the restyle merge (78c0c450, 2026-08-05 03:25).
  - Three stale `cargo test --lib` processes (PIDs 3448799/3568473/3681866)
    started at 02:15/03:02/03:53 on 2026-08-05 — BEFORE and around the restyle
    merge — were already spinning on the same tests when this check began.
- Repro (isolated deadlock, not just load):
  `target/debug/deps/boru_core-<hash> --test-threads=1 outbox_delivery::tests::test_different_peers_deliver_concurrently`
  → test starts, busy-spins (75% CPU) and never completes. Same for
  `test_same_peer_serialized`. `gossip_net_smoke` starts a real relay server +
  3 endpoints and does not complete under current load.
- Impact: blocks a clean "full `cargo test` passes" claim; unrelated to the UI
  restyle. Follow-up task created (see below).

### F2 — UI-RESTYLE-09/10 code not present in merged main (merge-state observation)

- `wt/t_2da7ff1a` (UI-RESTYLE-09: shared `SelectablePeerList` +
  `messageable_friends()` refactor) and `wt/t_25e03dbe` (UI-RESTYLE-10:
  entry-point audit docs) are completed on the board but their code/docs were
  never merged into the main lineage (HEAD = e8c8676a contains UI-RESTYLE
  02-07 via merge 78c0c450 only).
- The verification tasks 11/12 were executed against the 09/10 branches, so the
  shipped main state does not include the duplication reduction those tasks
  verified. This is a merge-state discrepancy for the orchestrator to resolve,
  not a code regression introduced by this workstream.
- Note: the functional/visual verification results reported by UI-RESTYLE-11/12
  remain valid for the creation dialogs (02-07 are in main); the
  "shared components used by other dialogs" claim from 09 is what differs.

### F3 — Environment: heavy machine load during verification

- Load average 11–20 on 6 cores while the suite was run (concurrent
  deflate-chain verification builds + stale test processes from other
  sessions). This inflated compile times and made `cargo test` time out in the
  compile phase; it did not affect the GUI suite (57s) or the 1808 lib tests
  that completed.
