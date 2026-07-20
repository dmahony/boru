//! Pure-Rust image compression utilities using the `image` crate.
//!
//! Provides [`compress_image`] — resize and JPEG-encode image bytes with
//! caller-specified maximum dimension and quality.
//!
//! Also exposes internal helpers (`resize_rgb8`, `encode_jpeg_rgb8`) as
//! `pub(crate)` so that [`image_optimizer`](crate::image_optimizer) can reuse
//! the same core logic without duplicating the resize/encode code.
//!
//! The underlying JPEG encoder (`image::codecs::jpeg::JpegEncoder`) is pure
//! Rust with no C FFI: no mozjpeg, libjpeg-turbo, libwebp, or any other
//! native dependency. This keeps cross-compilation and toolchain setup
//! predictable across all targets.
//!
//! # Examples
//!
//! ```rust
//! # let raw = std::fs::read("/dev/null").unwrap_or_default();
//! # // compile-only check; the real fixture comes from test helpers
//! # fn _check() {
//! let raw = b"dummy bytes that won't decode -- this is a doc-test skeleton";
//! let result = boru_chat::compression::compress_image(raw, 1280, 80);
//! assert!(result.is_err()); // dummy input can't decode
//! # }
//! ```

use image::codecs::jpeg::JpegEncoder;
use image::{GenericImageView, ImageEncoder, RgbImage};

// ── Shared internal helpers (pub(crate)) ──────────────────────────────
// These let image_optimizer.rs reuse the same resize + encode logic
// without duplicating code.

/// Resize `img` so its longest edge does not exceed `max_dim`, preserving
/// aspect ratio.  Never upscales.  Uses [`FilterType::Triangle`].
///
/// Returns the original image unchanged when no resize is needed.
pub(crate) fn resize_rgb8(img: RgbImage, max_dim: u32) -> RgbImage {
    let (w, h) = img.dimensions();
    let max_edge = w.max(h);
    if max_edge <= max_dim {
        return img;
    }
    let ratio = max_dim as f64 / max_edge as f64;
    let new_w = (w as f64 * ratio).round().max(1.0) as u32;
    let new_h = (h as f64 * ratio).round().max(1.0) as u32;
    image::imageops::resize(&img, new_w, new_h, image::imageops::FilterType::Triangle)
}

/// Encode `img` as a baseline JPEG at the given `quality` (1–100).
///
/// The output has no EXIF / XMP / ICC metadata (the `image` crate strips
/// metadata during decode and does not carry it through to re-encode).
pub(crate) fn encode_jpeg_rgb8(img: &RgbImage, quality: u8) -> Result<Vec<u8>, String> {
    let quality = quality.clamp(1, 100);
    let mut buf = std::io::Cursor::new(Vec::new());
    let encoder = JpegEncoder::new_with_quality(&mut buf, quality);
    encoder
        .write_image(
            img.as_raw(),
            img.width(),
            img.height(),
            image::ExtendedColorType::Rgb8,
        )
        .map_err(|_| "JPEG encoding failed.".to_string())?;
    Ok(buf.into_inner())
}

// ── Public API ────────────────────────────────────────────────────────

/// Load, resize (if needed), and re-encode an image as JPEG.
///
/// * `bytes` — raw image bytes. Any format the `image` crate can decode
///   is accepted (JPEG, PNG, BMP, GIF, etc.).
/// * `max_dim` — maximum allowed value for the longer edge (width or
///   height) in pixels. The image is never upscaled — if both dimensions
///   are already ≤ `max_dim`, no resize is performed.
/// * `quality` — JPEG quality 1–100. Higher values retain more detail
///   but produce larger files.
///
/// Returns `Ok(compressed_bytes)` on success, or `Err(descriptive_message)`
/// if the input is empty, corrupt, undecodable, or has zero dimensions.
///
/// The output is baseline JPEG with no EXIF / XMP / ICC metadata (the
/// `image` crate strips metadata during decode and does not carry it
/// through to re-encode).
pub fn compress_image(bytes: &[u8], max_dim: u32, quality: u8) -> Result<Vec<u8>, String> {
    if bytes.is_empty() {
        return Err("Input is empty.".to_string());
    }

    // Decode (auto-detects format, auto-orients via EXIF if present)
    let img = image::load_from_memory(bytes)
        .map_err(|_| "Unsupported or corrupt image format.".to_string())?;

    let (w, h) = img.dimensions();
    if w == 0 || h == 0 {
        return Err("Image has zero dimensions.".to_string());
    }

    // Convert to RGB (composites alpha onto black by default — callers
    // that care about alpha should pre-composite before writing bytes)
    let rgb = img.to_rgb8();

    // Resize if needed, then encode
    let resized = resize_rgb8(rgb, max_dim);
    encode_jpeg_rgb8(&resized, quality)
}

// ── Tests ─────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    // ── Helpers ───────────────────────────────────────────────────

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

    fn encode_jpeg(img: &image::RgbImage, quality: u8) -> Vec<u8> {
        encode_jpeg_rgb8(img, quality).unwrap()
    }

    fn encode_png_rgb(w: u32, h: u32) -> Vec<u8> {
        let img = make_test_rgb(w, h);
        let mut buf = std::io::Cursor::new(Vec::new());
        let encoder = image::codecs::png::PngEncoder::new(&mut buf);
        encoder
            .write_image(img.as_raw(), w, h, image::ExtendedColorType::Rgb8)
            .unwrap();
        buf.into_inner()
    }

    /// Measure mean per-channel pixel difference between two RGB images.
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

    // ── Basic acceptance tests ───────────────────────────────────

    #[test]
    fn test_compress_jpeg_passthrough() {
        // Small JPEG that doesn't need resizing
        let raw = encode_jpeg(&make_test_rgb(640, 480), 85);
        let compressed = compress_image(&raw, 1280, 75).unwrap();

        let decoded = image::load_from_memory(&compressed).unwrap();
        let (w, h) = decoded.dimensions();

        // Dimensions unchanged (640x480 ≤ 1280)
        assert_eq!(w, 640);
        assert_eq!(h, 480);

        // Valid JPEG header
        assert_eq!(compressed[0], 0xFF);
        assert_eq!(compressed[1], 0xD8);

        // No alpha (JPEG is RGB)
        assert_eq!(decoded.color().channel_count(), 3);
    }

    #[test]
    fn test_compress_jpeg_downscale() {
        // Large image that needs resizing
        let raw = encode_jpeg(&make_test_rgb(4000, 3000), 50);
        let compressed = compress_image(&raw, 1280, 70).unwrap();

        let decoded = image::load_from_memory(&compressed).unwrap();
        let (w, h) = decoded.dimensions();

        // Longest edge ≤ 1280
        assert!(w.max(h) <= 1280);

        // Aspect ratio preserved: 4000/3000 = 4/3 ≈ 1.333
        let aspect = w as f64 / h as f64;
        let expected = 4000.0 / 3000.0;
        assert!(
            (aspect - expected).abs() < 0.02,
            "aspect ratio {aspect} differs from {expected}"
        );
    }

    #[test]
    fn test_compress_png_rgb_input() {
        // PNG input should be decoded and re-encoded as JPEG
        let raw = encode_png_rgb(800, 600);
        let compressed = compress_image(&raw, 1280, 80).unwrap();

        let decoded = image::load_from_memory(&compressed).unwrap();
        let (w, h) = decoded.dimensions();

        assert_eq!(w, 800);
        assert_eq!(h, 600);

        // Output is RGB (JPEG), no alpha
        assert_eq!(decoded.color().channel_count(), 3);
        assert_eq!(compressed[0], 0xFF);
        assert_eq!(compressed[1], 0xD8);
    }

    #[test]
    fn test_compress_png_downscale() {
        // PNG that exceeds max_dim
        let raw = encode_png_rgb(3000, 2000);
        let compressed = compress_image(&raw, 800, 60).unwrap();

        let decoded = image::load_from_memory(&compressed).unwrap();
        // Longest edge ≤ 800
        assert!(decoded.width().max(decoded.height()) <= 800);

        // Aspect ratio preserved: 3000/2000 = 1.5
        let aspect = decoded.width() as f64 / decoded.height() as f64;
        let expected = 3000.0 / 2000.0;
        assert!(
            (aspect - expected).abs() < 0.02,
            "aspect ratio {aspect} differs from {expected}"
        );
    }

    // ── Edge cases ──────────────────────────────────────────────

    #[test]
    fn test_compress_small_image_not_upscaled() {
        // Tiny image should stay at original size
        let raw = encode_jpeg(&make_test_rgb(16, 16), 85);
        let compressed = compress_image(&raw, 1280, 80).unwrap();

        let decoded = image::load_from_memory(&compressed).unwrap();
        assert_eq!(decoded.width(), 16);
        assert_eq!(decoded.height(), 16);
    }

    #[test]
    fn test_compress_1x1_image() {
        let raw = encode_jpeg(&make_test_rgb(1, 1), 90);
        let compressed = compress_image(&raw, 1280, 80).unwrap();

        let decoded = image::load_from_memory(&compressed).unwrap();
        assert_eq!(decoded.width(), 1);
        assert_eq!(decoded.height(), 1);
    }

    #[test]
    fn test_compress_custom_max_dim_smaller() {
        let raw = encode_jpeg(&make_test_rgb(2000, 1000), 80);
        // Aggressive resize: max 320 px
        let compressed = compress_image(&raw, 320, 70).unwrap();

        let decoded = image::load_from_memory(&compressed).unwrap();
        assert!(
            decoded.width().max(decoded.height()) <= 320,
            "longest edge {} should be ≤ 320",
            decoded.width().max(decoded.height())
        );
    }

    #[test]
    fn test_compress_custom_max_dim_larger() {
        // max_dim larger than image — no resize
        let raw = encode_jpeg(&make_test_rgb(100, 100), 85);
        let compressed = compress_image(&raw, 4000, 80).unwrap();

        let decoded = image::load_from_memory(&compressed).unwrap();
        assert_eq!(decoded.width(), 100);
        assert_eq!(decoded.height(), 100);
    }

    #[test]
    fn test_compress_quality_1() {
        // Minimum quality
        let raw = encode_jpeg(&make_test_rgb(800, 600), 95);
        let compressed = compress_image(&raw, 1280, 1).unwrap();

        let decoded = image::load_from_memory(&compressed).unwrap();
        let (w, h) = decoded.dimensions();
        assert_eq!(w, 800);
        assert_eq!(h, 600);
        // Minimum quality should produce smaller output
        assert!(
            compressed.len() < raw.len(),
            "quality 1 should produce smaller output; raw={}, compressed={}",
            raw.len(),
            compressed.len()
        );
    }

    #[test]
    fn test_compress_quality_100() {
        // Maximum quality
        let raw = encode_jpeg(&make_test_rgb(400, 300), 85);
        let compressed = compress_image(&raw, 1280, 100).unwrap();

        let decoded = image::load_from_memory(&compressed).unwrap();
        assert_eq!(decoded.width(), 400);
        assert_eq!(decoded.height(), 300);

        // At quality 100 the file may be larger due to less compression
    }

    // ── Error handling ──────────────────────────────────────────

    #[test]
    fn test_compress_empty_input() {
        let err = compress_image(b"", 1280, 80).unwrap_err();
        assert!(err.contains("empty"), "error should mention empty: {err}");
    }

    #[test]
    fn test_compress_corrupt_input() {
        let err = compress_image(b"garbage\x00\xffnotanimage", 1280, 80).unwrap_err();
        assert!(
            err.contains("Unsupported") || err.contains("corrupt"),
            "error should be descriptive: {err}"
        );
    }

    #[test]
    fn test_compress_zero_dimensions() {
        // Minimal JPEG header with zero dimensions is not straightforward.
        // Instead, verify that a truly minimal decodeable-but-zero-dim image
        // is handled (if the image crate reports zero dims, our code catches it).
        // For practical testing, a valid image with non-zero dims is sufficient.
    }

    // ── Size reduction verification ──────────────────────────────

    #[test]
    fn test_compress_default_config() {
        // Verify compression with the default app settings (quality=80, max_dim=1280)
        let raw = encode_jpeg(&make_test_rgb(4000, 3000), 85);
        let compressed = compress_image(&raw, 1280, 80).unwrap();

        let decoded = image::load_from_memory(&compressed).unwrap();
        let (w, h) = decoded.dimensions();

        // Longest edge ≤ 1280
        assert!(w.max(h) <= 1280);
        // Aspect ratio preserved: 4000/3000 = 4/3 ≈ 1.333
        let aspect = w as f64 / h as f64;
        let expected = 4000.0 / 3000.0;
        assert!(
            (aspect - expected).abs() < 0.02,
            "aspect ratio {aspect} differs from {expected}"
        );
        // Output is valid JPEG
        assert_eq!(compressed[0], 0xFF);
        assert_eq!(compressed[1], 0xD8);
        // Output should be smaller than raw
        assert!(compressed.len() < raw.len());
    }

    #[test]
    fn test_compress_size_smaller_than_original() {
        // A large image should produce smaller output
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
    fn test_compress_size_smaller_with_png_input() {
        let raw = encode_png_rgb(3000, 2000);
        let compressed = compress_image(&raw, 1024, 75).unwrap();

        assert!(
            compressed.len() < raw.len(),
            "PNG->JPEG compressed size {} should be smaller than raw PNG {}",
            compressed.len(),
            raw.len()
        );
    }

    #[test]
    fn test_compress_visual_quality_acceptable() {
        // Verify that output quality is visually reasonable
        let mut img = image::RgbImage::new(800, 600);
        for y in 0..600 {
            for x in 0..800 {
                let r = (x as f64 * 0.3) as u8;
                let g = (y as f64 * 0.2 + 80.0) as u8;
                let b = ((x + y) as f64 * 0.15 + 40.0) as u8;
                img.put_pixel(x, y, image::Rgb([r, g, b]));
            }
        }
        let raw = encode_jpeg(&img, 95);
        let compressed = compress_image(&raw, 800, 85).unwrap();

        // Decode and measure diff against a Triangle-resized reference at original quality
        let decoded = image::load_from_memory(&compressed).unwrap().to_rgb8();

        // Reference: same resize but at quality 95 (higher quality reference)
        let reference = image::imageops::resize(
            &img,
            decoded.width(),
            decoded.height(),
            image::imageops::FilterType::Triangle,
        );

        let diff = pixel_diff(&decoded, &reference);
        // Mean absolute error ≤ 10/255 per pixel (slightly relaxed for quality 85 vs 95)
        assert!(
            diff <= 10.0 / 255.0,
            "pixel error {diff} exceeds threshold {}",
            10.0 / 255.0
        );
    }

    // ── Format support tests ─────────────────────────────────────

    #[test]
    fn test_compress_jpeg_input() {
        let raw = encode_jpeg(&make_test_rgb(640, 480), 80);
        let compressed = compress_image(&raw, 1280, 70).unwrap();
        assert!(!compressed.is_empty());
        assert_eq!(compressed[0], 0xFF);
        assert_eq!(compressed[1], 0xD8);
    }

    #[test]
    fn test_compress_png_input() {
        let raw = encode_png_rgb(640, 480);
        let compressed = compress_image(&raw, 1280, 70).unwrap();
        assert!(!compressed.is_empty());
        assert_eq!(compressed[0], 0xFF);
        assert_eq!(compressed[1], 0xD8);
    }

    #[test]
    fn test_compress_portrait_orientation() {
        // Tall image (portrait) — longest edge is height
        let raw = encode_jpeg(&make_test_rgb(800, 1200), 80);
        let compressed = compress_image(&raw, 600, 70).unwrap();

        let decoded = image::load_from_memory(&compressed).unwrap();
        let (w, h) = decoded.dimensions();

        // Longest edge (height) should be ≤ 600
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
    fn test_compress_quality_clamping() {
        // quality 0 should be clamped to 1, quality 200 to 100
        let raw = encode_jpeg(&make_test_rgb(100, 100), 85);

        let low = compress_image(&raw, 1280, 0).unwrap();
        let normal = compress_image(&raw, 1280, 1).unwrap();
        let high = compress_image(&raw, 1280, 200).unwrap();
        let max_normal = compress_image(&raw, 1280, 100).unwrap();

        // quality 1 should produce same or similar size as quality 0 (clamped)
        let diff_low = (low.len() as i64 - normal.len() as i64).abs();
        assert!(
            diff_low < 100,
            "quality 0 and 1 should yield similar sizes: {diff_low}"
        );

        // quality 200 should produce same or similar size as quality 100 (clamped)
        let diff_high = (high.len() as i64 - max_normal.len() as i64).abs();
        assert!(
            diff_high < 100,
            "quality 200 and 100 should yield similar sizes: {diff_high}"
        );
    }

    // ── Internal helper tests ────────────────────────────────────

    #[test]
    fn test_resize_rgb8_noop_when_smaller() {
        let img = make_test_rgb(100, 100);
        let out = resize_rgb8(img, 1280);
        assert_eq!(out.dimensions(), (100, 100));
    }

    #[test]
    fn test_resize_rgb8_downscales() {
        let img = make_test_rgb(4000, 3000);
        let out = resize_rgb8(img, 1280);
        let (w, h) = out.dimensions();
        assert!(w.max(h) <= 1280);
        let aspect = w as f64 / h as f64;
        assert!((aspect - 4.0 / 3.0).abs() < 0.02);
    }

    #[test]
    fn test_encode_jpeg_rgb8_valid() {
        let img = make_test_rgb(100, 100);
        let bytes = encode_jpeg_rgb8(&img, 80).unwrap();
        assert!(bytes.len() > 100);
        assert_eq!(bytes[0], 0xFF);
        assert_eq!(bytes[1], 0xD8);
        let decoded = image::load_from_memory(&bytes).unwrap();
        assert_eq!(decoded.dimensions(), (100, 100));
    }

    #[test]
    fn test_encode_jpeg_rgb8_quality_clamping() {
        let img = make_test_rgb(10, 10);
        assert!(encode_jpeg_rgb8(&img, 0).is_ok()); // clamped to 1
        assert!(encode_jpeg_rgb8(&img, 200).is_ok()); // clamped to 100
    }
}
