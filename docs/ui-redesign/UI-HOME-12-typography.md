# UI-HOME-12 — Apply the Central Typography System to the Home Screen

- Task: `t_4c86d88c` (UI-HOME-12)
- Plan source: `Boru_Home_Screen_Tidy_and_Fonts_Hermes_Kanban_Plan.pdf` (UI-HOME-12 card)
- Repo: `/home/dan/iroh-gossip-chat` @ `main`
- Status: COMPLETE. The home screen (greeting, hero, mesh card, quick actions,
  rail cards, connection footer) now resolves all text through the central
  `fonts::TypeRole` roles from UI-HOME-11. Manrope only for the greeting,
  Source Sans 3 for normal interface text, JetBrains Mono only for genuine
  technical values (tunnel `host:port` endpoints), Raleway only for the BORU
  wordmark (unchanged, outside the home screen). No text clips; the
  quick-action cards are now content-driven instead of a fixed 132 px box.

## 1. Token-to-component mapping

| Home-screen element | Before (legacy) | After (UI-HOME-11 role) |
|---|---|---|
| Page greeting `Good {time}, {name}` | `source_sans(Semibold)` @ `HOME_GREETING` (32) | `TypeRole::DisplayHeading` → Manrope Bold 700 @ 32 px, `LineHeight::Relative(1.2)` |
| Welcome subtitle | `text` @ `HOME_SUBTITLE` (16) | `TypeRole::Body` (SS3 Regular 400) @ 16 px (UI-HOME-02 size kept) |
| Status pill label (Connected/Connecting/…) | default font @ TYPO_SM | `TypeRole::Metadata` → SS3 Regular 400 @ 12 px |
| Hero headline (`Boru is connected…` / `Connecting…`) | default font @ TYPO_LG | `TypeRole::SectionTitle` → SS3 SemiBold 600 @ 20 px |
| Hero body (`Private communication, peer to peer.`) | default font | `TypeRole::Body` → SS3 Regular 400 @ 15 px, `LineHeight::Relative(1.45)` |
| Hero actions (Retry / Details) | default font | `TypeRole::ButtonLabel` → SS3 SemiBold 600 @ 14 px |
| Mesh card title (`Mesh Activity`) | default font @ TYPO_LG | `TypeRole::CardTitle` → SS3 SemiBold 600 @ 18 px |
| Mesh card subtitle (`Current connection status`) | default font | `TypeRole::SupportingText` → SS3 Regular 400 @ 13 px |
| Mesh status label (`Connected`, `Degraded — …`) | default font | `TypeRole::BodyEmphasised` → SS3 SemiBold 600 @ 15 px |
| Mesh status detail (`N direct · N relayed · N neighbors`) | default font | `TypeRole::SupportingText` → SS3 Regular 400 @ 13 px |
| Mesh `View details` | default font | `TypeRole::ButtonLabel` → SS3 SemiBold 600 @ 14 px |
| Quick-action label | `source_sans(Semibold)` @ TYPO_MD (15) | `TypeRole::CardTitle` → SS3 SemiBold 600 @ 18 px |
| Quick-action description | default font @ TYPO_XS (12) | `TypeRole::SupportingText` → SS3 Regular 400 @ 13 px, `LineHeight::Relative(1.45)` |
| Online Peers row name | default font @ TYPO_SM | `TypeRole::Body` → SS3 Regular 400 @ 15 px |
| Recent Activity description | default font @ TYPO_SM | `TypeRole::Body` → SS3 Regular 400 @ 15 px |
| Recent Activity timestamp | default font @ TYPO_XS | `TypeRole::Metadata` → SS3 Regular 400 @ 12 px |
| Tunnels row name | default font @ TYPO_SM | `TypeRole::Body` → SS3 Regular 400 @ 15 px |
| Tunnels endpoint (`host:port`) | `jetbrains_mono(Normal)` @ TYPO_XS | `TypeRole::TechnicalValue` → JetBrains Mono Regular 400 @ 12 px (genuine technical value) |
| Tunnels status | default font @ TYPO_XS | `TypeRole::Metadata` → SS3 Regular 400 @ 12 px |
| CardShell header title (Online Peers / Recent Activity / Tunnels) | `Typography::SecondaryText` @ 12 | `TypeRole::CardTitle` → SS3 SemiBold 600 @ 18 px (uppercase + muted rail look kept) |
| CardShell count badge (`3/12`) | `Typography::SecondaryText` @ 12 | `TypeRole::Metadata` → SS3 Regular 400 @ 12 px |
| CardShell `View all` | `Typography::SecondaryText` @ 12 | `TypeRole::ButtonLabel` → SS3 SemiBold 600 @ 14 px |
| CardShell empty message | `Typography::SecondaryText` @ 12 | `TypeRole::SupportingText` → SS3 Regular 400 @ 13 px |
| Connection footer (`Mesh Healthy · N direct · N relayed · QUIC encrypted · N neighbors`) | default font @ 16 (iced default) | `TypeRole::Metadata` → SS3 Regular 400 @ 12 px (all texts incl. separators) |
| Brand wordmark (BORU, app chrome) | Raleway ExtraBold | unchanged — Raleway ExtraBold remains the only Raleway use in the app |

Family discipline enforced: `TypeRole::DisplayHeading` is the *only* route to
Manrope on the home screen (no local `manrope()` calls); JetBrains Mono is used
only for the tunnel `host:port` endpoint (a genuine technical value) and the
pre-existing technical IDs elsewhere; no friendly display name uses JBM.

## 2. Local font-declaration removal summary

Local/semantic font declarations removed from the home screen and its shared
components (each replaced by a `TypeRole` call):

- `app.rs` (home region): greeting `source_sans(Semibold)`; hero/pill/mesh/
  rail texts that relied on the iced default font + `TYPO_*` sizes; unused
  `text` imports removed from `view_online_peers_card`,
  `view_recent_activity_card`, `view_tunnels_card`, `view_chat_list_content`.
- `quick_actions.rs`: label `source_sans(Semibold)` @ `TYPO_MD`; description
  default font @ `TYPO_XS`; `TYPO_MD`/`TYPO_XS` removed from the `crate::app`
  import; the fixed `.height(Length::Fixed(132.0))` removed (content-driven
  height — see §4); grid rows top-aligned.
- `ui_components.rs` `connection_footer`: 7 plain `text(…)` sites (footer
  texts + separators) now `type_role_text(Metadata, …)`.
- `card_shell.rs`: header title, count badge, `View all`, empty message —
  4 `Typography::SecondaryText` sites migrated to `TypeRole`; the
  `use crate::fonts::Typography;` import and the `text` widget import were
  removed (now unused in the shell); module doc updated.

Remaining legacy `Typography` tokens are intentionally untouched elsewhere
(chat header IDs, dialogs, gallery — out of scope; UI-HOME-13/14 cover chat
and chrome respectively).

## 3. Quick-action clipping fix

UI-HOME-01 identified the clipping root cause at `quick_actions.rs:88` — a
fixed 132 px card height. With real font metrics the tallest quick-action
content (40 px icon tile + 18 px CardTitle + two-line 13 px SupportingText at
1.45 line height + 12 px vertical padding ≈ 136 px) exceeds the old box, so:

- the fixed `.height(Length::Fixed(132.0))` was removed → cards are
  content-driven and grow with wrapped descriptions;
- grid rows now `.align_y(Alignment::Start)` so a taller card in a row never
  shifts its neighbours' icons/titles vertically;
- a clipping-math regression test
  (`quick_action_natural_height_exceeds_old_fixed_box`) documents why the
  fixed box cannot return.

Verified unclipped at the 1280×800 reference width by OCR of the quick-action
region (all four descriptions readable, wrapped to two lines where needed) and
visually at 1600/1280/1024 (see evidence below).

## 4. Verification

- Build: `cargo build --example boru --features gui` → OK (exit 0; 217
  pre-existing warnings only).
- Tests: `cargo test --example boru --features gui` → 849 passed, 0 failed
  (844 prior + 5 new UI-HOME-12 tests):
  - `fonts::tests::type_role_text_lh_builds_text_widget` (helper smoke test —
    carried from the pre-block run; +23 lines in `fonts.rs`)
  - `quick_actions::tests::quick_action_cards_are_content_driven_not_fixed_height`
    — no fixed 132 px box; label/description resolve through `TypeRole`
  - `quick_actions::tests::quick_action_natural_height_exceeds_old_fixed_box`
    — clipping-math guard (content needs > 132 px)
  - `card_shell::tests::card_shell_text_uses_type_role` — shell header/badge/
    action/empty use `TypeRole`, no legacy `Typography::` in the shell
  - `app::tests::home_screen_uses_type_role_roles` — greeting/hero/mesh/pill
    use the approved roles; no local `manrope(` on the home screen;
    `type_role_text_lh` applies a relative line height
  - `app::tests::home_rail_cards_use_type_role_roles` — rail names as `Body`,
    timestamps/status as `Metadata`, tunnel endpoint as `TechnicalValue`
- Screenshots (before = `main` @ 28b05438, after = this change), three widths,
  fresh-launch Connecting state, `--no-dht --no-relay`, Xvfb + `import`:
  - `docs/ui-redesign/evidence/t_4c86d88c/t_4c86d88c_home_1600x900_{before,after}.png`
  - `docs/ui-redesign/evidence/t_4c86d88c/t_4c86d88c_home_1280x800_{before,after}.png`
  - `docs/ui-redesign/evidence/t_4c86d88c/t_4c86d88c_home_1024x720_{before,after}.png`
- Quick-action clipping check: `scripts/ui_home12_typography_evidence.sh after`
  → `OCR OK: all four quick-action descriptions visible at 1280x800 (no clipping)`.

## 5. Changed files

- `examples/iced_chat/app.rs` — home screen + rail cards → `TypeRole`; unused
  `text` imports removed; +2 regression tests
- `examples/iced_chat/fonts.rs` — `type_role_text_lh()` helper + smoke test
  (carried from the pre-block run)
- `examples/iced_chat/quick_actions.rs` — label/description → `TypeRole`;
  fixed 132 px height removed; rows top-aligned; +2 regression tests
- `examples/iced_chat/ui_components.rs` — `connection_footer` texts →
  `TypeRole::Metadata`
- `examples/iced_chat/card_shell.rs` — header title/badge/action/empty →
  `TypeRole`; legacy `Typography`/`text` imports removed; +1 regression test
- `scripts/ui_home12_typography_evidence.sh` — evidence script (3 widths +
  upscaled OCR clipping check)
- `docs/ui-redesign/UI-HOME-12-typography.md` — this report
- `docs/ui-redesign/evidence/t_4c86d88c/*.png` — before/after evidence

## 6. Remaining risks / notes

- Layout height re-check: card titles grew from 12 → 18 px, so rail card
  headers are taller; bodies are height-bounded (max_height) so cards grow
  slightly but nothing overflows the page (page scrolls on
  `gutter_scrollable`). Verified visually at all three widths.
- Quick-action descriptions wrap to two lines at the medium 1280 width (that
  is the intended content-driven behaviour; the old box clipped them).
- `quick_actions.rs` still uses `RADIUS_LG` for its card corner; UI-HOME-03
  (t_7595a388) will standardise the shared `RADIUS_CARD` token and re-apply
  its CardShell extension on top of this task's `TypeRole` styling — the
  shell changes here are deliberately minimal so its rewrite can adopt them.
- Chat (UI-HOME-13) and chrome/dialogs (UI-HOME-14) typography remain as
  separate tasks; nothing in this change touched chat rendering.
