# app.rs as application coordinator (BORU-APP-010)

This document describes the final architecture of `examples/iced_chat/app.rs`
after the BORU-APP-001…010 decomposition: `app.rs` is the application
**coordinator**, not the application. Every domain concern lives in a sibling
module under `examples/iced_chat/app/`.

## Module map

| Module            | Owns                                                     |
|-------------------|----------------------------------------------------------|
| `sidebar`         | ChatList, profile header, section row rendering          |
| `settings`        | Settings screen, developer UI (BORU-APP-003)             |
| `contacts`        | Contact / friend book                                    |
| `calls`           | Audio/video call surface                                 |
| `chat`            | Chat log, composer, image/video playback                 |
| `discover`        | Discovered peers + public room discovery                 |
| `files`           | File transfers, dashboard cards                          |
| `rooms`           | Room lifecycle + room-category navigation                |
| `home`            | Home/landing cards, mesh status, connection events       |
| `groups`          | Group chat create/invite/view                            |
| `dialogs`         | Reusable dialog components                               |
| `tunnels`         | Tunnel (friend IP) management                            |
| `help_overlay`    | Help overlay (BORU-APP-002 reference domain)             |
| `notifications`   | Notification service, toasts, activity feed (BORU-APP-004) |

Each module follows the domain pattern described in
`examples/iced_chat/app/domain_pattern.md`: domain state + `DomainMessage` +
`update()` + `view()` helpers invoked from the coordinator's `AppMessage`
dispatcher.

## What stays in the coordinator

Only genuinely global shell / navigation / lifecycle state:

- **`IcedChat::new` wiring** — networking handles, stores, channels, the
  `Application` trait impl (`update` / `view` / `theme` / `subscription`),
  `AppMessage` dispatch.
- **Screen-level state** — `Screen`, active room/topic, layout cache, window
  sizing, dark mode.
- **Cross-domain plumbing** — net-event routing, persistence handles, shared
  UI helpers (`text_muted`, `container_card`, `section_card`, `now_ms`).

## Rules for keeping app.rs thin

1. **Single-owner helpers live with their owner.** A function used only by
   one domain module belongs in that module (e.g. `home_connection_variant` →
   `home.rs`, `transfer_state_name` → `files.rs`, `chat_image_display_size` →
   `chat.rs`, `profile_identity_card` → `settings.rs`). Make it `pub(crate)`
   so the coordinator's `pub(crate) use <module>::*` re-export keeps tests
   and cross-module callers working.
2. **Shared UI chrome stays put.** Helpers used by several domains
   (`section_card`, `text_muted`, `icon_svg`, color/`ICON_*` constants) stay
   in the coordinator as shared infrastructure.
3. **No dead compatibility shims.** Fields and constructor parameters
   retained only "to avoid changing the signature" (`persist_tx`, `notice`,
   `link_preview_fetch_index`, splash leftovers) are removed along with their
   callers in the same change. `#[expect(dead_code)]` marks must be true:
   an expect on an item that IS used produces an unfulfilled-expectation
   warning.
4. **Document invariants next to the code.** Source-guard tests
   (`method_source(...)` + `include_str!`) that anchor to moved functions
   must point at the module file (`app/sidebar.rs`, `app/settings.rs`, …) and
   use an end marker that still exists after the move (a function that
   follows in the same module, or `#[cfg(test)]` when the moved item is the
   last one before the test module).
5. **Match the domain pattern.** When adding a new feature, prefer a new
   domain module (or extending an existing one) over growing app.rs.

## Verification

- `rb check --bin boru --features gui,video-playback,terminal` (no new
  warnings over baseline).
- `rb test --bin boru --features gui,video-playback,terminal -- app::tests`
  (full application test suite).
- `git diff --check` clean.
