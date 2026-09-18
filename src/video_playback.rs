//! Reusable video metadata and ephemeral inline-player state.
//!
//! [`MediaMetadata`](crate::video_playback::MediaMetadata) is the small, serializable description that may be
//! attached to a stored message.  Decoder handles, widget state, local paths,
//! and playback position deliberately do not belong here: they are process
//! local and are represented by [`PlayerState`](crate::video_playback::PlayerState) and [`PlaybackCoordinator`](crate::video_playback::PlaybackCoordinator).

use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

use crate::proto::TopicId;

/// Maximum local video size admitted to the inline decoder path.
pub const MAX_INLINE_VIDEO_BYTES: u64 = 4 * 1024 * 1024 * 1024;

/// Reject peer-controlled names before they are joined to the downloads root.
pub fn validate_attachment_filename(name: &str) -> Result<(), String> {
    let path = Path::new(name);
    if name.is_empty()
        || name.contains('\0')
        || name.chars().any(char::is_control)
        || path.is_absolute()
        || path.components().count() != 1
        || path.file_name().and_then(|n| n.to_str()) != Some(name)
        || matches!(name, "." | "..")
    {
        return Err("attachment filename is not a safe local name".to_string());
    }
    Ok(())
}

/// Revalidate a completed attachment immediately before decoder creation.
///
/// The caller supplies the Boru-managed downloads root and the content hash
/// from the signed/blob ticket.  The path is canonicalised to prevent a
/// replaced symlink from escaping the managed directory, then hashed from the
/// local file; extension, MIME, and display names are never used as identity.
pub fn verify_local_attachment(
    path: &Path,
    managed_root: &Path,
    expected_hash: &str,
    expected_size: Option<u64>,
) -> Result<PathBuf, String> {
    verify_local_attachment_impl(path, managed_root, expected_hash, expected_size, true)
}

/// Like [`verify_local_attachment`] but with the managed-downloads-root
/// containment check relaxed.
///
/// `require_managed_root=false` is for the SENDER's own shared cards
/// (`DownloadState::Shared`): their path is the user-selected source file,
/// which legitimately lives outside the downloads directory. Identity
/// (size + BLAKE3 hash) is still fully verified — only the "must live under
/// the managed root" rule is skipped.
pub fn verify_local_attachment_unmanaged(
    path: &Path,
    managed_root: &Path,
    expected_hash: &str,
    expected_size: Option<u64>,
) -> Result<PathBuf, String> {
    verify_local_attachment_impl(path, managed_root, expected_hash, expected_size, false)
}
/// Recover a stale GUI transfer-size cache only after verifying the entire
/// file against its content identity. Never use this for a protocol size claim.
pub fn verified_completed_attachment_size(
    path: &Path,
    managed_root: &Path,
    expected_hash: &str,
) -> Result<u64, String> {
    let canonical = verify_local_attachment(path, managed_root, expected_hash, None)?;
    std::fs::metadata(canonical)
        .map(|metadata| metadata.len())
        .map_err(|error| format!("cannot inspect verified attachment: {error}"))
}

fn verify_local_attachment_impl(
    path: &Path,
    managed_root: &Path,
    expected_hash: &str,
    expected_size: Option<u64>,
    require_managed_root: bool,
) -> Result<PathBuf, String> {
    if expected_hash.len() != 64 || !expected_hash.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err("attachment has no valid content identity".to_string());
    }
    let canonical = path
        .canonicalize()
        .map_err(|e| format!("attachment file is missing: {e}"))?;
    if require_managed_root {
        let canonical_root = managed_root
            .canonicalize()
            .map_err(|e| format!("managed downloads directory unavailable: {e}"))?;
        if !canonical.starts_with(&canonical_root) {
            return Err("attachment path escapes the managed downloads directory".to_string());
        }
    }
    let metadata =
        std::fs::metadata(&canonical).map_err(|e| format!("cannot inspect attachment: {e}"))?;
    if !metadata.is_file() {
        return Err("attachment is not a regular file".to_string());
    }
    if metadata.len() == 0 || metadata.len() > MAX_INLINE_VIDEO_BYTES {
        return Err("attachment size is outside the inline playback limit".to_string());
    }
    if expected_size.is_some_and(|size| size != metadata.len()) {
        return Err("attachment size does not match its verified identity".to_string());
    }
    let mut file =
        std::fs::File::open(&canonical).map_err(|e| format!("cannot open attachment: {e}"))?;
    let mut hasher = blake3::Hasher::new();
    let mut buffer = [0u8; 64 * 1024];
    loop {
        let read = file
            .read(&mut buffer)
            .map_err(|e| format!("cannot verify attachment: {e}"))?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    let actual = hasher.finalize().to_hex().to_string();
    if !actual.eq_ignore_ascii_case(expected_hash) {
        return Err("attachment content identity does not match the verified hash".to_string());
    }
    Ok(canonical)
}

/// Maximum local file size admitted to the metadata probe.
///
/// Mirrors the poster-probe bound; the probe itself is a header-only read so
/// this guards against pathological inputs rather than actual memory use.
pub const MAX_METADATA_PROBE_BYTES: u64 = 512 * 1024 * 1024;

/// Probe a verified local video for intrinsic width, height, and duration.
///
/// Runs `ffprobe` (the same media toolchain used by the poster probe) with a
/// supervisor-enforced wall-clock timeout and a bounded input size. The result never fabricates
/// measurements: width/height/duration that the container does not expose
/// remain `None`, and the caller is expected to fall back to a bounded
/// generic media frame. This function is intentionally blocking; callers must
/// run it in a `spawn_blocking` task so media probing never runs in the Iced
/// update loop.
pub fn probe_local_video_metadata(path: &Path) -> Result<MediaMetadata, String> {
    let input_size = std::fs::metadata(path)
        .map_err(|e| format!("inspect video: {e}"))?
        .len();
    if input_size == 0 || input_size > MAX_METADATA_PROBE_BYTES {
        return Err("video is outside the metadata probe size limit".to_string());
    }
    let mut command = Command::new("ffprobe");
    command
        .args([
            "-v",
            "error",
            "-select_streams",
            "v:0",
            "-show_entries",
            "stream=width,height",
            "-show_entries",
            "format=duration",
            "-of",
            "default=noprint_wrappers=1:nokey=1",
        ])
        .arg(path);
    let output = run_command_with_timeout(&mut command, Duration::from_secs(10), "ffprobe")?;
    if !output.status.success() {
        let detail = String::from_utf8_lossy(&output.stderr);
        return Err(format!("ffprobe metadata probe failed: {}", detail.trim()));
    }
    Ok(parse_metadata_output(&String::from_utf8_lossy(
        &output.stdout,
    )))
}

/// Run a media subprocess with a hard wall-clock bound.
///
/// Both pipes are drained concurrently so noisy media tools cannot deadlock
/// on a full pipe. A timed-out child is killed and waited for before returning,
/// ensuring that it is reaped.
pub(crate) fn run_command_with_timeout(
    command: &mut Command,
    timeout: Duration,
    program: &str,
) -> Result<Output, String> {
    command.stdout(Stdio::piped()).stderr(Stdio::piped());
    let mut child = command
        .spawn()
        .map_err(|error| format!("start {program}: {error}"))?;
    let mut stdout = child.stdout.take().expect("piped stdout");
    let mut stderr = child.stderr.take().expect("piped stderr");
    let stdout_thread = thread::spawn(move || {
        let mut bytes = Vec::new();
        let _ = stdout.read_to_end(&mut bytes);
        bytes
    });
    let stderr_thread = thread::spawn(move || {
        let mut bytes = Vec::new();
        let _ = stderr.read_to_end(&mut bytes);
        bytes
    });
    let deadline = Instant::now() + timeout;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if Instant::now() >= deadline => {
                let _ = child.kill();
                let _ = child.wait();
                let _ = stdout_thread.join();
                let _ = stderr_thread.join();
                return Err(format!("{program} probe timed out after {timeout:?}"));
            }
            Ok(None) => thread::sleep(Duration::from_millis(10)),
            Err(error) => {
                let _ = child.kill();
                let _ = child.wait();
                let _ = stdout_thread.join();
                let _ = stderr_thread.join();
                return Err(format!("wait for {program}: {error}"));
            }
        }
    };
    let stdout = stdout_thread
        .join()
        .map_err(|_| format!("read {program} stdout"))?;
    let stderr = stderr_thread
        .join()
        .map_err(|_| format!("read {program} stderr"))?;
    Ok(Output {
        status,
        stdout,
        stderr,
    })
}

/// Parse the plain-text `ffprobe` output (`width\nheight\nduration`).
///
/// Handles the observed variants: video stream → `w\nh\ndur`; audio-only or
/// missing video stream → `dur`; missing duration → `w\nh`. Unknown values
/// stay `None` so callers can fall back to a bounded generic frame.
fn parse_metadata_output(output: &str) -> MediaMetadata {
    let values: Vec<&str> = output
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && *line != "N/A")
        .collect();
    let (width, height, duration_ms) = match values.as_slice() {
        [w, h, d, ..] => (
            w.parse::<u32>().ok().filter(|v| *v > 0),
            h.parse::<u32>().ok().filter(|v| *v > 0),
            d.parse::<f64>()
                .ok()
                .filter(|v| *v > 0.0)
                .map(|seconds| (seconds * 1000.0) as u64),
        ),
        [w, h] => (
            w.parse::<u32>().ok().filter(|v| *v > 0),
            h.parse::<u32>().ok().filter(|v| *v > 0),
            None,
        ),
        [d, ..] => (
            None,
            None,
            d.parse::<f64>()
                .ok()
                .filter(|v| *v > 0.0)
                .map(|seconds| (seconds * 1000.0) as u64),
        ),
        _ => (None, None, None),
    };
    MediaMetadata {
        duration_ms,
        width,
        height,
        media_type: MediaType::Video,
        probe_status: ProbeStatus::Ready,
        ..Default::default()
    }
}

/// Media classification recorded with an attachment when it is known.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum MediaType {
    /// A video suitable for inline playback.
    Video,
    /// An image attachment (kept for callers sharing the metadata shape).
    Image,
    /// An audio attachment.
    Audio,
    /// A type not handled by the inline player.
    Other(String),
}

impl Default for MediaType {
    fn default() -> Self {
        Self::Other("application/octet-stream".to_string())
    }
}

/// State of probing an attachment for reusable media information.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum ProbeStatus {
    /// No probe has been attempted, or the source is not locally available.
    #[default]
    Unknown,
    /// A local probe is in progress.  This is normally ephemeral and should
    /// not be persisted, but is accepted for forward-compatible data.
    Probing,
    /// The metadata fields are known and may be used by the UI.
    Ready,
    /// Probing failed; playback may still offer a download/retry action.
    Failed,
}

/// Durable, optional media information for an attachment.
///
/// All measurements are optional because old messages, incomplete downloads,
/// and formats that do not expose a duration or dimensions remain valid.
/// `poster_reference` is an attachment/content-store identifier, never an
/// absolute operating-system path.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct MediaMetadata {
    /// Duration in milliseconds, when known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duration_ms: Option<u64>,
    /// Encoded video width in pixels, when known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub width: Option<u32>,
    /// Encoded video height in pixels, when known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub height: Option<u32>,
    /// Content-store identifier for a poster frame, when available.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub poster_reference: Option<String>,
    /// MIME/media classification.
    #[serde(default)]
    pub media_type: MediaType,
    /// Result of local metadata probing.
    #[serde(default)]
    pub probe_status: ProbeStatus,
}

/// Stable identity for one inline video player.
///
/// The conversation and message identity prevent collisions between rooms;
/// `attachment_id` distinguishes multiple attachments on one message.  It is
/// normally a content hash or storage attachment id, not a list position.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct VideoInstanceKey {
    /// Stable conversation/topic identity.
    pub conversation_id: TopicId,
    /// Stable message/event identity within the conversation.
    pub message_id: u64,
    /// Stable attachment identity (content hash or attachment row id).
    pub attachment_id: String,
}

impl VideoInstanceKey {
    /// Construct a key from the conversation, message event, and attachment.
    pub fn new(
        conversation_id: TopicId,
        message_id: u64,
        attachment_id: impl Into<String>,
    ) -> Self {
        Self {
            conversation_id,
            message_id,
            attachment_id: attachment_id.into(),
        }
    }
}

/// Ephemeral state of an inline player.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PlayerState {
    /// No decoder has been requested.
    Idle,
    /// A decoder/player is being prepared for the local verified file.
    Preparing,
    /// Playback is currently advancing.
    Playing,
    /// Playback is paused at the current position.
    Paused,
    /// Playback reached its end.
    Ended,
    /// Playback or preparation failed; the message remains usable.
    Failed {
        /// Human-readable failure detail for recovery UI.
        error: String,
    },
}

impl Default for PlayerState {
    fn default() -> Self {
        Self::Idle
    }
}

/// Process-local policy and coordination for inline playback.
///
/// There is at most one active video key.  Starting a different key returns
/// the previously active key so the caller can pause/release its player.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PlaybackCoordinator {
    active_video: Option<VideoInstanceKey>,
    /// Whether starting a new video should pause the old one.
    pub pause_on_new_play: bool,
}

impl Default for PlaybackCoordinator {
    fn default() -> Self {
        Self::new()
    }
}

impl PlaybackCoordinator {
    /// Create a coordinator using the normal single-active-video policy.
    pub fn new() -> Self {
        Self {
            active_video: None,
            pause_on_new_play: true,
        }
    }

    /// Return the currently active video, if any.
    pub fn active_video(&self) -> Option<&VideoInstanceKey> {
        self.active_video.as_ref()
    }

    /// Activate a video and return the former active key, if it changed.
    pub fn activate(&mut self, key: VideoInstanceKey) -> Option<VideoInstanceKey> {
        if self.active_video.as_ref() == Some(&key) {
            return None;
        }
        self.active_video.replace(key)
    }

    /// Request playback for `key`, returning the key that must be paused first.
    /// Repeating a request for the already-active key is intentionally a no-op;
    /// the UI owns pause/resume toggling for that player.
    pub fn request_play(&mut self, key: VideoInstanceKey) -> Option<VideoInstanceKey> {
        self.activate(key)
    }

    /// Clear the active video, optionally only when it matches `key`.
    pub fn clear(&mut self, key: Option<&VideoInstanceKey>) {
        if key.is_none() || self.active_video.as_ref() == key {
            self.active_video = None;
        }
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn completed_size_cache_recovery_keeps_strict_identity_checks() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("video.mp4");
        let bytes = b"complete attachment, not a range progress total";
        std::fs::write(&path, bytes).unwrap();
        let hash = blake3::hash(bytes).to_hex().to_string();
        assert!(super::verify_local_attachment(&path, root.path(), &hash, Some(7)).is_err());
        let size = super::verified_completed_attachment_size(&path, root.path(), &hash).unwrap();
        assert_eq!(size, bytes.len() as u64);
        assert!(super::verify_local_attachment(&path, root.path(), &hash, Some(size)).is_ok());
        std::fs::write(&path, b"truncated").unwrap();
        assert!(super::verified_completed_attachment_size(&path, root.path(), &hash).is_err());
    }

    #[test]
    fn completed_size_cache_recovery_rejects_unmanaged_files() {
        let root = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let path = outside.path().join("video.mp4");
        std::fs::write(&path, b"video").unwrap();
        let hash = blake3::hash(b"video").to_hex().to_string();
        assert!(super::verified_completed_attachment_size(&path, root.path(), &hash).is_err());
    }

    use std::fs;

    use super::*;

    fn key(message_id: u64, attachment: &str) -> VideoInstanceKey {
        VideoInstanceKey::new(TopicId::from_bytes([7; 32]), message_id, attachment)
    }

    #[test]
    fn metadata_defaults_keep_partial_media_valid() {
        let metadata: MediaMetadata = serde_json::from_str("{}").unwrap();
        assert_eq!(metadata.duration_ms, None);
        assert_eq!(metadata.width, None);
        assert_eq!(metadata.height, None);
        assert_eq!(metadata.probe_status, ProbeStatus::Unknown);
    }

    #[test]
    fn metadata_defaults_include_generic_media_type() {
        let metadata = MediaMetadata::default();
        assert_eq!(metadata.media_type, MediaType::default());
        assert_eq!(
            metadata.media_type,
            MediaType::Other("application/octet-stream".into())
        );
        assert_eq!(metadata.probe_status, ProbeStatus::Unknown);
    }

    #[test]
    fn metadata_round_trip_preserves_known_video_fields() {
        let metadata = MediaMetadata {
            duration_ms: Some(1_250),
            width: Some(1920),
            height: Some(1080),
            poster_reference: Some("blake3:poster".into()),
            media_type: MediaType::Video,
            probe_status: ProbeStatus::Ready,
        };
        let encoded = serde_json::to_string(&metadata).unwrap();
        let decoded: MediaMetadata = serde_json::from_str(&encoded).unwrap();
        assert_eq!(decoded, metadata);
    }

    #[test]
    fn key_distinguishes_messages_and_attachments() {
        assert_ne!(key(1, "a"), key(2, "a"));
        assert_ne!(key(1, "a"), key(1, "b"));
    }

    #[test]
    fn coordinator_supports_no_active_video_and_replacement() {
        let mut coordinator = PlaybackCoordinator::new();
        assert_eq!(coordinator.active_video(), None);
        assert_eq!(coordinator.activate(key(1, "a")), None);
        assert_eq!(coordinator.activate(key(2, "b")), Some(key(1, "a")));
        coordinator.clear(Some(&key(2, "b")));
        assert_eq!(coordinator.active_video(), None);
    }

    #[test]
    fn repeated_requests_are_idempotent_and_stale_clear_cannot_wipe_new_video() {
        let mut coordinator = PlaybackCoordinator::new();
        let first = key(1, "a");
        let second = key(2, "b");

        assert_eq!(coordinator.request_play(first.clone()), None);
        assert_eq!(coordinator.request_play(first.clone()), None);
        assert_eq!(
            coordinator.request_play(second.clone()),
            Some(first.clone())
        );
        coordinator.clear(Some(&first));
        assert_eq!(coordinator.active_video(), Some(&second));
        coordinator.clear(Some(&second));
        assert_eq!(coordinator.active_video(), None);
    }

    #[test]
    fn player_states_have_stable_default_and_recoverable_failure_shape() {
        assert_eq!(PlayerState::default(), PlayerState::Idle);
        let states = [
            PlayerState::Idle,
            PlayerState::Preparing,
            PlayerState::Playing,
            PlayerState::Paused,
            PlayerState::Ended,
            PlayerState::Failed {
                error: "decoder unavailable".into(),
            },
        ];
        assert!(states
            .iter()
            .any(|state| matches!(state, PlayerState::Preparing)));
        assert!(states
            .iter()
            .any(|state| matches!(state, PlayerState::Playing)));
        assert!(states
            .iter()
            .any(|state| matches!(state, PlayerState::Paused)));
        assert!(states
            .iter()
            .any(|state| matches!(state, PlayerState::Ended)));
        assert!(states.iter().any(
            |state| matches!(state, PlayerState::Failed { error } if error == "decoder unavailable")
        ));
    }

    #[test]
    fn rejects_peer_controlled_filenames() {
        for name in [
            "../clip.mp4",
            "subdir/clip.mp4",
            "/tmp/clip.mp4",
            "https://evil/video",
        ] {
            assert!(
                validate_attachment_filename(name).is_err(),
                "accepted {name:?}"
            );
        }
        assert!(validate_attachment_filename("clip.mp4").is_ok());
    }

    #[test]
    fn rejects_missing_and_partial_files() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("clip.mp4");
        let hash = blake3::hash(b"complete").to_hex().to_string();
        assert!(verify_local_attachment(&path, root.path(), &hash, None).is_err());
        fs::write(&path, b"partial").unwrap();
        assert!(verify_local_attachment(&path, root.path(), &hash, Some(8)).is_err());
    }

    #[test]
    fn rejects_content_identity_mismatch_and_accepts_verified_file() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("clip.mp4");
        fs::write(&path, b"verified bytes").unwrap();
        let expected = blake3::hash(b"verified bytes").to_hex().to_string();
        assert!(verify_local_attachment(&path, root.path(), &expected, Some(14)).is_ok());
        assert!(verify_local_attachment(
            &path,
            root.path(),
            &blake3::hash(b"replaced").to_hex().to_string(),
            None
        )
        .is_err());
    }

    #[test]
    fn parse_metadata_output_reads_dimensions_and_duration() {
        let metadata = parse_metadata_output("320\n240\n2.000000\n");
        assert_eq!(metadata.width, Some(320));
        assert_eq!(metadata.height, Some(240));
        assert_eq!(metadata.duration_ms, Some(2_000));
        assert_eq!(metadata.probe_status, ProbeStatus::Ready);
        assert_eq!(metadata.media_type, MediaType::Video);
    }

    #[test]
    fn parse_metadata_output_handles_duration_only_input() {
        // Audio-only or a container without a video stream prints only the
        // format duration; width/height stay None so the caller falls back
        // to the bounded generic media frame.
        let metadata = parse_metadata_output("2.500000\n");
        assert_eq!(metadata.width, None);
        assert_eq!(metadata.height, None);
        assert_eq!(metadata.duration_ms, Some(2_500));
    }

    #[test]
    fn parse_metadata_output_handles_missing_duration() {
        // A stream that exposes dimensions but no format duration keeps the
        // dimensions and reports no duration.
        let metadata = parse_metadata_output("1280\n720\nN/A\n");
        assert_eq!(metadata.width, Some(1280));
        assert_eq!(metadata.height, Some(720));
        assert_eq!(metadata.duration_ms, None);
    }

    #[test]
    fn parse_metadata_output_rejects_unknown_values() {
        let metadata = parse_metadata_output("");
        assert_eq!(metadata.width, None);
        assert_eq!(metadata.height, None);
        assert_eq!(metadata.duration_ms, None);
        let metadata = parse_metadata_output("   \nN/A\n");
        assert_eq!(metadata.width, None);
        assert_eq!(metadata.duration_ms, None);
    }

    #[cfg(unix)]
    #[test]
    fn media_subprocess_timeout_kills_and_reaps_child() {
        let mut command = Command::new("sleep");
        command.arg("30");
        let started = Instant::now();
        let error = run_command_with_timeout(&mut command, Duration::from_millis(50), "test")
            .expect_err("sleeping child must time out");
        assert!(started.elapsed() < Duration::from_secs(2));
        assert!(error.contains("timed out"));
    }

    #[test]
    fn metadata_probe_rejects_missing_and_oversized_files() {
        assert!(probe_local_video_metadata(Path::new("/definitely/missing.mp4")).is_err());
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("huge.mp4");
        // Sparse file larger than the probe bound is rejected before ffprobe.
        let file = std::fs::File::create(&path).unwrap();
        file.set_len(MAX_METADATA_PROBE_BYTES + 1).unwrap();
        assert!(probe_local_video_metadata(&path).is_err());
    }
}
