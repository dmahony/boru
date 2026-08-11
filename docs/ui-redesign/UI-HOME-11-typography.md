# UI-HOME-11 — Central Boru Typography System + Font Registration

- Task: `t_318cd671` (UI-HOME-11)
- Plan source: `Boru_Home_Screen_Tidy_and_Fonts_Hermes_Kanban_Plan.pdf` (attached to `t_302a8ab8`, pages 17–18)
- Repo: `/home/dan/iroh-gossip-chat` @ `main`
- Status: COMPLETE. Central semantic typography roles registered; all five approved font
  families bundled and loaded; licence/source records in-repo; preview screenshot captured.

## 1. What was delivered

A central typography system in `examples/iced_chat/fonts.rs`:

- **`TypeRole`** — the canonical semantic-role enum with exactly the 15 plan roles
  (`display_heading`, `page_title`, `section_title`, `card_title`, `body`,
  `body_emphasised`, `button_label`, `supporting_text`, `metadata`, `chat_message`,
  `chat_sender`, `chat_metadata`, `composer_text`, `technical_value`, `brand_wordmark`).
  Each role exposes `family_name()`, `weight()`, `size_px()`, `font()`, plus explicit
  fallbacks (`fallback_family()`, `fallback_weight()`, `fallback_font()`).
- **`figtree()`** font constructor and `FIGTREE` family constant.
- **`load_fonts()`** now registers every required weight as a **static instance** — no
  synthetic bolding, no reliance on variable-font axis interpolation.
- The legacy `Typography` enum is untouched (160 existing call sites; screen migration is
  UI-HOME-12/13/14).

## 2. Font assets registered (all bundled, no remote font service at runtime)

| Family | Weights | Files (examples/iced_chat/fonts/) | Source (official, OFL-1.1) | Version |
|---|---|---|---|---|
| Source Sans 3 | 400, **500**, 600, 700 | `SourceSans3-Regular.ttf`, `SourceSans3-Medium.ttf` *(new)*, `SourceSans3-SemiBold.ttf`, `SourceSans3-Bold.ttf` | adobe-fonts/source-sans (release, TTF/) | 3.052 |
| Manrope | **600, 700** | `Manrope-SemiBold.ttf` *(new)*, `Manrope-Bold.ttf` *(new)* | static instances generated (fontTools varLib.instancer) from the official google/fonts variable `Manrope[wght].ttf` — the already-bundled `Manrope.ttf` (v4.504, byte-identical to the official variable) | 4.504 |
| Figtree | **400, 500, 600** | `Figtree-Regular.ttf`, `Figtree-Medium.ttf`, `Figtree-SemiBold.ttf` *(all new)* | erikdkennedy/figtree (fonts/ttf/) | 2.001 |
| Raleway | 800 | `Raleway-ExtraBold.ttf` (existing) | google/fonts ofl/raleway static ExtraBold | 4.026 |
| JetBrains Mono | **400, 500** | `JetBrainsMono-Regular.ttf`, `JetBrainsMono-Medium.ttf` *(both new)* | static instances generated (fontTools varLib.instancer) from the official JetBrains variable font — the already-bundled `JetBrainsMono.ttf` (v2.211) | 2.211 |
| Inter | 400/500/600/700 | existing files | google/fonts ofl/inter | legacy, not loaded |

Why generated statics for Manrope/JetBrains Mono instead of upstream static files:
- The upstream JetBrains Mono static `JetBrainsMono-Medium.ttf` uses family name
  "JetBrains Mono Medium" and OS/2 usWeightClass 436 — iced/fontdb cannot resolve that as
  family "JetBrains Mono" weight 500. The instancer-generated instance has family
  "JetBrains Mono" and usWeightClass 500 (verified with fontTools).
- Google Fonts only ships variable Manrope/Figtree; the official Manrope project repo no
  longer exposes the static TTF path used previously. Generating statics from the official
  variable (OFL-1.1 permits modification) keeps every required weight as a real,
  individually-registered face.

Family/weight metadata for every new file was verified with `fontTools` (name table +
`OS/2.usWeightClass`) and `fc-scan` before bundling.

## 3. Semantic role mapping (central tokens)

`fonts::TypeRole` mapping (plan UI-HOME-12/13 approved sizes):

| Role | Family | Weight | px | Fallback |
|---|---|---|---|---|
| display_heading | Manrope | Bold 700 | 32 | Source Sans 3 Bold |
| page_title | Source Sans 3 | SemiBold 600 | 28 | Source Sans 3 SemiBold |
| section_title | Source Sans 3 | SemiBold 600 | 20 | Source Sans 3 SemiBold |
| card_title | Source Sans 3 | SemiBold 600 | 18 | Source Sans 3 SemiBold |
| body | Source Sans 3 | Regular 400 | 15 | Source Sans 3 Regular |
| body_emphasised | Source Sans 3 | SemiBold 600 | 15 | Source Sans 3 SemiBold |
| button_label | Source Sans 3 | SemiBold 600 | 14 | Source Sans 3 SemiBold |
| supporting_text | Source Sans 3 | Regular 400 | 13 | Source Sans 3 Regular |
| metadata | Source Sans 3 | Regular 400 | 12 | Source Sans 3 Regular |
| chat_message | Figtree | Regular 400 | 15 | Source Sans 3 Regular |
| chat_sender | Figtree | SemiBold 600 | 14 | Source Sans 3 SemiBold |
| chat_metadata | Figtree | Regular 400 | 12 | Source Sans 3 Regular |
| composer_text | Figtree | Regular 400 | 15 | Source Sans 3 Regular |
| technical_value | JetBrains Mono | Regular 400 | 12 | platform monospace |
| brand_wordmark | Raleway | ExtraBold 800 | 28 | Source Sans 3 Bold |

Fallback policy (documented in `TypeRole`): every role degrades to Source Sans 3 at the
same (or nearest registered) weight; `technical_value` degrades to the platform monospace
family. iced's own default-font fallback (Source Sans 3, set in `main.rs`) covers any
unregistered family.

## 4. Files changed

- `examples/iced_chat/fonts.rs` — font consts, `FIGTREE`, `figtree()`, `TypeRole` (+15 roles),
  fallbacks, `load_fonts()` registering all required weights, 5 new unit tests (13 total).
- `examples/iced_chat/component_gallery.rs` — new "Typography (UI-HOME-11)" gallery section
  demonstrating every role + registered weights + fallback demo (Ctrl+Shift+G, debug).
- `DESIGN_SYSTEM.md` — §1 Typography: family table, canonical role table, fallback policy.
- `examples/iced_chat/fonts/` — 8 new font binaries, 4 new per-family OFL files,
  `THIRD_PARTY_NOTICES.md`.

## 5. Licence / source records (in-repo)

`examples/iced_chat/fonts/THIRD_PARTY_NOTICES.md` — per-family source, version, copyright
and license file mapping for Source Sans 3, Manrope, Figtree, Raleway, JetBrains Mono, Inter.
Per-family OFL-1.1 texts:
- `Figtree-OFL.txt`, `Manrope-OFL.txt`, `JetBrainsMono-OFL.txt`, `Raleway-OFL.txt` (new),
- `SourceSans3-OFL.txt`, `Inter-OFL.txt`, combined `OFL.txt` (existing).

No font files are exposed through task reports or artifacts.

## 6. Verification

- Build: `cargo build --bin boru --features gui` → OK (assets bundled via
  `include_bytes!`; binary at `target/debug/boru`). New fonts load at startup via
  `load_fonts()` chained in `main.rs:1602`; iced re-measures/re-renders when font loading
  completes, so layout invalidates correctly (fonts load before the first meaningful frame;
  non-fatal fallback on error).
- Tests: `cargo test --bin boru --features gui` → all pass (13 fonts tests including
  required-weight coverage, role-family/weight mapping, fallback policy, plan-role list).
- Evidence screenshot: `docs/ui-redesign/evidence/t_318cd671/t_318cd671_typography_preview.png`
  (1500×3000, Xvfb + Ctrl+Shift+G gallery, scrolled to the Typography section; OCR-verified:
  all 15 role captions, registered weights per family, fallback demo). Second capture at
  `t_318cd671_typography_preview_2.png`.

## 7. Remaining risks / notes for downstream cards (UI-HOME-12/13/14)

- Screens have NOT been migrated: `Typography` (legacy) remains the live token set used by
  the 160 call sites. `TypeRole` is ready for UI-HOME-12/13/14 to adopt.
- `SystemMessage` (legacy `Typography`) requests `Weight::Medium`; Source Sans 3 Medium is
  now real, so it renders at true 500 where before it fell back — a minor, intentional
  improvement, not a screen migration.
- The legacy bundled variable fonts (Manrope.ttf, JetBrainsMono.ttf, JetBrainsMono-Italic.ttf,
  Inter-*) remain in the repo unloaded, per the plan's "do not remove Inter yet" and to avoid
  churn; they can be pruned during UI-HOME-14 cleanup.
- Figtree is registered but unused until UI-HOME-13 applies it to chat; the preview proves
  it loads and renders.
- Two pre-existing warnings remain in touched files (`Typography::family_name` never used in
  non-test build; `self` import in component_gallery.rs) — both predate this task.
