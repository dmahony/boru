# UI-HOME-03 — Shared Dashboard Card Foundation

- Task: `t_7595a388` (UI-HOME-03)
- Plan source: `Boru_Home_Screen_Tidy_and_Fonts_Hermes_Kanban_Plan.pdf` (UI-HOME-03 card)
- Repo: `/home/dan/iroh-gossip-chat` @ `main`
- Status: COMPLETE. Every home-screen card now shares one card surface
  (`design_tokens::card_style` + `RADIUS_CARD`) and one shell primitive
  (`card_shell::CardShell`), extended with the semantic areas the plan
  requires (title, subtitle, header action, status badge, content body,
  footer). No business logic changed.

## 1. What was delivered

The existing rail-card primitive `CardShell` (per UI-HOME-01 audit §10.2)
was **extended** rather than duplicated into a new `DashboardCard`. The
home-screen card shells were then migrated onto it:

| Card | Before (one-off styling) | After (shared foundation) |
|---|---|---|
| Connection hero (large) | bespoke `container` style: `bg_surface`/`primary_soft` bg + `border_muted` 1px + `RADIUS_XL` (16) + **no shadow** | `card_style` base + background override for Ready (`primary_soft`) — same border, radius token, and low-opacity shadow as every other card |
| Mesh Activity | inline `container(Column…)` with its own header/body, `SPACE_16` padding, `card_style` but bespoke header rows | `CardShell::new("Mesh Activity", …)`: `title_case(false)` + `subtitle` + `status_badge` (Healthy/Degraded/Offline pill) + `header_action("View details")` + content-driven `body`; `SPACE_24` padding |
| Online Peers / Recent Activity / Tunnels | already `CardShell` | unchanged API call sites; inherit new padding + radius via the single shared build path |
| Quick Actions | button cards, `RADIUS_LG` (12) radius | `RADIUS_CARD` (16) — same corner rhythm as the rest of the dashboard (interactive hover/focus retained) |

## 2. Component API (CardShell, extended)

```rust
CardShell::new("Mesh Activity", vec![])   // title: impl Into<String>
    .title_case(false)                    // default: uppercase rail look
    .subtitle("Current connection status")
    .count(3).count_total(12)             // "3/12" badge (existing)
    .status_badge("Healthy", StatusBadgeKind::Success)
    .header_action("View details", AppMessage::OpenConnectionDetails)
    .body(content_element)                // content-driven height (new)
    .footer(summary_line)                 // optional, below body (new)
    .build(&theme);
```

Semantic areas supported by the shared shell:

- **Title** — `TypeRole::CardTitle` (Source Sans 3 SemiBold 18), uppercase by
  default, `title_case(false)` for sentence case.
- **Subtitle** — `TypeRole::SupportingText` (SS3 Regular 13), muted.
- **Count badge** — optional `count` / `count_total` (existing, `Metadata`).
- **Status badge** — new `StatusBadgeKind { Neutral, Success, Warning, Danger }`
  pill; maps to the token status palette (`success_soft` / `warning_soft` /
  `destructive_soft` + strong status colour).
- **Header action** — `header_action(label, msg)` (replaces `on_view_all`,
  which now delegates to it with label "View all").
- **Body** — `body(element)` renders arbitrary content with content-driven
  height (grows, never clips); otherwise the bounded scrollable list or the
  empty state are used as before.
- **Footer** — `footer(element)` rendered below the body with a small gap.

Surface: `design_tokens::card_style` = `surface` bg + `border_muted` 1px +
`RADIUS_CARD` (16 px, within the plan's 14–18 band) + `shadow_card`
(low-opacity). Padding: `[SPACE_24, SPACE_24]` (~24 px, within the plan's
22–28 band) — up from the rail's previous `[SPACE_12, SPACE_16]` horizontal
padding.

## 3. New/changed tokens (design_tokens.rs)

- `RADIUS_CARD = 16.0` — the card-container radius token (plan 14–18 band).
  `card_style` now uses it; `RADIUS_LG` comment narrowed to chat bubbles/dialogs.
- `success_soft(theme)` / `warning_soft(theme)` — translucent status tints,
  mirroring the existing `destructive_soft` so the status palette is symmetric.

## 4. Removed duplicate-style rules

- `app.rs` hero card: bespoke `container::Style` closure (background + border
  + `RADIUS_XL` literal, no shadow) → derives from `card_style`, overriding
  only the background. One less place with radius/border/shadow literals.
- `app.rs` mesh card: bespoke inline header row (icon + title + subtitle +
  "View details" ghost button) and body row → `CardShell` header/body API.
  Removed the mesh card's `SPACE_16` padding override and its "same radius
  system (hero RADIUS_XL, body cards RADIUS_LG)" comment — there is now one
  radius token for all dashboard cards.
- `quick_actions.rs`: `RADIUS_LG` → `RADIUS_CARD` for the action-button cards
  (two comment mentions updated). Fixed 132 px height untouched (UI-HOME-06).
- No new `dashboard_card_style` duplicate was created; `card_style` is the
  single shared surface.

## 5. Tests

- `cargo build --bin boru --features gui` — OK (exit 0).
- `cargo test --bin boru --features gui` — **864 passed / 0 failed**
  (prior: 853; +11 net new).
- New `design_tokens` tests (3): `RADIUS_CARD` in 14–18 band, `card_style`
  uses the token (border/radius/shadow assertions), status soft colours are
  translucent.
- New `card_shell` tests (10): stores subtitle / status badge / custom header
  action label / body+footer; `title_case` default; status kinds cover the
  palette; full-semantic-areas build doesn't panic; body overrides empty
  children; existing `card_shell_text_uses_type_role` extended.
- `component_gallery.rs` — `card_shell_gallery()` gained a third column
  demonstrating the full foundation: sentence-case title + subtitle + count
  badge + status pill + header action + content body + footer (gallery builds
  as part of the example; preview accessible via Ctrl+Shift+G).
- Updated UI-HOME-12 regression guard `home_screen_uses_type_role_roles`:
  mesh title now resolves through `TypeRole::CardTitle` inside CardShell, so
  the guard accepts either the inline role or a `CardShell::new("Mesh
  Activity", …)` construction (the role itself is still asserted in
  `card_shell_text_uses_type_role`).

## 6. Evidence

Before/after home screenshots at 1600×900, 1280×800, 1024×720 (fresh
`--no-dht --no-relay` launch, truthful "Connecting"/"Healthy" state):

- `docs/ui-redesign/evidence/t_7595a388/t_7595a388_home_1600x900_before.png`
- `docs/ui-redesign/evidence/t_7595a388/t_7595a388_home_1600x900_after.png`
- `docs/ui-redesign/evidence/t_7595a388/t_7595a388_home_1280x800_before.png`
- `docs/ui-redesign/evidence/t_7595a388/t_7595a388_home_1280x800_after.png`
- `docs/ui-redesign/evidence/t_7595a388/t_7595a388_home_1024x720_before.png`
- `docs/ui-redesign/evidence/t_7595a388/t_7595a388_home_1024x720_after.png`

OCR spot-check (1280×800): AFTER shows the mesh card header with
"Mesh Activity" + "Current connection status" + "Mesh Healthy" pill +
"View details" action, with Online Peers / Recent Activity / Tunnels rail
unchanged and no clipped text.

Evidence script: `scripts/ui_home03_card_evidence.sh before|after`.

## 7. Remaining risks

- `RADIUS_CARD` equals `RADIUS_XL` (16) today; the plan's 14–18 band allows
  either, and the two tokens document different intent (containers vs hero).
  A later visual pass can move `RADIUS_XL` down without touching cards.
- The hero card keeps `SPACE_24` padding via its own `container` wrapper
  (it is not a `CardShell` — it has no header structure); its surface/border/
  radius/shadow now come from `card_style`, so only the padding is local.
- `ui_components::Card` and settings cards inherit the new 16 px radius via
  `card_style` (intended standardisation; not visually audited in this card).
- Quick-action fixed 132 px height is out of scope (UI-HOME-06).
- Typography between BEFORE and AFTER differs because UI-HOME-12/13/14 landed
  in between; that is owned by those cards, not this one.

## 8. Changed files

- `examples/iced_chat/card_shell.rs` (foundation extension)
- `examples/iced_chat/design_tokens.rs` (RADIUS_CARD + status soft tokens)
- `examples/iced_chat/app.rs` (hero style + mesh card migration + guard update)
- `examples/iced_chat/quick_actions.rs` (RADIUS_CARD)
- `examples/iced_chat/component_gallery.rs` (full-foundation demo)
- `scripts/ui_home03_card_evidence.sh` (new)
- `docs/ui-redesign/evidence/t_7595a388/` (before/after screenshots)

No business/network/state logic touched. This card gates UI-HOME-04..08.
