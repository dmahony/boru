# Visual Designer Coding-Agent Guardrails Audit

**Scope:** PDF Task 28 (Coding Agent Guardrails), audited against the merged designer/layout work on 2026-08-16.

## Executive result

The designer chain remains an Iced-based, developer-only overlay over the production Boru UI. The audited implementation uses the existing typed `BoruTheme`/`LayoutConfig` seams, keeps pointer coordinates transient, writes only the established TOML override files, and routes edits through the normal Iced update loop. No designer operation starts, stops, or restarts the network/application services.

The requested feature-gated compile check passed on DEBSRV. The repository-wide `--all-targets` check is currently blocked by three pre-existing discovery-test call sites using the old four-argument `DiscoveryService::join` API; those failures are unrelated to the designer guardrails and are recorded below rather than changed in this audit.

## Per-guardrail findings

### 1. Boru was not rewritten away from Iced — PASS

- The GUI remains the Iced example target (`src/bin/boru/main.rs`), and designer messages are ordinary `AppMessage` variants handled by `IcedChat::update` (`src/bin/boru/app.rs:5586-5590`, `12657-12770`).
- Designer overlays return Iced `Element`s and use Iced widgets (`src/bin/boru/designer.rs:76-242`).
- The production Home renderer wraps the existing production cards with the overlay rather than replacing the renderer (`src/bin/boru/app/home.rs:1191-1210`, overlay calls beginning at `1360`).

### 2. No second layout persistence system — PASS

- Structural edits use `LayoutConfig`/`LayoutOverrides` and the pure `merge_layout_config` seam (`src/bin/boru/layout_merge.rs:846-887`).
- Loading and saving use the single `boru-layout.toml` path (`src/bin/boru/layout_config.rs:396-428`, `438-472`). The inspector's Save and Reload actions call those same functions (`src/bin/boru/app.rs:20646-20701`).
- The layout watcher parses the same file and sends a typed reload message into the app update loop (`src/bin/boru/layout_watcher.rs:38-40`, `62-150`).
- The separate `boru-ui.toml` path is used only for visual theme overrides (`src/bin/boru/theme_config.rs:1-25`); it is not a competing layout store.

### 3. Existing BoruTheme, LayoutConfig, TOML loading, validation, watcher, inspector, and gallery are reused — PASS

- Theme and layout are merged from typed defaults plus validated partial overrides (`src/bin/boru/theme_config.rs:1-25`, `layout_config.rs:287-428`, `layout_merge.rs:848-887`).
- Layout validation rejects duplicate/contradictory section lists before merge (`src/bin/boru/layout_config.rs:287-340`).
- Layout and theme watchers share the existing debounce/reload machinery (`src/bin/boru/layout_watcher.rs:9-16`, `38-45`).
- Inspector edits enter through `set_layout_overrides`, which validates, round-trips, merges, and bumps the live layout revision (`src/bin/boru/app.rs:19921-19979`).
- The gallery renders representative production components and explicitly documents that it does not use duplicate mocks (`src/bin/boru/component_gallery.rs:1-14`); its designer entry point consumes the live layout/designer state (`327-345`).

### 4. Raw desktop coordinates are not persisted for responsive content — PASS

- `DragOperation` and `ResizeOperation` contain pointer points only for the active gesture; the source explicitly states that pointer coordinates are not persisted (`src/bin/boru/designer.rs:369-387`).
- Drag commits produce semantic home-section ordering/index changes (`src/bin/boru/app.rs:19724-19813`), while resize commits translate the gesture into typed responsive layout values (`19814-19904`).
- Persisted values are serialized `LayoutOverrides` leaves, not `Point`/desktop bounds (`src/bin/boru/layout_config.rs:431-472`).

### 5. Designer operations do not restart network or application services — PASS

- `DesignerState` is explicitly transient and does not own chat, networking, room, media, transfer, or persistence state (`src/bin/boru/designer.rs:1-7`, `389-403`).
- `set_layout_config` changes only `active_layout`, the layout revision, and the layout prewarm invalidation flag (`src/bin/boru/app.rs:19906-19919`).
- `set_layout_overrides` changes only layout override/merge/inspector state (`19921-19979`).
- Designer update arms return `iced::Task::none()` after local layout handling (`12665-12769`); they do not call router, endpoint, tunnel, gossip, room, transfer, or persistence startup/shutdown APIs.
- Network/router construction occurs in startup before the Iced app is wired (`src/bin/boru/main.rs:841-858`, `1244-1258`), and the designer watcher only sends reload messages through a channel (`layout_watcher.rs:62-150`).

### 6. Production widgets are not duplicated merely to make them editable — PASS

- Production Home cards are built first by the existing Home renderer, then wrapped by `designer::overlay` (`src/bin/boru/app/home.rs:1191-1210`, overlay calls at `1360-1367` and subsequent section wrappers).
- The gallery uses exact production components, including chat messages, attachment cards, and video cards, rather than mock replacements (`src/bin/boru/component_gallery.rs:1-7`).
- The overlay is an explicitly developer-only interaction layer; its drag handle is separate from normal card click behavior (`src/bin/boru/designer.rs:105-128`).

### 7. Small reviewable stages with compile/test verification — PASS WITH AUDIT EVIDENCE

- The code contains separate, narrowly scoped modules for designer state, layout model/config/merge/watcher, inspector, theme config/watcher, and gallery (`src/bin/boru/` module declarations in `main.rs:12-60`).
- Existing unit/integration tests cover designer state, semantic snapping/history, layout parsing/validation/merge/save/reload, watcher behavior, and inspector seams (for example `designer.rs:600-744`, `layout_config.rs:505-1090`, `layout_merge.rs:889-1400`).
- `rb check --features dev-ui` passed on DEBSRV.
- `rb check --all-targets` did not pass because `tests/test_discovery_startup.rs:99` and `tests/test_discovery_two_node.rs:192,199` call `DiscoveryService::join` with four arguments while the current API requires a fifth `SecretKey`. This is unrelated to the designer chain and was not modified under this audit.

## Cross-cutting guardrails

- Designer functionality is feature-gated: `designer`, gallery, inspector, and designer state/message fields are declared under `#[cfg(feature = "dev-ui")]` (`src/bin/boru/main.rs:12-15`, `34-40`, `59-60`; `app.rs:64-65`, `3676-3682`, `5586-5590`).
- The runtime dev gate prevents release builds without `dev-ui` from loading either TOML file or spawning either watcher (`src/bin/boru/main.rs:223-270`, `579-630`, `1903-1931`). With the gate off, empty overrides merge to the existing defaults.
- Layout values are structural and theme values remain visual; the layout model intentionally does not read `BoruTheme` (`src/bin/boru/layout.rs:11-31`).
- Existing normal application behavior remains outside the designer reducer and overlay. The overlay returns the unmodified content when disabled (`src/bin/boru/designer.rs:84-86`).

## Verification record

- Remote filesystem check: DEBSRV has 77G available on `/`.
- `rb check --features dev-ui`: PASS (exit 0; warnings only).
- `rb check --all-targets`: BLOCKED by the unrelated `DiscoveryService::join` signature mismatch described above (exit 101).
- No guardrail violation requiring a code fix was found during this audit.
