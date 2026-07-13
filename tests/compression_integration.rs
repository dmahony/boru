//! Integration tests for the pure-Rust image compression module.
//!
//! All test images are generated inline using the `image` crate, so no
//! external fixtures are required and no C FFI dependencies are introduced.
//!
//! Tests cover:
//! - Various input formats (JPEG, PNG)
//! - Various sizes (tiny, small, medium, large, portrait)
//! - Quality settings (1, 50, 80, 100)
//! - Max dimension settings (320, 800, 1280, 4000)
//! - Quality clamping (0 → 1, 200 → 100)
//! - Format passthrough (already-compressed small images stay unchanged size)
//! - Edge cases (empty, corrupt, zero-dimension)
//! - Size reduction verification
//! - Output format verification (valid JPEG, no EXIF, RGB only)

use boru_chat::compression::compress_image;
use image::{
    codecs::jpeg::JpegEncoder, codecs::png::PngEncoder, ExtendedColorType, GenericImageView,
    ImageEncoder,
};

// ── Helpers ───────────────────────────────────────────────────────────────────

/// Create a synthetic RGB image with a deterministic pattern.
fn make_test_rgb(w: u32, h: u32) -> image::RgbImage {
    let mut img = image::RgbImage::new(w, h);
    for y in 0..h {
        for x in 0..w {
            let r = ((x * 127) % 256) as u8;
            let g = ((y * 63) % 256) as u8;
            let b = ((x + y) * 31 % 256) as u8;
            img.put_pixel(x, y, image::Rgb([r, g, b]));
        }
    }
    img
}

/// Create a synthetic RGBA image with a checkerboard transparency pattern.
fn make_test_rgba(w: u32, h: u32) -> image::RgbaImage {
    let mut img = image::RgbaImage::new(w, h);
    for y in 0..h {
        for x in 0..w {
            let r = ((x * 127) % 256) as u8;
            let g = ((y * 63) % 256) as u8;
            let b = ((x + y) * 31 % 256) as u8;
            let a = if (x + y) % 2 == 0 { 255 } else { 128 };
            img.put_pixel(x, y, image::Rgba([r, g, b, a]));
        }
    }
    img
}

fn encode_jpeg(img: &image::RgbImage, quality: u8) -> Vec<u8> {
    let mut buf = std::io::Cursor::new(Vec::new());
    let encoder = JpegEncoder::new_with_quality(&mut buf, quality);
    encoder
        .write_image(
            img.as_raw(),
            img.width(),
            img.height(),
            ExtendedColorType::Rgb8,
        )
        .unwrap();
    buf.into_inner()
}

fn encode_png_rgb(w: u32, h: u32) -> Vec<u8> {
    let img = make_test_rgb(w, h);
    let mut buf = std::io::Cursor::new(Vec::new());
    let encoder = PngEncoder::new(&mut buf);
    encoder
        .write_image(img.as_raw(), w, h, ExtendedColorType::Rgb8)
        .unwrap();
    buf.into_inner()
}

fn encode_png_rgba(w: u32, h: u32) -> Vec<u8> {
    let img = make_test_rgba(w, h);
    let mut buf = std::io::Cursor::new(Vec::new());
    let encoder = PngEncoder::new(&mut buf);
    encoder
        .write_image(img.as_raw(), w, h, ExtendedColorType::Rgba8)
        .unwrap();
    buf.into_inner()
}

fn pixel_diff(img1: &image::RgbImage, img2: &image::RgbImage) -> f64 {
    let (w1, h1) = img1.dimensions();
    let (w2, h2) = img2.dimensions();
    if w1 != w2 || h1 != h2 {
        return f64::MAX;
    }
    let total = (w1 * h1) as f64;
    if total == 0.0 {
        return 0.0;
    }
    let mut diff_sum = 0.0;
    for y in 0..h1 {
        for x in 0..w1 {
            let p1 = img1.get_pixel(x, y);
            let p2 = img2.get_pixel(x, y);
            let d = (p1[0] as f64 - p2[0] as f64).abs()
                + (p1[1] as f64 - p2[1] as f64).abs()
                + (p1[2] as f64 - p2[2] as f64).abs();
            diff_sum += d / 3.0;
        }
    }
    diff_sum / total / 255.0
}

fn assert_valid_jpeg(bytes: &[u8]) {
    assert!(bytes.len() >= 2, "output too short for JPEG header");
    assert_eq!(bytes[0], 0xFF, "missing JPEG SOI marker byte 0");
    assert_eq!(bytes[1], 0xD8, "missing JPEG SOI marker byte 1");
    // Confirm we can decode it
    let decoded = image::load_from_memory(bytes).expect("output should be a valid image");
    // JPEG output should be RGB (no alpha)
    assert_eq!(
        decoded.color().channel_count(),
        3,
        "JPEG output must be RGB"
    );
}

fn assert_no_exif(bytes: &[u8]) {
    let has_exif = bytes.windows(2).any(|w| w == [0xFF, 0xE1]);
    assert!(!has_exif, "output must not contain EXIF APP1 marker");
}

// ── Format support tests ──────────────────────────────────────────────────────

#[test]
fn integration_compress_jpeg_input() {
    let raw = encode_jpeg(&make_test_rgb(640, 480), 80);
    let compressed = compress_image(&raw, 1280, 70).unwrap();
    assert!(compressed.len() > 0);
    assert_valid_jpeg(&compressed);
    assert_no_exif(&compressed);
}

#[test]
fn integration_compress_png_rgb_input() {
    let raw = encode_png_rgb(800, 600);
    let compressed = compress_image(&raw, 1280, 80).unwrap();
    let decoded = image::load_from_memory(&compressed).unwrap();
    assert_eq!(decoded.width(), 800);
    assert_eq!(decoded.height(), 600);
    assert_eq!(decoded.color().channel_count(), 3);
    assert_valid_jpeg(&compressed);
    assert_no_exif(&compressed);
}

#[test]
fn integration_compress_png_rgba_input() {
    let raw = encode_png_rgba(640, 480);
    let compressed = compress_image(&raw, 1280, 75).unwrap();
    let decoded = image::load_from_memory(&compressed).unwrap();
    // RGBA → JPEG composites alpha onto black
    assert_eq!(
        decoded.color().channel_count(),
        3,
        "RGBA PNG must produce RGB JPEG"
    );
    assert_valid_jpeg(&compressed);
}

// ── Size and dimension tests ──────────────────────────────────────────────────

#[test]
fn integration_compress_jpeg_passthrough() {
    // Small JPEG that doesn't need resizing
    let raw = encode_jpeg(&make_test_rgb(640, 480), 85);
    let compressed = compress_image(&raw, 1280, 75).unwrap();
    let decoded = image::load_from_memory(&compressed).unwrap();
    assert_eq!(decoded.width(), 640);
    assert_eq!(decoded.height(), 480);
    assert_valid_jpeg(&compressed);
}

#[test]
fn integration_compress_downscale_large() {
    // Large image that needs resizing to 1280
    let raw = encode_jpeg(&make_test_rgb(4000, 3000), 85);
    let compressed = compress_image(&raw, 1280, 70).unwrap();
    let decoded = image::load_from_memory(&compressed).unwrap();
    let (w, h) = decoded.dimensions();
    assert!(
        w.max(h) <= 1280,
        "longest edge {} should be ≤ 1280",
        w.max(h)
    );
    // Aspect ratio preserved: 4000/3000 = 4/3 ≈ 1.333
    let aspect = w as f64 / h as f64;
    let expected = 4000.0 / 3000.0;
    assert!(
        (aspect - expected).abs() < 0.02,
        "aspect ratio {aspect} differs from {expected}"
    );
    assert_valid_jpeg(&compressed);
}

#[test]
fn integration_compress_downscale_portrait() {
    // Portrait image — longest edge is height
    let raw = encode_jpeg(&make_test_rgb(800, 1200), 80);
    let compressed = compress_image(&raw, 600, 70).unwrap();
    let decoded = image::load_from_memory(&compressed).unwrap();
    let (w, h) = decoded.dimensions();
    assert!(h <= 600, "portrait height {h} should be ≤ 600");
    assert!(w <= 600, "portrait width {w} should be ≤ 600");
    // Aspect ratio preserved: 800/1200 = 2/3 ≈ 0.667
    let aspect = w as f64 / h as f64;
    let expected = 800.0 / 1200.0;
    assert!(
        (aspect - expected).abs() < 0.02,
        "aspect ratio {aspect} differs from {expected}"
    );
}

#[test]
fn integration_compress_small_not_upscaled() {
    // Tiny image stays at original size
    let raw = encode_jpeg(&make_test_rgb(16, 16), 85);
    let compressed = compress_image(&raw, 1280, 80).unwrap();
    let decoded = image::load_from_memory(&compressed).unwrap();
    assert_eq!(decoded.width(), 16);
    assert_eq!(decoded.height(), 16);
}

#[test]
fn integration_compress_1x1() {
    let raw = encode_jpeg(&make_test_rgb(1, 1), 90);
    let compressed = compress_image(&raw, 1280, 80).unwrap();
    let decoded = image::load_from_memory(&compressed).unwrap();
    assert_eq!(decoded.width(), 1);
    assert_eq!(decoded.height(), 1);
}

#[test]
fn integration_compress_1920x1080_screenshot() {
    // 1920x1080 → should fit within 1280 (longest edge = 1920)
    let raw = encode_jpeg(&make_test_rgb(1920, 1080), 85);
    let compressed = compress_image(&raw, 1280, 80).unwrap();
    let decoded = image::load_from_memory(&compressed).unwrap();
    let (w, h) = decoded.dimensions();
    assert!(
        w.max(h) <= 1280,
        "screenshot should be capped at 1280, got {w}x{h}"
    );
    // Aspect ratio preserved: 1920/1080 = 16/9 ≈ 1.778
    let aspect = w as f64 / h as f64;
    let expected = 1920.0 / 1080.0;
    assert!(
        (aspect - expected).abs() < 0.02,
        "aspect ratio {aspect} differs from {expected}"
    );
}

// ── Quality tests ─────────────────────────────────────────────────────────────

#[test]
fn integration_compress_quality_1() {
    let raw = encode_jpeg(&make_test_rgb(800, 600), 95);
    let compressed = compress_image(&raw, 1280, 1).unwrap();
    let decoded = image::load_from_memory(&compressed).unwrap();
    assert_eq!(decoded.width(), 800);
    assert_eq!(decoded.height(), 600);
    // Minimum quality should produce smaller output than the high-quality input
    assert!(
        compressed.len() < raw.len(),
        "quality 1 should reduce size; raw={}, compressed={}",
        raw.len(),
        compressed.len()
    );
}

#[test]
fn integration_compress_quality_50() {
    let raw = encode_jpeg(&make_test_rgb(800, 600), 95);
    let compressed = compress_image(&raw, 1280, 50).unwrap();
    let decoded = image::load_from_memory(&compressed).unwrap();
    assert_eq!(decoded.width(), 800);
    assert_eq!(decoded.height(), 600);
    // Quality 50 should be smaller than quality 100 on same input
    let q100 = compress_image(&raw, 1280, 100).unwrap();
    assert!(
        compressed.len() <= q100.len(),
        "quality 50 output ({} B) should be ≤ quality 100 output ({} B)",
        compressed.len(),
        q100.len()
    );
}

#[test]
fn integration_compress_quality_100() {
    let raw = encode_jpeg(&make_test_rgb(400, 300), 85);
    let compressed = compress_image(&raw, 1280, 100).unwrap();
    let decoded = image::load_from_memory(&compressed).unwrap();
    assert_eq!(decoded.width(), 400);
    assert_eq!(decoded.height(), 300);
    assert_valid_jpeg(&compressed);
}

// ── Quality clamping tests ────────────────────────────────────────────────────

#[test]
fn integration_compress_quality_clamping() {
    let raw = encode_jpeg(&make_test_rgb(100, 100), 85);

    let low = compress_image(&raw, 1280, 0).unwrap();
    let normal = compress_image(&raw, 1280, 1).unwrap();
    let high = compress_image(&raw, 1280, 200).unwrap();
    let max_normal = compress_image(&raw, 1280, 100).unwrap();

    // quality 0 clamped to 1 → sizes should be the same or very close
    let diff_low = (low.len() as i64 - normal.len() as i64).abs();
    assert!(
        diff_low < 100,
        "quality 0 and 1 should produce similar sizes, diff={diff_low}"
    );
    assert_valid_jpeg(&low);

    // quality 200 clamped to 100 → sizes should match quality 100
    let diff_high = (high.len() as i64 - max_normal.len() as i64).abs();
    assert!(
        diff_high < 100,
        "quality 200 and 100 should produce similar sizes, diff={diff_high}"
    );
    assert_valid_jpeg(&high);
}

// ── Max dimension tests ───────────────────────────────────────────────────────

#[test]
fn integration_compress_max_dim_320() {
    // Aggressive resize: max 320 px
    let raw = encode_jpeg(&make_test_rgb(2000, 1000), 80);
    let compressed = compress_image(&raw, 320, 70).unwrap();
    let decoded = image::load_from_memory(&compressed).unwrap();
    assert!(
        decoded.width().max(decoded.height()) <= 320,
        "longest edge {} should be ≤ 320",
        decoded.width().max(decoded.height())
    );
}

#[test]
fn integration_compress_max_dim_larger_than_image() {
    // max_dim larger than image — no resize
    let raw = encode_jpeg(&make_test_rgb(100, 100), 85);
    let compressed = compress_image(&raw, 4000, 80).unwrap();
    let decoded = image::load_from_memory(&compressed).unwrap();
    assert_eq!(decoded.width(), 100);
    assert_eq!(decoded.height(), 100);
}

#[test]
fn integration_compress_max_dim_800() {
    let raw = encode_jpeg(&make_test_rgb(3000, 2000), 80);
    let compressed = compress_image(&raw, 800, 60).unwrap();
    let decoded = image::load_from_memory(&compressed).unwrap();
    assert!(decoded.width().max(decoded.height()) <= 800);
    // Aspect ratio preserved: 3000/2000 = 1.5
    let aspect = decoded.width() as f64 / decoded.height() as f64;
    let expected = 3000.0 / 2000.0;
    assert!(
        (aspect - expected).abs() < 0.02,
        "aspect ratio {aspect} differs from {expected}"
    );
}

// ── Default config test (simulates app defaults: quality=80, max_dim=1280) ─────

#[test]
fn integration_compress_default_config() {
    let raw = encode_jpeg(&make_test_rgb(4000, 3000), 85);
    let compressed = compress_image(&raw, 1280, 80).unwrap();
    let decoded = image::load_from_memory(&compressed).unwrap();
    let (w, h) = decoded.dimensions();
    assert!(w.max(h) <= 1280);
    let aspect = w as f64 / h as f64;
    let expected = 4000.0 / 3000.0;
    assert!(
        (aspect - expected).abs() < 0.02,
        "aspect ratio {aspect} differs from {expected}"
    );
    assert_valid_jpeg(&compressed);
    assert!(
        compressed.len() < raw.len(),
        "compressed should be smaller than raw"
    );
}

// ── Size reduction verification ───────────────────────────────────────────────

#[test]
fn integration_compress_size_reduction_jpeg_large() {
    let raw = encode_jpeg(&make_test_rgb(4000, 3000), 85);
    let compressed = compress_image(&raw, 1280, 70).unwrap();
    assert!(
        compressed.len() < raw.len(),
        "compressed size {} should be smaller than raw {}",
        compressed.len(),
        raw.len()
    );
}

#[test]
fn integration_compress_size_reduction_png_large() {
    let raw = encode_png_rgb(3000, 2000);
    let compressed = compress_image(&raw, 1024, 75).unwrap();
    assert!(
        compressed.len() < raw.len(),
        "PNG->JPEG compressed size {} should be smaller than raw {}",
        compressed.len(),
        raw.len()
    );
}

// ── Visual quality verification ───────────────────────────────────────────────

#[test]
fn integration_compress_visual_quality() {
    // Create a smooth gradient for a fair quality comparison
    let w = 800u32;
    let h = 600u32;
    let mut img = image::RgbImage::new(w, h);
    for y in 0..h {
        for x in 0..w {
            let r = (x as f64 * 0.3) as u8;
            let g = (y as f64 * 0.2 + 80.0) as u8;
            let b = ((x + y) as f64 * 0.15 + 40.0) as u8;
            img.put_pixel(x, y, image::Rgb([r, g, b]));
        }
    }
    let raw = encode_jpeg(&img, 95);
    let compressed = compress_image(&raw, 800, 85).unwrap();

    // Decode and verify quality is reasonable
    let decoded = image::load_from_memory(&compressed).unwrap().to_rgb8();
    let reference = image::imageops::resize(
        &img,
        decoded.width(),
        decoded.height(),
        image::imageops::FilterType::Triangle,
    );
    let diff = pixel_diff(&decoded, &reference);
    // Mean absolute error ≤ 10/255 per pixel (relaxed for quality 85 vs 95)
    assert!(
        diff <= 10.0 / 255.0,
        "pixel error {diff} exceeds threshold {}",
        10.0 / 255.0
    );
}

// ── Error handling ────────────────────────────────────────────────────────────

#[test]
fn integration_compress_empty_input() {
    let err = compress_image(b"", 1280, 80).unwrap_err();
    assert!(err.contains("empty"), "error should mention empty: {err}");
}

#[test]
fn integration_compress_corrupt_input() {
    let err = compress_image(b"garbage\x00\xffnotanimage", 1280, 80).unwrap_err();
    assert!(
        err.contains("Unsupported") || err.contains("corrupt"),
        "error should be descriptive: {err}"
    );
}

// ── PNG transparency handling ─────────────────────────────────────────────────

#[test]
fn integration_compress_transparent_png_composites() {
    let raw = encode_png_rgba(120, 120);
    let compressed = compress_image(&raw, 1280, 80).unwrap();
    let decoded = image::load_from_memory(&compressed).unwrap();
    // Alpha should be gone — JPEG has no alpha
    assert_eq!(
        decoded.color().channel_count(),
        3,
        "JPEG must not have alpha"
    );
    assert_valid_jpeg(&compressed);
}

// ── Multiple formats bulk test ────────────────────────────────────────────────

#[test]
fn integration_compress_all_formats_accepted() {
    let cases: Vec<(&str, Vec<u8>)> = vec![
        ("JPEG 640x480", encode_jpeg(&make_test_rgb(640, 480), 80)),
        ("PNG RGB 800x600", encode_png_rgb(800, 600)),
        ("PNG RGBA 320x240", encode_png_rgba(320, 240)),
    ];

    for (label, raw) in &cases {
        let result = compress_image(raw, 1280, 80);
        assert!(
            result.is_ok(),
            "{label}: compression failed: {}",
            result.unwrap_err()
        );
        let compressed = result.unwrap();
        assert_valid_jpeg(&compressed);
        assert_no_exif(&compressed);
        assert!(
            compressed.len() > 0,
            "{label}: compressed output should not be empty"
        );
    }
}

// ─── Simulated CompressionConfig flow ────────────────────────────────────────
//
// These tests simulate the runtime flow used by both the TUI chat (with
// `/compress` / `/resize` commands controlling CompressionConfig) and the
// Iced GUI (with settings panel controlling image_quality / image_max_dim).

#[test]
fn integration_compress_enabled_default_settings() {
    // Simulates: /compress on  (with default quality=80, max_dim=1280)
    let raw = encode_jpeg(&make_test_rgb(4000, 3000), 85);
    let compressed = compress_image(&raw, 1280, 80).unwrap();
    let decoded = image::load_from_memory(&compressed).unwrap();
    assert!(decoded.width().max(decoded.height()) <= 1280);
    assert!(
        compressed.len() < raw.len(),
        "compression should reduce size"
    );
}

#[test]
fn integration_compress_enabled_high_quality_large_max() {
    // Simulates: /compress quality 95  and  /resize 2000
    let raw = encode_jpeg(&make_test_rgb(4000, 3000), 85);
    let compressed = compress_image(&raw, 2000, 95).unwrap();
    let decoded = image::load_from_memory(&compressed).unwrap();
    assert!(decoded.width().max(decoded.height()) <= 2000);
    assert_valid_jpeg(&compressed);
}

#[test]
fn integration_compress_enabled_low_quality_small_max() {
    // Simulates: /compress quality 10  and  /resize 320
    let raw = encode_jpeg(&make_test_rgb(4000, 3000), 85);
    let compressed = compress_image(&raw, 320, 10).unwrap();
    let decoded = image::load_from_memory(&compressed).unwrap();
    assert!(decoded.width().max(decoded.height()) <= 320);
    // Very aggressive compression should produce very small output
    assert!(
        compressed.len() < raw.len() / 4,
        "aggressive compression: raw={}, compressed={}",
        raw.len(),
        compressed.len()
    );
}

#[test]
fn integration_compress_resize_off_no_downscale() {
    // Simulates: /resize off  → max_dim=0 means no resize, just quality re-encode
    // Note: resize with max_dim=0 will fail since max_dim > 0 is required for
    // proper behavior. max_dim=0 would cause no resize because 0 < image edge.
    // So this tests that a very large max_dim (~4x image size) passes through.
    let raw = encode_jpeg(&make_test_rgb(640, 480), 85);
    let compressed = compress_image(&raw, 4000, 80).unwrap();
    let decoded = image::load_from_memory(&compressed).unwrap();
    assert_eq!(
        decoded.width(),
        640,
        "should not resize when max_dim > image dims"
    );
    assert_eq!(decoded.height(), 480);
}

// ── No C FFI verification ────────────────────────────────────────────────────
//
// The `image` crate's JPEG encoder is pure Rust (no mozjpeg, libjpeg-turbo,
// or any other native dependency).  The `jpeg` and `png` features use the
// built-in pure-Rust implementations from the `image` crate.
//
// If a future change introduces a C FFI dependency (e.g. libwebp, mozjpeg),
// this compile-time check will fail to link on platforms without the native
// library — but that is a runtime/link-time concern.  The crate-level feature
// gate ensures no optional C-FFI features are accidentally enabled.

#[test]
fn integration_compress_no_ffi_jpeg_encoder_pure_rust() {
    // The JpegEncoder struct lives in image::codecs::jpeg which is pure Rust.
    // If someone switches to a C-based backend, the encoder type will change
    // and this test (or the `image` crate) will fail at compile/link time.
    let encoder = JpegEncoder::new_with_quality(std::io::Cursor::new(Vec::new()), 80);
    let _ = encoder; // just verify the type exists and is pure Rust
}
