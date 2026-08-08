//! Identity and session types shared by Boru's call-control and media subsystems.

/// Bounds for peer-controlled call negotiation values.
pub mod bounds;
pub mod media;
/// Consent-gated native camera enumeration and capture.
#[cfg(feature = "video-calls")]
pub mod video;
pub mod wire;

/// Lock-free bounded capture buffering for the CPAL real-time callback.
#[cfg(feature = "voice-calls")]
pub mod audio;
/// Native audio device access, kept separate from call and Iroh protocol code.
#[cfg(feature = "voice-calls")]
pub mod device;
/// Device-boundary PCM conversion and stateful resampling.
#[cfg(feature = "voice-calls")]
pub mod format;
/// Fixed-size audio frame timing and wrapping media clock.
#[cfg(feature = "voice-calls")]
pub mod frame;

#[cfg(feature = "voice-calls")]
pub use frame::{
    sequence_newer_than, sequence_older_than, AudioSeq, FRAME_DURATION, FRAME_MS,
    SAMPLES_PER_FRAME, SAMPLE_RATE,
};

/// Call actor, handle, and Iroh protocol registration.
#[cfg(feature = "net")]
pub mod adaptation;
#[cfg(feature = "net")]
pub mod manager;

use std::fmt;

use serde::{Deserialize, Serialize};

/// A random identity for one voice/video call.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct CallId([u8; 16]);

impl CallId {
    /// Construct a call identity from its wire representation.
    pub const fn from_bytes(bytes: [u8; 16]) -> Self {
        Self(bytes)
    }

    /// Generate a fresh call identity using the operating system CSPRNG.
    pub fn generate() -> Self {
        Self::try_generate().expect("OS CSPRNG unavailable for CallId")
    }

    /// Generate a fresh call identity, reporting CSPRNG failure to the caller.
    pub fn try_generate() -> Result<Self, getrandom::Error> {
        let mut bytes = [0; 16];
        getrandom::fill(&mut bytes)?;
        Ok(Self(bytes))
    }

    /// Alias for [`Self::generate`].
    pub fn new() -> Self {
        Self::generate()
    }

    /// Return the raw 128-bit identity.
    pub fn as_bytes(&self) -> &[u8; 16] {
        &self.0
    }
}

impl fmt::Display for CallId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        for byte in &self.0[..8] {
            write!(formatter, "{byte:02x}")?;
        }
        Ok(())
    }
}

impl fmt::Debug for CallId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_tuple("CallId")
            .field(&self.to_string())
            .finish()
    }
}

/// The media kind enabled for a call session.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
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
    use super::{CallId, CallKind};

    #[test]
    fn generated_call_ids_are_distinct() {
        assert_ne!(CallId::generate(), CallId::generate());
    }

    #[test]
    fn call_id_round_trips_through_postcard() {
        let original = CallId::generate();
        let encoded = postcard::to_stdvec(&original).expect("CallId should serialize");
        let decoded: CallId = postcard::from_bytes(&encoded).expect("CallId should deserialize");
        assert_eq!(original, decoded);
    }

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
