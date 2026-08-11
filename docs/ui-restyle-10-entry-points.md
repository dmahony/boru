# UI-RESTYLE-10 — Entry Point Audit (Create Group / Public Room / Tunnel)

Status: verification + integration — no entry-point code changes were required.
Repo: iroh-gossip-chat (boru) — `examples/iced_chat/` (Iced 0.14 GUI)
Date: 2026-08-05
Branch: `wt/t_25e03dbe`

Task 10 of the Boru UI restyle workstream: ensure every entry point that opens
the three creation flows routes to the new BoruDialog-based dialogs (built by
UI-RESTYLE-04/05/06), and that no stale entry point opens the old dark modal.

## Method

1. Merged parent branches onto `wt/t_25e03dbe`:
   - `wt/t_5d949475` (UI-RESTYLE-04, includes UI-RESTYLE-02/03) — fast-forward
     to `c599f696`
   - `eda0db36` (UI-RESTYLE-05) — cherry-picked (`a4bcea59`)
   - `57d96374` (UI-RESTYLE-06) — cherry-picked (`35ab0511`)
2. Audited every site that dispatches the creation-flow messages.
3. Verified the top-level overlay cascade still routes to the restyled view
   functions.
4. `cargo check --workspace --all-targets` PASS; `cargo test --bin boru
   --features gui` → 810 passed / 0 failed.

## Entry points → new dialogs (all verified)

### Create Public Room — `view_create_room_dialog` (app.rs:21388, BoruDialog "Create Public Room")

| Trigger | Location | Message |
|---|---|---|
| Home quick action "Create Public Room" | `quick_actions.rs:35` | `CreateNewRoom` |
| Sidebar CHATS header "+" | `app.rs:21858` | `CreateNewRoom` |
| Sidebar PUBLIC ROOMS header "+" | `app.rs:21907` | `CreateNewRoom` |
| Bottom utility row "New chat" + | `app.rs:21947` | `CreateNewRoom` |
| Sidebar empty state "Start Chat" | `app.rs:22254` | `CreateNewRoom` |
| Keyboard Ctrl+N (`Shortcut::NewChat`) | `app.rs:12540` | `CreateNewRoom` |
| GUI test command | `app.rs:16625` | `CreateNewRoom` |
| `NewChatCreated` completion | `app.rs:11269` | `CreateNewRoom` |

### Create Group Chat — `view_create_group_dialog` (app.rs:21452, BoruDialog "Create Group Chat")

| Trigger | Location | Message |
|---|---|---|
| Home quick action "Create Group Chat" | `quick_actions.rs:41` | `ShowCreateGroupDialog` |
| Sidebar GROUPS "Create Group" button | `app.rs:22314` | `ShowCreateGroupDialog` |

### Create Tunnel — picker `view_create_tunnel_dialog` (app.rs:21592, BoruDialog "Create Tunnel") + second stage `view_share_local_service_dialog` (app.rs:33402, BoruDialog "Create Tunnel")

| Trigger | Location | Message |
|---|---|---|
| Home "Tunnels" card "View all" | `app.rs:23880` (`CardShell.on_view_all`) | `ShowCreateTunnelDialog` |
| Chat details panel Tools → "Create Tunnel" | `app.rs:26376` | `CreateTunnel(pk)` (jumps straight to the friend's share-local-service dialog — new BoruDialog) |

All of these dispatch the same `AppMessage` variants as before the restyle; the
update handlers set the same `show_create_*_dialog` flags; the top-level overlay
cascade (`app.rs:21199-21213`) renders the restyled BoruDialog view functions.

## Stale dark modal check

The old hard-coded overlay style (`rgba(0.15, 0.15, 0.15, 0.95)`) remains in
exactly one dialog: `view_invite_member_dialog` (app.rs:21642, style at
app.rs:21720). That is the **"Invite to Group"** flow (adding members to an
existing group), which is **not** one of the three creation flows in this
workstream's scope. It was flagged in UI-RESTYLE-01 §5/§6 as a related
duplicate of the friend-picker pattern; Tasks 7/9 (usability / de-duplication)
are the correct owners if it should be migrated. The three creation dialogs
themselves have no dark-modal remnants.

## Notes for downstream tasks

- UI-RESTYLE-11 (functional verification): all three flows are reachable from
  the home screen quick actions / sidebar / keyboard shortcuts listed above.
  GUI-test commands exist only for the public-room flow (`CreateNewRoom`,
  `ConfirmCreateNewRoom`, `SetCreateRoomName`, `SetCreateRoomAdvertise`); group
  and tunnel flows have no per-flow GUI-test commands (pre-existing gap, noted
  in UI-RESTYLE-01 §6.8 — not introduced by this work).
- Escape handling: room → `CancelCreateRoom`; group → direct flag reset; tunnel
  picker → `on_close` wired in the new dialog but no `Shortcut::Escape` branch
  in the update handler (pre-existing gap; UI-RESTYLE-07 usability task owns
  this).
