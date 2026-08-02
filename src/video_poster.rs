//! Bounded, content-addressed poster generation for verified local videos.

use std::path::{Path, PathBuf};

/// Maximum encoded poster size kept in the local cache.
pub const MAX_POSTER_BYTES: usize = 512 * 1024;
/// Maximum poster edge sent to the GUI/image decoder.
pub const MAX_POSTER_EDGE: u32 = 320;
/// Maximum input size allowed for the optional poster probe.
pub const MAX_POSTER_INPUT_BYTES: u64 = 512 * 1024 * 1024;

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
    let input_size = std::fs::metadata(path)
        .map_err(|e| format!("inspect video: {e}"))?
        .len();
    if input_size == 0 || input_size > MAX_POSTER_INPUT_BYTES {
        return Err("video is outside the poster probe size limit".to_string());
    }
    let bytes = std::fs::read(path).map_err(|e| format!("read video: {e}"))?;
    let key = cache_key(&bytes);
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
        .args(["-ss", "0.5", "-i"])
        .arg(path)
        .args([
            "-frames:v",
            "1",
            "-vf",
            "scale='min(320,iw)':-2",
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
        && dimensions.0 <= MAX_POSTER_EDGE * 4
        && dimensions.1 <= MAX_POSTER_EDGE * 4)
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
    }
}
