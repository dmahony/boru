//! Feature-gated screen sharing interfaces and session identity types.
//!
//! This module intentionally contains no capture, codec, or network implementation.
//! Implementations can be added behind these small boundaries without coupling them
//! to chat conversations.

pub mod capture;
pub mod codec;
pub mod permissions;
pub mod platform;
pub mod protocol;
pub mod remote_input;
pub mod session;
pub mod transport;
pub mod viewer;

pub use capture::{CapturedFrame, ScreenCapture};
pub use codec::{EncodedFrame, ScreenShareCodec, VideoDecoder, VideoEncoder};
pub use remote_input::{InputEvent, RemoteInput};
pub use protocol::{ControlMessage, Hello, Permission, ScreenShareProtocol, SCREEN_SHARE_ALPN};
pub use session::{ScreenShareSession, ScreenShareSessionId, SessionEvent, SessionManager, SessionState};
pub use transport::ScreenTransport;

/// Error returned by a screen-sharing boundary.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScreenShareError(String);

impl ScreenShareError {
    /// Construct an error with a stable, user-safe description.
    pub fn new(description: impl Into<String>) -> Self {
        Self(description.into())
    }
}

impl std::fmt::Display for ScreenShareError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl std::error::Error for ScreenShareError {}

#[cfg(test)]
mod tests {
    use super::*;

    struct FakeCapture {
        next: Option<CapturedFrame>,
    }
    impl ScreenCapture for FakeCapture {
        fn capture(&mut self) -> Result<Option<CapturedFrame>, ScreenShareError> {
            Ok(self.next.take())
        }
    }

    struct FakeCodec;
    impl VideoEncoder for FakeCodec {
        fn encode(&mut self, frame: &CapturedFrame) -> Result<EncodedFrame, ScreenShareError> {
            Ok(EncodedFrame {
                timestamp_us: frame.timestamp_us,
                bytes: frame.pixels.clone(),
            })
        }
    }
    impl VideoDecoder for FakeCodec {
        fn decode(&mut self, frame: &EncodedFrame) -> Result<CapturedFrame, ScreenShareError> {
            Ok(CapturedFrame {
                timestamp_us: frame.timestamp_us,
                pixels: frame.bytes.clone(),
            })
        }
    }

    struct FakeTransport {
        sent: Vec<EncodedFrame>,
    }
    impl ScreenTransport for FakeTransport {
        fn send(&mut self, frame: EncodedFrame) -> Result<(), ScreenShareError> {
            self.sent.push(frame);
            Ok(())
        }
    }

    #[test]
    fn fake_boundaries_can_capture_encode_decode_and_send() {
        let mut capture = FakeCapture {
            next: Some(CapturedFrame {
                timestamp_us: 7,
                pixels: vec![1, 2, 3],
            }),
        };
        let frame = capture.capture().unwrap().unwrap();
        let mut codec = FakeCodec;
        let encoded = codec.encode(&frame).unwrap();
        let decoded = codec.decode(&encoded).unwrap();
        let mut transport = FakeTransport { sent: Vec::new() };
        transport.send(encoded).unwrap();
        assert_eq!(decoded, frame);
        assert_eq!(transport.sent.len(), 1);
    }

    #[test]
    fn session_ids_are_independent_and_unique_for_generated_sessions() {
        let first = ScreenShareSession::for_conversation(42);
        let second = ScreenShareSession::for_conversation(42);
        assert_ne!(first.id(), second.id());
        assert_eq!(first.conversation_id(), second.conversation_id());
    }
}
