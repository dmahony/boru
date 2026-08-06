//! Reusable video metadata and ephemeral inline-player state.
//!
//! [`MediaMetadata`] is the small, serializable description that may be
//! attached to a stored message.  Decoder handles, widget state, local paths,
//! and playback position deliberately do not belong here: they are process
//! local and are represented by [`PlayerState`] and [`PlaybackCoordinator`].

use std::collections::BTreeSet;
use std::io::Read;
use std::path::{Path, PathBuf};
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
    if expected_hash.len() != 64 || !expected_hash.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err("attachment has no valid content identity".to_string());
    }
    let canonical_root = managed_root
        .canonicalize()
        .map_err(|e| format!("managed downloads directory unavailable: {e}"))?;
    let canonical = path
        .canonicalize()
        .map_err(|e| format!("attachment file is missing: {e}"))?;
    if !canonical.starts_with(&canonical_root) {
        return Err("attachment path escapes the managed downloads directory".to_string());
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
/// hard `-timelimit` and a bounded input size. The result never fabricates
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
    let output = std::process::Command::new("ffprobe")
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
            "-timelimit",
            "10",
        ])
        .arg(path)
        .output()
        .map_err(|e| format!("start ffprobe: {e}"))?;
    if !output.status.success() {
        let detail = String::from_utf8_lossy(&output.stderr);
        return Err(format!("ffprobe metadata probe failed: {}", detail.trim()));
    }
    Ok(parse_metadata_output(&String::from_utf8_lossy(
        &output.stdout,
    )))
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

// ── Deadline-driven playout scheduling ─────────────────────────────────
//
// Adopted from telepathy's `AudioJitterBuffer`
// (rust/telepathy-core/src/internal/connections.rs).  Each video frame is
// assigned a monotonically increasing sequence number derived from the
// source playhead.  The first frame after a gap (start, resume, seek)
// becomes the talkspurt anchor; playout of that frame starts
// `jitter_delay` after it is observed, and every later frame is scheduled
// relative to the anchor using the source frame duration.  Frames are only
// presented once their wall-clock deadline has arrived, which tracks
// variable frame rates instead of a fixed timer.  A keepalive floor drops
// frames made obsolete by a pause or source disconnect, and frames whose
// deadline passes without the playhead reaching them are counted as lost.

/// True when `a` is older than `b` under u32 sequence-number wraparound.
pub fn seq_before(a: u32, b: u32) -> bool {
    a != b && a.wrapping_sub(b) > 0x8000_0000
}

/// Default playout delay before the first frame of a talkspurt is shown.
pub const DEFAULT_JITTER_DELAY: Duration = Duration::from_millis(75);

/// Fallback frame duration used until the source framerate is known (~30fps).
const DEFAULT_FRAME_DURATION: Duration = Duration::from_millis(33);

/// A sequence jump larger than this is treated as a new talkspurt (e.g. a
/// seek) instead of counting hundreds of fake frame losses.
const MAX_BUFFERED_FRAMES: u32 = 250;

/// Deadline-driven playout scheduler for inline video frames.
///
/// Mirrors telepathy's `AudioJitterBuffer` (connections.rs): frames carry a
/// monotonic sequence number; the first frame after a gap anchors playout at
/// `now + jitter_delay`, and every later frame is scheduled relative to that
/// anchor using the source frame duration.  Frames are presented only when
/// their wall-clock deadline has arrived, which tracks variable frame rates
/// instead of a fixed timer.  A keepalive floor drops frames made obsolete
/// by a pause or source disconnect, and frames whose deadline passes without
/// being delivered are counted as lost.
#[derive(Clone, Debug)]
pub struct VideoJitterBuffer {
    /// Sequence numbers delivered by the source but not yet presented.
    arrived: BTreeSet<u32>,
    /// Next frame sequence we want to present.
    next_seq: Option<u32>,
    /// Sequence used to map frame sequence to wall-clock playout time.
    anchor_seq: u32,
    /// Wall-clock time when `anchor_seq` should be presented.
    anchor_deadline: Option<Instant>,
    /// Anything before this has been made obsolete by playout or keepalive.
    min_seq: Option<u32>,
    /// Delay before the first frame of a talkspurt is presented.
    jitter_delay: Duration,
    /// Nominal duration of one frame, derived from the source framerate.
    frame_duration: Duration,
    /// Highest sequence number the source has delivered so far.
    last_seq: Option<u32>,
    /// Total frames whose deadline passed before the frame was delivered.
    total_losses: u64,
}

impl Default for VideoJitterBuffer {
    fn default() -> Self {
        Self::new(DEFAULT_JITTER_DELAY, DEFAULT_FRAME_DURATION)
    }
}

impl VideoJitterBuffer {
    /// Create a buffer with an explicit jitter delay and frame duration.
    pub fn new(jitter_delay: Duration, frame_duration: Duration) -> Self {
        Self {
            arrived: BTreeSet::new(),
            next_seq: None,
            anchor_seq: 0,
            anchor_deadline: None,
            min_seq: None,
            jitter_delay,
            frame_duration,
            last_seq: None,
            total_losses: 0,
        }
    }

    /// Update the nominal frame duration once the source framerate is known.
    pub fn set_frame_duration(&mut self, frame_duration: Duration) {
        self.frame_duration = frame_duration;
    }

    /// Total frames whose playout deadline passed before they were available.
    pub fn total_losses(&self) -> u64 {
        self.total_losses
    }

    fn advance_min_seq(&mut self, candidate: u32) {
        if self
            .min_seq
            .is_none_or(|min_seq| seq_before(min_seq, candidate))
        {
            self.min_seq = Some(candidate);
        }
    }

    /// Advance the keepalive floor, dropping frames from a prior talkspurt.
    ///
    /// Ignore stale keepalives from a talkspurt that is still ahead of the
    /// floor; otherwise clear the talkspurt and raise `min_seq` so the next
    /// observed frame anchors a fresh playout.
    pub fn reset_after_keepalive(&mut self, sequence_floor: u32) {
        if let Some(next_seq) = self.next_seq {
            if !seq_before(next_seq, sequence_floor) {
                return;
            }
        }
        self.arrived.clear();
        self.next_seq = None;
        self.anchor_deadline = None;
        self.last_seq = None;
        self.advance_min_seq(sequence_floor);
    }

    /// Record a single frame arrival at `seq`.
    ///
    /// Used by the unit tests and by callers that observe explicit frame
    /// delivery.  The first frame of a talkspurt (after a gap, pause, or
    /// seek) becomes the playout anchor.  Returns `false` when the frame is
    /// stale (before the floor or already played) so the caller can skip it.
    pub fn observe_frame(&mut self, seq: u32, now: Instant) -> bool {
        self.observe_impl(seq, now, false)
    }

    /// Record that the playhead has advanced to `seq`, delivering every frame
    /// in `(last_seq, seq]` contiguously (the local-decoder reality).
    ///
    /// Returns `false` when nothing new was delivered (stale/duplicate).
    pub fn observe_playhead(&mut self, seq: u32, now: Instant) -> bool {
        self.observe_impl(seq, now, true)
    }

    fn observe_impl(&mut self, seq: u32, now: Instant, fill_gap: bool) -> bool {
        if let Some(min_seq) = self.min_seq {
            if seq_before(seq, min_seq) {
                return false;
            }
        }

        let is_new = match self.last_seq {
            Some(last) => seq_before(last, seq),
            None => true,
        };
        if !is_new {
            return false;
        }

        // Talkspurt start: the first observed frame anchors playout.  Frames
        // before it are already past and must not be backfilled — the anchor
        // is the earliest frame we can actually present.
        if self.next_seq.is_none() {
            self.next_seq = Some(seq);
            self.anchor_seq = seq;
            self.anchor_deadline = Some(now + self.jitter_delay);
            self.arrived.insert(seq);
            self.last_seq = Some(seq);
            return true;
        }

        let next_seq = self.next_seq.unwrap();

        // Already played or skipped.
        if seq_before(seq, next_seq) {
            return false;
        }

        let ahead = seq.wrapping_sub(next_seq);

        // Huge jump usually means a seek or a new source: restart the
        // talkspurt instead of letting one frame force fake losses.
        if ahead > MAX_BUFFERED_FRAMES {
            self.arrived.clear();
            self.arrived.insert(seq);
            self.next_seq = Some(seq);
            self.anchor_seq = seq;
            self.anchor_deadline = Some(now + self.jitter_delay);
            self.last_seq = Some(seq);
            return true;
        }

        if fill_gap {
            // Mid-talkspurt the playhead advancing means every intermediate
            // frame arrived too (a local decoder never delivers out of order).
            let from = self.last_seq.map_or(seq, |last| last.wrapping_add(1));
            let mut s = from;
            loop {
                self.arrived.insert(s);
                if s == seq {
                    break;
                }
                s = s.wrapping_add(1);
            }
        } else {
            self.arrived.insert(seq);
        }
        self.last_seq = Some(seq);

        true
    }

    /// Wall-clock deadline at which frame `seq` should be presented.
    pub fn deadline_for(&self, seq: u32) -> Option<Instant> {
        let anchor_deadline = self.anchor_deadline?;
        let offset_frames = seq.wrapping_sub(self.anchor_seq);
        Some(anchor_deadline + self.frame_duration.saturating_mul(offset_frames))
    }

    /// Deadline of the next frame to present, if playout is active.
    pub fn next_deadline(&self) -> Option<Instant> {
        self.next_seq.and_then(|seq| self.deadline_for(seq))
    }

    /// Emit frames whose playout deadline has arrived.
    ///
    /// Returns:
    /// - `None`: not time to present anything yet
    /// - `Some(Some(seq))`: present frame `seq` now
    /// - `Some(None)`: the deadline for the next frame passed without the
    ///   frame being delivered; counted as lost and `next_seq` advanced
    pub fn pop_due(&mut self, now: Instant) -> Option<Option<u32>> {
        let next_seq = self.next_seq?;
        let deadline = self.deadline_for(next_seq)?;

        if now < deadline {
            return None;
        }

        let delivered = self.arrived.remove(&next_seq);
        let following = next_seq.wrapping_add(1);
        self.next_seq = Some(following);
        self.advance_min_seq(following);

        // Go idle when nothing is buffered ahead — a stalled or paused
        // source must not count as infinite frame loss.
        if self.arrived.is_empty() {
            self.next_seq = None;
            self.anchor_deadline = None;
        }

        if delivered {
            Some(Some(next_seq))
        } else {
            self.total_losses += 1;
            Some(None)
        }
    }

    /// Full reset (seek, new source): drop talkspurt state and the floor so
    /// the first observed frame after the seek anchors fresh playout.
    pub fn reset(&mut self) {
        self.arrived.clear();
        self.next_seq = None;
        self.anchor_deadline = None;
        self.min_seq = None;
        self.last_seq = None;
    }
}

#[cfg(test)]
mod jitter_tests {
    use super::*;

    const FRAME: Duration = Duration::from_millis(33);
    const JITTER: Duration = Duration::from_millis(75);

    #[test]
    fn seq_before_handles_wraparound() {
        assert!(seq_before(0, 1));
        assert!(!seq_before(1, 0));
        assert!(!seq_before(5, 5));
        // Wrap: u32::MAX is older than 0.
        assert!(seq_before(u32::MAX, 0));
        assert!(!seq_before(0, u32::MAX));
        // Exactly half-range forward is ambiguous and treated as "not before".
        assert!(!seq_before(100, 100 + 0x8000_0000));
        // Just under half-range forward is before.
        assert!(seq_before(100, 100 + 0x8000_0000 - 1));
        // Just past half-range forward is after (b is older).
        assert!(!seq_before(100, 100 + 0x8000_0000 + 1));
    }

    #[test]
    fn first_frame_anchors_playout_after_jitter_delay() {
        let t0 = Instant::now();
        let mut jitter = VideoJitterBuffer::new(JITTER, FRAME);
        assert!(jitter.observe_frame(0, t0));
        assert_eq!(jitter.next_deadline(), Some(t0 + JITTER));
        assert_eq!(jitter.deadline_for(1), Some(t0 + JITTER + FRAME));
    }

    #[test]
    fn pop_due_waits_until_deadline() {
        let t0 = Instant::now();
        let mut jitter = VideoJitterBuffer::new(JITTER, FRAME);
        jitter.observe_frame(0, t0);
        assert_eq!(jitter.pop_due(t0 + JITTER - Duration::from_millis(1)), None);
        assert_eq!(jitter.pop_due(t0 + JITTER), Some(Some(0)));
    }

    #[test]
    fn pop_due_advances_through_available_frames() {
        let t0 = Instant::now();
        let mut jitter = VideoJitterBuffer::new(JITTER, FRAME);
        // Playhead advances 0 → 1 → 2, delivering each frame contiguously.
        assert!(jitter.observe_playhead(0, t0));
        assert!(jitter.observe_playhead(1, t0));
        assert!(jitter.observe_playhead(2, t0));
        let now = t0 + JITTER + FRAME * 2;
        assert_eq!(jitter.pop_due(now), Some(Some(0)));
        assert_eq!(jitter.pop_due(now), Some(Some(1)));
        assert_eq!(jitter.pop_due(now), Some(Some(2)));
        // Buffer drained → idle, not an endless loss stream.
        assert_eq!(jitter.pop_due(now), None);
        assert_eq!(jitter.total_losses(), 0);
    }

    #[test]
    fn pop_due_counts_missing_frame_as_loss_when_later_frame_buffered() {
        let t0 = Instant::now();
        let mut jitter = VideoJitterBuffer::new(JITTER, FRAME);
        // Frame 1 never arrives, but frame 2 does — a real hole.
        jitter.observe_frame(0, t0);
        jitter.observe_frame(2, t0);
        let now = t0 + JITTER + FRAME * 2;
        assert_eq!(jitter.pop_due(now), Some(Some(0)));
        assert_eq!(jitter.pop_due(now), Some(None)); // frame 1 lost
        assert_eq!(jitter.pop_due(now), Some(Some(2)));
        assert_eq!(jitter.pop_due(now), None);
        assert_eq!(jitter.total_losses(), 1);
    }

    #[test]
    fn stale_keepalive_rejects_restarted_sequence_number() {
        let t0 = Instant::now();
        let mut jitter = VideoJitterBuffer::new(JITTER, FRAME);
        jitter.reset_after_keepalive(80);
        assert!(!jitter.observe_frame(0, t0));
    }

    #[test]
    fn keepalive_accepts_equal_floor_sequence_number() {
        let t0 = Instant::now();
        let mut jitter = VideoJitterBuffer::new(JITTER, FRAME);
        jitter.reset_after_keepalive(80);
        assert!(jitter.observe_frame(80, t0));
    }

    #[test]
    fn stale_keepalive_accepts_continued_sequence_number() {
        let t0 = Instant::now();
        let mut jitter = VideoJitterBuffer::new(JITTER, FRAME);
        jitter.reset_after_keepalive(80);
        assert!(jitter.observe_frame(81, t0));
    }

    #[test]
    fn huge_jump_restarts_talkspurt_instead_of_fake_losses() {
        let t0 = Instant::now();
        let mut jitter = VideoJitterBuffer::new(JITTER, FRAME);
        jitter.observe_frame(0, t0);
        let jumped = 0 + MAX_BUFFERED_FRAMES + 10;
        jitter.observe_frame(jumped, t0 + FRAME);
        // The jump re-anchors at the new frame; no loss is counted for the
        // gap, and playout starts from the new anchor.
        assert_eq!(jitter.next_deadline(), Some(t0 + FRAME + JITTER));
        assert_eq!(jitter.pop_due(t0 + FRAME + JITTER), Some(Some(jumped)));
        assert_eq!(jitter.total_losses(), 0);
    }

    #[test]
    fn reset_clears_talkspurt_and_floor() {
        let t0 = Instant::now();
        let mut jitter = VideoJitterBuffer::new(JITTER, FRAME);
        jitter.observe_frame(0, t0);
        jitter.reset();
        assert_eq!(jitter.next_deadline(), None);
        // Old frame is accepted again after a reset (fresh floor).
        assert!(jitter.observe_frame(0, t0 + FRAME));
        assert_eq!(jitter.next_deadline(), Some(t0 + FRAME + JITTER));
    }

    #[test]
    fn losses_accumulate_across_stalls() {
        let t0 = Instant::now();
        let mut jitter = VideoJitterBuffer::new(JITTER, FRAME);
        // Playhead advances to frame 3, delivering frames contiguously.
        assert!(jitter.observe_playhead(0, t0));
        assert!(jitter.observe_playhead(1, t0));
        assert!(jitter.observe_playhead(2, t0));
        assert!(jitter.observe_playhead(3, t0));
        let now = t0 + JITTER + FRAME * 3;
        assert_eq!(jitter.pop_due(now), Some(Some(0)));
        assert_eq!(jitter.pop_due(now), Some(Some(1)));
        assert_eq!(jitter.pop_due(now), Some(Some(2)));
        assert_eq!(jitter.pop_due(now), Some(Some(3)));
        assert_eq!(jitter.pop_due(now), None);
        assert_eq!(jitter.total_losses(), 0);
    }

    #[test]
    fn keepalive_floor_drops_stale_buffered_frames_on_resume() {
        let t0 = Instant::now();
        let mut jitter = VideoJitterBuffer::new(JITTER, FRAME);
        assert!(jitter.observe_playhead(0, t0));
        assert!(jitter.observe_playhead(50, t0));
        let now = t0 + JITTER + FRAME * 50;
        assert_eq!(jitter.pop_due(now), Some(Some(0)));
        // A pause arrives; the keepalive floor advances to the current head.
        jitter.reset_after_keepalive(50);
        assert!(!jitter.observe_frame(10, now));
        assert!(jitter.observe_frame(50, now));
    }

    #[test]
    fn playhead_reobservation_is_idempotent() {
        let t0 = Instant::now();
        let mut jitter = VideoJitterBuffer::new(JITTER, FRAME);
        assert!(jitter.observe_playhead(5, t0));
        assert!(!jitter.observe_playhead(5, t0));
        assert!(!jitter.observe_playhead(4, t0));
    }
}

#[cfg(test)]
mod tests {
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
