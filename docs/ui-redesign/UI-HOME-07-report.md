# UI-HOME-07 — Refine the Online Peers card

- Task: `t_85b7f19a` (UI-HOME-07)
- Repo: `/home/dan/iroh-gossip-chat` @ `main` (worktree `wt/t_85b7f19a`)
- Status: COMPLETE. The Online Peers card is now structured and intentional
  even with a single online peer: small-label heading, live online/known
  count badge, right-aligned "View all", two-line structured rows (avatar +
  online dot + display name + presence secondary line) at 60 px, a
  content-driven body with a ~220–280 px floor, and an intentional empty
  state. All rows/counts come from live friend + presence state — no fake
  peers.

## 1. What changed

### Shared foundation (card_shell.rs)

- New `PEER_ROW_HEIGHT = 60.0` token for two-line Online Peers rows (plan
  band 58–68 px), documented alongside the single-line `CARD_ROW_HEIGHT`
  (48 px) that Recent Activity / Tunnels keep.

### Online Peers card (app.rs)

- `OnlinePeerRow` now carries the live presence state
  (`PeerPresence::Online | Away | Connecting…` derived from
  `peer_presence_map` + `AWAY_THRESHOLD_MS`) so the row's secondary line is
  truthful and updates when a peer goes Online → Away.
- Rows are structured: avatar (36 px, live online dot) + display name
  (TypeRole::Body) + presence label (TypeRole::SupportingText) coloured with
  the status palette (green for Online, amber for Away/Connecting), inside a
  full-width 60 px button that preserves `OpenConversation(peer)`.
- Body is **content-driven with a floor** instead of a fixed 248 px panel:
  `online_peers_body_height(rows)` grows with the row count up to five
  visible rows (`PEERS_BODY_MAX` = 5×60 + 4×2 = 308, the 6th peer scrolls)
  and never collapses below `PEERS_BODY_MIN` = 128 (card total ≈ 224 px, in
  the plan's 220–280 px band). With one peer the card shows one 60 px row
  plus restrained whitespace — no tiny strip, no huge blank panel.
- Empty state is now intentional: "No peers online" centred in the
  min-height body (basic copy; final polish owned by UI-HOME-16).
- Hover treatment kept (`surface_hover` on `Status::Hovered`); **focus** is
  not stylable because iced 0.14's `button::Status` has no `Focused` variant
  (Active/Hovered/Pressed/Disabled only) and iced 0.14 buttons are not
  keyboard-focusable — noted in a code comment.
- Header/heading, count badge and right-aligned "View all" come from the
  UI-HOME-03 `CardShell` foundation (unchanged call sites: `.count(n)`,
  `.count_total(total)`, `.on_view_all(OpenFriendRequests)`).

## 2. State sources (live, confirmed)

| Visible value | Source |
|---|---|
| Row presence secondary line | `self.peer_presence(&pk)` → `peer_presence_map` last-seen + `AWAY_THRESHOLD_MS` (app.rs `peer_presence`) |
| Row display name | `self.resolve_name(&pk)` (friend label → announced name → session names → short key) |
| Row avatar / online dot | `self.friend_image_handles` + presence-derived dot |
| Count badge numerator | `dep.rows.len()` — live rows after Offline filter |
| Count badge denominator | `self.friends` filter `relationship.can_message()` |
| Empty state trigger | zero rows after the Offline filter (live) |

The selector boundary is preserved: the card still reads only
`friends`/`peer_presence_map`/`friend_image_handles`/`names`/`dark_mode`,
so `iced::widget::lazy` memoization per-card is unchanged (existing
reactivity tests still pass).

## 3. Tests

- `cargo build --bin boru --features gui` — OK (exit 0).
- `cargo test --bin boru --features gui` — **867 passed / 0 failed**
  (prior: 864; +3 net new).
- New tests:
  - `card_shell::tests::peer_row_height_token_in_58_68_band` — 60 px token
    stays in the 58–68 px band and is taller than `CARD_ROW_HEIGHT`.
  - `app::tests::online_peers_body_height_is_content_driven_with_min_and_cap`
    — 0/1/2 rows floor at `PEERS_BODY_MIN`, 3 rows grow to content, 5/8 rows
    cap at `PEERS_BODY_MAX`; one-peer card lands in the 220–280 px band.
  - `app::tests::online_peer_rows_carry_live_presence` — fresh last-seen →
    `Online` row, aged last-seen → `Away` row, no presence → excluded.
- Existing `home_online_peers_card_empty_state_builds` /
  `home_online_peers_card_populated_state_builds` (8 peers → scroll) still
  pass, as do the per-card dependency-isolation tests
  (`activity_push_changes_only_activity_card_data`,
  `peer_presence_toggle_changes_only_peers_card_data`, etc.).

## 4. Evidence (docs/ui-redesign/evidence/t_85b7f19a/)

All captures at 1280×800 with a fresh data dir, `--no-dht --no-relay`, and
presence driven through the production path
(`boru_gui_set_peer_presence` → `FriendEvent::StatusChanged`, the same route
real network events use — see `scripts/ui_home07_online_peers_evidence.sh`):

| Capture | What it proves (OCR/pixel-verified) |
|---|---|
| `t_85b7f19a_empty_1280x800.png` | "ONLINE PEERS" + `0/0` badge + "View all" + centred "No peers online" empty state |
| `t_85b7f19a_onepeer_1280x800.png` | Badge `1/1`; one 60 px row: avatar + "Ada" + presence line "Online" (green); Recent Activity shows "Ada came online" (live path) |
| `t_85b7f19a_onepeer_hover_1280x800.png` | Row background shifts white (255,255,255) → `surface_hover` tint (239,243,241) on cursor-over (pixel-verified) |
| `t_85b7f19a_onepeer_viewall_1280x800.png` | Clicking "View all" navigates to the Friend Requests screen ("SEND A FRIEND REQUEST", Incoming/Outgoing lists, «Back) |
| `t_85b7f19a_onepeer_chat_1280x800.png` | Clicking the Ada row navigates to the Chat screen (peer header + "No messages yet." + composer); peer name ink block verified at the header name position |

Interaction coordinates were verified from the captures via tesseract TSV
word boxes before scripting the clicks.

## 5. Changed files

- `examples/iced_chat/card_shell.rs` — `PEER_ROW_HEIGHT` token + docs + test
- `examples/iced_chat/app.rs` — `OnlinePeerRow.presence`, selector, card
  view (two-line rows, content-driven body, empty state), body-height helper
  + tests
- `scripts/ui_home07_online_peers_evidence.sh` — new evidence script
- `docs/ui-redesign/UI-HOME-07-report.md` — this report
- `docs/ui-redesign/evidence/t_85b7f19a/` — 5 PNG captures + README

No networking/discovery/chat/room/group/file-sharing/tunnel business logic
touched.

## 6. Remaining risks / notes for downstream cards

- iced 0.14 `button::Status` has no `Focused` variant and buttons are not
  keyboard-focusable, so rows have hover (surface_hover) but no separate
  focus style — framework limitation, not a regression. If keyboard focus
  becomes a requirement, rows would need a custom focusable widget.
- `PEERS_BODY_MIN` = 128 px makes the one-peer card ≈ 224 px tall (in the
  220–280 band). A reviewer can tune the constant if the visual pass wants
  the card slightly taller; the band assertion test will flag drift.
- The 6-peer+ overflow contract is unchanged (5 visible rows, scrolls), but
  visible-row height grew 48 → 60 px, so a fully-populated card is taller
  than before (≈ 404 px vs ≈ 342 px). Not "excessively tall" per acceptance,
  but UI-HOME-15 (responsive pass) may want to revisit the cap.
- Empty-state copy is basic ("No peers online"); UI-HOME-16 owns the final
  copy/illustration.
- `cargo fmt --all -- --check` still fails repo-wide on 284 pre-existing
  diffs; the three new formatting violations introduced by this card were
  fixed (my regions are clean).
