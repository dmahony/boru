# Boru Chat-Screen Redesign — Final Regression Report

Date: 2026-07-28
Branch: `ui/chat-screen-redesign` (at `604af4b1`)
Baseline: `main` (at `ca864a1d`)

## All Commits in this Redesign

| # | Commit | Message | Scope |
|---|--------|---------|-------|
| 1 | `272d3e97` | docs: establish chat UI redesign baseline | docs/ |
| 2 | `103ff3e7` | style: add chat interface design tokens | examples/iced_chat/design_tokens.rs |
| 3 | `20ba04ff` | ui: add responsive three-column chat shell | examples/iced_chat/app.rs |
| 4 | `a3199b16` | ui: redesign conversation header | examples/iced_chat/app.rs |
| 5 | `7c7d080f` | ui: refine system messages and delivery status | examples/iced_chat/app.rs |
| 6 | `34dcf9d5` | ui: redesign message composer | examples/iced_chat/app.rs |
| 7 | `38d3ce18` | ui: polish chat screen states and interactions | examples/iced_chat/app.rs |
| 8 | `604af4b1` | ui: improve chat responsiveness and accessibility | examples/iced_chat/app.rs, design_tokens.rs |
| 9 | *(this commit)* | test: verify chat screen redesign | docs/ (baseline update) |

## All Files Changed (vs. main)

**UI files (redesigned):**
- `examples/iced_chat/app.rs` — 801 lines modified (all UI presentation layer)
- `examples/iced_chat/design_tokens.rs` — 207 lines added (new design token system)

**Documentation:**
- `docs/chat-interface-design-tokens.md` — new
- `docs/chat-ui-redesign-baseline.md` — updated (this step)

**Formatting only (no functional changes):**
- `tests/test_two_peers_relay.rs` — 6 lines, braces reformatted by `cargo fmt`

**Pre-existing (not part of redesign):**
- `patched/` directory — vendored dependency patches from Phase 22

## Features Verified

The following features were verified by reviewing the diff and build:

| Feature | Status | Notes |
|---------|--------|-------|
| Application startup | Unchanged | main.rs not touched by redesign |
| Identity loading | Unchanged | Core library not touched |
| Selecting conversations | Verified | RoomSelected → RoomOpened flow |
| Switching conversations | Verified | No change to event flow |
| Receiving live messages | Verified | NetEvent → Message push unchanged |
| Sending messages | Verified | SendPressed → broadcast unchanged |
| Multiline messages | Verified | Composer handles newlines |
| Long unbroken messages | Verified | Wrapping::Word/Glyph unchanged |
| Scrolling (history) | Verified | Scrollable anchor_bottom() unchanged |
| Loading older messages | Verified | Backfill protocol unchanged |
| Unread counts | Verified | Read from conversation_store |
| Delivery/read state | Redesigned | Shown on last msg per group (Step 7) |
| Failed message handling | Verified | DeliveryState::Failed shown |
| Attachment selection | Unchanged | rfd::AsyncFileDialog |
| File sending | Unchanged | Blob protocol unchanged |
| Shared files | Unchanged | Catalogue protocol unchanged |
| Search | Unchanged | Not implemented at message level |
| Friends | Unchanged | Sidebar section cached |
| Presence changes | Unchanged | FriendOnlineCache |
| Peer discovery | Unchanged | No net changes |
| Public rooms | Unchanged | DirectoryRoomUpdate |
| Requests | Unchanged | Sidebar section cached |
| Details panel | New | Step 9 — conversation info |
| Window resizing | Verified | Responsive panel hiding (Step 11) |
| Keyboard navigation | Unchanged | Iced framework level |
| App restart/persisted state | Unchanged | SQLite stores unchanged |

## Tests and Build Commands Run

| Command | Result | Details |
|---------|--------|---------|
| `cargo fmt --check` | PASS | All files formatted |
| `cargo check --bin boru --features gui` | PASS | 96 warnings (all pre-existing) |
| `cargo clippy --bin boru --features gui` | PASS | 128 warnings (all pre-existing) |
| `cargo test --lib` | 1609 PASS, 9 FAIL, 2 HUNG | All failures/hangs pre-existing in core library |

## Accidental Changes Check

- **No backend changes**: All changed files are under `examples/iced_chat/` (UI presentation layer) and `docs/`
- **No protocol changes**: No serialization, signing, networking, or storage files modified
- **No storage changes**: SQLite and JSON stores untouched
- **No state model changes**: ChatEntry, AppMessage, event flows unchanged
- **No invented data**: All UI state sourced from real application state
- **Temporary debug code**: None found
- **`patched/` additions**: Pre-existing from Phase 22, not part of this redesign

## Known Visual Limitations

1. **Iced 0.14 Border limitations**: Per-side Border widths (Border.left) not supported — selection is still primarily color-coded via bg_selected background
2. **Keyboard focus styling**: Focused status not present in Iced 0.14's button::Status enum — keyboard focus cannot be styled independently
3. **prefers-reduced-motion**: Spinner animations do not respect reduced-motion preferences — Iced 0.14 has no CSS media query equivalent
4. **Bounce easing avoidance**: All easing is linear/ease-out — no spring/bounce per design intent
5. **No full-text search**: Only friend request search exists (pre-existing limitation)

## Unavailable Mock-up Information Deliberately Omitted

Per safety preamble:
- Connection quality / latency / encryption details — not provided by application state
- Typing indicators — removed feature
- Voice/video call state — not implemented
- Per-message read receipts for remote messages — only local delivery state tracked
- Message edit history — single `edited` flag only
