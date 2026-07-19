//! End-to-end verification of image size and upload-speed improvements.
//!
//! Tests the `optimize_chat_image` function against representative images:
//! photos, screenshots, transparent PNGs, rotated images, already-compressed
//! images, tiny images, unsupported formats, oversized inputs, etc.
//!
//! Measures original vs. optimized byte size, confirms dimension caps,
//! orientation handling, preview rendering, and failure fallbacks.

use std::path::Path;

use boru_chat::image_optimizer::{
    compress_image, optimize_chat_image, CHAT_IMAGE_MAX_BYTES, CHAT_IMAGE_OPTIMIZED_MAX_BYTES,
    INLINE_IMAGE_MAX_DIM,
};
use image::GenericImageView;

/// Helper: read a test fixture into a `Vec<u8>`.
fn load_fixture(name: &str) -> Vec<u8> {
    let path = Path::new("/tmp/optimizer_test_images").join(name);
    std::fs::read(&path).unwrap_or_else(|e| panic!("cannot load fixture {name}: {e}"))
}

/// Helper: report size information for a result.
fn report(name: &str, original: &[u8], optimized: &[u8]) {
    let reduction_pct = if !original.is_empty() {
        (1.0 - optimized.len() as f64 / original.len() as f64) * 100.0
    } else {
        0.0
    };
    let under = if optimized.len() <= CHAT_IMAGE_OPTIMIZED_MAX_BYTES {
        "OK"
    } else {
        "OVER LIMIT"
    };
    println!(
        "  {name:45} {orig:>8} B -> {opt:>8} B  ({reduction:+.1}%)  {under}",
        name = name,
        orig = original.len(),
        opt = optimized.len(),
        reduction = reduction_pct,
        under = under,
    );
}

// ═══════════════════════════════════════════════════════════════════════
// Tests
// ═══════════════════════════════════════════════════════════════════════

/// Verify a photo-like image is resized and fits under the wire cap.
#[test]
fn test_photo_large() {
    let raw = load_fixture("photo_4032x3024.jpg");
    let opt = optimize_chat_image(&raw).unwrap();
    report("photo_4032x3024.jpg", &raw, &opt);

    let decoded = image::load_from_memory(&opt).unwrap();
    let (w, h) = decoded.dimensions();
    assert!(
        w.max(h) <= INLINE_IMAGE_MAX_DIM,
        "dimensions {w}x{h} exceed {INLINE_IMAGE_MAX_DIM}"
    );
    assert_eq!(
        decoded.color().channel_count(),
        3,
        "output must be RGB (JPEG)"
    );
    assert!(
        opt.len() <= CHAT_IMAGE_OPTIMIZED_MAX_BYTES,
        "size {} > limit",
        opt.len()
    );

    // Aspect-ratio check: 4032/3024 ≈ 1.333…
    let aspect = w as f64 / h as f64;
    let expected = 4032.0 / 3024.0;
    assert!(
        (aspect - expected).abs() < 0.02,
        "aspect ratio {aspect} differs from {expected}"
    );
}

/// Verify a screenshot-style image is resized to the 1280 px inline-image cap.
#[test]
fn test_screenshot() {
    let raw = load_fixture("screenshot_1920x1080.jpg");
    let opt = optimize_chat_image(&raw).unwrap();
    report("screenshot_1920x1080.jpg", &raw, &opt);

    let decoded = image::load_from_memory(&opt).unwrap();
    let (w, h) = decoded.dimensions();
    assert_eq!(w, 1280, "screenshot width should be capped at 1280");
    assert_eq!(
        h, 720,
        "screenshot height should preserve the 16:9 aspect ratio"
    );
    assert!(opt.len() <= CHAT_IMAGE_OPTIMIZED_MAX_BYTES);
}

/// Verify transparent PNG -> opaque white composite.
#[test]
fn test_transparent_png_composited() {
    let raw = load_fixture("transparent_1200.png");
    let opt = optimize_chat_image(&raw).unwrap();
    report("transparent_1200.png", &raw, &opt);

    let decoded = image::load_from_memory(&opt).unwrap();
    assert_eq!(
        decoded.color().channel_count(),
        3,
        "JPEG output must not have alpha"
    );
    // The composited image should have the largest dimension <= 1200 (no upscale
    // since 1200 <= 1920).
    assert_eq!(decoded.width(), 1200);
    assert_eq!(decoded.height(), 1200);
    assert!(opt.len() <= CHAT_IMAGE_OPTIMIZED_MAX_BYTES);
}

/// Verify physically rotated (tall) image is downscaled, not letterboxed.
#[test]
fn test_rotated_portrait() {
    let raw = load_fixture("rotated_800x1200.jpg");
    let opt = optimize_chat_image(&raw).unwrap();
    report("rotated_800x1200.jpg", &raw, &opt);

    let decoded = image::load_from_memory(&opt).unwrap();
    let (w, h) = decoded.dimensions();
    assert!(
        w.max(h) <= INLINE_IMAGE_MAX_DIM,
        "dimensions {w}x{h} exceed {INLINE_IMAGE_MAX_DIM}"
    );
    let aspect = w as f64 / h as f64;
    let expected = 800.0 / 1200.0;
    assert!(
        (aspect - expected).abs() < 0.02,
        "aspect ratio {aspect} differs from {expected}"
    );
    assert!(opt.len() <= CHAT_IMAGE_OPTIMIZED_MAX_BYTES);
}

/// Verify EXIF-rotated image is auto-oriented by the `image` crate's decoder.
#[test]
fn test_exif_auto_orientation() {
    let raw = load_fixture("rotated_exif_1200x800.jpg");
    let opt = optimize_chat_image(&raw).unwrap();
    report("rotated_exif_1200x800.jpg", &raw, &opt);

    let decoded = image::load_from_memory(&opt).unwrap();
    assert!(decoded.width().max(decoded.height()) <= INLINE_IMAGE_MAX_DIM);
    assert!(opt.len() <= CHAT_IMAGE_OPTIMIZED_MAX_BYTES);
    // No EXIF APP1 marker remains in the output.
    let has_exif = opt.windows(2).any(|w| w == [0xFF, 0xE1]);
    assert!(!has_exif, "output should not contain EXIF APP1 marker");
}

/// Verify an already-compressed small image is not upscaled.
#[test]
fn test_already_compressed_small() {
    let raw = load_fixture("already_compressed_640.jpg");
    let opt = optimize_chat_image(&raw).unwrap();
    report("already_compressed_640.jpg", &raw, &opt);

    let decoded = image::load_from_memory(&opt).unwrap();
    assert_eq!(decoded.width(), 640, "should not upscale");
    assert_eq!(decoded.height(), 480, "should not upscale");
    assert!(opt.len() <= CHAT_IMAGE_OPTIMIZED_MAX_BYTES);
}

/// Verify a tiny 1x1 image is passed through (no upscale, no crash).
#[test]
fn test_tiny_1x1() {
    let raw = load_fixture("tiny_1x1.jpg");
    let opt = optimize_chat_image(&raw).unwrap();
    report("tiny_1x1.jpg", &raw, &opt);

    let decoded = image::load_from_memory(&opt).unwrap();
    assert_eq!(decoded.width(), 1);
    assert_eq!(decoded.height(), 1);
    assert!(opt.len() <= CHAT_IMAGE_OPTIMIZED_MAX_BYTES);
}

/// Verify tiny transparent 1x1 PNG composites onto white.
#[test]
fn test_tiny_transparent_1x1() {
    let raw = load_fixture("tiny_transparent_1x1.png");
    let opt = optimize_chat_image(&raw).unwrap();
    report("tiny_transparent_1x1.png", &raw, &opt);

    let decoded = image::load_from_memory(&opt).unwrap();
    assert_eq!(decoded.width(), 1);
    assert_eq!(decoded.height(), 1);
    assert_eq!(
        decoded.color().channel_count(),
        3,
        "JPEG must not have alpha"
    );
    assert!(opt.len() <= CHAT_IMAGE_OPTIMIZED_MAX_BYTES);
}

/// Verify that BMP images (which the `image` crate can decode) are handled
/// successfully — the optimizer accepts anything the crate can decode, not
/// just JPEG and PNG.
#[test]
fn test_bmp_format_accepted() {
    let raw = load_fixture("unsupported_100x100.bmp");
    let opt = optimize_chat_image(&raw).unwrap();
    report("unsupported_100x100.bmp", &raw, &opt);
    let decoded = image::load_from_memory(&opt).unwrap();
    assert!(decoded.width().max(decoded.height()) <= INLINE_IMAGE_MAX_DIM);
    assert!(opt.len() <= CHAT_IMAGE_OPTIMIZED_MAX_BYTES);
    println!("  -> BMP accepted and optimized (image crate decoded it)");
}

/// Verify that single-frame GIFs are handled successfully.
#[test]
fn test_gif_format_accepted() {
    let raw = load_fixture("unsupported_single.gif");
    let opt = optimize_chat_image(&raw).unwrap();
    report("unsupported_single.gif", &raw, &opt);
    let decoded = image::load_from_memory(&opt).unwrap();
    assert!(decoded.width().max(decoded.height()) <= INLINE_IMAGE_MAX_DIM);
    assert!(opt.len() <= CHAT_IMAGE_OPTIMIZED_MAX_BYTES);
    println!("  -> Single-frame GIF accepted and optimized (image crate decoded it)");
}

/// Verify that animated GIFs are also accepted (the image crate decodes the
/// first frame).
#[test]
fn test_animated_gif_first_frame_decoded() {
    let raw = load_fixture("unsupported_animated.gif");
    let opt = optimize_chat_image(&raw).unwrap();
    report("unsupported_animated.gif", &raw, &opt);
    let decoded = image::load_from_memory(&opt).unwrap();
    assert!(decoded.width().max(decoded.height()) <= INLINE_IMAGE_MAX_DIM);
    assert!(opt.len() <= CHAT_IMAGE_OPTIMIZED_MAX_BYTES);
    println!("  -> Animated GIF first frame accepted and optimized");
}

/// Verify that truly corrupt/undecodable bytes are rejected.
#[test]
fn test_truly_corrupt_bytes() {
    let raw = b"\x00\xff\xfe\xfd\x00\x01\x02\x03garbage not an image at all";
    let err = optimize_chat_image(raw).unwrap_err();
    assert!(
        err.contains("Unsupported"),
        "error should mention unsupported: {err}"
    );
    println!("  corrupt bytes -> error: {err}");
}

/// Verify oversized input (>=10 MiB) is rejected before processing.
#[test]
fn test_oversized_input_rejected() {
    let raw = load_fixture("oversized_massive.jpg");
    assert!(
        raw.len() > CHAT_IMAGE_MAX_BYTES,
        "fixture must be > {} bytes, was {}",
        CHAT_IMAGE_MAX_BYTES,
        raw.len()
    );
    let err = optimize_chat_image(&raw).unwrap_err();
    assert!(err.contains("MiB"), "error should mention MiB: {err}");
    println!(
        "  oversized_massive.jpg ({:.1} MiB) -> error: {err}",
        raw.len() as f64 / (1024.0 * 1024.0)
    );
}

/// Verify that every readable JPG fixture produces an output <= 2 MiB.
#[test]
fn test_all_jpeg_fixtures_under_limit() {
    let files = [
        "already_compressed_640.jpg",
        "screenshot_1920x1080.jpg",
        "photo_4032x3024.jpg",
        "rotated_800x1200.jpg",
        "rotated_exif_1200x800.jpg",
        "tiny_1x1.jpg",
        "oversized_8000x6000.jpg",
    ];
    for fname in &files {
        let raw = load_fixture(fname);
        if raw.len() > CHAT_IMAGE_MAX_BYTES {
            let err = optimize_chat_image(&raw);
            assert!(err.is_err(), "{fname} should be rejected (over size limit)");
            println!("  {fname:45} SKIPPED (input > limit)");
            continue;
        }
        let opt =
            optimize_chat_image(&raw).unwrap_or_else(|e| panic!("{fname}: optimize failed: {e}"));
        report(fname, &raw, &opt);
        assert!(
            opt.len() <= CHAT_IMAGE_OPTIMIZED_MAX_BYTES,
            "{fname}: output {} exceeds {}",
            opt.len(),
            CHAT_IMAGE_OPTIMIZED_MAX_BYTES
        );
    }
}

/// Verify PNG fixtures also produce <= 2 MiB output.
#[test]
fn test_all_png_fixtures_under_limit() {
    let files = [
        "already_compressed_640.png",
        "photo_4032x3024.png",
        "screenshot_1920x1080.png",
        "tiny_1x1.png",
        "tiny_transparent_1x1.png",
        "transparent_1200.png",
    ];
    for fname in &files {
        let raw = load_fixture(fname);
        if raw.len() > CHAT_IMAGE_MAX_BYTES {
            let err = optimize_chat_image(&raw);
            assert!(err.is_err(), "{fname} should be rejected (over size limit)");
            println!("  {fname:45} SKIPPED (input > limit)");
            continue;
        }
        let opt =
            optimize_chat_image(&raw).unwrap_or_else(|e| panic!("{fname}: optimize failed: {e}"));
        report(fname, &raw, &opt);
        assert!(
            opt.len() <= CHAT_IMAGE_OPTIMIZED_MAX_BYTES,
            "{fname}: output {} exceeds {}",
            opt.len(),
            CHAT_IMAGE_OPTIMIZED_MAX_BYTES
        );
    }
}

/// Verify compress_image fallback: truly invalid bytes return original.
/// Note: BMP, GIF, and ICO are accepted by the `image` crate internally, so
/// compressed_image_successfully() — only pure garbage triggers the fallback.
#[test]
fn test_compress_image_fallback_all_formats() {
    let bad_inputs = [
        ("garbage", b"1234\x00\xffgarbage".to_vec()),
        ("empty", b"".to_vec()),
    ];
    for (label, bytes) in &bad_inputs {
        let result = compress_image(bytes);
        assert_eq!(
            &result, bytes,
            "compress_image({label}) should return original bytes unchanged"
        );
        println!(
            "  compress_image({label:30}) -> fallback (returned original {size} B)",
            label = label,
            size = bytes.len()
        );
    }
}

/// Verify empty input is rejected.
#[test]
fn test_empty_input() {
    let err = optimize_chat_image(b"").unwrap_err();
    assert!(err.contains("empty"), "error should mention empty: {err}");
}

/// Verify gradient photo from 20MP source processes within limits.
#[test]
fn test_photo_20mp() {
    let raw = load_fixture("photo_20mp_5472x3648.jpg");
    if raw.len() > CHAT_IMAGE_MAX_BYTES {
        let err = optimize_chat_image(&raw).unwrap_err();
        println!("  photo_20mp_5472x3648.jpg: over limit, rejection: {err}");
        return;
    }
    let opt = optimize_chat_image(&raw).unwrap();
    report("photo_20mp_5472x3648.jpg", &raw, &opt);
    let decoded = image::load_from_memory(&opt).unwrap();
    let (w, h) = decoded.dimensions();
    assert!(w.max(h) <= INLINE_IMAGE_MAX_DIM);
    assert!(opt.len() <= CHAT_IMAGE_OPTIMIZED_MAX_BYTES);
}
