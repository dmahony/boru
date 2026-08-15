//! Feature-gated screen sharing interfaces and session identity types.
//!
//! This module intentionally contains no capture, codec, or network implementation.
//! Implementations can be added behind these small boundaries without coupling them
//! to chat conversations.

pub mod adaptation;
pub mod capture;
pub mod channels;
pub mod codec;
pub mod coords;
pub mod host;
pub mod permissions;
pub mod platform;
pub mod protocol;
pub mod reconnect;
pub mod remote_input;
pub mod session;
pub mod stats;
pub mod transport;
pub mod viewer;

#[cfg(test)]
mod media_path_bench;

#[cfg(test)]
mod encode_bench;

pub use capture::{
    CapturedFrame, CaptureConfig, CaptureSource, CaptureSourceId, CaptureSourceKind,
    DesktopCaptureBackend, DirtyRegion, FrameRect, FrameSink, PixelFormat, ScreenCapture,
    TestPatternCapture,
};
pub use coords::{
    composite_cursor, composite_cursor_rgba, cursor_viewport_rect, desktop_to_normalized,
    desktop_to_source, geometry_from_logical, logical_to_physical, normalized_to_desktop,
    normalized_to_source, physical_to_logical, scale_sprite_to, source_to_desktop, CursorMeta,
    CursorSprite, DesktopPoint, MonitorGeometry, NormalizedPoint, SourcePoint,
};
pub use channels::{
    BoundedFrameQueue, ControlChannel, ControlOut, MediaChannel, DEFAULT_CONTROL_QUEUE_CAPACITY,
    DEFAULT_MEDIA_QUEUE_CAPACITY,
};
pub use adaptation::{AdaptiveQuality, PacingController, PacingCounters, QualityDecision, ViewerQualityRequest};
pub use codec::{
    CodecConfig, CodecKind, CodecMetadata, EncodedFrame, EncodedPacket, OpenH264Decoder,
    OpenH264Encoder, QualityProfile, ScreenShareCodec, VideoDecoder, VideoEncoder,
    DEFAULT_QUEUE_CAPACITY, DEFAULT_WIDTH, DEFAULT_HEIGHT, DEFAULT_FPS, DEFAULT_BITRATE_BPS,
    DEFAULT_KEYFRAME_INTERVAL, TARGET_720P30_WIDTH, TARGET_720P30_HEIGHT,
    TARGET_720P30_BITRATE_BPS, TARGET_1080P30_WIDTH, TARGET_1080P30_HEIGHT,
    TARGET_1080P30_BITRATE_BPS,
};
pub use host::{run_host_session, HostCommand, SessionTermination, DEMO_FPS, DEMO_HEIGHT, DEMO_WIDTH};
pub use platform::{
    capture_dimensions, create_capture_source, ActiveCapture, CAPTURE_FPS,
};
#[cfg(target_os = "linux")]
pub use platform::{
    classify_display_server, detect_display_server, DisplayServer, X11Capture, X11Monitor,
};
pub use protocol::{
    ControlMessage, Hello, InboundMedia, InputEventKind, Permission, RedactedText,
    ScreenShareMessage, ScreenShareProtocol, SCREEN_SHARE_ALPN, SCREEN_SHARE_PROTOCOL_VERSION,
    MAX_INPUT_CODE, MAX_MODIFIER_MASK, MAX_SCREEN_SHARE_MESSAGE, MOD_ALT, MOD_CTRL, MOD_META,
    MOD_SHIFT, MAX_CLIPBOARD_TEXT,
};
pub use reconnect::{
    keyframe_request, retry_reconnect, ReconnectOutcome, ReconnectPolicy,
};
pub use permissions::{
    Capability, ControlToken, RequestRateLimiter, SessionPermissions, SlidingWindowRateLimiter,
    INPUT_RATE_WINDOW, MAX_INPUT_EVENTS_PER_WINDOW,
};
pub use remote_input::{
    authorize_input, authorize_nonce, build_keysym_to_keycode, device_mask_grants, map_pointer,
    normalize_to_capture, parse_devices_mask, x11_key_action, x11_pointer_actions, InputEvent,
    NormalizedPointer, RemoteInput, UnavailableInputBackend, X11Action,
};
pub use session::{
    NegotiatedConfig, NegotiationError, NegotiationManager, NegotiationRole, NegotiationState,
    ScreenShareSession, ScreenShareSessionId, SessionEvent, SessionManager, SessionState,
    MAX_ACTIVE_NEGOTIATIONS,
};
pub use transport::{decode_media, encode_media, LatestFrameQueue, MediaHeader, PathKind,
    QuicScreenTransport, ReadUnit, ScreenTransport, TransportCounters, MAX_MEDIA_FRAME};
pub use viewer::{DecodedFrame, ViewerPipeline};
pub use stats::{ScreenShareSessionMetrics, ScreenShareStats, ScreenShareStatsSnapshot};

/// Classification of a screen-sharing failure, used for diagnostics and
/// actionable runtime errors (PDF Task 5.2: "clear runtime errors when
/// PipeWire or a portal implementation is missing").
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ScreenShareErrorKind {
    /// Unclassified transport/session/codec failure.
    Generic,
    /// The PipeWire runtime library or server is missing/unreachable.
    PipeWireMissing,
    /// xdg-desktop-portal or the session bus is missing.
    PortalMissing,
    /// The PipeWire stream could not connect to the portal node.
    PipeWireConnect,
    /// Format negotiation produced an unusable result.
    FormatNegotiation,
    /// A stream/buffer-level failure (short buffer, bad stride, etc.).
    Stream,
}

/// Error returned by a screen-sharing boundary.
///
/// Carries a stable, user-safe description plus a [`ScreenShareErrorKind`]
/// so callers can react to missing-dependency conditions without parsing
/// message text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScreenShareError {
    message: String,
    kind: ScreenShareErrorKind,
}

impl ScreenShareError {
    /// Construct an error with a stable, user-safe description.
    pub fn new(description: impl Into<String>) -> Self {
        Self {
            message: description.into(),
            kind: ScreenShareErrorKind::Generic,
        }
    }

    /// Construct a PipeWire-missing error (runtime library not loadable or
    /// no PipeWire server). The message names the missing piece and the
    /// action to take.
    pub fn missing_pipewire(description: impl Into<String>) -> Self {
        Self::new(description).with_kind(ScreenShareErrorKind::PipeWireMissing)
    }

    /// Construct a portal-missing error (no session bus or no
    /// xdg-desktop-portal). The message names what is missing.
    pub fn missing_portal(description: impl Into<String>) -> Self {
        Self::new(description).with_kind(ScreenShareErrorKind::PortalMissing)
    }

    /// Construct a PipeWire stream-connect failure error.
    pub fn pipewire_connect(description: impl Into<String>) -> Self {
        Self::new(description).with_kind(ScreenShareErrorKind::PipeWireConnect)
    }

    /// Construct a format-negotiation failure error.
    pub fn format_negotiation(description: impl Into<String>) -> Self {
        Self::new(description).with_kind(ScreenShareErrorKind::FormatNegotiation)
    }

    /// Construct a stream/buffer-level failure error.
    pub fn stream(description: impl Into<String>) -> Self {
        Self::new(description).with_kind(ScreenShareErrorKind::Stream)
    }

    /// Set the error kind (builder-style; used internally).
    pub fn with_kind(mut self, kind: ScreenShareErrorKind) -> Self {
        self.kind = kind;
        self
    }

    /// The failure classification, for diagnostics and typed handling.
    pub fn kind(&self) -> ScreenShareErrorKind {
        self.kind
    }
}

impl std::fmt::Display for ScreenShareError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl std::error::Error for ScreenShareError {}

#[cfg(test)]
mod tests {
    use super::*;
    use n0_tracing_test::traced_test;

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
            Ok(EncodedFrame { timestamp_us: frame.timestamp_us, encode_timestamp_us: frame.timestamp_us, sequence: 0, keyframe: true,
                config_generation: 0, width: frame.width, height: frame.height,
                bytes: frame.pixels.clone() })
        }
        fn force_keyframe(&mut self) {}
        fn reconfigure_bitrate(&mut self, _bitrate_bps: u32) -> Result<(), ScreenShareError> { Ok(()) }
        fn metadata(&self) -> CodecMetadata {
            CodecMetadata { codec: CodecKind::H264,
                config: CodecConfig { width: 2, height: 2, target_fps: 1, target_bitrate_bps: 1,
                    keyframe_interval: 1, max_queue_depth: 1, quality_profile: QualityProfile::Balanced }, generation: 0 }
        }
    }
    impl VideoDecoder for FakeCodec {
        fn decode(&mut self, frame: &EncodedFrame) -> Result<Option<CapturedFrame>, ScreenShareError> {
            Ok(Some(CapturedFrame {
                timestamp_us: frame.timestamp_us, width: 1, height: 1,
                pixel_format: PixelFormat::Bgra8, stride: 4, pixels: frame.bytes.clone(),
                gpu_handle: None, dirty_region: None, cursor: None,
            }))
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
                cursor: None,
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

    #[test]
    fn screen_share_error_kinds_map_to_actionable_messages() {
        // Typed constructors classify the failure so callers can react
        // without parsing message text (PDF Task 5.2).
        let pipewire = ScreenShareError::missing_pipewire(
            "cannot load libpipewire-0.3.so.0 — install PipeWire",
        );
        assert_eq!(pipewire.kind(), ScreenShareErrorKind::PipeWireMissing);
        assert!(pipewire.to_string().contains("libpipewire"));

        let portal = ScreenShareError::missing_portal(
            "no session bus — is xdg-desktop-portal available?",
        );
        assert_eq!(portal.kind(), ScreenShareErrorKind::PortalMissing);

        let connect = ScreenShareError::pipewire_connect("pw_context_connect failed");
        assert_eq!(connect.kind(), ScreenShareErrorKind::PipeWireConnect);

        let negotiation = ScreenShareError::format_negotiation("unusable format");
        assert_eq!(negotiation.kind(), ScreenShareErrorKind::FormatNegotiation);

        let stream = ScreenShareError::stream("pipewire buffer too small");
        assert_eq!(stream.kind(), ScreenShareErrorKind::Stream);

        // The plain constructor stays Generic; the message is preserved.
        let generic = ScreenShareError::new("transport error");
        assert_eq!(generic.kind(), ScreenShareErrorKind::Generic);
        assert_eq!(generic.to_string(), "transport error");

        // Errors remain clone/equatable for state-machine comparison tests.
        assert_eq!(pipewire, pipewire.clone());
        assert_ne!(pipewire.kind(), portal.kind());
    }

    /// PDF Phase 12 redaction guardrail: never log screen contents, raw frame
    /// bytes, clipboard contents, or sensitive keystrokes.
    ///
    /// Runs the REAL viewer decode pipeline with a distinctive frame payload,
    /// then Debug-formats the clipboard-carrying types; none of the sensitive
    /// values may appear in tracing output. Clipboard redaction is structural
    /// ([`RedactedText`] hides the payload in Debug), so even a stray
    /// `?event` / `?message` log can never leak it.
    #[test]
    #[traced_test]
    fn screen_share_logging_never_exposes_frame_bytes_or_clipboard() {
        // 1. Frame bytes through the decode pipeline.
        let mut pixels = vec![0u8; 64];
        for (i, b) in pixels.iter_mut().enumerate() {
            *b = (i as u8).wrapping_mul(31).wrapping_add(7);
        }
        let pixel_hex = hex::encode(&pixels);
        let mut pipeline = ViewerPipeline::new(FakeCodec, [7; 16], 4).expect("pipeline");
        let header = MediaHeader {
            version: 1,
            session_id: [7; 16],
            sequence: 1,
            timestamp_us: 1,
            encode_timestamp_us: 1,
            codec: 1,
            flags: MediaHeader::FLAG_KEYFRAME,
            width: 2,
            height: 2,
            config_generation: 0,
            payload_len: pixels.len() as u32,
        };
        pipeline.enqueue(header, pixels.clone()).expect("enqueue");
        pipeline.process();
        let frame = pipeline.take_frame().expect("decoded frame");
        assert_eq!(frame.pixels, pixels, "pipeline must decode the payload");

        // 2. Clipboard contents must never reach Debug/log formatting.
        let secret = "super-secret-clipboard-value-987654321";
        let message = ScreenShareMessage::Clipboard {
            version: SCREEN_SHARE_PROTOCOL_VERSION,
            session_id: ScreenShareSessionId::from_bytes([7; 16]),
            nonce: [0xAB; 16],
            text: RedactedText::new(secret.to_string()),
        };
        let debug_message = format!("{message:?}");
        assert!(
            !debug_message.contains(secret),
            "Debug of Clipboard must redact the payload: {debug_message}"
        );
        let event = SessionEvent::ClipboardReceived {
            session_id: ScreenShareSessionId::from_bytes([7; 16]),
            text: RedactedText::new(secret.to_string()),
        };
        let debug_event = format!("{event:?}");
        assert!(
            !debug_event.contains(secret),
            "Debug of ClipboardReceived must redact the payload: {debug_event}"
        );

        // 3. The traced run must not contain frame bytes or clipboard text.
        for forbidden in [pixel_hex.as_str(), secret] {
            assert!(
                !logs_contain(forbidden),
                "sensitive value appeared in tracing output: {forbidden}"
            );
        }
    }
}
