# UI-HOME-14 — Typography Across Shared Application Chrome

- Task: `t_5c7a2325` (UI-HOME-14)
- Plan source: `Boru_Home_Screen_Tidy_and_Fonts_Hermes_Kanban_Plan.pdf` (UI-HOME-14 card)
- Repo: `/home/dan/iroh-gossip-chat` @ `main`
- Status: COMPLETE. Shared application chrome — file-sharing dashboard, creation
  dialogs, sidebar navigation, forms, buttons, tabs, chips, table text, log viewer
  and tooltips — now resolves text through the central `fonts::TypeRole` roles from
  UI-HOME-11. Legacy one-off font declarations in the migrated surfaces are removed.
  Behaviour and layout are unchanged.

## 1. What was delivered

Applied the central semantic typography roles (UI-HOME-11 `TypeRole`) to every
shared-chrome surface in scope for this card. The role → font mapping enforced
throughout:

| Role family | Used for | Scope rule |
|---|---|---|
| Source Sans 3 | General UI: page/section/card titles, body, button labels, form labels, metadata, supporting text | default app font + all general roles |
| Manrope | `DisplayHeading` only | major display headings ONLY |
| Figtree | `ChatMessage/ChatSender/ChatMetadata/ComposerText` | chat content + composer ONLY (UI-HOME-13, unchanged) |
| Raleway ExtraBold | `BrandWordmark` | BORU wordmark ONLY |
| JetBrains Mono | `TechnicalValue` | genuine technical identifiers only (log contents, peer IDs) |

## 2. Migrated surfaces

| Surface | Files | Change |
|---|---|---|
| Shared sidebar navigation (chat rows, groups, ticket join, discovered peers, public rooms, friends, requests) | `app.rs` | names → `Body`, previews/supporting lines → `SupportingText`, timestamps/badges → `Metadata`, empty-state actions → `ButtonLabel` |
| File-sharing dashboard (all tabs/views: Shared by Me table, Shared with Me, peer catalogue, sharing summary, recent activity) | `app.rs`, `shared_by_me_table.rs`, `sharing_summary.rs` | card/section titles → `CardTitle`/`SectionTitle`, rows → `Body`, table metadata → `Metadata`, buttons/menu items → `ButtonLabel`, status → `SupportingText` |
| Creation dialogs (Create Public Room, Create Group Chat, Create Tunnel) | `app.rs`, `boru_dialog.rs`, `form_components.rs` | dialog title → `SectionTitle`, subtitle/helper/error → `SupportingText`, buttons → `ButtonLabel`, form labels → `ButtonLabel`, inputs → `Body` size |
| Shared controls: buttons, tabs, chips, table headers, tooltips, sidebar sections | `ui_components.rs`, `icon_system.rs` | `Typography::ButtonLabel/SecondaryText/Body/SectionHeading` → `TypeRole::ButtonLabel/Metadata/Body/SupportingText/CardTitle` |
| Log viewer | `log_viewer.rs` | header → `SectionTitle`, reload → `ButtonLabel`, body → `Body`, log path → `Metadata`, log contents → `TechnicalValue` (JetBrains Mono) |
| Local profile block, profile identity card, settings profile editor | `app.rs` | removed raw `TYPO_` text sizes; resolved through `TypeRole` |

Specialised fonts stayed in their lanes:
- `raleway_extra_bold()` call sites: exactly 2, both the BORU wordmark (brand row + `BoruLogo`).
- Figtree appears outside `fonts.rs` only in tests asserting the chat/composer roles.
- Manrope outside `fonts.rs` only in a home-screen regression assertion that
  general UI must NOT use it.
- JetBrains Mono: log-viewer contents + technical identifiers (peer IDs) only.
- Usernames are **not** forced into monospace — they render as `Body`/`ChatSender`
  (Source Sans 3 / Figtree), only true technical IDs (public keys, peer IDs) use
  JetBrains Mono.

Default font: `main.rs` `.default_font()` already pins Source Sans 3 (UI-HOME-11
era), so text without an explicit role inherits the general UI font.

## 3. Obsolete declarations removed

- `app.rs`: removed `pub const TYPO_*` size constants that were used to hand-size
  text in shared chrome (sidebar, profile, dashboard, dialogs). Remaining `TYPO_*`
  uses are icon sizing (`icon_svg(…, TYPO_SM)`) and the chat layout cache / avatar
  initials — icon geometry and chat sizing, not text declarations.
- `icon_system.rs`, `log_viewer.rs`, `ui_components.rs`, `boru_dialog.rs`,
  `form_components.rs`, `shared_by_me_table.rs`, `sharing_summary.rs`: replaced
  `Typography::*` direct font/size calls with `TypeRole::*` equivalents; dropped
  now-unused imports (`Typography`, `Weight`, `TYPO_*`).
- `Typography::` remains in `component_gallery.rs` (dev-only design-system gallery,
  intentionally showcases legacy tokens) — not part of the shipped UI.

Note: `dashboard.rs`, `file_library.rs`, `invitation_qr.rs` are tracked but orphan
files (not declared as modules — dead code, no live rendering). They contain no
font declarations, so no migration was required; left untouched to avoid scope
creep.

## 4. Verification

- Build: `cargo build --example boru --features gui` → OK (exit 0, 43.6 s).
- Tests: `cargo test --example boru --features gui` → **853 passed, 0 failed**
  (62.9 s). Includes 4 new UI-HOME-14 regression guards:
  - `sidebar_navigation_uses_type_role_roles` — sidebar rows/buttons/previews
    resolve through `TypeRole::Body/SupportingText/Metadata/ButtonLabel`.
  - `file_sharing_dashboard_uses_type_role_roles` — catalogue + Shared with Me +
    sharing summary use `SectionTitle/CardTitle/ButtonLabel`.
  - `creation_dialogs_use_migrated_shared_components` — Create Public Room /
    Group Chat / Tunnel build on `BoruDialog` + `FormSection` (no local fonts).
  - `shared_chrome_no_raw_typo_text` — local profile block + profile identity card
    no longer declare raw `TYPO_` text sizes.
- Smoke test: launched `target/debug/examples/boru` under Xvfb (1280×800) with
  `--no-dht --no-relay`; app stayed alive the full 30 s window, rendered the
  window (verified via `import` capture, 1280×800 PNG), no panics; only expected
  libEGL software-rendering warnings. Clean exit via timeout.
- Evidence screenshots (all four required, captured under Xvfb 1280×800 with the
  evidence script, pixel-verified non-blank):
  - `docs/ui-redesign/evidence/t_5c7a2325/t_5c7a2325_home_1280x800.png`
  - `docs/ui-redesign/evidence/t_5c7a2325/t_5c7a2325_chat_1280x800.png`
  - `docs/ui-redesign/evidence/t_5c7a2325/t_5c7a2325_file_sharing_1280x800.png`
  - `docs/ui-redesign/evidence/t_5c7a2325/t_5c7a2325_create_group_1280x800.png`

## 5. Files changed (this task only)

- `examples/iced_chat/app.rs` — shared chrome + dashboard + creation dialogs + profile typography migration + 4 regression tests
- `examples/iced_chat/ui_components.rs` — shared control typography (buttons/tabs/chips/table headers/sidebar sections)
- `examples/iced_chat/shared_by_me_table.rs` — Shared by Me table typography
- `examples/iced_chat/sharing_summary.rs` — sharing summary card typography
- `examples/iced_chat/boru_dialog.rs` — dialog title/subtitle/button roles
- `examples/iced_chat/form_components.rs` — form labels/helper/error/input roles
- `examples/iced_chat/icon_system.rs` — tooltip text roles
- `examples/iced_chat/log_viewer.rs` — log viewer roles
- `docs/ui-redesign/UI-HOME-14-typography.md` — this report
- `docs/ui-redesign/evidence/t_5c7a2325/` — 4 evidence screenshots
- `scripts/ui14_home_evidence.sh` — reusable evidence-capture script
- `scripts/ui14_mcp_debug.sh` — MCP-navigation debug helper used during capture

## 6. Remaining risks / notes for downstream cards

- **Orphan files**: `dashboard.rs`, `file_library.rs`, `invitation_qr.rs` are
  tracked but not compiled (no `mod` declaration). No font work needed there;
  they could be deleted in a cleanup card.
- **`component_gallery.rs`** intentionally still exercises legacy `Typography`
  tokens (dev-only preview). Fine to leave; if the gallery should show the new
  tokens exclusively, that is a separate small card.
- `connection_details.rs` / `download_progress_view.rs` (not in the card's
  relevant-file list) still call `fonts::source_sans(...)` directly; family is
  Source Sans 3 so they are visually consistent with the system, but they do not
  route through `TypeRole`. Optional follow-up if strict token adoption is wanted.
- Remaining `TYPO_*` in `app.rs` are icon sizes and chat layout cache/avatar
  initials — intentional, not text declarations.
- No font files exposed in reports/artifacts (OFL records live in-repo from
  UI-HOME-11).
- This card (with 04–13) gates UI-HOME-15.
