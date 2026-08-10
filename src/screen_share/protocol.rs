//! Versioned, bounded control protocol for screen-sharing negotiation.
#![allow(missing_docs)]

use std::sync::Arc;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use tokio::io::AsyncWriteExt;
use tokio::sync::{mpsc, Mutex};

use super::session::{ScreenShareSessionId, SessionEvent, SessionManager};

/// ALPN registered on the shared Iroh endpoint router.
pub const SCREEN_SHARE_ALPN: &[u8] = b"boru/screen-share/1";
/// Current wire protocol version. Major versions are not compatible.
pub const SCREEN_SHARE_PROTOCOL_VERSION: u16 = 1;
/// Maximum encoded control frame, including no transport framing overhead.
pub const MAX_CONTROL_FRAME: usize = 16 * 1024;
/// Maximum codec names in one Hello.
pub const MAX_CODECS: usize = 16;
/// Maximum bytes in one codec name.
pub const MAX_CODEC_NAME: usize = 32;
/// Maximum reason text accepted from an untrusted peer.
pub const MAX_REASON: usize = 256;

/// A bounded, explicit view-only permission. Remote control is intentionally absent.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Permission {
    /// The viewer may only receive frames.
    ViewOnly,
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
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ControlMessage {
    /// Start negotiation; no capture starts merely because this is received.
    Hello(Hello),
    /// Explicit recipient consent for the named session.
    Accept { version: u16, session_id: ScreenShareSessionId },
    /// Explicit recipient refusal or protocol failure.
    Reject { version: u16, session_id: ScreenShareSessionId, reason: String },
    /// End a session. Repeating this message is safe and has no effect.
    EndSession { version: u16, session_id: ScreenShareSessionId },
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
                if message.codecs.len() > MAX_CODECS { return Err(ProtocolError::Malformed("too many codec capabilities".into())); }
                if message.codecs.iter().any(|codec| codec.is_empty() || codec.len() > MAX_CODEC_NAME || !codec.is_ascii()) { return Err(ProtocolError::Malformed("invalid codec capability".into())); }
                if message.width == 0 || message.height == 0 || message.width > 16_384 || message.height > 16_384 { return Err(ProtocolError::Malformed("invalid dimensions".into())); }
                if message.frame_rate == 0 || message.frame_rate > 240 { return Err(ProtocolError::Malformed("invalid frame rate".into())); }
                message.version
            }
            Self::Accept { version, .. } | Self::Reject { version, .. } | Self::EndSession { version, .. } => *version,
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

/// Iroh protocol handler for `boru/screen-share/1`.
#[derive(Debug, Clone)]
pub struct ScreenShareProtocol {
    manager: Arc<Mutex<SessionManager>>,
    events: mpsc::Sender<SessionEvent>,
}

impl ScreenShareProtocol {
    /// Create a handler and its session state store.
    pub fn new(events: mpsc::Sender<SessionEvent>) -> Self { Self { manager: Arc::new(Mutex::new(SessionManager::default())), events } }
    /// Access the state machine for locally initiated sessions.
    pub fn manager(&self) -> Arc<Mutex<SessionManager>> { Arc::clone(&self.manager) }
}

impl iroh::protocol::ProtocolHandler for ScreenShareProtocol {
    async fn accept(&self, connection: iroh::endpoint::Connection) -> Result<(), iroh::protocol::AcceptError> {
        loop {
            let (mut send, mut recv) = connection.accept_bi().await.map_err(iroh::protocol::AcceptError::from)?;
            let mut frame = vec![0u8; 4];
            if let Err(error) = recv.read_exact(&mut frame).await { return Err(iroh::protocol::AcceptError::from(std::io::Error::other(error.to_string()))); }
            let length = u32::from_be_bytes(frame.as_slice().try_into().expect("four-byte frame")) as usize;
            if length == 0 || length > MAX_CONTROL_FRAME { let _ = send.reset(0u32.into()); continue; }
            let mut bytes = vec![0u8; length];
            if let Err(error) = recv.read_exact(&mut bytes).await { return Err(iroh::protocol::AcceptError::from(std::io::Error::other(error.to_string()))); }
            let message = match decode(&bytes) { Ok(message) => message, Err(error) => {
                let _ = write_message(&mut send, &ControlMessage::Reject { version: SCREEN_SHARE_PROTOCOL_VERSION, session_id: ScreenShareSessionId::zero(), reason: error.to_string() }).await;
                continue;
            }};
            let response = { self.manager.lock().await.apply_remote(connection.remote_id(), message, &self.events) };
            if let Some(response) = response { let _ = write_message(&mut send, &response).await; }
        }
    }
}

async fn write_message(send: &mut iroh::endpoint::SendStream, message: &ControlMessage) -> Result<(), ProtocolError> {
    let bytes = encode(message)?;
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

    fn hello() -> Hello { Hello { version: 1, session_id: ScreenShareSessionId::zero(), host_id: iroh::SecretKey::generate().public(), conversation_id: 7, codecs: vec!["h264".into()], width: 1920, height: 1080, frame_rate: 30, permission: Permission::ViewOnly } }
    #[test] fn round_trip() { let message = ControlMessage::Hello(hello()); assert_eq!(decode(&encode(&message).unwrap()).unwrap(), message); }
    #[test] fn malformed_and_unsupported_are_rejected() { assert!(decode(&[0xff]).is_err()); let mut message = hello(); message.version = 2; assert!(matches!(encode(&ControlMessage::Hello(message)), Err(ProtocolError::UnsupportedVersion { .. }))); }
    #[test] fn accept_is_explicit() { let mut manager = SessionManager::default(); let id = ScreenShareSessionId::zero(); manager.start_invitation(id, hello().host_id, 7); assert_eq!(manager.state(id), Some(SessionState::AwaitingAcceptance)); }
}
