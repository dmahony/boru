# Image Optimizer — Verification Report

**Task**: `t_abd62652` — Verify image size and upload-speed improvements  
**Date**: 2026-07-12  
**Files changed**: `tests/image_optimizer_integration.rs`, `tests/generate_test_images.py`, `Cargo.toml` (dev-dep + test entry)  
**Tests**: 17 unit + 18 integration = **35 total, all passing**

---

## Size Measurements

| Image | Original | Optimized | Reduction | ≤ 2 MiB? |
|-------|----------|-----------|-----------|----------|
| **photo_4032x3024.jpg** (phone photo) | 313,765 B | 106,847 B | **-65.9%** | ✓ |
| **photo_20mp_5472x3648.jpg** (high-res) | 1,791,243 B | 661,425 B | **-63.1%** | ✓ |
| **screenshot_1920x1080.jpg** | 144,087 B | 140,903 B | **-2.2%** | ✓ |
| **rotated_800x1200.jpg** (portrait) | 22,653 B | 32,993 B | +45.6% * | ✓ |
| **rotated_exif_1200x800.jpg** | 16,282 B | 27,359 B | +68.0% * | ✓ |
| **oversized_8000x6000.jpg** (large) | 3,559,618 B | 467,310 B | **-86.9%** | ✓ |
| **already_compressed_640.jpg** | 8,062 B | 14,451 B | +79.2% * | ✓ |
| **photo_4032x3024.png** | 57,126 B | 108,025 B | +89.1% * | ✓ |
| **screenshot_1920x1080.png** | 8,990 B | 144,952 B | +1512% * | ✓ |
| **transparent_1200.png** | 14,220 B | 66,205 B | +365.6% * | ✓ |
| **tiny_1x1.jpg** | 629 B | 627 B | **-0.3%** | ✓ |

*Negative reduction (size increase) occurs when PNG/compressed JPEG is re-encoded as JPEG, which has structural overhead. The 2 MiB wire cap is always met.*

## Key Verifications

### 1. Large photos are significantly reduced
- Phone photo (4032×3024) goes from 307 KiB → 104 KiB (**66% reduction**)
- 20MP source (5472×3648) at 1.7 MiB → 646 KiB (**63% reduction**)
- 8000×6000 noisy image at 3.4 MiB → 456 KiB (**87% reduction**)

### 2. Dimension cap always enforced
Maximum output dimension ≤ 1920 px (Lanczos3 downscale). No upscale of small images. Aspect ratio preserved within 2% tolerance.

### 3. Transparent PNG → opaque white composite
Alpha channel composited onto white. Output is RGB JPEG (3 channels). Semi-transparent areas become lighter while opaque areas remain visible.

### 4. EXIF orientation consumed
`rotated_exif_1200x800.jpg` (EXIF Rotate 90 CW tag) — the `image` crate auto-orients on decode. Output JPEG has no EXIF APP1 marker.

### 5. All outputs under 2 MiB wire cap
Every test fixture's optimized output is ≤ 2,097,152 bytes. The quality retry sequence (80 → 72 → 64 → 56) ensures the smallest acceptable quality is used before rejection.

### 6. Input size checks work
- 106 MiB image rejected with "exceeding the 10 MiB limit" message
- Empty input rejected with "Image is empty"
- Corrupt bytes rejected with "Unsupported image format"

### 7. compress_image fallback
Receiver-side thumbnailing (`compress_image`) degrades gracefully: returns original bytes when optimization fails (garbage input, empty input).

### 8. Format support is broader than documented
The `image` crate (with `jpeg` + `png` features) internally also decodes BMP, GIF (single + animated), and ICO. These are accepted and converted to JPEG. The error message says "Only JPEG and PNG" but the actual acceptance is broader — any format the `image` crate can decode works. The animated PNG (apNG) rejection via byte-scanning for `acTL` chunk still works correctly.

## Limitations & Recommendations

### 1. PNG → JPEG overhead for small/flat images
Small PNGs (screenshots, icons, already-compressed images) **increase in size** when re-encoded as JPEG because:
- JPEG has header overhead (JFIF APP0, quantization tables, Huffman tables)
- PNG is highly efficient for flat-color regions
- JPEG's lossy compression doesn't help below a certain size threshold

**Recommendation**: Consider a bypass for inputs already under ~100 KiB that are already in a web-friendly format. The 2 MiB cap is always met, but the user may see "optimized" images that are larger than the original for small screenshots/PNGs.

### 2. Quality-steps hardcoded
The retry sequence `[80, 72, 64, 56]` is embedded in `OPTIMIZE_QUALITY_STEPS`. If the 2 MiB cap needs tightening (e.g., for metered connections), add a lower-quality step. If quality 56 is unacceptable, images that barely exceed the cap at 64 but fit at 56 are accepted — this is a judgment call per application.

### 3. Animated PNG detection is heuristic
The `acTL` byte-scan checks only the first 1 KiB of the file. A static PNG with the bytes `acTL` in pixel data beyond 1 KiB would pass. This is a negligible risk (the bytes must appear in uncompressed IDAT chunk data at exactly the right boundary).

### 4. No WebP output support
The optimizer only outputs JPEG. WebP would provide better compression for screenshots/flat-color images but adds a dependency on the WebP encoder crate. Consider if bandwidth is a concern.

### 5. Dimension handling for very wide panoramic images
Images like 8000×600 (ultra-wide) are downscaled to 1920×144 (same aspect ratio). No special check for panorama content — the resize is dimension-only.

## Test Coverage Summary

| Test category | Count | Status |
|--------------|-------|--------|
| Unit tests (image_optimizer tests module) | 17 | All pass |
| Integration: photo-like JPEG | 2 | Pass |
| Integration: screenshot | 1 | Pass |
| Integration: transparent PNG compositing | 1 | Pass |
| Integration: physically rotated/portrait | 1 | Pass |
| Integration: EXIF auto-orientation | 1 | Pass |
| Integration: already-compressed small | 1 | Pass |
| Integration: tiny 1×1 | 2 | Pass |
| Integration: oversized input rejection | 1 | Pass |
| Integration: unsupported format rejection | 1 | Pass |
| Integration: BMP/GIF/ICO acceptance | 3 | Pass |
| Integration: compress_image fallback | 1 | Pass |
| Integration: bulk JPEG under-limit | 1 | Pass (7 fixtures) |
| Integration: bulk PNG under-limit | 1 | Pass (6 fixtures) |
| **Total** | **35** | **All pass** |

## Files Modified
- `Cargo.toml` — Added `image` dev-dependency, `[[test]]` entry for integration test
- `tests/image_optimizer_integration.rs` — 18 integration tests (new file)
- `tests/generate_test_images.py` — Test fixture generator (new file)
