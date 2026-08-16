# SVG rendering in Iced — BORU-TWEMOJI-03 findings

Status: done (commit for task t_6da70ba5).

## What was already there (no change needed)

- `Cargo.toml` (~line 196) already declares the iced 0.14 `svg` feature:
  `iced = { version = "0.14", default-features = false, features = ["tokio",
  "x11", "wayland", "tiny-skia", "image", "lazy", "wgpu", "svg", "canvas",
  "advanced"], optional = true }`.
- Feature chain is complete for both renderers:
  `iced/svg` → `iced_widget/svg` → `iced_renderer/svg` →
  `iced_tiny_skia/svg` (resvg) and `iced_wgpu/svg`.
- `resvg` 0.45.1 / `usvg` 0.45.1 are already resolved in `Cargo.lock`.
- SVG rendering is already exercised in the GUI build by the Lucide icon
  system: `app.rs::icon_svg()` (`iced::widget::svg(Handle::from_memory(..))`)
  and `icon_system.rs` / `status_card.rs` use `iced::widget::svg::Style`.

So "enable the SVG feature" was already done. This task only added proof.

## What changed

1. `tests/svg_render_proof.rs` (new) — automated, headless proof, gated
   `#![cfg(feature = "gui")]`:
   - vendored `assets/emoji/twemoji/svg/1f600.svg` exists and parses as SVG;
   - `iced::widget::svg::Handle` (from_path + from_memory) and the
     `svg::Svg::new(handle).width(..).height(..)` widget builder accept the
     asset at 16/32/64 px (the API BORU-TWEMOJI-10 will use);
   - rasterizes through resvg 0.45 (the same rasterizer iced uses) at
     16/32/64 px, asserting non-transparent pixel counts scale with size.
   - Run: `rb test --features gui --test svg_render_proof`
2. `examples/svg_render_proof.rs` (new) — minimal iced view showing the
   Twemoji SVG at 16/32/64/128 px for visual confirmation.
   Run: `rb run --example svg_render_proof --features gui`
   (or `cargo run --example svg_render_proof --features gui`).
   Unlike `icon_svg`, no `svg::Style` colour filter is applied — Twemoji is
   multi-colour and must keep its intrinsic colours.
3. `Cargo.toml` — added dev-dependency `resvg = { version = "0.45",
   default-features = false }` (same 0.45.1 line iced already activates via
   `iced_tiny_skia/svg`; used only by the test). No Iced upgrade, no new
   runtime dependency, no unrelated API migration.

## Notes for later tasks

- Multi-colour Twemoji SVGs must NOT be built with a `svg::Style` colour
  filter (that tints the whole glyph, as `icon_svg` does for monochrome
  Lucide icons).
- The renderer (BORU-TWEMOJI-06..08) should cache `svg::Handle` per codepoint
  — `Handle::from_memory` is cheap to build but decoding happens in the
  renderer each frame; the picker should reuse handles (BORU-TWEMOJI-09).
- `cargo check --all-targets` still fails on pre-existing test-target
  breakage (DiscoveryService::join 5-arg vs stale tests — see task
  t_aaed0e07 comment); the targeted commands above pass.
