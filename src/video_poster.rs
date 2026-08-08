//! Bounded, content-addressed poster generation for verified local videos.

use std::path::{Path, PathBuf};

/// Maximum encoded poster size kept in the local cache.
pub const MAX_POSTER_BYTES: usize = 512 * 1024;
/// Maximum poster edge sent to the GUI/image decoder.
pub const MAX_POSTER_EDGE: u32 = 320;
/// Maximum decoded edge accepted for a poster before it is handed to the
/// image decoder. Our own generated posters are capped at `MAX_POSTER_EDGE`
/// (320 px); a 4x headroom rejects decompression-bomb posters from a
/// misbehaving sender while leaving every legitimate preview untouched
/// (VIDCARD-18 guardrail: bound dimensions before allocating preview
/// resources).
pub const MAX_POSTER_DECODED_EDGE: u32 = MAX_POSTER_EDGE * 4;

/// True when decoded poster dimensions are inside the accepted bounds and
/// safe to hand to the image decoder. `None` (unparseable header) or any
/// non-positive / oversized dimension is rejected so a hostile sender
/// cannot force a large surface allocation through the preview path.
pub fn dimensions_within_bounds(dimensions: Option<(u32, u32)>) -> bool {
    dimensions.is_some_and(|(width, height)| {
        width > 0
            && height > 0
            && width <= MAX_POSTER_DECODED_EDGE
            && height <= MAX_POSTER_DECODED_EDGE
    })
}
/// Maximum input size allowed for the optional poster probe.
pub const MAX_POSTER_INPUT_BYTES: u64 = 512 * 1024 * 1024;
/// ffmpeg scale filter for poster extraction.
///
/// `min(320, iw)` keeps the poster at its intrinsic width when the source
/// is smaller than the cap, so tiny videos are never upscaled. `-2` derives
/// the height from the width while preserving the aspect ratio.
pub const POSTER_SCALE_FILTER: &str = "scale='min(320,iw)':-2";
/// Explicitly apply rotation metadata during poster extraction. ffmpeg
/// applies `autorotate` by default for video inputs, but being explicit
/// keeps orientation correct even if the input stream carries a display
/// matrix from a phone/tablet capture.
pub const POSTER_AUTOROTATE: &str = "-autorotate";

#[derive(Clone, Debug, PartialEq, Eq)]
/// A cached poster and its decoded dimensions.
pub struct Poster {
    /// Bounded WebP bytes suitable for an Iced image handle.
    pub bytes: Vec<u8>,
    /// Dimensions decoded from the poster, when available.
    pub dimensions: Option<(u32, u32)>,
    /// Content-addressed cache path used for this poster.
    pub cache_path: PathBuf,
}

/// Return the cache filename for a file's content, never its display name.
pub fn cache_key(content: &[u8]) -> String {
    blake3::hash(content).to_hex().to_string()
}

/// Probe a verified local file and cache one bounded WebP poster.
///
/// This function is intentionally blocking; callers must run it in a
/// `spawn_blocking` task so media probing never runs in the Iced update loop.
pub fn generate(path: &Path, cache_dir: &Path) -> Result<Poster, String> {
    generate_inner(path, cache_dir, None)
}

/// Like [`generate`], but with the file's content hash already known.
///
/// The chat send path knows the video's iroh blob hash (BLAKE3 of the
/// content) before generating the poster, so it can skip the full-file read
/// that [`generate`] uses to derive the cache key. The key format is
/// identical (BLAKE3 hex of the file bytes), so both callers share one
/// cache namespace and existing cached posters stay valid.
pub fn generate_with_content_hash(
    path: &Path,
    cache_dir: &Path,
    content_hash: &iroh_blobs::Hash,
) -> Result<Poster, String> {
    generate_inner(path, cache_dir, Some(content_hash))
}

fn generate_inner(
    path: &Path,
    cache_dir: &Path,
    content_hash: Option<&iroh_blobs::Hash>,
) -> Result<Poster, String> {
    let input_size = std::fs::metadata(path)
        .map_err(|e| format!("inspect video: {e}"))?
        .len();
    if input_size == 0 || input_size > MAX_POSTER_INPUT_BYTES {
        return Err("video is outside the poster probe size limit".to_string());
    }
    let key = match content_hash {
        // The blob hash is BLAKE3 of the file bytes — exactly what
        // `cache_key` computes, but without re-reading the file.
        Some(hash) => blake3::Hash::from_bytes(*hash.as_bytes())
            .to_hex()
            .to_string(),
        None => {
            let bytes = std::fs::read(path).map_err(|e| format!("read video: {e}"))?;
            cache_key(&bytes)
        }
    };
    let cache_path = cache_dir.join(format!("{key}.webp"));
    if let Ok(cached) = std::fs::read(&cache_path) {
        if !cached.is_empty() && cached.len() <= MAX_POSTER_BYTES {
            return Ok(Poster {
                dimensions: dimensions(&cached),
                bytes: cached,
                cache_path,
            });
        }
    }

    std::fs::create_dir_all(cache_dir).map_err(|e| format!("create poster cache: {e}"))?;
    let output = std::process::Command::new("ffmpeg")
        // `-autorotate` is an INPUT option in ffmpeg >= 6: it must precede
        // `-i`, otherwise ffmpeg exits 234 ("cannot be applied to output
        // url") and the poster probe fails on every call.
        .args([POSTER_AUTOROTATE, "-ss", "0.5", "-i"])
        .arg(path)
        .args([
            "-frames:v",
            "1",
            "-vf",
            POSTER_SCALE_FILTER,
            "-f",
            "image2pipe",
            "-c:v",
            "libwebp",
            "-quality",
            "80",
            "-threads",
            "1",
            "-timelimit",
            "10",
            "-v",
            "error",
            "-",
        ])
        .output()
        .map_err(|e| format!("start ffmpeg: {e}"))?;
    if !output.status.success() || output.stdout.is_empty() {
        let detail = String::from_utf8_lossy(&output.stderr);
        return Err(format!("ffmpeg poster probe failed: {}", detail.trim()));
    }
    if output.stdout.len() > MAX_POSTER_BYTES {
        return Err(format!("poster exceeds {} bytes", MAX_POSTER_BYTES));
    }
    let tmp_path = cache_path.with_extension("webp.tmp");
    std::fs::write(&tmp_path, &output.stdout).map_err(|e| format!("write poster: {e}"))?;
    std::fs::rename(&tmp_path, &cache_path).map_err(|e| format!("publish poster: {e}"))?;
    Ok(Poster {
        dimensions: dimensions(&output.stdout),
        bytes: output.stdout,
        cache_path,
    })
}

fn dimensions(bytes: &[u8]) -> Option<(u32, u32)> {
    let dimensions = image::ImageReader::new(std::io::Cursor::new(bytes))
        .with_guessed_format()
        .ok()?
        .into_dimensions()
        .ok()?;
    (dimensions.0 > 0
        && dimensions.1 > 0
        && dimensions.0 <= MAX_POSTER_DECODED_EDGE
        && dimensions.1 <= MAX_POSTER_DECODED_EDGE)
        .then_some(dimensions)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cache_key_is_content_based_not_filename_based() {
        assert_eq!(cache_key(b"same content"), cache_key(b"same content"));
        assert_ne!(cache_key(b"video-a"), cache_key(b"video-b"));
    }

    #[test]
    fn poster_limits_are_bounded() {
        assert_eq!(MAX_POSTER_EDGE, 320);
        assert_eq!(MAX_POSTER_BYTES, 512 * 1024);
        assert_eq!(MAX_POSTER_INPUT_BYTES, 512 * 1024 * 1024);
        assert_eq!(MAX_POSTER_DECODED_EDGE, 1280);
    }

    #[test]
    fn decoded_dimension_bounds_accept_legitimate_posters() {
        // Our own generated posters are capped at MAX_POSTER_EDGE (320px);
        // anything at or under the 4x headroom is a legitimate preview.
        assert!(dimensions_within_bounds(Some((320, 180))));
        assert!(dimensions_within_bounds(Some((1, 1))));
        assert!(dimensions_within_bounds(Some((
            MAX_POSTER_DECODED_EDGE,
            MAX_POSTER_DECODED_EDGE
        ))));
    }

    #[test]
    fn decoded_dimension_bounds_reject_hostile_posters() {
        // A misbehaving sender must not force a large surface allocation
        // through the preview path: non-positive, unparseable, or oversized
        // decoded dimensions are all rejected before the image decoder runs.
        assert!(!dimensions_within_bounds(None));
        assert!(!dimensions_within_bounds(Some((0, 100))));
        assert!(!dimensions_within_bounds(Some((100, 0))));
        assert!(!dimensions_within_bounds(Some((
            MAX_POSTER_DECODED_EDGE + 1,
            100
        ))));
        assert!(!dimensions_within_bounds(Some((
            100,
            MAX_POSTER_DECODED_EDGE + 1
        ))));
        assert!(!dimensions_within_bounds(Some((u32::MAX, 100))));
        assert!(!dimensions_within_bounds(Some((100, u32::MAX))));
    }

    #[test]
    fn poster_scale_filter_never_upscales_tiny_videos() {
        // `min(320, iw)` keeps a 64px-wide source at 64px instead of blowing
        // it up to the 320px cap. Height `-2` preserves the aspect ratio.
        assert!(POSTER_SCALE_FILTER.contains("min(320,iw)"));
        assert!(POSTER_SCALE_FILTER.contains("-2"));
        assert!(!POSTER_SCALE_FILTER.contains("iw*"));
        assert!(!POSTER_SCALE_FILTER.contains("320:320"));
    }

    #[test]
    fn poster_scale_filter_preserves_aspect_ratio() {
        // `-2` (even height derived from width) keeps the intrinsic ratio;
        // a hard-coded height pair would squash portrait/landscape videos.
        assert!(!POSTER_SCALE_FILTER.contains(":1080"));
        assert!(!POSTER_SCALE_FILTER.contains(":720"));
    }

    #[test]
    fn poster_extraction_applies_rotation_metadata() {
        // Orientation metadata (phone/tablet captures) must be honoured so
        // portrait videos do not appear sideways in the card.
        assert_eq!(POSTER_AUTOROTATE, "-autorotate");
    }
}
