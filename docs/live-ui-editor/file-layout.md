# Live UI editor — file layout (BORU-UI-22 / PDF Task 22)

This document describes the **final** organization of the live UI editor
subsystem in the Boru repository, maps each file to a responsibility, and
explains how it relates to the layout suggested by PDF Task 22 of
`Boru_Live_UI_Editor_Agent_Tasks.pdf` — including where and why it deviates.

The PDF's suggested layout is:

```text
src/ui/theme/{mod.rs,tokens.rs,config.rs,loader.rs,watcher.rs}
src/ui/dev/{mod.rs,inspector.rs,gallery.rs,component_ids.rs,messages.rs,state.rs}
boru-ui.toml
boru-ui.example.toml
```

The PDF itself says *"Adapt this structure to the existing Boru repository
instead of forcing it"*. Boru's GUI does **not** live in `src/ui/` — it lives
in the `boru` binary crate under `src/bin/boru/` (see
`docs/gui-architecture.md`). The theme subsystem was therefore built as flat
modules inside that crate, following the existing convention
(`design_tokens.rs`, `card_shell.rs`, `component_gallery.rs`, … were already
flat). That adaptation is the deliberate, final shape; no re-organization is
warranted.

## Final layout (as of BORU-UI-22)

```text
src/bin/boru/
  theme.rs                 # BoruTheme typed model + all token groups (light/dark)
  theme_config.rs          # boru-ui.toml config structs, parse/load/save, errors
  theme_merge.rs           # defaults + overrides -> ActiveTheme merge
  theme_watcher.rs         # boru-ui.toml file watcher + debounce + reload tracker
  theme_regression.rs      # regression-test matrix (PDF Task 20), #[cfg(test)]
  inspector.rs             # dev UI Inspector (Ctrl+Shift+D), #[cfg(feature="dev-ui")]
  component_gallery.rs     # component gallery / playground (Ctrl+Shift+G), #[cfg(feature="dev-ui")]
  main.rs                  # module declarations + dev-ui gate decision point
  app.rs                   # IcedChat state, AppMessage::UiThemeReloaded / Inspector handlers
boru-ui.example.toml       # documented example of the dev theme override file (repo root)
docs/live-ui-editor/
  constants-audit.md       # BORU-UI-01 map of existing visual constants
  dev-mode-gate.md         # BORU-UI-08 dev-ui gate design
  manual-acceptance.md     # BORU-UI-21 manual acceptance evidence
  file-layout.md           # this document
```

## File → responsibility

### Theme model — `src/bin/boru/theme.rs`

The typed theme model (BORU-UI-02, PDF Task 2). Contains every token group in
one file, mirroring `BoruTheme`'s nested structure:

- `ColorTokens` (+ `light()` / `dark()` palettes and the canonical semantic
  accessors from BORU-UI-17: `background()`, `border()`, `accent()`,
  `accent_hover()`, `*_soft()`),
- `TypographyTokens` (`family_for` / `weight_for` / `size_for` /
  `line_height_for` by `fonts::TypeRole`),
- `SpacingTokens`, `RadiusTokens`, `IconTokens`, `AvatarTokens`, `ListTokens`,
  `BorderTokens`, `ResponsiveTokens`, `MotionTokens`,
- per-area themes: `SidebarTheme` (+`SidebarPadding`), `HomeTheme`,
  `ChatTheme`, `AttachmentTheme` (+`FileTableColumns`, `SharedTableColumns`,
  `VideoTokens`), `RoomTheme`, `TunnelTheme`, `DialogTheme`, `CallTheme`,
  `ControlTokens`,
- the root `BoruTheme` struct (BORU-UI-02) and in-module tests.

Why one file instead of `theme/mod.rs` + `theme/tokens.rs`? The model is a
single coherent struct tree; the token groups are its fields, not an
independent abstraction. Splitting them would add a module boundary with no
discoverability gain, and the existing crate convention is flat modules.

### Config — `src/bin/boru/theme_config.rs`

The `boru-ui.toml` override file (BORU-UI-04, PDF Task 4). Holds:

- the mirror `UiThemeConfig`/`*Config` serde structs (every `BoruTheme` group
  has a matching `*Config` with the same field names; all fields optional),
- the load path: `parse_ui_theme_config`, `load_ui_theme_config`,
  `reload_ui_theme_config`, `ui_theme_config_to_toml`, `save_ui_theme_config`,
- structured developer errors (`UiThemeConfigError`,
  `ThemeReloadErrorKind`, `ThemeReloadError` — BORU-UI-18).

This maps to the PDF's `theme/config.rs` (plus the file-reading half of the
PDF's `theme/loader.rs`).

### Merge / loader — `src/bin/boru/theme_merge.rs`

The pure merge step (BORU-UI-05, PDF Task 5):

```text
BoruTheme::default()  +  UiThemeConfig overrides  ->  ActiveTheme
```

`merge_ui_theme(base, cfg) -> (BoruTheme, Vec<String>)` applies only the
fields explicitly present in the TOML, keeping defaults as the source of
truth and returning warnings for out-of-range developer-error candidates.
This is the "apply overrides" half of the PDF's `theme/loader.rs`; the repo
splits the load path across `theme_config.rs` (read + parse) and
`theme_merge.rs` (merge) because the two have different failure modes and
test surfaces.

### Watcher — `src/bin/boru/theme_watcher.rs`

The `boru-ui.toml` file watcher (BORU-UI-06, PDF Task 6). Watches the parent
data directory (not the file itself, so atomic editor replacements are
caught), debounces save storms, parses off the render path, and sends
`UiThemeReloadMsg` into the app. Also owns the `Debouncer` and
`ReloadTracker` (generation-based dedupe). Maps directly to the PDF's
`theme/watcher.rs`.

### Dev UI Inspector — `src/bin/boru/inspector.rs`

The hidden developer inspector (BORU-UI-09..12, 16, 18, PDF Tasks 9-12).
Gated by `#[cfg(feature = "dev-ui")]` in `main.rs`.

It consolidates what the PDF splits into `dev/{mod.rs,inspector.rs,
component_ids.rs,messages.rs,state.rs}`:

- **component ids** — `ComponentId` enum (PDF `dev/component_ids.rs`): the
  per-area identity used by inspect-hover/select and the gallery;
- **messages** — `InspectorMsg` enum (PDF `dev/messages.rs`): all inspector
  interactions (`SetFloat`, `SetChoice`, `SetBool`, `ColorTextChanged`,
  `SaveTheme`, `ReloadFromDisk`, `ResetSection`, `InspectSelect`, …);
- **state** — `InspectorDraft`, `InspectorSection`, plus the display-side
  read helpers that translate an active `BoruTheme` into section rows (PDF
  `dev/state.rs`); the long-lived inspector flags themselves live on
  `IcedChat` in `app.rs` (`inspector_visible`, `inspect_ui_enabled`,
  `inspect_hover`, `inspect_selected`);
- **view** — `view_inspector` and its helpers (PDF `dev/inspector.rs`).

Why consolidate? `ComponentId`, `InspectorMsg`, and the section/state types
are one tight cycle (a message carries a `ComponentId`, the state reads the
theme per component, the view maps both). Keeping them in a single
dev-only module makes the dev feature trivially gateable and avoids a
5-module sub-tree for one panel; the section markers inside the file keep it
navigable. The dev module *mod.rs* equivalent is the `#[cfg(feature =
"dev-ui")] mod inspector;` declaration in `main.rs` plus the runtime gate
described in `docs/live-ui-editor/dev-mode-gate.md`.

### Gallery — `src/bin/boru/component_gallery.rs`

The component gallery / UI playground (BORU-UI-14, 15; PDF Tasks 14-15),
gated by `#[cfg(feature = "dev-ui")]`. Matches the PDF's `dev/gallery.rs` —
the file keeps the name it already had before this chain began (it existed
as a standalone gallery module), so no rename was done.

### Regression tests — `src/bin/boru/theme_regression.rs`

`#[cfg(test)]`-only module added by BORU-UI-20 mapping the PDF Task 20 test
matrix 1:1 (parse complete/partial config, malformed TOML, out-of-range
values, dark-mode independence, watcher/regression matrix, …). There is no
PDF equivalent file; it is test infrastructure.

### Wire-up — `src/bin/boru/main.rs` and `app.rs`

- `main.rs` declares the modules and is the **single dev-ui gate decision
  point** (`dev_ui_gate_on`, `dev_ui_enabled` — BORU-UI-08; see
  `docs/live-ui-editor/dev-mode-gate.md`). It loads the config at startup
  only when the gate is on, spawns the watcher, and passes the initial
  `UiThemeConfig` into `IcedChat::new`.
- `app.rs` holds the app state (`ui_theme_config`, `boru_theme()`,
  inspector flags, `gallery_state`) and the `AppMessage` handlers:
  `UiThemeReloaded` (watcher → update loop, BORU-UI-07), the
  `Inspector(InspectorMsg)` dispatch (`update_inspector`), and the
  gallery messages. `set_ui_theme_config` is the single seam through which
  every theme edit (startup, watcher reload, inspector edit, dark-mode
  toggle) flows, so reloads replace **only** theme state.

## Mapping to the PDF suggestion

| PDF Task 22 suggested | Boru file | Notes |
|---|---|---|
| `src/ui/theme/mod.rs` | `src/bin/boru/theme.rs` | model lives with the tokens; no `src/ui/` tree in this repo |
| `src/ui/theme/tokens.rs` | `src/bin/boru/theme.rs` | token groups are fields of `BoruTheme`; kept together |
| `src/ui/theme/config.rs` | `src/bin/boru/theme_config.rs` | serde structs + parse/load/save |
| `src/ui/theme/loader.rs` | `theme_config.rs` + `theme_merge.rs` | read/parse in config, merge in merge |
| `src/ui/theme/watcher.rs` | `src/bin/boru/theme_watcher.rs` | direct match |
| `src/ui/dev/mod.rs` | `main.rs` `#[cfg(feature="dev-ui")] mod …` | dev feature gate is the module boundary |
| `src/ui/dev/inspector.rs` | `src/bin/boru/inspector.rs` | direct match (dev-only) |
| `src/ui/dev/gallery.rs` | `src/bin/boru/component_gallery.rs` | existing pre-chain name kept |
| `src/ui/dev/component_ids.rs` | `inspector.rs` (`ComponentId`) | consolidated — see above |
| `src/ui/dev/messages.rs` | `inspector.rs` (`InspectorMsg`) | consolidated — see above |
| `src/ui/dev/state.rs` | `inspector.rs` (+ `IcedChat` fields in `app.rs`) | consolidated — see above |
| `boru-ui.toml` | runtime file in the data dir (never committed) | created by `save_ui_theme_config`; documented by the example |
| `boru-ui.example.toml` | `boru-ui.example.toml` (repo root) | direct match — BORU-UI-04 |

## boru-ui.example.toml

`boru-ui.example.toml` at the repo root is the authoritative sample of the
dev theme override format. It is **complete** as of BORU-UI-22:

- header documents the copy location for each OS, the "only visual values"
  rule, the "every key optional" rule, and the malformed-file behaviour;
- a **Units / ranges** section documents units and accepted ranges:
  lengths/sizes/padding/spacing/radii/widths/heights are positive pixel
  floats (`0.0` allowed), `chat.bubble_width_ratio` is a fraction `0..=1`,
  `home.quick_action_desc_line_height` is a unitless line-height ratio,
  `motion.sidebar_fade_frames` is an integer ≥ 1, colours are `"#RRGGBB"` /
  `"#RRGGBBAA"` hex or `0..=1` float arrays;
- every config group (`colors`, `typography`, `spacing`, `radii`, `icons`,
  `avatars`, `lists`, `borders`, `responsive`, `motion`, `sidebar` +
  `sidebar.padding`, `home`, `chat`, `attachments` + `file_table` +
  `shared_table` + `video`, `rooms`, `tunnels`, `dialogs`, `calls`,
  `controls`) is present, commented out, with the current baseline value —
  mirroring `UiThemeConfig` 1:1.

The file is meant to be copied to `<data_dir>/boru-ui.toml` (the path
printed at startup / `--data-dir`); `boru-ui.toml` itself is a runtime
artifact and is never committed.

## Design invariants this layout preserves

- **Visual config is separate from behaviour.** `theme_config.rs` /
  `theme_merge.rs` / `theme.rs` contain only presentation values; no
  protocol/network/file-transfer/video/tunnel/lobby/room/persistence
  behaviour lives in the live-editor modules (see `constants-audit.md`).
- **Ordinary users are unaffected.** `dev-ui` is not in the default feature
  set; without it (or `--dev-ui`/`BORU_DEV_UI=1` in a debug build) the
  config file is never read and the watcher never spawns
  (`docs/live-ui-editor/dev-mode-gate.md`).
- **The editor can be compiled out.** `inspector.rs` and
  `component_gallery.rs` are `#[cfg(feature = "dev-ui")]`-gated; the theme
  model/config/merge/watcher are always compiled but inert when the gate is
  off (the runtime-gate rationale is documented in `dev-mode-gate.md`).
- **One edit seam.** All theme changes — startup config, watcher reload,
  inspector sliders, dark-mode toggle — flow through
  `IcedChat::set_ui_theme_config`, so a reload can never clobber chat,
  transfer, or video state (verified by BORU-UI-20/21 tests).
