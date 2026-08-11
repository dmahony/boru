# FONTS-01 — Audit of Existing Typography Declarations

- Task: `t_4ded971e` (FONTS-01)
- Plan source: `Boru_FONTS` spec, Task 1 — Audit Existing Typography (attached to `t_15c72313`)
- Repo: `iroh-gossip-chat` @ `main` (branch `wt/t_4ded971e` during audit)
- Status: COMPLETE (audit only — **no production code modified**)
- Scope: `examples/iced_chat/` Boru iced app + whole-repo search for font declarations.

This document is the input for downstream FONTS cards (add Archivo SemiCondensed,
add IBM Plex Sans, update `TypeRole`, migrate screens, remove Source Sans 3 /
Manrope / Inter). All line numbers are as of commit `590bd110` (the worktree base).

---

## 0. Executive summary

The Boru app has a **single central typography module** — `examples/iced_chat/fonts.rs`
(889 lines) — that owns font-file registration (`include_bytes!` + `font::load`),
family-name constants, font constructors, and two token systems:

1. **`TypeRole`** (fonts.rs:357) — the canonical semantic role enum (15 roles). This is
   what the shipped UI uses today.
2. **`Typography`** (fonts.rs:237) — legacy token enum, marked `#[expect(dead_code)]`.
   It survives only in the dev-only `component_gallery.rs` (design-system preview) and
   in fonts.rs unit tests. No shipped screen uses it.

Font families in play (fonts.rs header doc, lines 7–17):

| Family | Weights loaded at startup | Scope |
|---|---|---|
| Source Sans 3 | 400 · 500 · 600 · 700 | Primary app font |
| Manrope | 600 (SemiBold) · 700 (Bold) | Display headings only (`DisplayHeading`) |
| Figtree | 400 · 500 · 600 | Chat messages / sender / metadata / composer |
| Raleway | 800 (ExtraBold) | BORU wordmark ONLY |
| JetBrains Mono | 400 · 500 | Technical values |
| Inter | 400 · 500 · 600 · 700 (bundled, **NOT loaded**) | Legacy fallback — removable |

Key facts for the migration:

- The **only direct (non-token) production font calls** are: `raleway_extra_bold()` at
  app.rs:407 (BoruLogo) and app.rs:23358 (sidebar brand row); `source_sans(Semibold)`
  at ui_components.rs:1656 (`system_event_chip`); the app-level default font in
  main.rs:1618–1623. Everything else resolves through `TypeRole`/`type_role_text`.
- **BORU wordmark must NOT change** — it is Raleway ExtraBold 800 via
  `BoruLogo` (app.rs:402–414) and the sidebar brand row (app.rs:23356–23361).
- **Inter is completely removable today** (no runtime reference; all consts are
  `#[expect(dead_code)]`).
- **Source Sans 3 and Manrope removal** requires touching fonts.rs token mappings,
  main.rs default font, one direct call site (ui_components.rs:1656), component_gallery
  demo text, tests, and docs — see §8.

---

## 1. Where each font family is loaded (fonts.rs + startup)

### 1.1 Font file bytes (`include_bytes!`)

All font binaries are bundled at compile time in `examples/iced_chat/fonts.rs`
lines 33–95. **No other file in the repo embeds a font** (whole-repo search for
`include_bytes!("fonts/` returns only `fonts.rs`).

| Const | Line | Asset | Weight |
|---|---|---|---|
| `SOURCE_SANS_REGULAR_BYTES` | 34 | `fonts/SourceSans3-Regular.ttf` | 400 |
| `SOURCE_SANS_SEMI_BOLD_BYTES` | 37 | `fonts/SourceSans3-SemiBold.ttf` | 600 |
| `SOURCE_SANS_MEDIUM_BYTES` | 40 | `fonts/SourceSans3-Medium.ttf` | 500 |
| `SOURCE_SANS_BOLD_BYTES` | 43 | `fonts/SourceSans3-Bold.ttf` | 700 |
| `INTER_REGULAR_BYTES` | 47 | `fonts/Inter-Regular.ttf` | 400 (dead_code) |
| `INTER_MEDIUM_BYTES` | 51 | `fonts/Inter-Medium.ttf` | 500 (dead_code) |
| `INTER_SEMI_BOLD_BYTES` | 55 | `fonts/Inter-SemiBold.ttf` | 600 (dead_code) |
| `INTER_BOLD_BYTES` | 59 | `fonts/Inter-Bold.ttf` | 700 (dead_code) |
| `MANROPE_BYTES` | 63 | `fonts/Manrope.ttf` (variable 200–800) | legacy, NOT loaded |
| `MANROPE_SEMI_BOLD_BYTES` | 66 | `fonts/Manrope-SemiBold.ttf` | 600 |
| `MANROPE_BOLD_BYTES` | 69 | `fonts/Manrope-Bold.ttf` | 700 |
| `FIGTREE_REGULAR_BYTES` | 72 | `fonts/Figtree-Regular.ttf` | 400 |
| `FIGTREE_MEDIUM_BYTES` | 75 | `fonts/Figtree-Medium.ttf` | 500 |
| `FIGTREE_SEMI_BOLD_BYTES` | 78 | `fonts/Figtree-SemiBold.ttf` | 600 |
| `RALEWAY_EXTRA_BOLD_BYTES` | 81 | `fonts/Raleway-ExtraBold.ttf` | 800 |
| `JETBRAINS_MONO_BYTES` | 85 | `fonts/JetBrainsMono.ttf` (variable) | legacy, NOT loaded |
| `JETBRAINS_MONO_ITALIC_BYTES` | 89 | `fonts/JetBrainsMono-Italic.ttf` (variable) | legacy, NOT loaded |
| `JETBRAINS_MONO_REGULAR_BYTES` | 92 | `fonts/JetBrainsMono-Regular.ttf` | 400 |
| `JETBRAINS_MONO_MEDIUM_BYTES` | 95 | `fonts/JetBrainsMono-Medium.ttf` | 500 |

### 1.2 Family name constants (fonts.rs:99–116)

- `SOURCE_SANS` = "Source Sans 3" (100)
- `INTER` = "Inter" (104, dead_code)
- `MANROPE` = "Manrope" (107)
- `FIGTREE` = "Figtree" (110)
- `RALEWAY` = "Raleway" (113)
- `JETBRAINS_MONO` = "JetBrains Mono" (116)

### 1.3 Font constructors (fonts.rs:121–179)

| Constructor | Line | Produces |
|---|---|---|
| `source_sans(weight)` | 121–128 | `Font { family: Family::Name(SOURCE_SANS), … }` |
| `inter(weight)` | 132–139 | Inter (dead_code) |
| `manrope(weight)` | 142–149 | Manrope |
| `figtree(weight)` | 152–159 | Figtree |
| `raleway_extra_bold()` | 162–169 | Raleway ExtraBold 800 |
| `jetbrains_mono(weight)` | 172–179 | JetBrains Mono |

### 1.4 Runtime registration (`load_fonts()`, fonts.rs:597–617)

Called exactly once at app startup — `main.rs:1607`:
`let task = task.chain(fonts::load_fonts());` inside `IcedChat::new`.

Registers (each `font::load(...).map(|_| AppMessage::Noop)`):
- Source Sans 3 ×4 (400/500/600/700) — lines 600–603
- Manrope ×2 (600/700) — lines 605–606
- Figtree ×3 (400/500/600) — lines 608–610
- Raleway ExtraBold — line 612
- JetBrains Mono ×2 (400/500) — lines 614–615

**NOT registered at startup**: Inter (all four weights), `Manrope.ttf` variable,
`JetBrainsMono.ttf` variable, `JetBrainsMono-Italic.ttf`. These are
compiled-in but never loaded (dead_code consts).

### 1.5 App default font (main.rs:1618–1623)

```rust
.default_font(iced::Font {
    family: iced::font::Family::Name(crate::fonts::SOURCE_SANS),
    weight: iced::font::Weight::Normal,
    stretch: iced::font::Stretch::Normal,
    style: iced::font::Style::Normal,
})
```

This is the ONLY `default_font` in the repo. Any `text(...)` widget that does not
explicitly set `.font(...)` inherits Source Sans 3 Regular from this default.

---

## 2. Direct constructor references vs semantic tokens

### 2.1 Direct constructor calls in production code (non-test, outside fonts.rs)

| Call site | File:line | Component / context |
|---|---|---|
| `crate::fonts::raleway_extra_bold()` | app.rs:407 | `BoruLogo::into()` (the `From<BoruLogo> for Element` impl, app.rs:402–414) — wordmark |
| `crate::fonts::raleway_extra_bold()` | app.rs:23358 | `view_sidebar()` brand row — `text("BORU").font(...).size(20.0)` |
| `crate::fonts::source_sans(iced::font::Weight::Semibold)` | ui_components.rs:1656 | `system_event_chip()` — label chip in timeline system entries |

### 2.2 Direct `iced::Font { … }` struct construction outside fonts.rs

| File:line | Context |
|---|---|
| main.rs:1618–1623 | `.default_font(...)` — app default (see §7) |
| component_gallery.rs:1693–1698 | `weight_sample()` helper — dev-only gallery demo of registered weights |

### 2.3 Semantic-token consumers (TypeRole / type_role_text / type_role_text_lh)

These are the components that already resolve typography through the central system
and therefore need NO per-component font edits when families are swapped — they only
change via the `TypeRole` mapping in fonts.rs:

- `type_role_text(role, …)` / `type_role_text_lh(role, …, line_height)` helpers defined
  at fonts.rs:531–549; used across the app (see file-by-file usage counts below).
- `TypeRole::X.font()` direct call sites (per file):
  - `boru_dialog.rs` — 4 (SectionTitle:185, SupportingText:194, ButtonLabel:298,312)
  - `shared_by_me_table.rs` — 35 (CardTitle/ButtonLabel/Body/BodyEmphasised/SupportingText/Metadata across card, share menu, table, details, empty/error states)
  - `ui_components.rs` — 37 (buttons, inputs, chips, tabs, cards, empty states, sidebar sections)
  - `sharing_summary.rs` — 4 (CardTitle:178, Metadata:184, PageTitle:214, Metadata:221)
  - `form_components.rs` — 9 (ButtonLabel/SupportingText/Body/Metadata across form_label, helper_text, error_text, FormSection, peer_list, remove_chip, destructive_button)
  - `connection_details.rs` — 2 (TechnicalValue:449,456 in `connection_detail_row`)
  - `app.rs` — 29 production `.font(TypeRole::…)` sites (see §4)
  - `component_gallery.rs` — `TypeRole` gallery demo (see §2.4)
- `type_role_text` usage (fonts.rs helper): app.rs (~460 refs), ui_components.rs,
  card_shell.rs (289–519), icon_system.rs (386, 398 tooltips), file_type_icon.rs (466),
  quick_actions.rs (115, 126–130), video_file_card.rs (340–1461), log_viewer.rs (42–71),
  download_progress_view.rs (345–1018), connection_details.rs (319–543),
  component_gallery.rs (1670).

### 2.4 Legacy `Typography` token usage (fonts.rs:237 enum)

- `Typography::` direct references outside fonts.rs:
  - `component_gallery.rs` — 27 refs (dev-only design-system preview; intentionally
    showcases legacy tokens per UI-HOME-14 report §3). Gallery heading/sections at
    110–136, card gallery at 257–407, dialog example at 1020–1024, typography section
    captions at 1675/1707/1755.
  - `card_shell.rs` — 0 production refs; only a test assertion (line 872:
    `!prod.contains("Typography::")`) proving the shell avoids legacy tokens.
- `typo_text` / `with_typo` / `typo_text_scaled` (fonts.rs:556–582): **all
  `#[expect(dead_code)]` and unused outside fonts.rs** (whole-repo search).
- Fonts.rs unit tests exercise both enums (§ fonts.rs:621–889).

### 2.5 Files with ZERO font references (checked)

`design_tokens.rs`, `icon_system.rs` (fonts via helper only), `download_progress_view.rs`
(helper only), `video_file_card.rs` (helper only), `quick_actions.rs` (helper only),
`terminal_view.rs`, `notification/*`, `mcp_server.rs`, `gui_test_actions.rs`,
`focusable_button.rs`, `perf_tracker.rs`, `presentation.rs`, view-model files,
`dashboard_view_model.rs`, etc. Orphan files `dashboard.rs`, `file_library.rs`,
`invitation_qr.rs` are not declared as modules and contain no font declarations.

---

## 3. `TypeRole` variants and current mapping (quoted from fonts.rs)

Enum declaration (fonts.rs:355–388):

```rust
/// Canonical semantic typography roles.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TypeRole {
    /// Hero / page greeting — Manrope Bold 32.
    DisplayHeading,
    /// Application page title — Source Sans 3 SemiBold 28.
    PageTitle,
    /// Section heading — Source Sans 3 SemiBold 20.
    SectionTitle,
    /// Card title — Source Sans 3 SemiBold 18.
    CardTitle,
    /// Body copy / descriptions — Source Sans 3 Regular 15.
    Body,
    /// Emphasised body copy — Source Sans 3 SemiBold 15.
    BodyEmphasised,
    /// Button and interactive label — Source Sans 3 SemiBold 14.
    ButtonLabel,
    /// Supporting / secondary copy — Source Sans 3 Regular 13.
    SupportingText,
    /// Metadata (timestamps, counts) — Source Sans 3 Regular 12.
    Metadata,
    /// Chat message body — Figtree Regular 15.
    ChatMessage,
    /// Chat sender name — Figtree SemiBold 14.
    ChatSender,
    /// Chat message metadata (timestamp/status) — Figtree Regular 12.
    ChatMetadata,
    /// Composer input and placeholder — Figtree Regular 15.
    ComposerText,
    /// Technical identifier (peer ID, hash, port, fingerprint) — JetBrains Mono Regular 12.
    TechnicalValue,
    /// BORU wordmark — Raleway ExtraBold 28.
    BrandWordmark,
}
```

Current mapping (from `family_name()` fonts.rs:392–407, `weight()` 411–429,
`size_px()` 432–444):

| Role | Family | Weight | Size px | Fallback family / weight |
|---|---|---|---|---|
| `DisplayHeading` | Manrope | Bold 700 | 32 | SS3 Bold |
| `PageTitle` | Source Sans 3 | SemiBold 600 | 28 | SS3 SemiBold |
| `SectionTitle` | Source Sans 3 | SemiBold 600 | 20 | SS3 SemiBold |
| `CardTitle` | Source Sans 3 | SemiBold 600 | 18 | SS3 SemiBold |
| `Body` | Source Sans 3 | Regular 400 | 15 | SS3 Regular |
| `BodyEmphasised` | Source Sans 3 | SemiBold 600 | 15 | SS3 SemiBold |
| `ButtonLabel` | Source Sans 3 | SemiBold 600 | 14 | SS3 SemiBold |
| `SupportingText` | Source Sans 3 | Regular 400 | 13 | SS3 Regular |
| `Metadata` | Source Sans 3 | Regular 400 | 12 | SS3 Regular |
| `ChatMessage` | Figtree | Regular 400 | 15 | SS3 Regular |
| `ChatSender` | Figtree | SemiBold 600 | 14 | SS3 SemiBold |
| `ChatMetadata` | Figtree | Regular 400 | 12 | SS3 Regular |
| `ComposerText` | Figtree | Regular 400 | 15 | SS3 Regular |
| `TechnicalValue` | JetBrains Mono | Regular 400 | 12 | platform monospace |
| `BrandWordmark` | Raleway | ExtraBold 800 | 28 | SS3 Bold |

`font()` (fonts.rs:447–455) dispatches on `family_name()` to the constructors;
`fallback_family()` (461–466) returns `Family::Name(SOURCE_SANS)` for everything
except `TechnicalValue` (monospace); `fallback_weight()` (470–477) clamps to a
registered SS3 weight; `fallback_font()` (480–487) builds the fallback `Font`.
`label()` (490–508) and `ALL` (511–527) enumerate all 15 roles.

---

## 4. Every direct `.font(...)` call site in app.rs and view files

### 4.1 app.rs (44,545 lines) — 29 production sites + 8 in tests

| Line | Component / function | Font |
|---|---|---|
| 408 | `From<BoruLogo> for Element` | `raleway_extra_bold()` (BORU wordmark) |
| 23358 | `view_sidebar()` brand row | `raleway_extra_bold()` (BORU, size 20) |
| 24038 | `view_sidebar_ticket_join()` | `TypeRole::Body.font()` |
| 26624, 26628, 26655, 26659 | `view_chat_panel()` | `Body` / `SupportingText` |
| 27099 | `view_gif_picker()` | `Body` |
| 27646 | `view_chat_search_panel()` | `Body` |
| 28202 | `view_details_panel()` | `Body` |
| 29180, 29193, 29206, 29216 | `view_chat_log()` sender labels | `ChatSender` |
| 29233, 29247, 29260, 29358, 29368, 29388, 29402, 29423, 29509, 29704, 29725, 29761 | `view_chat_log()` message bodies | `ChatMessage` |
| 29329 | `view_chat_log()` metadata | `ChatMetadata` |
| 29840 | `view_composer()` input | `ComposerText` |
| 35331 | `view_file_sharing_content()` | `Body` |

Test-only `.font(...)` string assertions (app.rs, `#[cfg(test)]`):
38328/38329/38332/38333/38336/38337 (chat timeline roles), 38362/38363 (composer),
38348 (asserts NO `source_sans(Semibold)` in timeline).

### 4.2 boru_dialog.rs

| Line | Component | Font |
|---|---|---|
| 185 | `BoruDialog::build` title | `SectionTitle` |
| 194 | `BoruDialog::build` subtitle | `SupportingText` |
| 298, 312 | `footer_row` buttons | `ButtonLabel` |

### 4.3 shared_by_me_table.rs (35 sites)

| Line | Component | Font |
|---|---|---|
| 406 | `view_shared_by_me_card` | `SupportingText` |
| 449 | `share_menu` | `ButtonLabel` |
| 504, 510, 528, 549 | `card_header` | `CardTitle` / `SupportingText` / `ButtonLabel` / `Metadata` |
| 577 | `column_header` | `Metadata` |
| 655 | `footer_count` | `Metadata` |
| 673, 678 | `view_row` | `Metadata` |
| 788, 799 | `name_cell` | `Body` / `Metadata` |
| 867, 880 | `shared_with_cell` | `Metadata` |
| 926 | `recipient_chip` | `Metadata` |
| 954 | `all_friends_chip` | `Metadata` |
| 979 | `downloads_cell` | `Metadata` |
| 1026 | `action_menu` | `ButtonLabel` |
| 1092, 1101, 1111, 1131 | `stop_sharing_confirmation` | `BodyEmphasised` / `SupportingText` / `ButtonLabel` ×2 |
| 1185–1331 | `details_panel` | `Metadata`/`SupportingText`/`ButtonLabel`/`BodyEmphasised` |
| 1400, 1407 | `empty_body` | `Body` / `SupportingText` |
| 1468, 1475 | `error_body` | `Body` / `SupportingText` |

### 4.4 ui_components.rs (37 sites)

| Line | Component | Font |
|---|---|---|
| 464 | `primary_button` | `ButtonLabel` |
| 500 | `primary_button_icon` | `ButtonLabel` |
| 531 | `secondary_button` | `ButtonLabel` |
| 583 | `ghost_icon_button` | `Metadata` |
| 668, 743, 774 | `text_input_field_opts` | `Body` / `Metadata` ×2 |
| 897, 907 | `SelectablePeerRow::build` | `Body` / `SupportingText` |
| 1005, 1016 | `empty_state` | `CardTitle` / `SupportingText` |
| 1154–1202 | `PeerChip`/`chip_row`-style build | `Metadata` |
| 1277 | `section_header` | `ButtonLabel` |
| 1321 | `card_header` | `CardTitle` |
| 1416, 1431 | `SearchableSelect::build` | `ButtonLabel` / `Metadata` |
| 1551, 1559, 1577 | `sidebar_empty_state` | `Body` / `SupportingText` / `ButtonLabel` |
| **1656** | **`system_event_chip`** | **`source_sans(Semibold)` — DIRECT** |
| 1730–1733 | `TabStrip::build` | `ButtonLabel` (both branches) |
| 1961 | `build` (recent activity row) | `Metadata` |
| 2020, 2026 | `build` (list rows) | `Body` / `SupportingText` |
| 2115 | `build` | `Metadata` |
| 2170, 2176 | `build` (dashboard card) | `CardTitle` / `Metadata` |
| 2336, 2360 | `build` (sidebar section) | `SupportingText` / `ButtonLabel` |
| 2427 | `build` | `Metadata` |
| 2590, 2610 | `build` (status card) | `SupportingText` / `ButtonLabel` |

### 4.5 sharing_summary.rs

| Line | Component | Font |
|---|---|---|
| 178, 184 | `view_sharing_summary_card` | `CardTitle` / `Metadata` |
| 214, 221 | `metric_cell` | `PageTitle` / `Metadata` |

### 4.6 form_components.rs

| Line | Component | Font |
|---|---|---|
| 81 | `form_label` | `ButtonLabel` |
| 92 | `helper_text` | `SupportingText` |
| 106 | `error_text` | `SupportingText` |
| 153 | `FormSection::build` | `ButtonLabel` |
| 782, 794 | `FormTextInput::build` | `Body` / `SupportingText` |
| 846 | `peer_list` | `SupportingText` |
| 899 | `remove_chip` | `Metadata` |
| 1174 | `destructive_button` | `ButtonLabel` |

### 4.7 connection_details.rs

| Line | Component | Font |
|---|---|---|
| 449, 456 | `connection_detail_row` | `TechnicalValue` (JetBrains Mono — peer IDs, fingerprints) |

### 4.8 component_gallery.rs (dev-only)

| Line | Component | Font |
|---|---|---|
| 113–114 | `gallery_heading` | `Typography::PageTitle` |
| 122–123 | `gallery_section` | `Typography::SectionHeading` |
| 333–341, 385–407 | `card_shell_row` / `card_shell_gallery` | `Typography::Body` / `SecondaryText` |
| 1024 | `dialog_example` | `Typography::SectionHeading` |
| 1670 | `typography_gallery` role demo | `type_role_text(role, …)` |
| 1700 | `weight_sample` | inline `iced::Font { family: Family::Name(family), … }` |
| 1760 | `fallback_sample` | `role.fallback_font()` |

### 4.9 Helper-only files (no direct `.font(...)`)

`icon_system.rs`, `card_shell.rs`, `file_type_icon.rs`, `quick_actions.rs`,
`video_file_card.rs`, `log_viewer.rs`, `download_progress_view.rs` — all resolve
through `type_role_text(...)` / `type_role_text_lh(...)` (see §2.3 for lines).

---

## 5. Screen-specific font overrides

There is **no screen-level "font family" override** — no screen sets a different family
globally. The app-wide family is the main.rs default (Source Sans 3). What exists is a
small set of **size/line-height overrides on top of a `TypeRole`** and two brand-row
exceptions:

1. **Home greeting (app.rs:25872–25882)** — `type_role_text_lh(DisplayHeading, …,
   line_height: 1.2)`: Manrope Bold 32 with relative line-height 1.2. Explicitly
   documented as UI-HOME-12 (display_heading). Wraps with `Wrapping::WordOrGlyph`.
2. **Home subtitle (app.rs:25884–25887)** — `type_role_text(Body, "Welcome to Boru")`
   with `.size(HOME_SUBTITLE)` (=16.0, fonts.rs:199), overriding the `Body` default of
   15 px. Comment notes this is intentional (UI-HOME-02 size token 15–17 px band).
3. **Sidebar brand row (app.rs:23356–23361)** — `text("BORU").font(raleway_extra_bold())
   .size(20.0)` — the only place Raleway is used at a non-28px size (wordmark must NOT
   change).
4. **Splash screen (app.rs:22625–22647)** — `boru_logo(LogoSize::Large)` (Raleway
   ExtraBold 44 px) + plain `text(...).size(14)` version line that inherits the default
   font (Source Sans 3).
5. **`system_event_chip` (ui_components.rs:1656)** — timeline system entries use
   `source_sans(Semibold)` directly (not a token). It is a "screen-specific" styling
   override in the chat timeline; a test (app.rs:38348) asserts the chat **sender**
   labels do NOT use this direct call.
6. **Tab strip (ui_components.rs:1730–1733)** — both active and inactive tabs resolve to
   `TypeRole::ButtonLabel.font()` (the conditional is vestigial; no visual difference).

No other per-screen family overrides were found.

---

## 6. BORU logo usage sites (must NOT change)

| Site | File:line | Details |
|---|---|---|
| `BRAND_LOGO_WEIGHT` const | app.rs:330–332 | `pub(crate) const BRAND_LOGO_WEIGHT: u16 = 800;` (dead_code; documents the brand weight) |
| `LogoSize` enum | app.rs:336–357 | Small=16 px, Medium=28 px (dead_code variant), Large=44 px |
| `boru_logo()` factory | app.rs:372–378 | `BoruLogo { size, color: None, … }` |
| `BoruLogo` builder + `into_element()` | app.rs:388–400 | |
| `From<BoruLogo> for Element` | app.rs:402–414 | **`let font = crate::fonts::raleway_extra_bold();` (407)**; `text("BORU").font(font).size(font_size)` (408) |
| Splash screen usage | app.rs:22647 | `boru_logo(LogoSize::Large).color(text_color).into_element()` |
| Sidebar brand row | app.rs:23356–23361 | `text("BORU").font(crate::fonts::raleway_extra_bold()).size(20.0)` |
| Component gallery sample | component_gallery.rs:1642, 1659 | `TypeRole::BrandWordmark => "BORU"` / `"Raleway ExtraBold 800"` |

Downstream constraint: the BORU wordmark is Raleway ExtraBold in exactly two render
sites (BoruLogo component and the sidebar brand row) plus the gallery demo string. The
FONTS plan says "Do not change the existing BORU logo text/font" — these sites stay
Raleway ExtraBold; only the *label* strings in component_gallery could be updated if the
family name changes.

---

## 7. main.rs `.default_font()` usage

- main.rs:1618–1623 — the single `.default_font(...)` call; pins `Family::Name(SOURCE_SANS)`
  at `Weight::Normal`. Inherited by every `text(...)` widget that doesn't set a font.
- main.rs:1607 — `fonts::load_fonts()` chained into the startup task (font loading).
- main.rs:26 — `mod fonts;` declaration.

---

## 8. Old declarations/assets that can be removed later + files to touch first

### 8.1 Inter — removable NOW (no runtime references)

All Inter consts are `#[expect(dead_code)]`:
- fonts.rs:46–59 (4 byte consts), fonts.rs:103–104 (`INTER` family const),
  fonts.rs:131–139 (`inter()` constructor).
- Assets: `fonts/Inter-Regular.ttf`, `Inter-Medium.ttf`, `Inter-SemiBold.ttf`,
  `Inter-Bold.ttf`, `Inter-OFL.txt`.
- Unit tests reference Inter bytes: fonts.rs:637–640.
- Doc references: fonts.rs header (line 16), THIRD_PARTY_NOTICES.md.

Files to touch for Inter removal: `examples/iced_chat/fonts.rs` (consts, constructor,
tests, header doc), delete the 5 asset files, update `THIRD_PARTY_NOTICES.md`.
(Removal is optional — plan says remove after new migration.)

### 8.2 Source Sans 3 — removal candidates (only after IBM Plex Sans lands)

If Source Sans 3 is replaced by IBM Plex Sans as the general UI font, these must be
touched **before** deleting the assets:

1. `examples/iced_chat/fonts.rs`:
   - byte consts 34–43; family const `SOURCE_SANS` 100; constructor 121–128;
   - `load_fonts()` entries 600–603 (swap to IBM Plex Sans bytes);
   - `TypeRole::family_name()` 395–402 (Source Sans arms → IBM Plex Sans),
     `weight()` 411–429 (unchanged weights), `size_px()` 432–444 (unchanged),
     `font()` 447–455 (`_ => source_sans(...)` → new constructor),
     `fallback_family()` 461–466 (`Family::Name(SOURCE_SANS)` → new family const),
     `fallback_weight()` unchanged;
   - legacy `Typography` enum family_name 281 (`_ => SOURCE_SANS`), font() 322–328;
   - unit tests: 626–654 (byte checks), 657–691 (family/weight registration),
     694–708 (role family assertions), 711–740 (real weights), 743–765 (fallbacks),
     805–825 (`source_sans_is_primary_for_ui_text`), 869–874 (sidebar label),
     877–888 (legacy sizes).
2. `examples/iced_chat/main.rs:1618–1623` — `.default_font(...)` family name.
3. `examples/iced_chat/ui_components.rs:1656` — `system_event_chip` direct
   `source_sans(Semibold)` → switch to a TypeRole or the new constructor.
4. `examples/iced_chat/component_gallery.rs` — demo strings "Source Sans 3" and the
   `weight_sample` family-name params (1713–1720), fallback note text (1751–1756),
   role captions (1644–1659), sample body copy (1630–1633).
5. `examples/iced_chat/app.rs` — home subtitle size override stays (uses `Body` role +
   size), no direct SS3 refs in production; tests at 38380–38396 assert
   `TypeRole::ChatMessage.family_name() == FIGTREE` (unaffected); test at 38348 asserts
   absence of `source_sans(Semibold)` in timeline (may need updating if the chip's
   direct call moves).
6. Docs: `DESIGN_SYSTEM.md` §1 (lines 13–49), `docs/ui-redesign/UI-HOME-11-typography.md`,
   `docs/ui-redesign/UI-HOME-12-typography.md`, `docs/ui-redesign/UI-HOME-13-chat-typography.md`,
   `docs/ui-redesign/UI-HOME-14-typography.md`, `docs/ui-redesign/UI-HOME-FONTS-final-report.md`
   (if present), `examples/iced_chat/fonts/THIRD_PARTY_NOTICES.md`, fonts.rs header doc (9–25).
7. Assets: delete `SourceSans3-*.ttf` (4 files) + `SourceSans3-OFL.txt` (only after all
   refs above are migrated and build/test pass).

### 8.3 Manrope — removal candidates (only after Archivo SemiCondensed lands)

Manrope is used ONLY by `DisplayHeading` (and gallery demos/tests):
1. `examples/iced_chat/fonts.rs`: byte consts 63 (variable, dead), 66, 69; family const
   `MANROPE` 107; constructor 142–149; `load_fonts()` 605–606; `TypeRole::family_name()`
   394 (`DisplayHeading => MANROPE`), `weight()` 413 (Bold), `size_px()` 434 (32),
   `font()` 449 (`MANROPE => manrope(weight)`); tests 663–664, 676–677, 695–697,
   717–720, 773 (gallery sample), plus `typography_gallery` demo in component_gallery.rs
   (1645, 1722–1728, 1768–1770).
2. `examples/iced_chat/component_gallery.rs` — Manrope demo rows.
3. Assets: `Manrope.ttf` (variable, already unloaded), `Manrope-SemiBold.ttf`,
   `Manrope-Bold.ttf`, `Manrope-OFL.txt`.
4. Docs: DESIGN_SYSTEM.md, UI-HOME-11/12/14, THIRD_PARTY_NOTICES.md, fonts.rs header.
5. Home-screen test (app.rs:38458) asserts the home view must NOT declare Manrope
   locally — stays valid (it already forbids direct Manrope use).

### 8.4 Other legacy assets (already unloaded, can be pruned anytime)

- `fonts/JetBrainsMono.ttf` (variable) — fonts.rs:83–85 (dead_code)
- `fonts/JetBrainsMono-Italic.ttf` — fonts.rs:88–89 (dead_code)
- Test refs: fonts.rs:634–635.

### 8.5 Note on `Typography` legacy enum

The whole `Typography` enum (fonts.rs:237–329) + `typo_text`/`with_typo`/
`typo_text_scaled` (556–582) can be deleted once component_gallery no longer showcases
legacy tokens. Only component_gallery.rs uses them today (27 refs) plus fonts.rs tests.
`Typography::BoruWordmark`/`TechnicalValue` are the only legacy variants with non-SS3
families (fonts.rs:279–282, 324–327).

---

## 9. Font licence files present in examples/iced_chat/fonts/

All assets are SIL OFL-1.1. In-repo records (git-tracked, 8 files + 22 binaries):

| File | Purpose |
|---|---|
| `OFL.txt` | Combined multi-family notice (legacy, 4985 bytes) |
| `Figtree-OFL.txt` | Figtree (4388 B) |
| `Inter-OFL.txt` | Inter (4380 B) |
| `JetBrainsMono-OFL.txt` | JetBrains Mono (4399 B) |
| `Manrope-OFL.txt` | Manrope (4384 B) |
| `Raleway-OFL.txt` | Raleway (4497 B) |
| `SourceSans3-OFL.txt` | Source Sans 3 (4579 B) |
| `THIRD_PARTY_NOTICES.md` | Source/version/copyright table for every family (4730 B) |

Font binaries (22 total): Figtree 3 (Regular/Medium/SemiBold), Inter 4
(Regular/Medium/SemiBold/Bold), JetBrains Mono 4 (Regular/Medium + variable + Italic),
Manrope 3 (SemiBold/Bold + variable), Raleway 1 (ExtraBold), Source Sans 3 4
(Regular/Medium/SemiBold/Bold). Full listing in §1.1 and the `ls` output at commit time.

THIRD_PARTY_NOTICES.md records per-family versions: Source Sans 3 3.052, Manrope 4.504,
Figtree 2.001, Raleway 4.026, JetBrains Mono 2.211 (static instances generated with
fontTools from official variable fonts — see UI-HOME-11 report §2 for why).

---

## Appendix A — Whole-repo search results (method)

Searched (git worktree, all files): `Source Sans`, `SourceSans`, `Manrope`, `Figtree`,
`Raleway`, `JetBrains Mono`, `Inter`, `font-family`, `font_family`, `FontFamily`,
`.font(`, `default_font`, `include_bytes!`, `font::load`, `fonts::`, `TypeRole::`,
`Typography::`, `typo_text`, `type_role_text`.

Findings outside `examples/iced_chat/`:
- `src/` — zero font references.
- Other examples (`catalogue_browser.rs`, `dht_harness.rs`, `doctor.rs`, `setup.rs`,
  `test_addr.rs`, `video_backend_probe.rs`) — zero font references.
- HTML/CSS: only static mockups (`docs/screenshots/dashboard-mockup.html`,
  `report.html`) use `font-family: sans-serif`/`system-ui`; they are not part of the app.
- Markdown docs referencing the families: listed in §8.2 item 6 + `CHANGELOG.md`,
  `docs/chat-ui-redesign-baseline.md`, `docs/ui-redesign/*` reports (informational only).

## Appendix B — Files that consume typography, quick reference

| File | Direct `.font()` | `type_role_text*` | `TypeRole::` refs | Legacy `Typography` |
|---|---|---|---|---|
| `app.rs` | 29 prod + 8 test | ~460 | 463 | 0 |
| `ui_components.rs` | 37 | 8 | 77 | 0 |
| `shared_by_me_table.rs` | 35 | 0 | 71 | 0 |
| `component_gallery.rs` | 10 | 1 | 33 | 27 |
| `form_components.rs` | 9 | 0 | 20 | 0 |
| `card_shell.rs` | 0 | 7 | 14 | 0 (test only) |
| `video_file_card.rs` | 0 | 16 | 16 | 0 |
| `download_progress_view.rs` | 0 | 16 | 16 | 0 |
| `connection_details.rs` | 2 | 11 | 11 | 0 |
| `boru_dialog.rs` | 4 | 0 | 8 | 0 |
| `quick_actions.rs` | 0 | 2 | 8 | 0 |
| `sharing_summary.rs` | 4 | 0 | 8 | 0 |
| `log_viewer.rs` | 0 | 6 | 6 | 0 |
| `icon_system.rs` | 0 | 2 | 2 | 0 |
| `file_type_icon.rs` | 0 | 1 | 1 | 0 |
| `main.rs` | default_font only | 0 | 0 | 0 |
| `design_tokens.rs` | 0 | 0 | 0 | 0 |

(`TypeRole::` counts include both `.font()` and `type_role_text(...)` uses; helper
files route everything through `type_role_text`.)

## Appendix C — Verification performed

- Whole-repo greps above (search_files / ripgrep) — complete inventory.
- No production files modified during this audit (git status clean apart from this note).
- Build gate to run (per card): `rb check --bin boru --features gui,video-playback,terminal`
  on debsrv (remote). Since no production code changed, the audit note is additive.
