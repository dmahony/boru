//! Conservative classification of downloadable attachments.

/// The rendering category for an attachment.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MediaKind {
    /// The attachment is intended for inline video rendering.
    Video,
    /// Keep the generic file/download behaviour.
    NonVideo,
}

const VIDEO_EXTENSIONS: &[&str] = &[
    "3gp", "avi", "flv", "m4v", "mkv", "mov", "mp4", "webm", "wmv",
];

/// Classify an attachment using normalized MIME metadata when it is available.
///
/// A video MIME type is accepted only when the filename is absent or has a
/// supported video extension. A known non-video MIME type, an unsupported
/// extension, or contradictory metadata remains a generic file. If MIME data
/// is absent, the extension is used as a conservative fallback.
pub fn classify_attachment(mime_type: Option<&str>, filename: &str) -> MediaKind {
    let extension = filename
        .rsplit_once('.')
        .map(|(_, extension)| extension.trim().to_ascii_lowercase());
    let extension_is_video = extension
        .as_deref()
        .is_some_and(|extension| VIDEO_EXTENSIONS.contains(&extension));
    let has_extension = extension.is_some();

    let mime = mime_type
        .map(str::trim)
        .filter(|mime| !mime.is_empty())
        .map(str::to_ascii_lowercase);

    match mime.as_deref() {
        Some(mime) if mime.starts_with("video/") => {
            if !has_extension || extension_is_video {
                MediaKind::Video
            } else {
                MediaKind::NonVideo
            }
        }
        Some(_) => MediaKind::NonVideo,
        None if extension_is_video => MediaKind::Video,
        None => MediaKind::NonVideo,
    }
}

#[cfg(test)]
mod tests {
    use super::{classify_attachment, MediaKind};

    #[test]
    fn common_video_extensions_are_supported_case_insensitively() {
        for filename in ["clip.mp4", "clip.WEBM", "clip.MOV", "clip.mkv"] {
            assert_eq!(classify_attachment(None, filename), MediaKind::Video);
        }
    }

    #[test]
    fn normalized_video_mime_is_used_without_an_extension() {
        assert_eq!(
            classify_attachment(Some(" Video/MP4 "), "clip"),
            MediaKind::Video
        );
    }

    #[test]
    fn known_non_video_mime_overrides_a_video_looking_name() {
        assert_eq!(
            classify_attachment(Some("image/png"), "clip.MP4"),
            MediaKind::NonVideo
        );
    }

    #[test]
    fn contradictory_video_mime_and_extension_stays_generic() {
        assert_eq!(
            classify_attachment(Some("video/mp4"), "notes.txt"),
            MediaKind::NonVideo
        );
    }

    #[test]
    fn unknown_or_missing_metadata_stays_generic() {
        assert_eq!(
            classify_attachment(None, "archive.unknown"),
            MediaKind::NonVideo
        );
        assert_eq!(classify_attachment(Some(""), "archive"), MediaKind::NonVideo);
    }
}
