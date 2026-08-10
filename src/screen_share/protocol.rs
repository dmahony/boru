//! Versioned, bounded control protocol for screen-sharing negotiation.
#![allow(missing_docs)]

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use tokio::io::AsyncWriteExt;
use tokio::sync::{mpsc, Mutex};

use super::session::{ScreenShareSessionId, SessionEvent, SessionManager};
use super::transport::{MediaHeader, QuicScreenTransport, ReadUnit};
use super::permissions::{Capability, MAX_CAPABILITIES};
use super::ScreenShareError;

/// ALPN registered on the shared Iroh endpoint router.
pub const SCREEN_SHARE_ALPN: &[u8] = b"boru/screen-share/1";
/// Current wire protocol version. Major versions are not compatible.
pub const SCREEN_SHARE_PROTOCOL_VERSION: u16 = 1;
/// Upper bound for the input `code` field (X11 keysyms live below 0xFFFF).
pub const MAX_INPUT_CODE: u32 = 0xFFFF;
/// Maximum encoded control frame, including no transport framing overhead.
pub const MAX_CONTROL_FRAME: usize = 16 * 1024;
/// Maximum codec names in one Hello.
pub const MAX_CODECS: usize = 16;
/// Maximum bytes in one codec name.
pub const MAX_CODEC_NAME: usize = 32;
/// Maximum reason text accepted from an untrusted peer.
pub const MAX_REASON: usize = 256;

/// A bounded, explicit view-only permission. Remote control is intentionally absent.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Permission {
    /// The viewer may only receive frames.
    ViewOnly,
    /// Explicit capabilities for a session. ViewScreen is the only capability
    /// granted by the normal acceptance path; control requires a later grant.
    Capabilities(Vec<Capability>),
}

/// Negotiation capabilities advertised by the host.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Hello {
    /// Wire protocol version.
    pub version: u16,
    /// Session being negotiated.
    pub session_id: ScreenShareSessionId,
    /// Identity that initiated the invitation.
    pub host_id: iroh::PublicKey,
    /// Application conversation reference (not used for media transport).
    pub conversation_id: u64,
    /// Codec names, ordered by preference.
    pub codecs: Vec<String>,
    /// Capture width in pixels.
    pub width: u16,
    /// Capture height in pixels.
    pub height: u16,
    /// Target frame rate in frames per second.
    pub frame_rate: u16,
    /// Permission granted after acceptance.
    pub permission: Permission,
}

/// Recipient response to a Hello.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum ControlMessage {
    /// Start negotiation; no capture starts merely because this is received.
    Hello(Hello),
    /// Explicit recipient consent for the named session.
    Accept { version: u16, session_id: ScreenShareSessionId },
    /// Explicit recipient refusal or protocol failure.
    Reject { version: u16, session_id: ScreenShareSessionId, reason: String },
    /// End a session. Repeating this message is safe and has no effect.
    EndSession { version: u16, session_id: ScreenShareSessionId },
    /// Viewer asks the host for one or more explicitly selected controls.
    RequestControl { version: u16, session_id: ScreenShareSessionId, capabilities: Vec<Capability> },
    /// Host grants the requested controls with a fresh session nonce.
    GrantControl { version: u16, session_id: ScreenShareSessionId, capabilities: Vec<Capability>, nonce: [u8; 16] },
    /// Host revokes control without ending view-only streaming.
    RevokeControl { version: u16, session_id: ScreenShareSessionId },
    /// Input always carries the current grant nonce; stale input is rejected.
    /// `code` is a button id (1-3) for pointer events or an X11 keysym for
    /// keyboard; `x`/`y` are normalized viewer coordinates (0..1 relative to
    /// the image rect) for pointer events and 0 for keyboard; `pressed` is the
    /// key/button state.
    Input { version: u16, session_id: ScreenShareSessionId, capability: Capability, nonce: [u8; 16], code: u32, x: f32, y: f32, pressed: bool },
}

/// Stable user-facing protocol failure.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProtocolError {
    /// The peer advertised a different major version.
    UnsupportedVersion { received: u16, supported: u16 },
    /// The message violated a bounded field or semantic invariant.
    Malformed(String),
    /// The stream ended or could not be read/written.
    Io(String),
}

impl std::fmt::Display for ProtocolError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::UnsupportedVersion { received, supported } => write!(f, "screen sharing protocol version {received} is unsupported (this peer supports {supported})"),
            Self::Malformed(reason) => write!(f, "malformed screen sharing control message: {reason}"),
            Self::Io(reason) => write!(f, "screen sharing protocol connection failed: {reason}"),
        }
    }
}
impl std::error::Error for ProtocolError {}

impl ControlMessage {
    /// Validate untrusted wire data before applying it to session state.
    pub fn validate(&self) -> Result<(), ProtocolError> {
        let version = match self {
            Self::Hello(message) => {
                if message.session_id == ScreenShareSessionId::zero() {
                    return Err(ProtocolError::Malformed("empty session id".into()));
                }
                if message.codecs.len() > MAX_CODECS { return Err(ProtocolError::Malformed("too many codec capabilities".into())); }
                if message.codecs.iter().any(|codec| codec.is_empty() || codec.len() > MAX_CODEC_NAME || !codec.is_ascii()) { return Err(ProtocolError::Malformed("invalid codec capability".into())); }
                if let Permission::Capabilities(capabilities) = &message.permission {
                    if capabilities.is_empty() || capabilities.len() > MAX_CAPABILITIES || capabilities.iter().any(|capability| capabilities.iter().filter(|candidate| *candidate == capability).count() > 1) {
                        return Err(ProtocolError::Malformed("invalid permission capability list".into()));
                    }
                }
                if message.width == 0 || message.height == 0 || message.width > 16_384 || message.height > 16_384 { return Err(ProtocolError::Malformed("invalid dimensions".into())); }
                if message.frame_rate == 0 || message.frame_rate > 240 { return Err(ProtocolError::Malformed("invalid frame rate".into())); }
                message.version
            }
            Self::Accept { version, .. } | Self::Reject { version, .. } | Self::EndSession { version, .. } | Self::RevokeControl { version, .. } => *version,
            Self::RequestControl { version, capabilities, .. } | Self::GrantControl { version, capabilities, .. } => {
                if capabilities.is_empty() || capabilities.len() > MAX_CAPABILITIES || capabilities.iter().any(|capability| *capability == Capability::ViewScreen) {
                    return Err(ProtocolError::Malformed("invalid control capability request".into()));
                }
                *version
            }
            Self::Input { version, capability, code, x, y, .. } => {
                if !matches!(capability, Capability::ControlPointer | Capability::ControlKeyboard) { return Err(ProtocolError::Malformed("input requires a control capability".into())); }
                if *code > MAX_INPUT_CODE { return Err(ProtocolError::Malformed("input code out of range".into())); }
                if !x.is_finite() || !y.is_finite() || !(0.0..=1.0).contains(x) || !(0.0..=1.0).contains(y) { return Err(ProtocolError::Malformed("input coordinates out of range".into())); }
                *version
            }
        };
        if version != SCREEN_SHARE_PROTOCOL_VERSION { return Err(ProtocolError::UnsupportedVersion { received: version, supported: SCREEN_SHARE_PROTOCOL_VERSION }); }
        if let Self::Reject { reason, .. } = self { if reason.is_empty() || reason.len() > MAX_REASON { return Err(ProtocolError::Malformed("invalid rejection reason".into())); } }
        Ok(())
    }
}

/// Encode one postcard control message with a hard size bound.
pub fn encode(message: &ControlMessage) -> Result<Vec<u8>, ProtocolError> {
    message.validate()?;
    let bytes = postcard::to_stdvec(message).map_err(|e| ProtocolError::Malformed(e.to_string()))?;
    if bytes.len() > MAX_CONTROL_FRAME { return Err(ProtocolError::Malformed("control frame exceeds size limit".into())); }
    Ok(bytes)
}

/// Decode one postcard control message with a hard size bound.
pub fn decode(bytes: &[u8]) -> Result<ControlMessage, ProtocolError> {
    if bytes.is_empty() || bytes.len() > MAX_CONTROL_FRAME { return Err(ProtocolError::Malformed("invalid control frame length".into())); }
    let message: ControlMessage = postcard::from_bytes(bytes).map_err(|e| ProtocolError::Malformed(e.to_string()))?;
    message.validate()?;
    Ok(message)
}

/// One validated media unit forwarded from an inbound connection to the app.
#[derive(Debug, Clone)]
pub struct InboundMedia {
    /// Session the media belongs to; the app's decode worker filters on this.
    pub session_id: ScreenShareSessionId,
    /// Validated media header.
    pub header: MediaHeader,
    /// Payload bytes (already bounded by transport validation).
    pub payload: Vec<u8>,
}

/// Iroh protocol handler for `boru/screen-share/1`.
#[derive(Debug, Clone)]
pub struct ScreenShareProtocol {
    manager: Arc<Mutex<SessionManager>>,
    events: mpsc::Sender<SessionEvent>,
    media_tx: mpsc::Sender<InboundMedia>,
    /// Inbound connections per session so the app can respond (Accept/Reject/
    /// EndSession) on the same connection the invitation arrived on.
    connections: Arc<Mutex<HashMap<ScreenShareSessionId, (usize, iroh::endpoint::Connection)>>>,
}

impl ScreenShareProtocol {
    /// Create a handler and its session state store. Media units are dropped.
    pub fn new(events: mpsc::Sender<SessionEvent>) -> Self {
        let (media_tx, _dropped_rx) = mpsc::channel(1);
        Self::with_channels(events, media_tx)
    }

    /// Create a handler that forwards inbound media to `media_tx`.
    pub fn with_channels(
        events: mpsc::Sender<SessionEvent>,
        media_tx: mpsc::Sender<InboundMedia>,
    ) -> Self {
        Self {
            manager: Arc::new(Mutex::new(SessionManager::default())),
            events,
            media_tx,
            connections: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    /// Access the state machine for locally initiated sessions.
    pub fn manager(&self) -> Arc<Mutex<SessionManager>> { Arc::clone(&self.manager) }

    /// Send one control message on the inbound connection for `session_id`.
    ///
    /// Used by the app to respond to an invitation (Accept/Reject) or end a
    /// session on the same connection the peer dialed in on.
    pub async fn send_control(
        &self,
        session_id: ScreenShareSessionId,
        message: ControlMessage,
    ) -> Result<(), ScreenShareError> {
        let connection = {
            let connections = self.connections.lock().await;
            connections.get(&session_id).map(|(_, connection)| connection.clone())
        };
        let Some(connection) = connection else {
            return Err(ScreenShareError::new(
                "no inbound connection for screen-share session",
            ));
        };
        let transport = QuicScreenTransport::new(connection, *session_id.as_bytes())?;
        transport.send_control(&message).await
    }
}

impl iroh::protocol::ProtocolHandler for ScreenShareProtocol {
    async fn accept(&self, connection: iroh::endpoint::Connection) -> Result<(), iroh::protocol::AcceptError> {
        let stable_id = connection.stable_id();
        loop {
            let (mut send, recv) = connection.accept_bi().await.map_err(iroh::protocol::AcceptError::from)?;
            let message = match super::transport::read_unit(recv).await {
                Ok(ReadUnit::Control(message)) => message,
                Ok(ReadUnit::Media(header, payload)) => {
                    let _ = self.media_tx.try_send(InboundMedia {
                        session_id: ScreenShareSessionId::from_bytes(header.session_id),
                        header,
                        payload,
                    });
                    continue;
                }
                Err(_error) => { let _ = send.reset(0u32.into()); continue; }
            };
            let response = { self.manager.lock().await.apply_remote(connection.remote_id(), message.clone(), &self.events) };
            match &message {
                ControlMessage::Hello(hello) => {
                    // Keep the inbound connection so the app can respond to the
                    // invitation (Accept/Reject) on the same connection.
                    if response.is_none() {
                        self.connections.lock().await.insert(hello.session_id, (stable_id, connection.clone()));
                    }
                }
                ControlMessage::EndSession { session_id, .. } | ControlMessage::Reject { session_id, .. } => {
                    // The session ended or was refused; release its connection slot.
                    self.connections.lock().await.remove(session_id);
                }
                ControlMessage::Accept { .. } | ControlMessage::RequestControl { .. } | ControlMessage::GrantControl { .. } | ControlMessage::RevokeControl { .. } | ControlMessage::Input { .. } => {}
            }
            if let Some(response) = response { let _ = write_message(&mut send, &response).await; }
        }
    }
}

async fn write_message(send: &mut iroh::endpoint::SendStream, message: &ControlMessage) -> Result<(), ProtocolError> {
    let bytes = encode(message)?;
    send.write_u8(0x01).await.map_err(|e| ProtocolError::Io(e.to_string()))?;
    send.write_u32(bytes.len() as u32).await.map_err(|e| ProtocolError::Io(e.to_string()))?;
    send.write_all(&bytes).await.map_err(|e| ProtocolError::Io(e.to_string()))?;
    send.finish().map_err(|e| ProtocolError::Io(e.to_string()))?;
    Ok(())
}

/// A conservative timeout for negotiation streams.
pub const NEGOTIATION_TIMEOUT: Duration = Duration::from_secs(30);

#[cfg(test)]
mod tests {
    use super::*;
    use crate::screen_share::session::SessionState;
    use crate::screen_share::{
        codec::{CodecConfig, OpenH264Decoder, OpenH264Encoder, VideoEncoder, DEFAULT_QUEUE_CAPACITY},
        capture::{PixelFormat, ScreenCapture},
        transport::{read_unit, QuicScreenTransport, ReadUnit},
        viewer::ViewerPipeline,
        TestPatternCapture,
    };
    use iroh::endpoint::presets;
    use iroh::protocol::Router;

    fn hello() -> Hello { Hello { version: 1, session_id: ScreenShareSessionId::from_bytes([1; 16]), host_id: iroh::SecretKey::generate().public(), conversation_id: 7, codecs: vec!["h264".into()], width: 1920, height: 1080, frame_rate: 30, permission: Permission::ViewOnly } }
    #[test] fn round_trip() { let message = ControlMessage::Hello(hello()); assert_eq!(decode(&encode(&message).unwrap()).unwrap(), message); }
    #[test] fn input_wire_round_trip_carries_pointer_state() {
        let message = ControlMessage::Input { version: SCREEN_SHARE_PROTOCOL_VERSION, session_id: ScreenShareSessionId::from_bytes([7; 16]), capability: Capability::ControlPointer, nonce: [3; 16], code: 1, x: 0.5, y: 0.25, pressed: true };
        assert_eq!(decode(&encode(&message).unwrap()).unwrap(), message);
        let bad_x = ControlMessage::Input { version: SCREEN_SHARE_PROTOCOL_VERSION, session_id: ScreenShareSessionId::from_bytes([7; 16]), capability: Capability::ControlPointer, nonce: [3; 16], code: 1, x: 1.5, y: 0.25, pressed: true };
        assert!(encode(&bad_x).is_err());
    }
    #[test] fn malformed_and_unsupported_are_rejected() { assert!(decode(&[0xff]).is_err()); let mut message = hello(); message.version = 2; assert!(matches!(encode(&ControlMessage::Hello(message)), Err(ProtocolError::UnsupportedVersion { .. }))); }
    #[test] fn accept_is_explicit() { let mut manager = SessionManager::default(); let id = ScreenShareSessionId::from_bytes([2; 16]); manager.start_invitation(id, hello().host_id, 7); assert_eq!(manager.state(id), Some(SessionState::AwaitingAcceptance)); }

    /// Full QUIC round trip: host dials the viewer, Hello → Invitation,
    /// viewer responds Accept on the inbound connection, host streams a
    /// synthetic H.264 frame, and the viewer decodes it through the pipeline.
    #[tokio::test]
    async fn end_to_end_invite_accept_media_decode() {
        // Viewer endpoint with the protocol handler registered on the router.
        let viewer = iroh::Endpoint::bind(presets::Minimal).await.unwrap();
        let (events_tx, mut events_rx) = mpsc::channel(32);
        let (media_tx, mut media_rx) = mpsc::channel(64);
        let protocol = ScreenShareProtocol::with_channels(events_tx, media_tx);
        let router = Router::builder(viewer.clone())
            .accept(SCREEN_SHARE_ALPN, protocol.clone())
            .spawn();

        // Host endpoint dials the viewer with the screen-share ALPN.
        let host = iroh::Endpoint::bind(presets::Minimal).await.unwrap();
        let host_pk = host.secret_key().public();
        let connection = host.connect(viewer.addr(), SCREEN_SHARE_ALPN).await.unwrap();
        let session_id = ScreenShareSessionId::generate();
        let transport = QuicScreenTransport::new(connection.clone(), *session_id.as_bytes()).unwrap();

        // Host sends the Hello; viewer emits an Invitation event.
        let hello = Hello {
            version: SCREEN_SHARE_PROTOCOL_VERSION,
            session_id,
            host_id: host_pk,
            conversation_id: 7,
            codecs: vec!["h264".into()],
            width: 640,
            height: 360,
            frame_rate: 15,
            permission: Permission::ViewOnly,
        };
        transport.send_control(&ControlMessage::Hello(hello)).await.unwrap();
        let event = events_rx.recv().await.unwrap();
        let SessionEvent::Invitation { session_id: got_id, host_id, .. } = event else {
            panic!("expected Invitation, got {event:?}");
        };
        assert_eq!(got_id, session_id);
        assert_eq!(host_id, host_pk);

        // Viewer explicitly accepts on the same inbound connection.
        protocol
            .send_control(session_id, ControlMessage::Accept { version: SCREEN_SHARE_PROTOCOL_VERSION, session_id })
            .await
            .unwrap();

        // Host reads the Accept response through its own accept loop.
        let (mut send, recv) = connection.accept_bi().await.unwrap();
        match read_unit(recv).await.unwrap() {
            ReadUnit::Control(ControlMessage::Accept { session_id: id, .. }) => {
                assert_eq!(id, session_id);
            }
            other => panic!("expected Accept control, got {other:?}"),
        }
        drop(send);

        // Host captures + encodes one synthetic frame and streams it.
        let config = CodecConfig {
            width: 640,
            height: 360,
            target_fps: 15,
            ..CodecConfig::default()
        };
        let mut capture = TestPatternCapture::new(640, 360).unwrap();
        let mut encoder = OpenH264Encoder::new(config).unwrap();
        let frame = capture.capture().unwrap().unwrap();
        let encoded = encoder.encode(&frame).unwrap();
        assert!(encoded.keyframe, "first encoded frame must be a keyframe");
        transport.send_frame(&encoded).await.unwrap();

        // Viewer protocol forwards the media unit to the app-facing channel.
        let media = media_rx.recv().await.unwrap();
        assert_eq!(media.session_id, session_id);
        assert_eq!(media.header.sequence, encoded.sequence);
        assert_eq!(media.header.width as u32, 640);
        assert_eq!(media.header.height as u32, 360);

        // Viewer decodes through the production pipeline into an RGBA frame.
        let mut pipeline = ViewerPipeline::new(
            OpenH264Decoder::default_profile().unwrap(),
            *session_id.as_bytes(),
            DEFAULT_QUEUE_CAPACITY,
        )
        .unwrap();
        pipeline.enqueue(media.header, media.payload).unwrap();
        pipeline.process();
        let decoded = pipeline.take_frame().expect("decoded frame available");
        assert_eq!((decoded.width, decoded.height), (640, 360));
        assert_eq!(decoded.pixel_format, PixelFormat::Rgba8);
        assert_eq!(decoded.pixels.len(), 640 * 360 * 4);

        router.shutdown().await.unwrap();
    }
}
