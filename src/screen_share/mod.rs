//! Feature-gated screen sharing interfaces and session identity types.
//!
//! This module intentionally contains no capture, codec, or network implementation.
//! Implementations can be added behind these small boundaries without coupling them
//! to chat conversations.

pub mod adaptation;
pub mod capture;
pub mod codec;
pub mod host;
pub mod permissions;
pub mod platform;
pub mod protocol;
pub mod remote_input;
pub mod session;
pub mod stats;
pub mod transport;
pub mod viewer;

pub use capture::{
    CapturedFrame, CaptureConfig, CaptureSource, CaptureSourceId, CaptureSourceKind,
    DesktopCaptureBackend, DirtyRegion, FrameRect, FrameSink, PixelFormat, ScreenCapture,
    TestPatternCapture,
};
pub use adaptation::{AdaptiveQuality, QualityDecision};
pub use codec::{
    CodecConfig, CodecKind, CodecMetadata, EncodedFrame, EncodedPacket, OpenH264Decoder,
    OpenH264Encoder, ScreenShareCodec, VideoDecoder, VideoEncoder, DEFAULT_QUEUE_CAPACITY,
};
pub use host::{run_host_session, HostCommand, DEMO_FPS, DEMO_HEIGHT, DEMO_WIDTH};
pub use platform::{
    capture_dimensions, create_capture_source, ActiveCapture, CAPTURE_FPS,
};
pub use protocol::{
    ControlMessage, Hello, InboundMedia, Permission, ScreenShareMessage, ScreenShareProtocol,
    SCREEN_SHARE_ALPN, SCREEN_SHARE_PROTOCOL_VERSION, MAX_INPUT_CODE, MAX_SCREEN_SHARE_MESSAGE,
};
pub use permissions::{Capability, ControlToken, RequestRateLimiter, SessionPermissions};
pub use remote_input::{
    authorize_input, authorize_nonce, map_pointer, normalize_to_capture, InputEvent,
    NormalizedPointer, RemoteInput, UnavailableInputBackend,
};
pub use session::{
    NegotiatedConfig, NegotiationError, NegotiationManager, NegotiationRole, NegotiationState,
    ScreenShareSession, ScreenShareSessionId, SessionEvent, SessionManager, SessionState,
    MAX_ACTIVE_NEGOTIATIONS,
};
pub use transport::{decode_media, encode_media, LatestFrameQueue, MediaHeader, PathKind,
    QuicScreenTransport, ReadUnit, ScreenTransport, TransportCounters, MAX_MEDIA_FRAME};
pub use viewer::{DecodedFrame, ViewerPipeline};
pub use stats::{ScreenShareStats, ScreenShareStatsSnapshot};

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
        fn configure(&mut self, _config: CodecConfig) -> Result<(), ScreenShareError> { Ok(()) }
        fn encode(&mut self, frame: &CapturedFrame) -> Result<EncodedFrame, ScreenShareError> {
            Ok(EncodedFrame { timestamp_us: frame.timestamp_us, sequence: 0, keyframe: true,
                config_generation: 0, width: frame.width, height: frame.height,
                bytes: frame.pixels.clone() })
        }
        fn force_keyframe(&mut self) {}
        fn reconfigure_bitrate(&mut self, _bitrate_bps: u32) -> Result<(), ScreenShareError> { Ok(()) }
        fn metadata(&self) -> CodecMetadata {
            CodecMetadata { codec: CodecKind::H264,
                config: CodecConfig { width: 2, height: 2, target_fps: 1, target_bitrate_bps: 1,
                    keyframe_interval: 1, max_queue_depth: 1 }, generation: 0 }
        }
    }
    impl VideoDecoder for FakeCodec {
        fn decode(&mut self, frame: &EncodedFrame) -> Result<Option<CapturedFrame>, ScreenShareError> {
            Ok(Some(CapturedFrame { timestamp_us: frame.timestamp_us, width: 1, height: 1,
                pixel_format: PixelFormat::Bgra8, stride: 4, pixels: frame.bytes.clone(),
                gpu_handle: None, dirty_region: None }))
        }
        fn metadata(&self) -> CodecMetadata { <Self as VideoEncoder>::metadata(self) }
        fn reset(&mut self) -> Result<(), ScreenShareError> { Ok(()) }
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
                width: 1,
                height: 1,
                pixel_format: PixelFormat::Bgra8,
                stride: 4,
                pixels: vec![1, 2, 3],
                gpu_handle: None,
                dirty_region: None,
            }),
        };
        let frame = capture.capture().unwrap().unwrap();
        let mut codec = FakeCodec;
        let encoded = codec.encode(&frame).unwrap();
        let decoded = codec.decode(&encoded).unwrap();
        let mut transport = FakeTransport { sent: Vec::new() };
        transport.send(encoded).unwrap();
        assert_eq!(decoded, Some(frame));
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
