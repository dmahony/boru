//! Versioned, bounded control protocol for screen-sharing negotiation.
#![allow(missing_docs)]

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use tokio::io::AsyncWriteExt;
use tokio::sync::{mpsc, Mutex};

use super::codec::QualityProfile;
use super::session::{NegotiationManager, ScreenShareSessionId, SessionEvent, SessionManager};
use super::transport::{MediaHeader, QuicScreenTransport, ReadUnit, MAX_MEDIA_FRAME};
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
/// Maximum resolutions advertised in one offer.
pub const MAX_RESOLUTIONS: usize = 16;
/// Maximum encoded screen-share protocol message. Control messages are small;
/// this bound exists so a single `VideoPacket` (media payload) can be carried
/// by a protocol message while still capping untrusted input.
pub const MAX_SCREEN_SHARE_MESSAGE: usize = MAX_MEDIA_FRAME + 4096;
/// Maximum UTF-8 bytes in one text-only clipboard payload (PDF Task 9.3).
/// Text-only sync; files and rich clipboard formats are deferred.
pub const MAX_CLIPBOARD_TEXT: usize = 512 * 1024;
/// Maximum UTF-8 bytes in a source title advertised by a `SourceChanged`
/// message (PDF Phase 10). Monitor names are short (`DP-1: 1920x1080`);
/// this bound keeps untrusted peer text out of unbounded allocations.
pub const MAX_SOURCE_NAME: usize = 128;

/// A text payload that must NEVER be formatted into logs (PDF Phase 12:
/// "Never log screen contents, raw frame bytes, clipboard contents, or
/// sensitive keystrokes").
///
/// The `Debug` impl prints a fixed redaction marker, so a stray
/// `tracing::debug!(?message, ...)` or `{:?}` format can never leak
/// clipboard contents even if a future caller forgets to redact by hand.
/// The inner text stays fully accessible through the accessors and
/// serde/postcard (wire format is identical to a bare `String`).
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RedactedText(String);

impl RedactedText {
    /// Wrap a text payload.
    pub fn new(text: String) -> Self {
        Self(text)
    }
    /// Borrow the payload for validation/use.
    pub fn as_str(&self) -> &str {
        &self.0
    }
    /// Consume and return the payload.
    pub fn into_inner(self) -> String {
        self.0
    }
}

impl std::fmt::Debug for RedactedText {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("RedactedText(<redacted>)")
    }
}

/// A bounded, explicit view-only permission. Remote control is intentionally absent.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Permission {
    /// The viewer may only receive frames.
    ViewOnly,
    /// Explicit capabilities for a session. ViewScreen is the only capability
    /// granted by the normal acceptance path; control requires a later grant.
    Capabilities(Vec<Capability>),
}

/// Modifier-mask bits carried by every [`ControlMessage::Input`] and by the
/// explicit `ModifierChange` event (PDF Task 9.2). The mask is the aggregate
/// state of the modifier keys the viewer reports holding; a key event with
/// `modifiers` set is unambiguous at the host (e.g. Ctrl+click vs click).
pub const MOD_SHIFT: u32 = 1 << 0;
pub const MOD_CTRL: u32 = 1 << 1;
pub const MOD_ALT: u32 = 1 << 2;
pub const MOD_META: u32 = 1 << 3;
/// All valid modifier bits.
pub const MAX_MODIFIER_MASK: u32 = MOD_SHIFT | MOD_CTRL | MOD_ALT | MOD_META;

/// Explicit input event kinds (PDF Task 9.2). The wire `Input` message carries
/// one of these so the receiver never has to guess intent from a bare button
/// code: pointer motion, button press/release, wheel ticks, key down/up, and
/// modifier-state changes are all first-class. `x`/`y` are always normalized
/// viewer coordinates (`0..=1` relative to the shared source image rect) for
/// pointer kinds, making them independent of the sender's local window size.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum InputEventKind {
    /// Pointer moved to `(x, y)`. `code` must be 0.
    PointerMove,
    /// Pointer button changed; `code` is a button id (1-3), `pressed` down/up.
    PointerButton,
    /// Wheel tick; `code` is the X11 wheel button (4 up, 5 down, 6 left,
    /// 7 right), `pressed` true for a tick.
    Wheel,
    /// Key changed; `code` is an X11 keysym, `pressed` down/up.
    Key,
    /// Modifier mask changed; `code` is the new held-modifier bitmask.
    ModifierChange,
}

impl InputEventKind {
    /// The capability a kind requires. Pointer kinds ride `ControlPointer`,
    /// keyboard/modifier kinds ride `ControlKeyboard`.
    pub fn capability(self) -> Capability {
        match self {
            Self::PointerMove | Self::PointerButton | Self::Wheel => Capability::ControlPointer,
            Self::Key | Self::ModifierChange => Capability::ControlKeyboard,
        }
    }
    /// True for the pointer kinds that carry normalized coordinates.
    pub fn is_pointer(self) -> bool {
        matches!(self, Self::PointerMove | Self::PointerButton | Self::Wheel)
    }
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
    /// `kind` says what kind of event this is (move/button/wheel/key/modifier);
    /// `code` is a button id (1-3) for pointer buttons, an X11 wheel button
    /// (4-7) for wheel ticks, or an X11 keysym for keyboard; `x`/`y` are
    /// normalized viewer coordinates (0..1 relative to the image rect) for
    /// pointer kinds and 0 for keyboard; `pressed` is the key/button state;
    /// `modifiers` is the viewer's current held-modifier bitmask.
    Input { version: u16, session_id: ScreenShareSessionId, nonce: [u8; 16], kind: InputEventKind, code: u32, x: f32, y: f32, pressed: bool, modifiers: u32 },
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
            Self::Input { version, kind, code, x, y, modifiers, .. } => {
                // The kind determines the capability; there is no separate
                // wire capability to mismatch (PDF Task 9.2).
                if !kind.is_pointer() {
                    // Keyboard/modifier events carry no pointer coordinates.
                    if *code > MAX_INPUT_CODE { return Err(ProtocolError::Malformed("input code out of range".into())); }
                    if *x != 0.0 || *y != 0.0 { return Err(ProtocolError::Malformed("keyboard input coordinates must be zero".into())); }
                    if matches!(kind, InputEventKind::ModifierChange) && *code & !MAX_MODIFIER_MASK != 0 {
                        return Err(ProtocolError::Malformed("modifier change code must be a valid modifier mask".into()));
                    }
                } else {
                    if !x.is_finite() || !y.is_finite() || !(0.0..=1.0).contains(x) || !(0.0..=1.0).contains(y) { return Err(ProtocolError::Malformed("input coordinates out of range".into())); }
                    match kind {
                        InputEventKind::PointerMove => { if *code != 0 { return Err(ProtocolError::Malformed("pointer move code must be zero".into())); } }
                        InputEventKind::PointerButton => { if !(1..=3).contains(code) { return Err(ProtocolError::Malformed("invalid pointer button code".into())); } }
                        InputEventKind::Wheel => { if !(4..=7).contains(code) { return Err(ProtocolError::Malformed("invalid wheel code".into())); } }
                        _ => {}
                    }
                }
                if *modifiers & !MAX_MODIFIER_MASK != 0 { return Err(ProtocolError::Malformed("invalid modifier mask".into())); }
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

/// Versioned screen-share protocol message set (screen-sharing task plan,
/// Task 2.3).
///
/// This is the canonical protocol vocabulary for the screen-share subsystem:
/// negotiation (`ScreenShareOffer`/`ScreenShareAccept`/`ScreenShareReject`),
/// lifecycle (`ScreenShareStarted`/`ScreenShareStopped`), stream configuration
/// (`StreamConfig`), media (`VideoPacket`), and control (`KeyframeRequest`,
/// `QualityUpdate`, `Error`). Every variant carries a `version` field that must
/// equal [`SCREEN_SHARE_PROTOCOL_VERSION`]; a different value is rejected
/// cleanly as [`ProtocolError::UnsupportedVersion`] before any session state
/// is touched.
///
/// These types are deliberately separate from the chat message types
/// (`crate::Message`) and from the low-level transport control encoding
/// [`ControlMessage`], which remains the wire encoding used by the current
/// session/host/viewer wiring. The negotiation and transport tasks that follow
/// (session negotiation, channel separation) consume this versioned set.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ScreenShareMessage {
    /// Initiator → recipient: propose a screen-share session. No capture
    /// begins merely because this is received; the recipient must accept.
    ScreenShareOffer {
        /// Wire protocol version.
        version: u16,
        /// Session being negotiated.
        session_id: ScreenShareSessionId,
        /// Identity that initiates the invitation.
        host_id: iroh::PublicKey,
        /// Application conversation reference (not used for media transport).
        conversation_id: u64,
        /// Codec names, ordered by preference.
        codecs: Vec<String>,
        /// Supported capture resolutions, ordered by preference. Each entry
        /// is `(width, height)` in pixels.
        resolutions: Vec<(u16, u16)>,
        /// Minimum acceptable frame rate in frames per second.
        frame_rate_min: u16,
        /// Maximum acceptable frame rate in frames per second.
        frame_rate_max: u16,
        /// Target bitrate in bits per second.
        target_bitrate_bps: u32,
        /// Whether the initiator offers remote control for this session.
        remote_control: bool,
    },
    /// Recipient → initiator: explicit consent for the named session,
    /// carrying the mutually supported configuration the recipient selected.
    ScreenShareAccept {
        /// Wire protocol version.
        version: u16,
        /// Session being accepted.
        session_id: ScreenShareSessionId,
        /// Selected codec (must appear in the offer's `codecs` list).
        codec: String,
        /// Selected capture width in pixels.
        width: u16,
        /// Selected capture height in pixels.
        height: u16,
        /// Selected frame rate in frames per second.
        frame_rate: u16,
    },
    /// Recipient → initiator: explicit refusal or protocol failure.
    ScreenShareReject {
        /// Wire protocol version.
        version: u16,
        /// Session being refused.
        session_id: ScreenShareSessionId,
        /// Stable, user-safe reason.
        reason: String,
    },
    /// Initiator → recipient: capture and encoding have begun.
    ScreenShareStarted {
        /// Wire protocol version.
        version: u16,
        /// Session that started streaming.
        session_id: ScreenShareSessionId,
    },
    /// Either side → other: session has ended. Repeating is safe and has no
    /// effect.
    ScreenShareStopped {
        /// Wire protocol version.
        version: u16,
        /// Session that ended.
        session_id: ScreenShareSessionId,
        /// Stable, user-safe reason for the stop.
        reason: String,
    },
    /// Initiator → recipient: stream parameters. Sent before the first video
    /// packet of a configuration (including after a resolution or bitrate
    /// change) so the decoder can (re)initialize.
    StreamConfig {
        /// Wire protocol version.
        version: u16,
        /// Session the configuration applies to.
        session_id: ScreenShareSessionId,
        /// Capture width in pixels.
        width: u16,
        /// Capture height in pixels.
        height: u16,
        /// Target frame rate in frames per second.
        frame_rate: u16,
        /// Target bitrate in bits per second.
        target_bitrate_bps: u32,
        /// Codec name (must match one offered in `ScreenShareOffer`).
        codec: String,
        /// Maximum distance between keyframes, in frames.
        keyframe_interval: u32,
        /// Encoder quality profile (`QualityProfile::as_u8`): 0 = Balanced,
        /// 1 = LowLatency, 2 = HighQuality. Unknown values are rejected.
        quality_profile: u8,
    },
    /// Initiator → recipient: one encoded video packet.
    VideoPacket {
        /// Wire protocol version.
        version: u16,
        /// Session the packet belongs to.
        session_id: ScreenShareSessionId,
        /// Monotonic packet sequence number. Must be non-zero.
        sequence: u64,
        /// Capture timestamp in microseconds.
        timestamp_us: u64,
        /// Whether this packet is a keyframe (decoder resync point).
        keyframe: bool,
        /// Encoder configuration generation; a change means the decoder must
        /// re-initialize (e.g. after a resolution change).
        config_generation: u32,
        /// Encoded width in pixels.
        width: u16,
        /// Encoded height in pixels.
        height: u16,
        /// Encoded access-unit bytes.
        payload: Vec<u8>,
    },
    /// Recipient → initiator: request a fresh keyframe (e.g. after joining a
    /// session or after media loss).
    KeyframeRequest {
        /// Wire protocol version.
        version: u16,
        /// Session a keyframe is requested for.
        session_id: ScreenShareSessionId,
    },
    /// Recipient → initiator (or app → host): quality preference change.
    QualityUpdate {
        /// Wire protocol version.
        version: u16,
        /// Session the quality preference applies to.
        session_id: ScreenShareSessionId,
        /// Requested bitrate in bits per second.
        target_bitrate_bps: u32,
        /// Maximum acceptable frame rate.
        max_frame_rate: u16,
        /// Relative scale of the encoded resolution, 1..=100 (100 = full).
        scale_factor: u8,
    },
    /// Either side → other: protocol or streaming error. Informational by
    /// itself; it never ends a session.
    Error {
        /// Wire protocol version.
        version: u16,
        /// Session the error belongs to.
        session_id: ScreenShareSessionId,
        /// Stable error code (non-zero).
        code: u16,
        /// Stable, user-safe message.
        message: String,
    },
    /// One direction → other: a text-only clipboard payload (PDF Task 9.3 /
    /// BORU-SS-25). Clipboard sync is a SEPARATE optional capability — it is
    /// never enabled automatically with remote control, and the receiver
    /// authorizes the payload against the explicitly granted `Clipboard`
    /// capability before applying it to the local clipboard. Text-only for
    /// now; files and rich clipboard formats are deferred.
    Clipboard {
        /// Wire protocol version.
        version: u16,
        /// Session the clipboard payload belongs to.
        session_id: ScreenShareSessionId,
        /// Current grant nonce (freshness gate, mirroring input messages).
        nonce: [u8; 16],
        /// UTF-8 text payload (bounded by [`MAX_CLIPBOARD_TEXT`]). Wrapped in
        /// [`RedactedText`] so Debug formatting can never leak clipboard
        /// contents into logs (PDF Phase 12 guardrail).
        text: RedactedText,
    },
    /// Host → viewer: the shared source (monitor/window) changed and the
    /// following media units use the NEW geometry. Sent BEFORE the first
    /// frame with the new dimensions so the viewer can re-initialise its
    /// decoder / update its UI before the dimensions actually change (PDF
    /// Phase 10: "send an explicit source-change/config-change message
    /// before media dimensions change"). Also sent when the platform
    /// renegotiates the capture geometry (monitor resize, portal format
    /// change) with the host's current source identity.
    SourceChanged {
        /// Wire protocol version.
        version: u16,
        /// Session the source change applies to.
        session_id: ScreenShareSessionId,
        /// Stable id of the newly selected source (monitor). Non-zero.
        source_id: u64,
        /// Human-readable source name (e.g. `DP-1: 1920x1080`), bounded by
        /// [`MAX_SOURCE_NAME`].
        title: String,
        /// New capture width in pixels.
        width: u16,
        /// New capture height in pixels.
        height: u16,
        /// New target frame rate in frames per second.
        frame_rate: u16,
    },
}

impl ScreenShareMessage {
    /// Validate untrusted wire data before applying it to session state.
    pub fn validate(&self) -> Result<(), ProtocolError> {
        let version = match self {
            Self::ScreenShareOffer { version, session_id, codecs, resolutions, frame_rate_min, frame_rate_max, target_bitrate_bps, .. } => {
                if *session_id == ScreenShareSessionId::zero() { return Err(ProtocolError::Malformed("empty session id".into())); }
                if codecs.is_empty() || codecs.len() > MAX_CODECS { return Err(ProtocolError::Malformed("invalid codec capability list".into())); }
                if codecs.iter().any(|codec| codec.is_empty() || codec.len() > MAX_CODEC_NAME || !codec.is_ascii()) { return Err(ProtocolError::Malformed("invalid codec capability".into())); }
                if resolutions.is_empty() || resolutions.len() > MAX_RESOLUTIONS { return Err(ProtocolError::Malformed("invalid resolution list".into())); }
                if resolutions.iter().any(|(width, height)| *width == 0 || *height == 0 || *width > 16_384 || *height > 16_384) { return Err(ProtocolError::Malformed("invalid dimensions".into())); }
                if *frame_rate_min == 0 || *frame_rate_max == 0 || *frame_rate_min > 240 || *frame_rate_max > 240 || *frame_rate_min > *frame_rate_max { return Err(ProtocolError::Malformed("invalid frame rate range".into())); }
                if *target_bitrate_bps == 0 { return Err(ProtocolError::Malformed("invalid bitrate".into())); }
                *version
            }
            Self::ScreenShareAccept { version, session_id, codec, width, height, frame_rate } => {
                if *session_id == ScreenShareSessionId::zero() { return Err(ProtocolError::Malformed("empty session id".into())); }
                if codec.is_empty() || codec.len() > MAX_CODEC_NAME || !codec.is_ascii() { return Err(ProtocolError::Malformed("invalid selected codec".into())); }
                if *width == 0 || *height == 0 || *width > 16_384 || *height > 16_384 { return Err(ProtocolError::Malformed("invalid dimensions".into())); }
                if *frame_rate == 0 || *frame_rate > 240 { return Err(ProtocolError::Malformed("invalid frame rate".into())); }
                *version
            }
            Self::ScreenShareStarted { version, session_id }
            | Self::KeyframeRequest { version, session_id } => {
                if *session_id == ScreenShareSessionId::zero() { return Err(ProtocolError::Malformed("empty session id".into())); }
                *version
            }
            Self::ScreenShareReject { version, session_id, reason }
            | Self::ScreenShareStopped { version, session_id, reason } => {
                if *session_id == ScreenShareSessionId::zero() { return Err(ProtocolError::Malformed("empty session id".into())); }
                if reason.is_empty() || reason.len() > MAX_REASON { return Err(ProtocolError::Malformed("invalid reason text".into())); }
                *version
            }
            Self::StreamConfig { version, session_id, width, height, frame_rate, target_bitrate_bps, codec, keyframe_interval, quality_profile } => {
                if *session_id == ScreenShareSessionId::zero() { return Err(ProtocolError::Malformed("empty session id".into())); }
                if *width == 0 || *height == 0 || *width > 16_384 || *height > 16_384 { return Err(ProtocolError::Malformed("invalid dimensions".into())); }
                if *frame_rate == 0 || *frame_rate > 240 { return Err(ProtocolError::Malformed("invalid frame rate".into())); }
                if *target_bitrate_bps == 0 { return Err(ProtocolError::Malformed("invalid bitrate".into())); }
                if codec.is_empty() || codec.len() > MAX_CODEC_NAME || !codec.is_ascii() { return Err(ProtocolError::Malformed("invalid codec".into())); }
                if *keyframe_interval == 0 { return Err(ProtocolError::Malformed("invalid keyframe interval".into())); }
                if QualityProfile::from_u8(*quality_profile).is_none() { return Err(ProtocolError::Malformed("invalid quality profile".into())); }
                *version
            }
            Self::VideoPacket { version, session_id, sequence, width, height, payload, .. } => {
                if *session_id == ScreenShareSessionId::zero() { return Err(ProtocolError::Malformed("empty session id".into())); }
                if *sequence == 0 { return Err(ProtocolError::Malformed("invalid media sequence".into())); }
                if *width == 0 || *height == 0 || *width > 16_384 || *height > 16_384 { return Err(ProtocolError::Malformed("invalid dimensions".into())); }
                if payload.is_empty() || payload.len() > MAX_MEDIA_FRAME { return Err(ProtocolError::Malformed("invalid media payload".into())); }
                *version
            }
            Self::QualityUpdate { version, session_id, target_bitrate_bps, max_frame_rate, scale_factor } => {
                if *session_id == ScreenShareSessionId::zero() { return Err(ProtocolError::Malformed("empty session id".into())); }
                if *target_bitrate_bps == 0 { return Err(ProtocolError::Malformed("invalid bitrate".into())); }
                if *max_frame_rate == 0 || *max_frame_rate > 240 { return Err(ProtocolError::Malformed("invalid frame rate".into())); }
                if *scale_factor == 0 || *scale_factor > 100 { return Err(ProtocolError::Malformed("invalid quality scale".into())); }
                *version
            }
            Self::Error { version, session_id, code, message } => {
                if *session_id == ScreenShareSessionId::zero() { return Err(ProtocolError::Malformed("empty session id".into())); }
                if *code == 0 { return Err(ProtocolError::Malformed("invalid error code".into())); }
                if message.is_empty() || message.len() > MAX_REASON { return Err(ProtocolError::Malformed("invalid error message".into())); }
                *version
            }
            Self::Clipboard { version, session_id, text, .. } => {
                if *session_id == ScreenShareSessionId::zero() { return Err(ProtocolError::Malformed("empty session id".into())); }
                if text.as_str().is_empty() || text.as_str().len() > MAX_CLIPBOARD_TEXT { return Err(ProtocolError::Malformed("invalid clipboard text".into())); }
                *version
            }
            Self::SourceChanged { version, session_id, source_id, title, width, height, frame_rate } => {
                if *session_id == ScreenShareSessionId::zero() { return Err(ProtocolError::Malformed("empty session id".into())); }
                if *source_id == 0 { return Err(ProtocolError::Malformed("empty source id".into())); }
                if title.is_empty() || title.len() > MAX_SOURCE_NAME || !title.is_ascii() { return Err(ProtocolError::Malformed("invalid source title".into())); }
                if *width == 0 || *height == 0 || *width > 16_384 || *height > 16_384 { return Err(ProtocolError::Malformed("invalid dimensions".into())); }
                if *frame_rate == 0 || *frame_rate > 240 { return Err(ProtocolError::Malformed("invalid frame rate".into())); }
                *version
            }
        };
        if version != SCREEN_SHARE_PROTOCOL_VERSION { return Err(ProtocolError::UnsupportedVersion { received: version, supported: SCREEN_SHARE_PROTOCOL_VERSION }); }
        Ok(())
    }

    /// Encode one postcard protocol message after validation, with a hard
    /// size bound.
    pub fn encode(&self) -> Result<Vec<u8>, ProtocolError> {
        self.validate()?;
        let bytes = postcard::to_stdvec(self).map_err(|e| ProtocolError::Malformed(e.to_string()))?;
        if bytes.len() > MAX_SCREEN_SHARE_MESSAGE { return Err(ProtocolError::Malformed("screen-share message exceeds size limit".into())); }
        Ok(bytes)
    }

    /// Decode one postcard protocol message with a hard size bound, then
    /// validate. Returns an error (never panics) on truncated input, an
    /// unknown discriminant, or a semantic invariant violation.
    pub fn decode(bytes: &[u8]) -> Result<Self, ProtocolError> {
        if bytes.is_empty() || bytes.len() > MAX_SCREEN_SHARE_MESSAGE { return Err(ProtocolError::Malformed("invalid screen-share message length".into())); }
        let message: Self = postcard::from_bytes(bytes).map_err(|e| ProtocolError::Malformed(e.to_string()))?;
        message.validate()?;
        Ok(message)
    }
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
    negotiations: Arc<Mutex<NegotiationManager>>,
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
            negotiations: Arc::new(Mutex::new(NegotiationManager::default())),
            events,
            media_tx,
            connections: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    /// Access the state machine for locally initiated sessions.
    pub fn manager(&self) -> Arc<Mutex<SessionManager>> { Arc::clone(&self.manager) }

    /// Access the versioned negotiation state machine (PDF Task 3.1).
    pub fn negotiations(&self) -> Arc<Mutex<NegotiationManager>> { Arc::clone(&self.negotiations) }

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

    /// Send one versioned protocol message on the inbound connection for
    /// `session_id`. Used by the app to answer a versioned offer (Accept with
    /// the selected configuration, or Reject) on the connection the offer
    /// arrived on.
    pub async fn send_screen_share(
        &self,
        session_id: ScreenShareSessionId,
        message: ScreenShareMessage,
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
        transport.send_screen_share(&message).await
    }
}

impl iroh::protocol::ProtocolHandler for ScreenShareProtocol {
    async fn accept(&self, connection: iroh::endpoint::Connection) -> Result<(), iroh::protocol::AcceptError> {
        let stable_id = connection.stable_id();
        let remote_id = connection.remote_id();
        let mut timeout_tick = tokio::time::interval(std::time::Duration::from_secs(1));
        timeout_tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            tokio::select! {
                biased;
                // Drive negotiation timeouts even when no stream is open: a
                // pending offer must not wait forever for a decision.
                _ = timeout_tick.tick() => {
                    self.negotiations.lock().await.expire_pending(std::time::Instant::now(), &self.events);
                }
                r = connection.accept_bi() => {
                    let (mut send, recv) = match r {
                        Ok(pair) => pair,
                        // The peer is gone: close every negotiation with this
                        // peer so the app never waits on a dead offer.
                        Err(_) => {
                            self.negotiations.lock().await.peer_disconnected(remote_id, &self.events);
                            {
                                let mut connections = self.connections.lock().await;
                                connections.retain(|_, (_, conn)| conn.stable_id() != stable_id);
                            }
                            return Ok(());
                        }
                    };
                    match super::transport::read_unit(recv).await {
                        Ok(ReadUnit::Control(message)) => {
                            let response = { self.manager.lock().await.apply_remote(remote_id, message.clone(), &self.events) };
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
                        Ok(ReadUnit::ScreenShare(message)) => {
                            self.handle_screen_share(remote_id, &connection, stable_id, message).await;
                        }
                        Ok(ReadUnit::Media(header, payload)) => {
                            if header.sequence == 0 || header.sequence % 150 == 0 {
                                tracing::info!(session = ?header.session_id, sequence = header.sequence, bytes = payload.len(), "screen-share: viewer received media");
                            }
                            let sequence = header.sequence;
                            let dropped = self
                                .media_tx
                                .try_send(InboundMedia {
                                    session_id: ScreenShareSessionId::from_bytes(header.session_id),
                                    header,
                                    payload,
                                })
                                .is_err();
                            if dropped {
                                tracing::warn!(sequence, "screen-share: viewer media dropped (channel full)");
                            }
                        }
                        Err(_error) => { let _ = send.reset(0u32.into()); }
                    }
                }
            }
        }
    }
}

impl ScreenShareProtocol {
    /// Apply one versioned protocol message to the negotiation state machine,
    /// keeping the inbound connection for app responses and writing any
    /// protocol-level reply (duplicate offer rejection) back on `send`.
    async fn handle_screen_share(
        &self,
        remote_id: iroh::PublicKey,
        connection: &iroh::endpoint::Connection,
        stable_id: usize,
        message: ScreenShareMessage,
    ) {
        match message {
            ScreenShareMessage::ScreenShareOffer { session_id, .. } => {
                let result = {
                    self.negotiations.lock().await
                        .receive_offer(remote_id, message, NEGOTIATION_TIMEOUT, &self.events)
                };
                match result {
                    Ok(()) => {
                        // Keep the inbound connection so the app can answer the
                        // offer (Accept/Reject) on the same connection.
                        self.connections.lock().await.insert(session_id, (stable_id, connection.clone()));
                    }
                    Err(error) => {
                        let reason = negotiation_reject_reason(&error);
                        // Protocol-level replies go on a FRESH stream: the peer
                        // opened the stream this message arrived on and reads
                        // replies via accept_bi(), so writing on `send` would
                        // strand the reply on a stream nobody reads.
                        let _ = write_screen_share_new_stream(
                            connection,
                            &ScreenShareMessage::ScreenShareReject {
                                version: SCREEN_SHARE_PROTOCOL_VERSION,
                                session_id,
                                reason,
                            },
                        ).await;
                    }
                }
            }
            ScreenShareMessage::ScreenShareAccept { session_id, .. } => {
                let result = {
                    self.negotiations.lock().await.handle_accept(remote_id, message, &self.events)
                };
                if let Err(error) = result {
                    tracing::warn!(?session_id, ?error, "screen-share: versioned Accept rejected");
                    let reason = negotiation_reject_reason(&error);
                    let _ = write_screen_share_new_stream(
                        connection,
                        &ScreenShareMessage::ScreenShareReject {
                            version: SCREEN_SHARE_PROTOCOL_VERSION,
                            session_id,
                            reason,
                        },
                    ).await;
                }
                self.connections.lock().await.remove(&session_id);
            }
            ScreenShareMessage::ScreenShareReject { session_id, .. } => {
                let _ = { self.negotiations.lock().await.handle_reject(remote_id, message, &self.events) };
                self.connections.lock().await.remove(&session_id);
            }
            ScreenShareMessage::ScreenShareStopped { session_id, .. } => {
                self.connections.lock().await.remove(&session_id);
            }
            // Text-only clipboard sync (PDF Task 9.3 / BORU-SS-25): the host
            // pushes its local clipboard to the viewer. Clipboard is a
            // SEPARATE optional capability — the payload is authorized
            // against the explicitly granted Clipboard capability (with the
            // current grant nonce as the freshness gate) and only then
            // surfaced to the app, which places it on the local clipboard.
            // Payloads are never logged (PDF guardrail).
            ScreenShareMessage::Clipboard { session_id, nonce, text, .. } => {
                let authorized = self
                    .manager
                    .lock()
                    .await
                    .permissions(session_id)
                    .is_some_and(|permissions| {
                        crate::screen_share::remote_input::authorize_nonce(
                            permissions,
                            session_id,
                            remote_id,
                            Capability::Clipboard,
                            nonce,
                        )
                        .is_ok()
                    });
                if authorized {
                    tracing::info!("screen-share: viewer applied host clipboard payload (text)");
                    // The event carries the RedactedText wrapper so Debug can
                    // never leak the payload (PDF Phase 12); the app unwraps
                    // it when placing the text on the local clipboard.
                    let _ = self.events.try_send(SessionEvent::ClipboardReceived { session_id, text });
                }
            }
            // PDF Phase 10: the host switched the shared source (monitor) or
            // the platform renegotiated geometry. Surface the change to the
            // app BEFORE the following media units carry the new dimensions
            // so the viewer can update its UI / decoder state in time.
            ScreenShareMessage::SourceChanged { session_id, source_id, title, width, height, frame_rate, .. } => {
                tracing::info!(session = ?session_id, source_id, title = %title, width, height, frame_rate, "screen-share: viewer source change announced");
                let _ = self.events.try_send(SessionEvent::SourceChanged {
                    session_id,
                    source_id,
                    title,
                    width: width as u32,
                    height: height as u32,
                });
            }
            // Remaining lifecycle/media messages are handled by the host/viewer
            // once streaming starts (BORU-SS-09+); the negotiation loop does
            // not act on them.
            _ => {}
        }
    }
}

/// Map a negotiation failure to a stable, user-safe reject reason.
fn negotiation_reject_reason(error: &crate::screen_share::session::NegotiationError) -> String {
    use crate::screen_share::session::NegotiationError as E;
    match error {
        E::UnknownSession => "session is not available".into(),
        E::DuplicateOffer => "duplicate offer".into(),
        E::Capacity => "too many concurrent negotiations".into(),
        E::WrongState => "negotiation is not in the expected state".into(),
        E::PeerMismatch => "offer identity does not match the connected peer".into(),
        E::UnsupportedConfig(detail) => format!("selected configuration is not mutually supported: {detail}"),
        E::EmptySessionId => "empty session id".into(),
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

/// Write one versioned protocol message on an accepted stream, mirroring the
/// transport's `SCREEN_SHARE_KIND` framing.
async fn write_screen_share_message(send: &mut iroh::endpoint::SendStream, message: &ScreenShareMessage) -> Result<(), ProtocolError> {
    let bytes = message.encode()?;
    send.write_u8(0x03).await.map_err(|e| ProtocolError::Io(e.to_string()))?;
    send.write_u32(bytes.len() as u32).await.map_err(|e| ProtocolError::Io(e.to_string()))?;
    send.write_all(&bytes).await.map_err(|e| ProtocolError::Io(e.to_string()))?;
    send.finish().map_err(|e| ProtocolError::Io(e.to_string()))?;
    Ok(())
}

/// Open a fresh stream on `connection` and write one versioned protocol
/// message on it. Protocol-level replies (a Reject for a bad offer) must go
/// on a NEW stream: the peer opened the stream the request arrived on and is
/// reading replies via `accept_bi()`, so writing on the request's own stream
/// would strand the reply where nobody reads it.
async fn write_screen_share_new_stream(connection: &iroh::endpoint::Connection, message: &ScreenShareMessage) -> Result<(), ProtocolError> {
    let (mut send, _) = connection.open_bi().await.map_err(|e| ProtocolError::Io(e.to_string()))?;
    write_screen_share_message(&mut send, message).await
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
        let message = ControlMessage::Input { version: SCREEN_SHARE_PROTOCOL_VERSION, session_id: ScreenShareSessionId::from_bytes([7; 16]), nonce: [3; 16], kind: InputEventKind::PointerButton, code: 1, x: 0.5, y: 0.25, pressed: true, modifiers: MOD_SHIFT };
        assert_eq!(decode(&encode(&message).unwrap()).unwrap(), message);
        let bad_x = ControlMessage::Input { version: SCREEN_SHARE_PROTOCOL_VERSION, session_id: ScreenShareSessionId::from_bytes([7; 16]), nonce: [3; 16], kind: InputEventKind::PointerButton, code: 1, x: 1.5, y: 0.25, pressed: true, modifiers: 0 };
        assert!(encode(&bad_x).is_err());
        // A wheel tick with a valid direction round-trips too.
        let wheel = ControlMessage::Input { version: SCREEN_SHARE_PROTOCOL_VERSION, session_id: ScreenShareSessionId::from_bytes([7; 16]), nonce: [3; 16], kind: InputEventKind::Wheel, code: 4, x: 0.5, y: 0.25, pressed: true, modifiers: 0 };
        assert_eq!(decode(&encode(&wheel).unwrap()).unwrap(), wheel);
    }
    #[test] fn input_kind_validation_is_explicit() {
        let sid = ScreenShareSessionId::from_bytes([7; 16]);
        // Pointer move must carry code 0 and normalized coordinates.
        let move_ok = ControlMessage::Input { version: SCREEN_SHARE_PROTOCOL_VERSION, session_id: sid, nonce: [0; 16], kind: InputEventKind::PointerMove, code: 0, x: 0.5, y: 0.5, pressed: false, modifiers: 0 };
        assert!(encode(&move_ok).is_ok());
        let move_bad_code = ControlMessage::Input { version: SCREEN_SHARE_PROTOCOL_VERSION, session_id: sid, nonce: [0; 16], kind: InputEventKind::PointerMove, code: 1, x: 0.5, y: 0.5, pressed: false, modifiers: 0 };
        assert!(encode(&move_bad_code).is_err());
        // Pointer buttons are 1-3.
        let button_bad = ControlMessage::Input { version: SCREEN_SHARE_PROTOCOL_VERSION, session_id: sid, nonce: [0; 16], kind: InputEventKind::PointerButton, code: 9, x: 0.5, y: 0.5, pressed: false, modifiers: 0 };
        assert!(encode(&button_bad).is_err());
        // Wheel is 4-7.
        let wheel_bad = ControlMessage::Input { version: SCREEN_SHARE_PROTOCOL_VERSION, session_id: sid, nonce: [0; 16], kind: InputEventKind::Wheel, code: 1, x: 0.5, y: 0.5, pressed: false, modifiers: 0 };
        assert!(encode(&wheel_bad).is_err());
        // Keyboard events carry no coordinates.
        let key_with_coords = ControlMessage::Input { version: SCREEN_SHARE_PROTOCOL_VERSION, session_id: sid, nonce: [0; 16], kind: InputEventKind::Key, code: 0x61, x: 0.5, y: 0.0, pressed: false, modifiers: 0 };
        assert!(encode(&key_with_coords).is_err());
        // Modifier mask is bounded to the known bits.
        let bad_mods = ControlMessage::Input { version: SCREEN_SHARE_PROTOCOL_VERSION, session_id: sid, nonce: [0; 16], kind: InputEventKind::Key, code: 0x61, x: 0.0, y: 0.0, pressed: false, modifiers: 1 << 20 };
        assert!(encode(&bad_mods).is_err());
        // A modifier change with a valid mask round-trips.
        let mod_change = ControlMessage::Input { version: SCREEN_SHARE_PROTOCOL_VERSION, session_id: sid, nonce: [0; 16], kind: InputEventKind::ModifierChange, code: MOD_SHIFT | MOD_CTRL, x: 0.0, y: 0.0, pressed: false, modifiers: MOD_SHIFT | MOD_CTRL };
        assert_eq!(decode(&encode(&mod_change).unwrap()).unwrap(), mod_change);
    }
    #[test] fn input_kind_derives_control_capability() {
        assert_eq!(InputEventKind::PointerMove.capability(), Capability::ControlPointer);
        assert_eq!(InputEventKind::PointerButton.capability(), Capability::ControlPointer);
        assert_eq!(InputEventKind::Wheel.capability(), Capability::ControlPointer);
        assert_eq!(InputEventKind::Key.capability(), Capability::ControlKeyboard);
        assert_eq!(InputEventKind::ModifierChange.capability(), Capability::ControlKeyboard);
        assert!(InputEventKind::PointerMove.is_pointer());
        assert!(!InputEventKind::Key.is_pointer());
    }
    #[test] fn malformed_and_unsupported_are_rejected() { assert!(decode(&[0xff]).is_err()); let mut message = hello(); message.version = 2; assert!(matches!(encode(&ControlMessage::Hello(message)), Err(ProtocolError::UnsupportedVersion { .. }))); }
    #[test] fn accept_is_explicit() { let mut manager = SessionManager::default(); let id = ScreenShareSessionId::from_bytes([2; 16]); let host = hello().host_id; let viewer = iroh::SecretKey::generate().public(); manager.start_invitation(id, host, viewer, 7); assert_eq!(manager.state(id), Some(SessionState::AwaitingAcceptance)); }

    fn sid() -> ScreenShareSessionId { ScreenShareSessionId::from_bytes([7; 16]) }
    fn offer() -> ScreenShareMessage {
        ScreenShareMessage::ScreenShareOffer { version: SCREEN_SHARE_PROTOCOL_VERSION, session_id: sid(), host_id: iroh::SecretKey::generate().public(), conversation_id: 7, codecs: vec!["h264".into()], resolutions: vec![(1920, 1080), (1280, 720)], frame_rate_min: 15, frame_rate_max: 30, target_bitrate_bps: 2_000_000, remote_control: false }
    }
    fn accept() -> ScreenShareMessage { ScreenShareMessage::ScreenShareAccept { version: SCREEN_SHARE_PROTOCOL_VERSION, session_id: sid(), codec: "h264".into(), width: 1280, height: 720, frame_rate: 30 } }
    fn reject() -> ScreenShareMessage { ScreenShareMessage::ScreenShareReject { version: SCREEN_SHARE_PROTOCOL_VERSION, session_id: sid(), reason: "user declined".into() } }
    fn started() -> ScreenShareMessage { ScreenShareMessage::ScreenShareStarted { version: SCREEN_SHARE_PROTOCOL_VERSION, session_id: sid() } }
    fn stopped() -> ScreenShareMessage { ScreenShareMessage::ScreenShareStopped { version: SCREEN_SHARE_PROTOCOL_VERSION, session_id: sid(), reason: "host ended".into() } }
    fn stream_config() -> ScreenShareMessage { ScreenShareMessage::StreamConfig { version: SCREEN_SHARE_PROTOCOL_VERSION, session_id: sid(), width: 1280, height: 720, frame_rate: 30, target_bitrate_bps: 1_500_000, codec: "h264".into(), keyframe_interval: 120, quality_profile: QualityProfile::Balanced.as_u8() } }
    fn video_packet() -> ScreenShareMessage { ScreenShareMessage::VideoPacket { version: SCREEN_SHARE_PROTOCOL_VERSION, session_id: sid(), sequence: 1, timestamp_us: 1_000, keyframe: true, config_generation: 0, width: 640, height: 360, payload: vec![0xAB; 32] } }
    fn keyframe_request() -> ScreenShareMessage { ScreenShareMessage::KeyframeRequest { version: SCREEN_SHARE_PROTOCOL_VERSION, session_id: sid() } }
    fn quality_update() -> ScreenShareMessage { ScreenShareMessage::QualityUpdate { version: SCREEN_SHARE_PROTOCOL_VERSION, session_id: sid(), target_bitrate_bps: 1_000_000, max_frame_rate: 30, scale_factor: 100 } }
    fn protocol_error() -> ScreenShareMessage { ScreenShareMessage::Error { version: SCREEN_SHARE_PROTOCOL_VERSION, session_id: sid(), code: 1, message: "encode failure".into() } }
    fn clipboard() -> ScreenShareMessage { ScreenShareMessage::Clipboard { version: SCREEN_SHARE_PROTOCOL_VERSION, session_id: sid(), nonce: [0xAB; 16], text: RedactedText::new("hello clipboard".into()) } }
    fn source_changed() -> ScreenShareMessage { ScreenShareMessage::SourceChanged { version: SCREEN_SHARE_PROTOCOL_VERSION, session_id: sid(), source_id: 7, title: "DP-1: 1920x1080".into(), width: 1920, height: 1080, frame_rate: 30 } }

    /// Every one of the Task 2.3 message types plus the Task 9.3 Clipboard
    /// and Task 10 SourceChanged messages must survive a postcard encode →
    /// decode round trip unchanged.
    #[test]
    fn round_trip_all_screen_share_messages() {
        let messages = [offer(), accept(), reject(), started(), stopped(), stream_config(), video_packet(), keyframe_request(), quality_update(), protocol_error(), clipboard(), source_changed()];
        assert_eq!(messages.len(), 12, "the Task 2.3 message set (ten) plus Task 9.3 Clipboard and Task 10 SourceChanged must have twelve types");
        for message in messages {
            let bytes = message.encode().expect("encode should succeed");
            assert_eq!(ScreenShareMessage::decode(&bytes).expect("decode should succeed"), message);
        }
    }

    /// Truncated wire input must be rejected with an error, never a panic.
    #[test]
    fn truncated_message_is_rejected_without_panicking() {
        let bytes = video_packet().encode().unwrap();
        for cut in [0usize, 1, 2, bytes.len() / 2, bytes.len() - 1] {
            let result = ScreenShareMessage::decode(&bytes[..cut]);
            assert!(result.is_err(), "truncated input at byte {cut} must be rejected, got {result:?}");
        }
    }

    /// PDF Task 9.3 / BORU-SS-25: the clipboard payload is bounded, must be
    /// non-empty, and must reference a live session. A clipboard message with
    /// no text, oversized text, or an empty session id is rejected.
    #[test]
    fn clipboard_validation_bounds_text_and_session() {
        let base = clipboard();
        // Empty text is rejected.
        let mut empty = base.clone();
        if let ScreenShareMessage::Clipboard { text, .. } = &mut empty { text.0.clear(); }
        assert!(matches!(empty.encode(), Err(ProtocolError::Malformed(_))));
        // Oversized text is rejected.
        let mut huge = base.clone();
        if let ScreenShareMessage::Clipboard { text, .. } = &mut huge { *text = RedactedText::new("x".repeat(MAX_CLIPBOARD_TEXT + 1)); }
        assert!(matches!(huge.encode(), Err(ProtocolError::Malformed(_))));
        // An empty session id is rejected.
        let mut empty_session = base.clone();
        if let ScreenShareMessage::Clipboard { session_id, .. } = &mut empty_session { *session_id = ScreenShareSessionId::zero(); }
        assert!(matches!(empty_session.encode(), Err(ProtocolError::Malformed(_))));
        // The valid fixture still round-trips.
        assert_eq!(ScreenShareMessage::decode(&base.encode().unwrap()).unwrap(), base);
    }

    /// A message carrying a non-current protocol version is rejected cleanly on
    /// both the encode and decode paths (no panic, no state mutation).
    #[test]
    fn bad_version_is_rejected_cleanly() {
        let mut message = offer();
        {
            let ScreenShareMessage::ScreenShareOffer { version, .. } = &mut message else { panic!("wrong variant") };
            *version = 2;
        }
        assert!(matches!(message.encode(), Err(ProtocolError::UnsupportedVersion { received: 2, supported: 1 })));
        // Decode path: serialize the bad version directly (bypassing validate)
        // and confirm decode rejects it with UnsupportedVersion, not a panic.
        let bytes = postcard::to_stdvec(&message).unwrap();
        assert!(matches!(ScreenShareMessage::decode(&bytes), Err(ProtocolError::UnsupportedVersion { received: 2, supported: 1 })));
    }

    /// Unknown enum discriminants (postcard varints that map to no variant)
    /// are rejected cleanly.
    #[test]
    fn unknown_discriminant_is_rejected_cleanly() {
        // The enum has twelve variants → postcard discriminants 0..=11.
        assert!(ScreenShareMessage::decode(&[12, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0]).is_err());
        // A multi-byte varint far outside the variant range.
        assert!(ScreenShareMessage::decode(&[0xff, 0xff, 0xff, 0xff]).is_err());
    }

    /// PDF Phase 10: the source-change message is bounded and must reference a
    /// real source. A SourceChanged message with no source id, no title, an
    /// oversized/non-ASCII title, invalid dimensions, or an invalid frame rate
    /// is rejected.
    #[test]
    fn source_changed_validation_bounds_fields() {
        let base = source_changed();
        // Empty source id is rejected.
        let mut empty_id = base.clone();
        if let ScreenShareMessage::SourceChanged { source_id, .. } = &mut empty_id { *source_id = 0; }
        assert!(matches!(empty_id.encode(), Err(ProtocolError::Malformed(_))));
        // Empty title is rejected.
        let mut empty_title = base.clone();
        if let ScreenShareMessage::SourceChanged { title, .. } = &mut empty_title { title.clear(); }
        assert!(matches!(empty_title.encode(), Err(ProtocolError::Malformed(_))));
        // Oversized title is rejected.
        let mut huge_title = base.clone();
        if let ScreenShareMessage::SourceChanged { title, .. } = &mut huge_title { *title = "x".repeat(MAX_SOURCE_NAME + 1); }
        assert!(matches!(huge_title.encode(), Err(ProtocolError::Malformed(_))));
        // Non-ASCII titles are rejected (untrusted peer text stays ASCII).
        let mut bad_title = base.clone();
        if let ScreenShareMessage::SourceChanged { title, .. } = &mut bad_title { *title = "モニター".into(); }
        assert!(matches!(bad_title.encode(), Err(ProtocolError::Malformed(_))));
        // Zero dimensions are rejected.
        let mut zero_dims = base.clone();
        if let ScreenShareMessage::SourceChanged { width, .. } = &mut zero_dims { *width = 0; }
        assert!(matches!(zero_dims.encode(), Err(ProtocolError::Malformed(_))));
        // Zero frame rate is rejected.
        let mut zero_fps = base.clone();
        if let ScreenShareMessage::SourceChanged { frame_rate, .. } = &mut zero_fps { *frame_rate = 0; }
        assert!(matches!(zero_fps.encode(), Err(ProtocolError::Malformed(_))));
        // An empty session id is rejected.
        let mut empty_session = base.clone();
        if let ScreenShareMessage::SourceChanged { session_id, .. } = &mut empty_session { *session_id = ScreenShareSessionId::zero(); }
        assert!(matches!(empty_session.encode(), Err(ProtocolError::Malformed(_))));
        // The valid fixture still round-trips.
        assert_eq!(ScreenShareMessage::decode(&base.encode().unwrap()).unwrap(), base);
    }

    /// Semantic invariants are enforced by validate() on both encode and
    /// decode; violations are clean errors.
    #[test]
    fn semantic_validation_rejects_invalid_fields() {
        let mut m = offer();
        {
            let ScreenShareMessage::ScreenShareOffer { session_id, .. } = &mut m else { panic!("wrong variant") };
            *session_id = ScreenShareSessionId::zero();
        }
        assert!(matches!(m.encode(), Err(ProtocolError::Malformed(_))));
        {
            let ScreenShareMessage::ScreenShareOffer { resolutions, .. } = &mut m else { panic!("wrong variant") };
            resolutions.clear();
        }
        assert!(matches!(m.encode(), Err(ProtocolError::Malformed(_))));
        {
            let ScreenShareMessage::ScreenShareOffer { frame_rate_min, .. } = &mut m else { panic!("wrong variant") };
            *frame_rate_min = 0;
        }
        assert!(matches!(m.encode(), Err(ProtocolError::Malformed(_))));
        {
            let ScreenShareMessage::ScreenShareOffer { frame_rate_max, .. } = &mut m else { panic!("wrong variant") };
            *frame_rate_max = 10; // below the (restored) minimum of 15
        }
        assert!(matches!(m.encode(), Err(ProtocolError::Malformed(_))));
        {
            let ScreenShareMessage::ScreenShareOffer { codecs, .. } = &mut m else { panic!("wrong variant") };
            *codecs = vec!["not ascii ☃".into()];
        }
        assert!(matches!(m.encode(), Err(ProtocolError::Malformed(_))));
        // Restore every mutated field so the offer encodes cleanly again.
        {
            let ScreenShareMessage::ScreenShareOffer { session_id, resolutions, frame_rate_min, frame_rate_max, codecs, .. } = &mut m else { panic!("wrong variant") };
            *session_id = sid();
            *resolutions = vec![(1920, 1080), (1280, 720)];
            *frame_rate_min = 15;
            *frame_rate_max = 30;
            *codecs = vec!["h264".into()];
        }
        assert!(m.encode().is_ok(), "restored offer must encode cleanly");

        let mut m = reject();
        {
            let ScreenShareMessage::ScreenShareReject { reason, .. } = &mut m else { panic!("wrong variant") };
            reason.clear();
        }
        assert!(matches!(m.encode(), Err(ProtocolError::Malformed(_))));

        let mut m = video_packet();
        {
            let ScreenShareMessage::VideoPacket { payload, .. } = &mut m else { panic!("wrong variant") };
            payload.clear();
        }
        assert!(matches!(m.encode(), Err(ProtocolError::Malformed(_))));
        {
            let ScreenShareMessage::VideoPacket { sequence, .. } = &mut m else { panic!("wrong variant") };
            *sequence = 0;
        }
        assert!(matches!(m.encode(), Err(ProtocolError::Malformed(_))));
        // Oversized video payloads are rejected by the size bound.
        {
            let ScreenShareMessage::VideoPacket { payload, .. } = &mut m else { panic!("wrong variant") };
            *payload = vec![0; MAX_MEDIA_FRAME + 1];
        }
        assert!(matches!(m.encode(), Err(ProtocolError::Malformed(_))));

        let mut m = stream_config();
        {
            let ScreenShareMessage::StreamConfig { keyframe_interval, .. } = &mut m else { panic!("wrong variant") };
            *keyframe_interval = 0;
        }
        assert!(matches!(m.encode(), Err(ProtocolError::Malformed(_))));

        let mut m = quality_update();
        {
            let ScreenShareMessage::QualityUpdate { scale_factor, .. } = &mut m else { panic!("wrong variant") };
            *scale_factor = 0;
        }
        assert!(matches!(m.encode(), Err(ProtocolError::Malformed(_))));

        let mut m = protocol_error();
        {
            let ScreenShareMessage::Error { code, .. } = &mut m else { panic!("wrong variant") };
            *code = 0;
        }
        assert!(matches!(m.encode(), Err(ProtocolError::Malformed(_))));
    }

    /// The accept must carry a selected codec/resolution/fps and the offer
    /// must carry a sane resolution list and frame-rate range; every new
    /// field is validated on both encode and decode.
    #[test]
    fn negotiation_selection_fields_are_validated() {
        let mut m = accept();
        {
            let ScreenShareMessage::ScreenShareAccept { codec, .. } = &mut m else { panic!("wrong variant") };
            codec.clear();
        }
        assert!(matches!(m.encode(), Err(ProtocolError::Malformed(_))));
        {
            let ScreenShareMessage::ScreenShareAccept { width, .. } = &mut m else { panic!("wrong variant") };
            *width = 0;
        }
        assert!(matches!(m.encode(), Err(ProtocolError::Malformed(_))));
        {
            let ScreenShareMessage::ScreenShareAccept { frame_rate, .. } = &mut m else { panic!("wrong variant") };
            *frame_rate = 0;
        }
        assert!(matches!(m.encode(), Err(ProtocolError::Malformed(_))));
        // Restore so the accept round-trips again.
        {
            let ScreenShareMessage::ScreenShareAccept { codec, width, frame_rate, .. } = &mut m else { panic!("wrong variant") };
            *codec = "h264".into();
            *width = 1280;
            *frame_rate = 30;
        }
        assert!(m.encode().is_ok(), "restored accept must encode cleanly");

        let mut m = offer();
        {
            let ScreenShareMessage::ScreenShareOffer { resolutions, .. } = &mut m else { panic!("wrong variant") };
            resolutions.push((0, 0));
        }
        assert!(matches!(m.encode(), Err(ProtocolError::Malformed(_))));
        {
            let ScreenShareMessage::ScreenShareOffer { resolutions, frame_rate_min, frame_rate_max, .. } = &mut m else { panic!("wrong variant") };
            resolutions.pop();
            *frame_rate_min = 30;
            *frame_rate_max = 15; // inverted range
        }
        assert!(matches!(m.encode(), Err(ProtocolError::Malformed(_))));
        {
            let ScreenShareMessage::ScreenShareOffer { frame_rate_min, frame_rate_max, .. } = &mut m else { panic!("wrong variant") };
            *frame_rate_min = 15;
            *frame_rate_max = 30;
        }
        assert!(m.encode().is_ok(), "restored offer must encode cleanly");
    }

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

    /// Full QUIC reconnect round trip (PDF Task 3.3): after a transient media
    /// failure the host re-dials the viewer and re-sends the SAME Hello (same
    /// session id, same host). The viewer's protocol handler must treat this
    /// as a reconnect — NOT a duplicate-offer rejection — keep the session
    /// alive, reset remote-control permissions to view-only (REC-2), and
    /// accept a fresh Accept on the new connection. The host then completes
    /// the reconnect and the session is Streaming again; control is NOT
    /// silently resumed.
    #[tokio::test]
    async fn end_to_end_reconnect_after_media_failure() {
        // Viewer endpoint with the protocol handler registered on the router.
        let viewer = iroh::Endpoint::bind(presets::Minimal).await.unwrap();
        let (events_tx, mut events_rx) = mpsc::channel(32);
        let (media_tx, _media_rx) = mpsc::channel(64);
        let protocol = ScreenShareProtocol::with_channels(events_tx.clone(), media_tx);
        let router = Router::builder(viewer.clone())
            .accept(SCREEN_SHARE_ALPN, protocol.clone())
            .spawn();

        // Host endpoint (with its own local session manager, as the real host
        // driver uses).
        let host = iroh::Endpoint::bind(presets::Minimal).await.unwrap();
        let host_pk = host.secret_key().public();
        let viewer_pk = viewer.secret_key().public();
        let session_id = ScreenShareSessionId::generate();
        let mut host_manager = SessionManager::default();
        let (host_events_tx, mut host_events_rx) = mpsc::channel(32);
        host_manager.start_invitation(session_id, host_pk, viewer_pk, 7);

        // ---- First negotiation: Hello → Invitation → Accept → Streaming.
        let connection = host.connect(viewer.addr(), SCREEN_SHARE_ALPN).await.unwrap();
        let transport = QuicScreenTransport::new(connection.clone(), *session_id.as_bytes()).unwrap();
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
        transport.send_control(&ControlMessage::Hello(hello.clone())).await.unwrap();
        let event = events_rx.recv().await.unwrap();
        let SessionEvent::Invitation { session_id: got_id, host_id, .. } = event else {
            panic!("expected Invitation, got {event:?}");
        };
        assert_eq!(got_id, session_id);
        assert_eq!(host_id, host_pk);

        // Viewer accepts on the inbound connection; host applies the Accept.
        protocol
            .send_control(session_id, ControlMessage::Accept { version: SCREEN_SHARE_PROTOCOL_VERSION, session_id })
            .await
            .unwrap();
        host_manager.apply_remote(viewer_pk, ControlMessage::Accept { version: SCREEN_SHARE_PROTOCOL_VERSION, session_id }, &host_events_tx);
        assert_eq!(host_manager.state(session_id), Some(SessionState::Streaming));

        // Viewer had remote control granted; the reconnect must drop it.
        protocol
            .manager()
            .lock()
            .await
            .grant_control(session_id, vec![Capability::ControlPointer], &events_tx);
        assert!(
            protocol
                .manager()
                .lock()
                .await
                .permissions(session_id)
                .unwrap()
                .allows(session_id, host_pk, Capability::ControlPointer),
            "control granted before reconnect"
        );
        // Drain the ControlChanged(active:true) emitted by the grant.
        let event = events_rx.recv().await.unwrap();
        assert!(
            matches!(event, SessionEvent::ControlChanged { session_id: id, active: true, .. } if id == session_id),
            "expected ControlChanged(active:true), got {event:?}"
        );

        // ---- Transient media failure: host enters Reconnecting locally.
        assert!(host_manager.begin_reconnect(session_id, &host_events_tx));
        assert_eq!(host_manager.state(session_id), Some(SessionState::Reconnecting));
        // Drain the host events emitted so far (Accepted, Reconnecting,
        // ControlChanged(active:false)).
        let event = host_events_rx.recv().await.unwrap();
        assert!(
            matches!(event, SessionEvent::Accepted { session_id: id, .. } if id == session_id),
            "expected Accepted, got {event:?}"
        );
        let event = host_events_rx.recv().await.unwrap();
        assert!(
            matches!(event, SessionEvent::Reconnecting { session_id: id } if id == session_id),
            "expected Reconnecting, got {event:?}"
        );
        let event = host_events_rx.recv().await.unwrap();
        assert!(
            matches!(event, SessionEvent::ControlChanged { session_id: id, active: false, .. } if id == session_id),
            "expected ControlChanged(active:false), got {event:?}"
        );

        // ---- Host re-dials and re-sends the SAME Hello on a NEW connection.
        let reconnect_connection = host.connect(viewer.addr(), SCREEN_SHARE_ALPN).await.unwrap();
        let reconnect_transport =
            QuicScreenTransport::new(reconnect_connection.clone(), *session_id.as_bytes()).unwrap();
        reconnect_transport.send_control(&ControlMessage::Hello(hello)).await.unwrap();

        // The viewer must NOT reject the re-Hello: it emits Reconnecting and
        // resets permissions to view-only (REC-2).
        let event = events_rx.recv().await.unwrap();
        assert!(
            matches!(event, SessionEvent::Reconnecting { session_id: id } if id == session_id),
            "expected Reconnecting, got {event:?}"
        );
        let event = events_rx.recv().await.unwrap();
        assert!(
            matches!(event, SessionEvent::ControlChanged { session_id: id, active: false, .. } if id == session_id),
            "expected ControlChanged(active:false), got {event:?}"
        );
        assert_eq!(
            protocol.manager().lock().await.state(session_id),
            Some(SessionState::Reconnecting)
        );
        assert_eq!(
            protocol.manager().lock().await.permissions(session_id).unwrap().capabilities(),
            &[Capability::ViewScreen],
            "reconnect must reset to view-only — control is not silently resumed"
        );

        // ---- Viewer re-accepts on the NEW connection and requests a fresh
        // keyframe (REC-1).
        protocol
            .send_control(session_id, ControlMessage::Accept { version: SCREEN_SHARE_PROTOCOL_VERSION, session_id })
            .await
            .unwrap();
        protocol
            .send_screen_share(session_id, ScreenShareMessage::KeyframeRequest { version: SCREEN_SHARE_PROTOCOL_VERSION, session_id })
            .await
            .unwrap();

        // ---- Host reads the fresh Accept and completes the reconnect.
        let (mut send, recv) = reconnect_connection.accept_bi().await.unwrap();
        match read_unit(recv).await.unwrap() {
            ReadUnit::Control(ControlMessage::Accept { session_id: id, .. }) => {
                assert_eq!(id, session_id);
            }
            other => panic!("expected Accept control, got {other:?}"),
        }
        drop(send);

        // The host applies the fresh Accept: Reconnecting → Streaming.
        host_manager.apply_remote(viewer_pk, ControlMessage::Accept { version: SCREEN_SHARE_PROTOCOL_VERSION, session_id }, &host_events_tx);
        assert_eq!(host_manager.state(session_id), Some(SessionState::Streaming));
        let event = host_events_rx.recv().await.unwrap();
        assert!(
            matches!(event, SessionEvent::Reconnected { session_id: id } if id == session_id),
            "expected Reconnected, got {event:?}"
        );

        // Host also receives the viewer's fresh-keyframe request.
        let (mut send, recv) = reconnect_connection.accept_bi().await.unwrap();
        match read_unit(recv).await.unwrap() {
            ReadUnit::ScreenShare(ScreenShareMessage::KeyframeRequest { session_id: id, .. }) => {
                assert_eq!(id, session_id);
            }
            other => panic!("expected KeyframeRequest, got {other:?}"),
        }
        drop(send);

        // Host-side permissions are view-only too — control requires fresh
        // explicit consent after a reconnect.
        assert_eq!(
            host_manager.permissions(session_id).unwrap().capabilities(),
            &[Capability::ViewScreen],
            "host permissions must not resume control after reconnect"
        );

        router.shutdown().await.unwrap();
    }

    /// Versioned negotiation over real QUIC (PDF Task 3.1): the initiator
    /// sends a ScreenShareOffer with codecs/resolutions/fps range, the
    /// recipient's protocol handler emits a NegotiationInvitation and keeps
    /// the inbound connection, the recipient answers with a mutually supported
    /// ScreenShareAccept, and the initiator's negotiation reaches Accepted so
    /// capture may begin. Also verifies a duplicate offer is answered with an
    /// explicit ScreenShareReject.
    #[tokio::test]
    async fn end_to_end_versioned_negotiation_offer_accept() {
        let viewer = iroh::Endpoint::bind(presets::Minimal).await.unwrap();
        let (events_tx, mut events_rx) = mpsc::channel(32);
        let negotiation_events_tx = events_tx.clone();
        let (media_tx, _media_rx) = mpsc::channel(64);
        let protocol = ScreenShareProtocol::with_channels(events_tx, media_tx);
        let router = Router::builder(viewer.clone())
            .accept(SCREEN_SHARE_ALPN, protocol.clone())
            .spawn();

        let host = iroh::Endpoint::bind(presets::Minimal).await.unwrap();
        let host_pk = host.secret_key().public();
        let connection = host.connect(viewer.addr(), SCREEN_SHARE_ALPN).await.unwrap();
        let session_id = ScreenShareSessionId::generate();
        let transport = QuicScreenTransport::new(connection.clone(), *session_id.as_bytes()).unwrap();

        // Initiator records the offer in its OWN local manager (the host app
        // does not share state with the recipient's protocol handler), then
        // sends it over the wire.
        let mut host_negotiations = crate::screen_share::session::NegotiationManager::new();
        let offer = ScreenShareMessage::ScreenShareOffer {
            version: SCREEN_SHARE_PROTOCOL_VERSION,
            session_id,
            host_id: host_pk,
            conversation_id: 7,
            codecs: vec!["h264".into(), "vp8".into()],
            resolutions: vec![(1920, 1080), (1280, 720)],
            frame_rate_min: 15,
            frame_rate_max: 30,
            target_bitrate_bps: 2_000_000,
            remote_control: false,
        };
        host_negotiations.start_offer(offer.clone(), viewer.secret_key().public(), NEGOTIATION_TIMEOUT).unwrap();
        transport.send_screen_share(&offer).await.unwrap();

        // Recipient: protocol handler emits a NegotiationInvitation.
        let event = events_rx.recv().await.unwrap();
        let SessionEvent::NegotiationInvitation { session_id: got_id, host_id, offer: got_offer, .. } = event else {
            panic!("expected NegotiationInvitation, got {event:?}");
        };
        assert_eq!(got_id, session_id);
        assert_eq!(host_id, host_pk);
        let ScreenShareMessage::ScreenShareOffer { resolutions, .. } = &got_offer else { panic!("offer") };
        assert_eq!(resolutions, &vec![(1920, 1080), (1280, 720)]);

        // Recipient answers on the same inbound connection with an accept
        // carrying a mutually supported configuration.
        let selected = crate::screen_share::session::NegotiatedConfig::select(
            &got_offer,
            &["h264".to_string()],
        )
        .unwrap();
        let accept = ScreenShareMessage::ScreenShareAccept {
            version: SCREEN_SHARE_PROTOCOL_VERSION,
            session_id,
            codec: selected.codec,
            width: selected.width,
            height: selected.height,
            frame_rate: selected.frame_rate,
        };
        protocol.send_screen_share(session_id, accept.clone()).await.unwrap();

        // Initiator reads the Accept and applies it; capture is then allowed.
        let (mut send, recv) = connection.accept_bi().await.unwrap();
        match read_unit(recv).await.unwrap() {
            ReadUnit::ScreenShare(ScreenShareMessage::ScreenShareAccept { session_id: got, codec, width, height, frame_rate, .. }) => {
                assert_eq!(got, session_id);
                assert_eq!(codec, "h264");
                assert_eq!((width, height), (1920, 1080));
                assert_eq!(frame_rate, 30);
            }
            other => panic!("expected ScreenShareAccept, got {other:?}"),
        }
        drop(send);
        {
            host_negotiations.handle_accept(viewer.secret_key().public(), accept, &negotiation_events_tx).unwrap();
        }
        assert!(host_negotiations.can_start_capture(session_id), "capture allowed after explicit accept");

        // Duplicate offer: the same session id is refused with an explicit
        // ScreenShareReject rather than a silent state change.
        transport.send_screen_share(&offer).await.unwrap();
        let (mut send, recv) = connection.accept_bi().await.unwrap();
        match read_unit(recv).await.unwrap() {
            ReadUnit::ScreenShare(ScreenShareMessage::ScreenShareReject { session_id: id, reason, .. }) => {
                assert_eq!(id, session_id);
                assert_eq!(reason, "duplicate offer");
            }
            other => panic!("expected ScreenShareReject for duplicate, got {other:?}"),
        }
        drop(send);

        router.shutdown().await.unwrap();
    }
}
