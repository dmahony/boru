# FONTS-01 — Typography Audit (existing state)

- Task: `t_8dfc72dd` (FONTS-01: Audit existing typography)
- Epic spec: `Boru_FONTS` (attached to `t_15c72313`), "Task 1 — Audit Existing Typography"
- Repo: iroh-gossip-chat @ `main` (worktree `wt/t_8dfc72dd`)
- Date: 2026-08-07
- Purpose: baseline evidence for the FONTS epic (Archivo SemiCondensed + IBM Plex Sans migration).
  No production code was modified by this audit.

---

## 1. Architecture summary

The Boru iced app (examples/iced_chat) has a **central typography system** in
`examples/iced_chat/fonts.rs`:

- **Font data** is bundled at compile time via `include_bytes!` (fonts.rs:34-95).
- **Family name constants** (fonts.rs:99-116) name the registered families.
- **Font constructors** (fonts.rs:120-179) build `iced::Font` values per family/weight.
- **Two token enums** exist:
  - `Typography` (fonts.rs:237-273) — the **legacy** token set, `#[expect(dead_code)]`,
    almost fully migrated away. Only `component_gallery.rs` still consumes it in prod code.
  - `TypeRole` (fonts.rs:357-388) — the **canonical** semantic role set (15 roles,
    UI-HOME-11), used by ~1036 call sites across app.rs and the component modules.
- **`load_fonts()`** (fonts.rs:597-617) registers every required static weight at
  application startup. Called once from `main.rs:1607` (iced startup task chain).
- **Default font** is Source Sans 3, set on the iced application in `main.rs:1618-1623`.

No runtime/remote font loading exists anywhere (verified below in §7).

---

## 2. Every font registration site (file:line)

### 2.1 include_bytes! constants — examples/iced_chat/fonts.rs

| Constant | Line | File | Status |
|---|---|---|---|
| `SOURCE_SANS_REGULAR_BYTES` | 34 | fonts/SourceSans3-Regular.ttf | loaded (400) |
| `SOURCE_SANS_SEMI_BOLD_BYTES` | 37 | fonts/SourceSans3-SemiBold.ttf | loaded (600) |
| `SOURCE_SANS_MEDIUM_BYTES` | 40 | fonts/SourceSans3-Medium.ttf | loaded (500) |
| `SOURCE_SANS_BOLD_BYTES` | 43 | fonts/SourceSans3-Bold.ttf | loaded (700) |
| `INTER_REGULAR_BYTES` | 47 | fonts/Inter-Regular.ttf | dead_code — NOT loaded |
| `INTER_MEDIUM_BYTES` | 51 | fonts/Inter-Medium.ttf | dead_code — NOT loaded |
| `INTER_SEMI_BOLD_BYTES` | 55 | fonts/Inter-SemiBold.ttf | dead_code — NOT loaded |
| `INTER_BOLD_BYTES` | 59 | fonts/Inter-Bold.ttf | dead_code — NOT loaded |
| `MANROPE_BYTES` | 63 | fonts/Manrope.ttf (variable) | dead_code — NOT loaded |
| `MANROPE_SEMI_BOLD_BYTES` | 66 | fonts/Manrope-SemiBold.ttf | loaded (600) |
| `MANROPE_BOLD_BYTES` | 69 | fonts/Manrope-Bold.ttf | loaded (700) |
| `FIGTREE_REGULAR_BYTES` | 72 | fonts/Figtree-Regular.ttf | loaded (400) |
| `FIGTREE_MEDIUM_BYTES` | 75 | fonts/Figtree-Medium.ttf | loaded (500) |
| `FIGTREE_SEMI_BOLD_BYTES` | 78 | fonts/Figtree-SemiBold.ttf | loaded (600) |
| `RALEWAY_EXTRA_BOLD_BYTES` | 81 | fonts/Raleway-ExtraBold.ttf | loaded (800) |
| `JETBRAINS_MONO_BYTES` | 85 | fonts/JetBrainsMono.ttf (variable) | dead_code — NOT loaded |
| `JETBRAINS_MONO_ITALIC_BYTES` | 89 | fonts/JetBrainsMono-Italic.ttf | dead_code — NOT loaded |
| `JETBRAINS_MONO_REGULAR_BYTES` | 92 | fonts/JetBrainsMono-Regular.ttf | loaded (400) |
| `JETBRAINS_MONO_MEDIUM_BYTES` | 95 | fonts/JetBrainsMono-Medium.ttf | loaded (500) |

### 2.2 Family name constants — fonts.rs

| Constant | Line | Value |
|---|---|---|
| `SOURCE_SANS` | 100 | "Source Sans 3" |
| `INTER` | 104 | "Inter" (dead_code) |
| `MANROPE` | 107 | "Manrope" |
| `FIGTREE` | 110 | "Figtree" |
| `RALEWAY` | 113 | "Raleway" |
| `JETBRAINS_MONO` | 116 | "JetBrains Mono" |

### 2.3 Font constructors — fonts.rs

| Constructor | Line | Family |
|---|---|---|
| `source_sans(weight)` | 121 | SOURCE_SANS |
| `inter(weight)` | 132 | INTER (dead_code) |
| `manrope(weight)` | 142 | MANROPE |
| `figtree(weight)` | 152 | FIGTREE |
| `raleway_extra_bold()` | 162 | RALEWAY, weight ExtraBold (800) |
| `jetbrains_mono(weight)` | 172 | JETBRAINS_MONO |

### 2.4 load_fonts() — which weights are registered at startup (fonts.rs:597-617)

- Source Sans 3: 400 (Regular), 500 (Medium), 600 (SemiBold), 700 (Bold)
- Manrope: 600 (SemiBold), 700 (Bold)
- Figtree: 400 (Regular), 500 (Medium), 600 (SemiBold)
- Raleway: 800 (ExtraBold)
- JetBrains Mono: 400 (Regular), 500 (Medium)

NOT loaded at startup (compiled-in only, dead_code): Inter 400/500/600/700, Manrope
variable `Manrope.ttf`, JetBrains Mono variable `JetBrainsMono.ttf` + Italic.

### 2.5 load_fonts() call site

- `examples/iced_chat/main.rs:1607` — `let task = task.chain(fonts::load_fonts());`
  inside the `iced::application` initializer closure (startup).
- Default application font: `main.rs:1618-1623` —
  `.default_font(iced::Font { family: Family::Name(SOURCE_SANS), weight: Normal, ... })`.

---

## 3. Every typography token with current family/weight/size mapping

### 3.1 Canonical `TypeRole` (fonts.rs:357-388) — the system to migrate

| Role | family_name() | weight() | size_px() | line refs |
|---|---|---|---|---|
| DisplayHeading | MANROPE | Bold (700) | 32.0 | family 394, weight 413, size 434 |
| PageTitle | SOURCE_SANS | Semibold (600) | 28.0 | family 395, weight 414, size 435 |
| SectionTitle | SOURCE_SANS | Semibold (600) | 20.0 | family 396, weight 415, size 436 |
| CardTitle | SOURCE_SANS | Semibold (600) | 18.0 | family 397, weight 416, size 437 |
| Body | SOURCE_SANS | Normal (400) | 15.0 | family 398, weight 421, size 438 |
| BodyEmphasised | SOURCE_SANS | Semibold (600) | 15.0 | family 399, weight 417, size 438 |
| ButtonLabel | SOURCE_SANS | Semibold (600) | 14.0 | family 400, weight 418, size 439 |
| SupportingText | SOURCE_SANS | Normal (400) | 13.0 | family 401, weight 422, size 441 |
| Metadata | SOURCE_SANS | Normal (400) | 12.0 | family 402, weight 423, size 442 |
| ChatMessage | FIGTREE | Normal (400) | 15.0 | family 403, weight 424, size 438 |
| ChatSender | FIGTREE | Semibold (600) | 14.0 | family 403, weight 419, size 440 |
| ChatMetadata | FIGTREE | Normal (400) | 12.0 | family 403, weight 425, size 442 |
| ComposerText | FIGTREE | Normal (400) | 15.0 | family 403, weight 426, size 438 |
| TechnicalValue | JETBRAINS_MONO | Normal (400) | 12.0 | family 404, weight 427, size 442 |
| BrandWordmark | RALEWAY | ExtraBold (800) | 28.0 | family 405, weight 420, size 435 |

`TypeRole::font()` dispatch at fonts.rs:447-455; `fallback_family()` at 461-466
(TechnicalValue → platform Monospace; everything else → Source Sans 3);
`fallback_font()` at 480-487; `ALL` array at 511-527.

Helper widgets: `type_role_text()` (fonts.rs:530-536) and `type_role_text_lh()`
(fonts.rs:540-549, applies relative line-height).

### 3.2 Legacy `Typography` enum (fonts.rs:237-273) — `#[expect(dead_code)]`

All roles map to Source Sans 3 except:
- `TechnicalValue` → JetBrains Mono (family 280, size 12, font 325)
- `BoruWordmark` → Raleway ExtraBold (family 279, size 28, font 324)

Size mapping (`Typography::size_px()`, fonts.rs:302-319): PageTitle/BoruWordmark 28,
SectionHeading 18 (CONVERSATION_TITLE), SidebarIdentity 16, ChatMessage 15,
Body/ButtonLabel/NavigationLabel/FormLabel/SystemMessage 14, SecondaryText/
SidebarSectionLabel/Timestamp/DeliveryState/TechnicalValue 12.

### 3.3 Size constants — fonts.rs `mod sizes` (fonts.rs:192-226)

| Constant | Line | px |
|---|---|---|
| `PAGE_TITLE` | 196 | 28.0 |
| `HOME_GREETING` | 198 | 32.0 |
| `HOME_SUBTITLE` | 200 | 16.0 |
| `CONVERSATION_TITLE` | 202 | 18.0 |
| `SIDEBAR_IDENTITY` | 204 | 16.0 |
| `CHAT_MESSAGE` | 206 | 15.0 |
| `BODY` | 208 | 14.0 |
| `SECONDARY` | 210 | 12.0 |
| Legacy aliases `XL/LG/MD/SM/XS/XXS` | 214-224 | 28/18/15/14/12/12 |

app.rs re-exports the legacy aliases as `TYPO_*` at app.rs:326-328
(`LG as TYPO_LG, MD as TYPO_MD, SM as TYPO_SM, XL as TYPO_XL, XS as TYPO_XS, XXS as TYPO_XXS`).

### 3.4 design_tokens.rs — NO typography tokens

`examples/iced_chat/design_tokens.rs` contains **colors, spacing, radii, control
sizes only**. There is no font/typography section (verified: grep for font/Font/
Typography/TypeRole in design_tokens.rs returns 0 hits in code; the file header's
token table lists only Backgrounds/Text-colors/Accents/Borders — line 13
`text_<role>` refers to colour functions `text_primary()` etc., not fonts).
Size-scale tokens therefore live only in fonts.rs (`sizes` module + `TypeRole::size_px()`
hard-coded values). DESIGN_SYSTEM.md §"Typography" (lines ~40-95) documents the
same sizes but is doc-only.

---

## 4. Every direct font-family declaration / override site (file:line)

### 4.1 Direct `Family::Name(...)` / `iced::Font { ... }` constructions

| Site | Line | What |
|---|---|---|
| main.rs `.default_font(iced::Font { family: Family::Name(SOURCE_SANS), ... })` | 1618-1623 | App default font = Source Sans 3 Normal |
| component_gallery.rs `iced::Font { family: Family::Name(family), ... }` | 1693-1698 | Gallery weight-sample demo (parametrised by family string) |
| fonts.rs constructor bodies | 122-178 | All constructors (definition sites) |

### 4.2 Direct family-name string literals in component code

- component_gallery.rs:1713, 1717-1720 ("Source Sans 3" weight demo rows),
  1722, 1726-1727 ("Manrope"), 1729, 1733-1735 ("Figtree"),
  1737, 1741 ("Raleway"), 1743, 1747-1748 ("JetBrains Mono"),
  1645-1659 (TypeRole gallery label strings), 1752-1753 (fallback note).
- fonts.rs doc comments and family constants (as listed in §2.2).

### 4.3 Direct font-constructor calls in components (bypassing tokens)

| Site | Line | Call |
|---|---|---|
| app.rs (BoruLogo widget) | 407 | `crate::fonts::raleway_extra_bold()` — **BORU wordmark (must NOT change)** |
| app.rs (sidebar brand row) | 23358 | `crate::fonts::raleway_extra_bold()` — **BORU wordmark (must NOT change)** |
| ui_components.rs (system event chip label) | 1656 | `crate::fonts::source_sans(iced::font::Weight::Semibold)` — **the only non-wordmark direct font call** |

`main.rs:1619` (`Family::Name(SOURCE_SANS)`) is the app default — a direct declaration.

No other direct `manrope()`, `figtree()`, or `jetbrains_mono()` call sites exist
outside fonts.rs (grep across examples/iced_chat/*.rs confirmed).

---

## 5. Components using semantic tokens vs direct font calls

### 5.1 Files consuming TypeRole (canonical tokens) — match counts

| File | TypeRole / type_role_text* matches |
|---|---|
| app.rs | 681 |
| ui_components.rs | 85 |
| shared_by_me_table.rs | 71 |
| component_gallery.rs | 35 |
| video_file_card.rs | 26 |
| download_progress_view.rs | 23 |
| form_components.rs | 20 |
| card_shell.rs | 18 |
| connection_details.rs | 17 |
| quick_actions.rs | 10 |
| log_viewer.rs | 9 |
| sharing_summary.rs | 8 |
| boru_dialog.rs | 8 |
| icon_system.rs | 2 |
| file_type_icon.rs | 1 |

Representative call sites:
- Home greeting → `TypeRole::DisplayHeading` via `type_role_text_lh(..., 1.2)` at app.rs:25872-25881.
- Home subtitle → `TypeRole::Body` size-overridden with `HOME_SUBTITLE` at app.rs:25884-25889.
- Sidebar section headers ("CHATS"/"GROUPS"/"FRIENDS"/"DISCOVER"/"PUBLIC ROOMS")
  → `SidebarSectionHeader` (ui_components.rs:1349) uses `TypeRole::ButtonLabel.font()`
  at ui_components.rs:1415-1417; instantiations at app.rs:23450-23499.
- Chat timeline / composer → `TypeRole::ChatMessage/ChatSender/ChatMetadata/ComposerText`
  at app.rs:29180-29840 (chat) and 29840 (composer).
- Quick actions → `TypeRole::CardTitle` + `SupportingText` via `type_role_text_lh` at
  quick_actions.rs:115-128.
- File Sharing page title → `TypeRole::PageTitle` at app.rs:35313; search input →
  `TypeRole::Body` at app.rs:35330-35331.
- Connection details dialog → PageTitle/ButtonLabel/TechnicalValue at connection_details.rs:441-544.
- CardShell cards → CardTitle/SupportingText/ButtonLabel/Metadata at card_shell.rs:289-519.
- Dialogs (Create Group / Create Public Room / Create Tunnel) → BoruDialog at
  boru_dialog.rs:33, 185-195, 298-313 (SectionTitle/SupportingText/ButtonLabel).
- Forms → form_components.rs:68, 81-107, 153-154, 782-900, 1174-1175.
- Shared-by-me table → shared_by_me_table.rs:35, 405-925 (SupportingText/ButtonLabel/CardTitle/Metadata/Body).
- Sharing summary → sharing_summary.rs:127, 177-221 (CardTitle/Metadata/PageTitle).
- Download progress / video cards → download_progress_view.rs + video_file_card.rs
  (ButtonLabel/Metadata/BodyEmphasised/TechnicalValue).
- Log viewer → log_viewer.rs:42-72 (SectionTitle/Metadata/ButtonLabel/Body/TechnicalValue).
- Icon labels → icon_system.rs:386, 398; file_type_icon.rs:466 (Metadata).

### 5.2 Files still consuming legacy `Typography` tokens

- component_gallery.rs — gallery headings/sections/panels use
  `Typography::PageTitle/SectionHeading/Body/SecondaryText` (component_gallery.rs:113-114,
  122-123, 136, 260, 267, 333-341, 385-407, 870-872, 1024-1084).
- card_shell.rs:872 — test asserting `!prod.contains("Typography::")` (production must not use it).
- app.rs:38872 — test `shared_chrome_no_raw_typo_text` asserting shared chrome does not
  use raw `TYPO_` sizes.
- fonts.rs tests.

### 5.3 Direct font call sites (bypassing tokens) in production code

- app.rs:407, app.rs:23358 — `raleway_extra_bold()` = BORU wordmark (brand, keep).
- ui_components.rs:1656 — `source_sans(Semibold)` for the system-event chip label
  (candidate to migrate to a token, e.g. TypeRole).
- main.rs:1618-1623 — default font declaration (Source Sans 3; will become IBM Plex Sans).

### 5.4 Files with NO font usage

activity_log_view_model.rs, dashboard_view_model.rs (pure view models), file_category.rs /
file_type_resolver.rs (mention "Font" only as a file-category enum, unrelated to typography),
terminal_view.rs (uses `iced_term::settings::FontSettings::default()` — terminal font,
not UI typography).

---

## 6. Font asset inventory — examples/iced_chat/fonts/

| File | Role | Licence file |
|---|---|---|
| SourceSans3-Regular.ttf | loaded (400) | SourceSans3-OFL.txt |
| SourceSans3-Medium.ttf | loaded (500) | SourceSans3-OFL.txt |
| SourceSans3-SemiBold.ttf | loaded (600) | SourceSans3-OFL.txt |
| SourceSans3-Bold.ttf | loaded (700) | SourceSans3-OFL.txt |
| Manrope.ttf | legacy variable, NOT loaded (dead_code) | Manrope-OFL.txt |
| Manrope-SemiBold.ttf | loaded (600) | Manrope-OFL.txt |
| Manrope-Bold.ttf | loaded (700) | Manrope-OFL.txt |
| Figtree-Regular.ttf | loaded (400) | Figtree-OFL.txt |
| Figtree-Medium.ttf | loaded (500) | Figtree-OFL.txt |
| Figtree-SemiBold.ttf | loaded (600) | Figtree-OFL.txt |
| Raleway-ExtraBold.ttf | loaded (800) | Raleway-OFL.txt |
| JetBrainsMono.ttf | legacy variable, NOT loaded (dead_code) | JetBrainsMono-OFL.txt |
| JetBrainsMono-Italic.ttf | legacy variable, NOT loaded (dead_code) | JetBrainsMono-OFL.txt |
| JetBrainsMono-Regular.ttf | loaded (400) | JetBrainsMono-OFL.txt |
| JetBrainsMono-Medium.ttf | loaded (500) | JetBrainsMono-OFL.txt |
| Inter-Regular.ttf | legacy, NOT loaded (dead_code) | Inter-OFL.txt |
| Inter-Medium.ttf | legacy, NOT loaded (dead_code) | Inter-OFL.txt |
| Inter-SemiBold.ttf | legacy, NOT loaded (dead_code) | Inter-OFL.txt |
| Inter-Bold.ttf | legacy, NOT loaded (dead_code) | Inter-OFL.txt |
| OFL.txt | combined legacy notice | — |
| THIRD_PARTY_NOTICES.md | source/version/licence records for all families | — |

Licence: every family is SIL OFL-1.1; per-family `*-OFL.txt` files present and
`THIRD_PARTY_NOTICES.md` records upstream sources + versions (Source Sans 3 v3.052,
Manrope v4.504, Figtree v2.001, Raleway v4.026, JetBrains Mono v2.211, Inter legacy).

---

## 7. Runtime / remote font loading — NONE

Verified with repository-wide grep: no `googleapis`, `fonts.google`, `http.*font`,
`https.*font`, or remote `font::load` calls anywhere in the repo (including src/,
examples/, docs/). Fonts are bundled at compile time (`include_bytes!`), loaded at
startup via `fonts::load_fonts()` (main.rs:1607), and registered as static instances.
The only `font-family` strings found are in doc-only HTML mockups
(docs/screenshots/dashboard-mockup.html, report.html) — not shipped code.

---

## 8. Candidate removal list (post-migration) vs must-stay

### 8.1 Can be removed after migration to Archivo SemiCondensed + IBM Plex Sans

**Source Sans 3** (replaced by IBM Plex Sans as primary UI font):
- Assets: SourceSans3-Regular/Medium/SemiBold/Bold.ttf + SourceSans3-OFL.txt
- Code: `SOURCE_SANS` const (fonts.rs:100), `source_sans()` (121), all four
  `SOURCE_SANS_*_BYTES` include_bytes (34-43), load_fonts() entries (600-603),
  `TypeRole::PageTitle/SectionTitle/CardTitle/Body/BodyEmphasised/ButtonLabel/
  SupportingText/Metadata` family mappings (395-402), `Typography` fallback branch
  (281, 326), `fallback_family()` default (464), main.rs default_font (1619),
  ui_components.rs:1656 direct call, tests (656-740, 804-873).
- Doc: DESIGN_SYSTEM.md, fonts.rs header comment, THIRD_PARTY_NOTICES.md section.

**Manrope** (replaced by Archivo SemiCondensed for display headings):
- Assets: Manrope.ttf (variable), Manrope-SemiBold.ttf, Manrope-Bold.ttf + Manrope-OFL.txt
- Code: `MANROPE` const (107), `manrope()` (142), `MANROPE_*_BYTES` (63-69),
  load_fonts() entries (605-606), `TypeRole::DisplayHeading` family mapping (394),
  `TypeRole::font()` dispatch (449).

**Inter** (already legacy; never loaded, never used):
- Assets: Inter-Regular/Medium/SemiBold/Bold.ttf + Inter-OFL.txt
- Code: `INTER` const (104, dead_code), `inter()` (132, dead_code),
  `INTER_*_BYTES` (47-59, dead_code), tests (637-640).

### 8.2 Must stay

- **Figtree** — chat messages / sender / metadata / composer
  (TypeRole ChatMessage/ChatSender/ChatMetadata/ComposerText, fonts.rs:403, 424-426).
- **JetBrains Mono** — technical values (TypeRole::TechnicalValue, fonts.rs:404, 427).
- **Raleway** — BORU wordmark / branding ONLY, must NOT change
  (TypeRole::BrandWordmark, Typography::BoruWordmark, BoruLogo app.rs:407,
  sidebar brand app.rs:23358). Spec constraint: "Do not change the BORU logo".

---

## 9. Migration-relevant notes for downstream FONTS cards

1. **Token remap is the key lever (FONTS-04).** Changing `TypeRole::family_name()`
   mappings alone will migrate ~1036 call sites without touching component code.
   Only three direct font call sites need individual attention: main.rs default_font,
   ui_components.rs:1656, and the two wordmark sites (which must stay Raleway).
2. **TypeRole::size_px() is hard-coded** (fonts.rs:432-444); the `sizes` module
   (fonts.rs:192-226) exists but TypeRole bypasses it for most values
   (DisplayHeading 32.0, SectionTitle 20.0, SupportingText 13.0, etc. are literals).
   design_tokens.rs has no type-scale section to reconcile.
3. **Fallback policy** currently: everything → Source Sans 3; TechnicalValue →
   platform monospace (fonts.rs:461-466). After migration this should become
   IBM Plex Sans / system sans (FONTS-14), with Archivo → Arial Narrow chain.
4. **Static-instance policy**: every registered weight is a static TTF registered by
   exact family+weight (no synthetic bolding). New fonts (Archivo SemiCondensed 700,
   IBM Plex Sans 400/500/600(/700)) should follow the same pattern (FONTS-02/03).
5. **The legacy `Typography` enum is effectively dead** outside component_gallery.rs
   and can be removed in FONTS-12 once gallery migrates to TypeRole.
6. **Tests lock current mappings**: fonts.rs tests (656-740), app.rs enforcement tests
   (38320-38461, 38649-38702), card_shell.rs:856-869, quick_actions.rs:387-395,
   video_file_card.rs:2620. These will need updating in the same commits as the token
   remap, or the suite fails.
7. **Home screen** currently: greeting = TypeRole::DisplayHeading (Manrope Bold 32,
   app.rs:25872), subtitle = TypeRole::Body overridden to 16 px HOME_SUBTITLE
   (app.rs:25884-25889). FONTS-05 targets these.

---

## 10. Baseline evidence (grep outputs, screenshot-free)

Repository state at audit time: branch `wt/t_8dfc72dd`, base `590bd110`
(PAPIRUS-21). No production code modified by this card.

Key grep counts (see §5 for per-file detail):

```
$ grep -rniE "font" --include="*.rs" -l examples/iced_chat/ | wc -l
20   # files referencing font/Font in any form

$ grep -rn "TypeRole::\|type_role_text\|type_role_text_lh" --include="*.rs" examples/iced_chat/ | wc -l
1036  # canonical-token call sites

$ grep -rn "Typography::\|typo_text\|with_typo" --include="*.rs" examples/iced_chat/ | wc -l
60    # legacy-token call sites (27 prod in component_gallery.rs, rest tests)

$ grep -rn "source_sans(\|manrope(\|figtree(\|raleway_extra_bold(\|jetbrains_mono(\|inter(" --include="*.rs" examples/iced_chat/ | wc -l
20    # direct constructor call sites (15 in fonts.rs, 4 in app.rs, 1 in ui_components.rs)

$ grep -rn "googleapis\|fonts\.google\|font\.load(.*http" -r .
0     # no remote font loading
```

Verified build (debsrv): `rb check --example boru --features gui,video-playback,terminal`
— see task completion metadata for result.
