//! Reusable video metadata and ephemeral inline-player state.
//!
//! [`MediaMetadata`] is the small, serializable description that may be
//! attached to a stored message.  Decoder handles, widget state, local paths,
//! and playback position deliberately do not belong here: they are process
//! local and are represented by [`PlayerState`] and [`PlaybackCoordinator`].

use serde::{Deserialize, Serialize};

use crate::proto::TopicId;

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

    /// Clear the active video, optionally only when it matches `key`.
    pub fn clear(&mut self, key: Option<&VideoInstanceKey>) {
        if key.is_none() || self.active_video.as_ref() == key {
            self.active_video = None;
        }
    }
}

#[cfg(test)]
mod tests {
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
}
