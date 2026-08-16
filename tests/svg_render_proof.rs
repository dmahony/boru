//! BORU-TWEMOJI-03: render proof for bundled Twemoji SVG assets.
//!
//! Verifies that the existing iced 0.14 `svg` feature (already declared in
//! Cargo.toml and already used by the Lucide icon system) can load and render
//! one vendored Twemoji SVG at multiple widget sizes:
//!
//! 1. The vendored asset exists and parses as an SVG document.
//! 2. The iced `svg::Handle` / `svg::Svg` widget API accepts the asset bytes
//!    and builds widgets at 16/32/64 px — the exact API the emoji picker will
//!    use in BORU-TWEMOJI-10.
//! 3. The SVG rasterizes through resvg 0.45 — the same rasterizer iced's
//!    `svg` feature uses via `iced_tiny_skia/svg` — at 16/32/64 px, with
//!    non-transparent pixel counts that scale with the canvas size.
//!
//! Run with: cargo test --features gui --test svg_render_proof
//! (or `rb test --features gui --test svg_render_proof` on DEBSRV)

#![cfg(feature = "gui")]

use std::path::PathBuf;

/// Path of one vendored Twemoji asset (grinning face, U+1F600).
fn twemoji_asset(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("assets/emoji/twemoji/svg")
        .join(name)
}

/// Widget sizes the emoji picker will need (small inline, medium, large).
const SIZES: [u32; 3] = [16, 32, 64];

#[test]
fn vendored_twemoji_svg_exists_and_is_valid_svg() {
    let path = twemoji_asset("1f600.svg");
    let bytes = std::fs::read(&path)
        .unwrap_or_else(|e| panic!("vendored Twemoji SVG must exist at {}: {e}", path.display()));
    assert!(!bytes.is_empty(), "vendored Twemoji SVG must not be empty");

    // usvg is resvg's parser — the same crate iced_tiny_skia feeds SVG bytes to.
    resvg::usvg::Tree::from_data(&bytes, &resvg::usvg::Options::default())
        .expect("vendored Twemoji SVG must parse as an SVG document");
}

#[test]
fn iced_svg_widget_api_accepts_vendored_twemoji_at_multiple_sizes() {
    let path = twemoji_asset("1f600.svg");
    let bytes = std::fs::read(&path).expect("vendored Twemoji SVG must exist");

    // 1. Handle from a file path — what a future asset resolver would produce.
    let handle_from_path = iced::widget::svg::Handle::from_path(path);
    assert_ne!(
        handle_from_path.id(),
        0,
        "path handle must have a stable id"
    );

    // 2. Handle from in-memory bytes — the pattern app.rs::icon_svg uses.
    let handle = iced::widget::svg::Handle::from_memory(bytes);
    assert_ne!(handle.id(), 0, "memory handle must have a stable id");

    // 3. Build the actual widget at every size step. Constructing proves the
    //    svg feature compiles and the widget API accepts a vendored Twemoji
    //    asset at each size — the same code the picker will emit.
    for size in SIZES {
        let svg: iced::widget::svg::Svg<'_, iced::Theme> =
            iced::widget::svg::Svg::new(handle.clone())
                .width(iced::Length::Fixed(size as f32))
                .height(iced::Length::Fixed(size as f32));
        // Widget construction succeeded at this size step.
        let _ = svg;
    }
}

#[test]
fn vendored_twemoji_rasterizes_at_multiple_sizes() {
    let path = twemoji_asset("1f600.svg");
    let bytes = std::fs::read(&path).expect("vendored Twemoji SVG must exist");

    let tree = resvg::usvg::Tree::from_data(&bytes, &resvg::usvg::Options::default())
        .expect("Twemoji SVG must parse");

    let mut counts: Vec<(u32, usize)> = Vec::new();
    for size in SIZES {
        let mut pixmap =
            resvg::tiny_skia::Pixmap::new(size, size).expect("pixmap allocation must succeed");
        // resvg renders the tree at its intrinsic size (36x36 viewBox); scale
        // it to fill each canvas — this is the resize the picker relies on.
        let scale = size as f32 / tree.size().width().max(tree.size().height());
        resvg::render(
            &tree,
            resvg::tiny_skia::Transform::from_scale(scale, scale),
            &mut pixmap.as_mut(),
        );

        let painted = pixmap.data().chunks_exact(4).filter(|px| px[3] > 0).count();
        counts.push((size, painted));

        // The grinning face fills most of its 36x36 viewBox, so a solid
        // fraction of the canvas must be painted; a blank or failed render
        // would fail this even at the smallest size.
        let min_painted = (size * size) as usize / 3;
        assert!(
            painted >= min_painted,
            "size {size}: only {painted}/{} pixels painted — SVG did not render",
            size * size
        );
    }

    // Pixel count must grow with the canvas — the SVG really rescales instead
    // of painting a fixed-size blob in the corner.
    assert!(
        counts[0].1 < counts[1].1 && counts[1].1 < counts[2].1,
        "painted pixels must scale with size, got {counts:?}"
    );
}
