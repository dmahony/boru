//! Types shared by Boru call sessions.

/// The media kind enabled for a call session.
///
/// Video calls include audio; there is no separate video-only session type.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum CallKind {
    /// An audio-only call.
    Voice,
    /// An audio and video call.
    Video,
}

impl CallKind {
    /// Returns whether audio is enabled for this call.
    pub const fn audio_enabled(&self) -> bool {
        true
    }

    /// Returns whether video is enabled for this call.
    pub const fn video_enabled(&self) -> bool {
        matches!(self, Self::Video)
    }

    /// Returns the user-facing label for this call kind.
    pub const fn label(&self) -> &'static str {
        match self {
            Self::Voice => "Voice",
            Self::Video => "Video",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::CallKind;

    #[test]
    fn voice_enables_audio_only() {
        assert!(CallKind::Voice.audio_enabled());
        assert!(!CallKind::Voice.video_enabled());
        assert_eq!(CallKind::Voice.label(), "Voice");
    }

    #[test]
    fn video_enables_audio_and_video() {
        assert!(CallKind::Video.audio_enabled());
        assert!(CallKind::Video.video_enabled());
        assert_eq!(CallKind::Video.label(), "Video");
    }
}