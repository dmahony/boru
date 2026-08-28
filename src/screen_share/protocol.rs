//! Versioned, bounded control protocol for screen-sharing negotiation.
#![allow(missing_docs)]

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use tokio::io::AsyncWriteExt;
use tokio::sync::{mpsc, Mutex};

use super::codec::QualityProfile;
use super::permissions::{Capability, MAX_CAPABILITIES};
use super::session::{NegotiationManager, ScreenShareSessionId, SessionEvent, SessionManager};
use super::transport::{MediaHeader, QuicScreenTransport, ReadUnit, MAX_MEDIA_FRAME};
use super::ScreenShareError;

/// ALPN registered on the shared Iroh endpoint router.
pub const SCREEN_SHARE_ALPN: &[u8] = b"boru/screen-share/1";
/// Current wire protocol version. Major versions are not compatible.
pub const SCREEN_SHARE_PROTOCOL_VERSION: u16 = 1;
/// Upper bound for the input `code` field (X11 keysyms live below 0xFFFF).
pub const MAX_INPUT_CODE: u32 = 0xFFFF;
/// Maximum encoded control frame, including no transport framing overhead.
pub const MAX_CONTROL_FRAME: usize = 16 * 1024;
/// Maximum cursor sprite edge in pixels (BORU-SS-33). Cursor sprites are tiny
/// (typically <= 64x64); the cap keeps a `CursorShape` message far below the
/// control-frame bound even after hotspot padding.
pub const MAX_CURSOR_DIM: u16 = 128;
/// Maximum cursor sprite payload bytes (BORU-SS-33): `MAX_CURSOR_DIM^2 * 4`.
pub const MAX_CURSOR_SHAPE_BYTES: usize = (MAX_CURSOR_DIM as usize) * (MAX_CURSOR_DIM as usize) * 4;
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
/// Maximum encoded audio frame accepted in one `AudioPacket` (BORU-SS-37).
/// Opus packets are at most 1275 bytes (RFC 6716 §3.2); 4096 gives headroom
/// for future codecs while keeping untrusted peer input bounded.
pub const MAX_AUDIO_FRAME: usize = 4096;
/// Minimum Opus sample rate (RFC 6716 §2.1.1 supports 8/12/16/24/48 kHz).
pub const MIN_AUDIO_SAMPLE_RATE: u32 = 8_000;
/// Maximum Opus sample rate.
pub const MAX_AUDIO_SAMPLE_RATE: u32 = 48_000;

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

/// How the host maps the shared desktop onto the stream (PDF Phase 14 /
/// BORU-SS-38 multi-monitor switching).
///
/// The mode is carried on [`ScreenShareMessage::StreamConfig`] so the viewer
/// knows whether the source is a single monitor, one display at a time with
/// viewer-requested switching, or the whole virtual desktop in one stream.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum SourceMode {
    /// One fixed monitor, no switching.
    #[default]
    Single,
    /// One display at a time; the viewer may request a switch to another
    /// display (BORU-SS-38 `RequestSource`), the host decides.
    PerDisplay,
    /// The whole virtual desktop as a single stream (whole-root capture on
    /// X11; portal/Windows limits documented at the backend).
    Spanning,
}

impl SourceMode {
    /// Compact wire representation (0 = Single, 1 = PerDisplay, 2 = Spanning).
    pub const fn as_u8(self) -> u8 {
        match self {
            SourceMode::Single => 0,
            SourceMode::PerDisplay => 1,
            SourceMode::Spanning => 2,
        }
    }

    /// Decode the compact wire representation; `None` for unknown values.
    pub const fn from_u8(value: u8) -> Option<Self> {
        match value {
            0 => Some(SourceMode::Single),
            1 => Some(SourceMode::PerDisplay),
            2 => Some(SourceMode::Spanning),
            _ => None,
        }
    }
}

impl Serialize for SourceMode {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_u8(self.as_u8())
    }
}

// Backward compatibility (PDF Phase 14 / BORU-SS-38): `source_mode` was
// added as a TRAILING field on `StreamConfig`. Old peers encoded the struct
// WITHOUT it, so postcard's `SeqAccess::next_element_seed` returns `Err(EOF)`
// — not `Ok(None)` — when the buffer is exhausted before the declared field
// count, and serde's `#[serde(default)]` machinery never kicks in. The field
// is a single byte, so any remaining byte decodes as u8: `Err` ⟺ empty
// buffer ⟺ legacy message. Treat that exactly like the legacy default
// (`Single`), mirroring the `SignedMessage::compression` pattern.
impl<'de> Deserialize<'de> for SourceMode {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        match u8::deserialize(deserializer) {
            Ok(value) => SourceMode::from_u8(value)
                .ok_or_else(|| serde::de::Error::custom("invalid source mode")),
            Err(_) => Ok(SourceMode::Single),
        }
    }
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
    Accept {
        version: u16,
        session_id: ScreenShareSessionId,
    },
    /// Explicit recipient refusal or protocol failure.
    Reject {
        version: u16,
        session_id: ScreenShareSessionId,
        reason: String,
    },
    /// End a session. Repeating this message is safe and has no effect.
    EndSession {
        version: u16,
        session_id: ScreenShareSessionId,
    },
    /// Viewer asks the host for one or more explicitly selected controls.
    RequestControl {
        version: u16,
        session_id: ScreenShareSessionId,
        capabilities: Vec<Capability>,
    },
    /// Host grants the requested controls with a fresh session nonce.
    GrantControl {
        version: u16,
        session_id: ScreenShareSessionId,
        capabilities: Vec<Capability>,
        nonce: [u8; 16],
    },
    /// Host revokes control without ending view-only streaming.
    RevokeControl {
        version: u16,
        session_id: ScreenShareSessionId,
    },
    /// Input always carries the current grant nonce; stale input is rejected.
    /// `kind` says what kind of event this is (move/button/wheel/key/modifier);
    /// `code` is a button id (1-3) for pointer buttons, an X11 wheel button
    /// (4-7) for wheel ticks, or an X11 keysym for keyboard; `x`/`y` are
    /// normalized viewer coordinates (0..1 relative to the image rect) for
    /// pointer kinds and 0 for keyboard; `pressed` is the key/button state;
    /// `modifiers` is the viewer's current held-modifier bitmask.
    Input {
        version: u16,
        session_id: ScreenShareSessionId,
        nonce: [u8; 16],
        kind: InputEventKind,
        code: u32,
        x: f32,
        y: f32,
        pressed: bool,
        modifiers: u32,
    },
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
                if message.codecs.len() > MAX_CODECS {
                    return Err(ProtocolError::Malformed(
                        "too many codec capabilities".into(),
                    ));
                }
                if message.codecs.iter().any(|codec| {
                    codec.is_empty() || codec.len() > MAX_CODEC_NAME || !codec.is_ascii()
                }) {
                    return Err(ProtocolError::Malformed("invalid codec capability".into()));
                }
                if let Permission::Capabilities(capabilities) = &message.permission {
                    if capabilities.is_empty()
                        || capabilities.len() > MAX_CAPABILITIES
                        || capabilities.iter().any(|capability| {
                            capabilities
                                .iter()
                                .filter(|candidate| *candidate == capability)
                                .count()
                                > 1
                        })
                    {
                        return Err(ProtocolError::Malformed(
                            "invalid permission capability list".into(),
                        ));
                    }
                }
                if message.width == 0
                    || message.height == 0
                    || message.width > 16_384
                    || message.height > 16_384
                {
                    return Err(ProtocolError::Malformed("invalid dimensions".into()));
                }
                if message.frame_rate == 0 || message.frame_rate > 240 {
                    return Err(ProtocolError::Malformed("invalid frame rate".into()));
                }
                message.version
            }
            Self::Accept { version, .. }
            | Self::Reject { version, .. }
            | Self::EndSession { version, .. }
            | Self::RevokeControl { version, .. } => *version,
            Self::RequestControl {
                version,
                capabilities,
                ..
            }
            | Self::GrantControl {
                version,
                capabilities,
                ..
            } => {
                if capabilities.is_empty()
                    || capabilities.len() > MAX_CAPABILITIES
                    || capabilities.contains(&Capability::ViewScreen)
                {
                    return Err(ProtocolError::Malformed(
                        "invalid control capability request".into(),
                    ));
                }
                *version
            }
            Self::Input {
                version,
                kind,
                code,
                x,
                y,
                modifiers,
                ..
            } => {
                // The kind determines the capability; there is no separate
                // wire capability to mismatch (PDF Task 9.2).
                if !kind.is_pointer() {
                    // Keyboard/modifier events carry no pointer coordinates.
                    if *code > MAX_INPUT_CODE {
                        return Err(ProtocolError::Malformed("input code out of range".into()));
                    }
                    if *x != 0.0 || *y != 0.0 {
                        return Err(ProtocolError::Malformed(
                            "keyboard input coordinates must be zero".into(),
                        ));
                    }
                    if matches!(kind, InputEventKind::ModifierChange)
                        && *code & !MAX_MODIFIER_MASK != 0
                    {
                        return Err(ProtocolError::Malformed(
                            "modifier change code must be a valid modifier mask".into(),
                        ));
                    }
                } else {
                    if !x.is_finite()
                        || !y.is_finite()
                        || !(0.0..=1.0).contains(x)
                        || !(0.0..=1.0).contains(y)
                    {
                        return Err(ProtocolError::Malformed(
                            "input coordinates out of range".into(),
                        ));
                    }
                    match kind {
                        InputEventKind::PointerMove => {
                            if *code != 0 {
                                return Err(ProtocolError::Malformed(
                                    "pointer move code must be zero".into(),
                                ));
                            }
                        }
                        InputEventKind::PointerButton => {
                            if !(1..=3).contains(code) {
                                return Err(ProtocolError::Malformed(
                                    "invalid pointer button code".into(),
                                ));
                            }
                        }
                        InputEventKind::Wheel if !(4..=7).contains(code) => {
                            return Err(ProtocolError::Malformed("invalid wheel code".into()));
                        }
                        _ => {}
                    }
                }
                if *modifiers & !MAX_MODIFIER_MASK != 0 {
                    return Err(ProtocolError::Malformed("invalid modifier mask".into()));
                }
                *version
            }
        };
        if version != SCREEN_SHARE_PROTOCOL_VERSION {
            return Err(ProtocolError::UnsupportedVersion {
                received: version,
                supported: SCREEN_SHARE_PROTOCOL_VERSION,
            });
        }
        if let Self::Reject { reason, .. } = self {
            if reason.is_empty() || reason.len() > MAX_REASON {
                return Err(ProtocolError::Malformed("invalid rejection reason".into()));
            }
        }
        Ok(())
    }
}

/// Encode one postcard control message with a hard size bound.
pub fn encode(message: &ControlMessage) -> Result<Vec<u8>, ProtocolError> {
    message.validate()?;
    let bytes =
        postcard::to_stdvec(message).map_err(|e| ProtocolError::Malformed(e.to_string()))?;
    if bytes.len() > MAX_CONTROL_FRAME {
        return Err(ProtocolError::Malformed(
            "control frame exceeds size limit".into(),
        ));
    }
    Ok(bytes)
}

/// Decode one postcard control message with a hard size bound.
pub fn decode(bytes: &[u8]) -> Result<ControlMessage, ProtocolError> {
    if bytes.is_empty() || bytes.len() > MAX_CONTROL_FRAME {
        return Err(ProtocolError::Malformed(
            "invalid control frame length".into(),
        ));
    }
    let message: ControlMessage =
        postcard::from_bytes(bytes).map_err(|e| ProtocolError::Malformed(e.to_string()))?;
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
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
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
        /// How the host maps the shared desktop onto this stream
        /// (PDF Phase 14 / BORU-SS-38): `Single` (one fixed monitor),
        /// `PerDisplay` (one display at a time, viewer may request
        /// switches), or `Spanning` (whole virtual desktop). Backward
        /// compatible: a message from an OLD peer that predates this field
        /// decodes as [`SourceMode::Single`].
        #[serde(default)]
        source_mode: SourceMode,
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
    /// Viewer → host: request a switch to a different shared source (PDF
    /// Phase 14 / BORU-SS-38 multi-monitor switching). The host is the final
    /// arbiter: it honors the request only when the requested source is in
    /// its CURRENT enumeration (a monitor that was unplugged, or an id that
    /// was never valid, is denied). View-only peers may request — switching
    /// which display is shown does not imply any control capability.
    RequestSource {
        /// Wire protocol version.
        version: u16,
        /// Session the request applies to.
        session_id: ScreenShareSessionId,
        /// Stable id of the requested source, from the most recent
        /// `SourcesEnumerated` / `SourceChanged` (non-zero).
        source_id: u64,
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
    /// Host → viewer: one encoded Opus audio frame (BORU-SS-37 / PDF Phase 14
    /// system-audio sharing). System audio is a SEPARATE optional capability —
    /// it is never enabled automatically with the screen share; the host
    /// grants `Capability::Audio` (mirroring clipboard, PDF Task 9.3) before
    /// the first packet, and the viewer authorizes each packet against that
    /// grant. The payload is an Opus access unit (RFC 6716), bounded by
    /// [`MAX_AUDIO_FRAME`]. Audio rides a dedicated audio stream kind on the
    /// media path (drop-tolerant, never blocks video); this message is the
    /// canonical versioned representation used for negotiation/tests.
    AudioPacket {
        /// Wire protocol version.
        version: u16,
        /// Session the audio belongs to.
        session_id: ScreenShareSessionId,
        /// Monotonic packet sequence number. Must be non-zero.
        sequence: u64,
        /// Capture timestamp in microseconds.
        timestamp_us: u64,
        /// Sample rate of the decoded PCM in Hz (8000..=48000, RFC 6716).
        sample_rate: u32,
        /// Channel count of the decoded PCM (1 = mono, 2 = stereo).
        channels: u16,
        /// Encoded Opus access-unit bytes.
        payload: Vec<u8>,
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
        /// How the shared desktop maps onto the stream after this change
        /// (PDF Phase 14 / BORU-SS-38): `Single`, `PerDisplay` or
        /// `Spanning`. Backward compatible: a message from an OLD peer that
        /// predates this field decodes as [`SourceMode::Single`].
        #[serde(default)]
        source_mode: SourceMode,
    },
    /// Host → viewer: a cursor SHAPE update (PDF Task 5.3 `Metadata` cursor
    /// mode / BORU-SS-33). Sent on shape change only, never per move; the
    /// viewer caches the sprite and re-composites it at the latest
    /// [`Self::CursorPosition`].
    CursorShape {
        /// Wire protocol version.
        version: u16,
        /// Session the cursor shape belongs to.
        session_id: ScreenShareSessionId,
        /// Opaque shape identity assigned by the host (a monotonic counter).
        /// The viewer uses it to dedupe repeated shapes.
        shape_id: u32,
        /// Sprite width in pixels (1..=[`MAX_CURSOR_DIM`]).
        width: u16,
        /// Sprite height in pixels (1..=[`MAX_CURSOR_DIM`]).
        height: u16,
        /// Hotspot offset from the sprite's top-left (the pixel that "is"
        /// the cursor position).
        hotspot_x: u16,
        /// Hotspot offset from the sprite's top edge.
        hotspot_y: u16,
        /// BGRA8 sprite pixels, `width * height * 4` bytes, bounded by
        /// [`MAX_CURSOR_SHAPE_BYTES`].
        pixels: Vec<u8>,
    },
    /// Host → viewer: a cursor POSITION update (PDF Task 5.3 `Metadata`
    /// cursor mode / BORU-SS-33). Sent per move; the viewer re-composites
    /// the cached sprite at the normalized position. Position is normalized
    /// against the shared source image rect (`0..=1`), matching the input
    /// coordinate contract.
    CursorPosition {
        /// Wire protocol version.
        version: u16,
        /// Session the cursor position belongs to.
        session_id: ScreenShareSessionId,
        /// Normalized horizontal position within the source (`0..=1`).
        x: f32,
        /// Normalized vertical position within the source (`0..=1`).
        y: f32,
        /// Whether the cursor is currently visible.
        visible: bool,
    },
}

impl ScreenShareMessage {
    /// Validate untrusted wire data before applying it to session state.
    pub fn validate(&self) -> Result<(), ProtocolError> {
        let version = match self {
            Self::ScreenShareOffer {
                version,
                session_id,
                codecs,
                resolutions,
                frame_rate_min,
                frame_rate_max,
                target_bitrate_bps,
                ..
            } => {
                if *session_id == ScreenShareSessionId::zero() {
                    return Err(ProtocolError::Malformed("empty session id".into()));
                }
                if codecs.is_empty() || codecs.len() > MAX_CODECS {
                    return Err(ProtocolError::Malformed(
                        "invalid codec capability list".into(),
                    ));
                }
                if codecs.iter().any(|codec| {
                    codec.is_empty() || codec.len() > MAX_CODEC_NAME || !codec.is_ascii()
                }) {
                    return Err(ProtocolError::Malformed("invalid codec capability".into()));
                }
                if resolutions.is_empty() || resolutions.len() > MAX_RESOLUTIONS {
                    return Err(ProtocolError::Malformed("invalid resolution list".into()));
                }
                if resolutions.iter().any(|(width, height)| {
                    *width == 0 || *height == 0 || *width > 16_384 || *height > 16_384
                }) {
                    return Err(ProtocolError::Malformed("invalid dimensions".into()));
                }
                if *frame_rate_min == 0
                    || *frame_rate_max == 0
                    || *frame_rate_min > 240
                    || *frame_rate_max > 240
                    || *frame_rate_min > *frame_rate_max
                {
                    return Err(ProtocolError::Malformed("invalid frame rate range".into()));
                }
                if *target_bitrate_bps == 0 {
                    return Err(ProtocolError::Malformed("invalid bitrate".into()));
                }
                *version
            }
            Self::ScreenShareAccept {
                version,
                session_id,
                codec,
                width,
                height,
                frame_rate,
            } => {
                if *session_id == ScreenShareSessionId::zero() {
                    return Err(ProtocolError::Malformed("empty session id".into()));
                }
                if codec.is_empty() || codec.len() > MAX_CODEC_NAME || !codec.is_ascii() {
                    return Err(ProtocolError::Malformed("invalid selected codec".into()));
                }
                if *width == 0 || *height == 0 || *width > 16_384 || *height > 16_384 {
                    return Err(ProtocolError::Malformed("invalid dimensions".into()));
                }
                if *frame_rate == 0 || *frame_rate > 240 {
                    return Err(ProtocolError::Malformed("invalid frame rate".into()));
                }
                *version
            }
            Self::ScreenShareStarted {
                version,
                session_id,
            }
            | Self::KeyframeRequest {
                version,
                session_id,
            } => {
                if *session_id == ScreenShareSessionId::zero() {
                    return Err(ProtocolError::Malformed("empty session id".into()));
                }
                *version
            }
            Self::RequestSource {
                version,
                session_id,
                source_id,
            } => {
                if *session_id == ScreenShareSessionId::zero() {
                    return Err(ProtocolError::Malformed("empty session id".into()));
                }
                if *source_id == 0 {
                    return Err(ProtocolError::Malformed("empty source id".into()));
                }
                *version
            }
            Self::ScreenShareReject {
                version,
                session_id,
                reason,
            }
            | Self::ScreenShareStopped {
                version,
                session_id,
                reason,
            } => {
                if *session_id == ScreenShareSessionId::zero() {
                    return Err(ProtocolError::Malformed("empty session id".into()));
                }
                if reason.is_empty() || reason.len() > MAX_REASON {
                    return Err(ProtocolError::Malformed("invalid reason text".into()));
                }
                *version
            }
            Self::StreamConfig {
                version,
                session_id,
                width,
                height,
                frame_rate,
                target_bitrate_bps,
                codec,
                keyframe_interval,
                quality_profile,
                source_mode,
            } => {
                if *session_id == ScreenShareSessionId::zero() {
                    return Err(ProtocolError::Malformed("empty session id".into()));
                }
                if *width == 0 || *height == 0 || *width > 16_384 || *height > 16_384 {
                    return Err(ProtocolError::Malformed("invalid dimensions".into()));
                }
                if *frame_rate == 0 || *frame_rate > 240 {
                    return Err(ProtocolError::Malformed("invalid frame rate".into()));
                }
                if *target_bitrate_bps == 0 {
                    return Err(ProtocolError::Malformed("invalid bitrate".into()));
                }
                if codec.is_empty() || codec.len() > MAX_CODEC_NAME || !codec.is_ascii() {
                    return Err(ProtocolError::Malformed("invalid codec".into()));
                }
                if *keyframe_interval == 0 {
                    return Err(ProtocolError::Malformed("invalid keyframe interval".into()));
                }
                if QualityProfile::from_u8(*quality_profile).is_none() {
                    return Err(ProtocolError::Malformed("invalid quality profile".into()));
                }
                if SourceMode::from_u8(source_mode.as_u8()).is_none() {
                    return Err(ProtocolError::Malformed("invalid source mode".into()));
                }
                *version
            }
            Self::VideoPacket {
                version,
                session_id,
                sequence,
                width,
                height,
                payload,
                ..
            } => {
                if *session_id == ScreenShareSessionId::zero() {
                    return Err(ProtocolError::Malformed("empty session id".into()));
                }
                if *sequence == 0 {
                    return Err(ProtocolError::Malformed("invalid media sequence".into()));
                }
                if *width == 0 || *height == 0 || *width > 16_384 || *height > 16_384 {
                    return Err(ProtocolError::Malformed("invalid dimensions".into()));
                }
                if payload.is_empty() || payload.len() > MAX_MEDIA_FRAME {
                    return Err(ProtocolError::Malformed("invalid media payload".into()));
                }
                *version
            }
            Self::QualityUpdate {
                version,
                session_id,
                target_bitrate_bps,
                max_frame_rate,
                scale_factor,
            } => {
                if *session_id == ScreenShareSessionId::zero() {
                    return Err(ProtocolError::Malformed("empty session id".into()));
                }
                if *target_bitrate_bps == 0 {
                    return Err(ProtocolError::Malformed("invalid bitrate".into()));
                }
                if *max_frame_rate == 0 || *max_frame_rate > 240 {
                    return Err(ProtocolError::Malformed("invalid frame rate".into()));
                }
                if *scale_factor == 0 || *scale_factor > 100 {
                    return Err(ProtocolError::Malformed("invalid quality scale".into()));
                }
                *version
            }
            Self::Error {
                version,
                session_id,
                code,
                message,
            } => {
                if *session_id == ScreenShareSessionId::zero() {
                    return Err(ProtocolError::Malformed("empty session id".into()));
                }
                if *code == 0 {
                    return Err(ProtocolError::Malformed("invalid error code".into()));
                }
                if message.is_empty() || message.len() > MAX_REASON {
                    return Err(ProtocolError::Malformed("invalid error message".into()));
                }
                *version
            }
            Self::Clipboard {
                version,
                session_id,
                text,
                ..
            } => {
                if *session_id == ScreenShareSessionId::zero() {
                    return Err(ProtocolError::Malformed("empty session id".into()));
                }
                if text.as_str().is_empty() || text.as_str().len() > MAX_CLIPBOARD_TEXT {
                    return Err(ProtocolError::Malformed("invalid clipboard text".into()));
                }
                *version
            }
            Self::AudioPacket {
                version,
                session_id,
                sequence,
                sample_rate,
                channels,
                payload,
                ..
            } => {
                if *session_id == ScreenShareSessionId::zero() {
                    return Err(ProtocolError::Malformed("empty session id".into()));
                }
                if *sequence == 0 {
                    return Err(ProtocolError::Malformed("invalid audio sequence".into()));
                }
                if *sample_rate < MIN_AUDIO_SAMPLE_RATE || *sample_rate > MAX_AUDIO_SAMPLE_RATE {
                    return Err(ProtocolError::Malformed("invalid audio sample rate".into()));
                }
                if *channels == 0 || *channels > 2 {
                    return Err(ProtocolError::Malformed(
                        "invalid audio channel count".into(),
                    ));
                }
                if payload.is_empty() || payload.len() > MAX_AUDIO_FRAME {
                    return Err(ProtocolError::Malformed("invalid audio payload".into()));
                }
                *version
            }
            Self::SourceChanged {
                version,
                session_id,
                source_id,
                title,
                width,
                height,
                frame_rate,
                source_mode,
            } => {
                if *session_id == ScreenShareSessionId::zero() {
                    return Err(ProtocolError::Malformed("empty session id".into()));
                }
                if *source_id == 0 {
                    return Err(ProtocolError::Malformed("empty source id".into()));
                }
                if title.is_empty() || title.len() > MAX_SOURCE_NAME || !title.is_ascii() {
                    return Err(ProtocolError::Malformed("invalid source title".into()));
                }
                if *width == 0 || *height == 0 || *width > 16_384 || *height > 16_384 {
                    return Err(ProtocolError::Malformed("invalid dimensions".into()));
                }
                if *frame_rate == 0 || *frame_rate > 240 {
                    return Err(ProtocolError::Malformed("invalid frame rate".into()));
                }
                if SourceMode::from_u8(source_mode.as_u8()).is_none() {
                    return Err(ProtocolError::Malformed("invalid source mode".into()));
                }
                *version
            }
            Self::CursorShape {
                version,
                session_id,
                width,
                height,
                hotspot_x,
                hotspot_y,
                pixels,
                ..
            } => {
                if *session_id == ScreenShareSessionId::zero() {
                    return Err(ProtocolError::Malformed("empty session id".into()));
                }
                if *width == 0
                    || *height == 0
                    || *width > MAX_CURSOR_DIM
                    || *height > MAX_CURSOR_DIM
                {
                    return Err(ProtocolError::Malformed("invalid cursor dimensions".into()));
                }
                if *hotspot_x >= *width || *hotspot_y >= *height {
                    return Err(ProtocolError::Malformed(
                        "cursor hotspot outside sprite".into(),
                    ));
                }
                let expected = (*width as usize) * (*height as usize) * 4;
                if pixels.len() != expected {
                    return Err(ProtocolError::Malformed(
                        "cursor sprite pixel buffer mismatch".into(),
                    ));
                }
                if pixels.len() > MAX_CURSOR_SHAPE_BYTES {
                    return Err(ProtocolError::Malformed(
                        "cursor sprite exceeds size limit".into(),
                    ));
                }
                *version
            }
            Self::CursorPosition {
                version,
                session_id,
                x,
                y,
                ..
            } => {
                if *session_id == ScreenShareSessionId::zero() {
                    return Err(ProtocolError::Malformed("empty session id".into()));
                }
                if !x.is_finite()
                    || !y.is_finite()
                    || !(0.0..=1.0).contains(x)
                    || !(0.0..=1.0).contains(y)
                {
                    return Err(ProtocolError::Malformed(
                        "cursor position out of range".into(),
                    ));
                }
                *version
            }
        };
        if version != SCREEN_SHARE_PROTOCOL_VERSION {
            return Err(ProtocolError::UnsupportedVersion {
                received: version,
                supported: SCREEN_SHARE_PROTOCOL_VERSION,
            });
        }
        Ok(())
    }

    /// Encode one postcard protocol message after validation, with a hard
    /// size bound.
    pub fn encode(&self) -> Result<Vec<u8>, ProtocolError> {
        self.validate()?;
        let bytes =
            postcard::to_stdvec(self).map_err(|e| ProtocolError::Malformed(e.to_string()))?;
        if bytes.len() > MAX_SCREEN_SHARE_MESSAGE {
            return Err(ProtocolError::Malformed(
                "screen-share message exceeds size limit".into(),
            ));
        }
        Ok(bytes)
    }

    /// Decode one postcard protocol message with a hard size bound, then
    /// validate. Returns an error (never panics) on truncated input, an
    /// unknown discriminant, or a semantic invariant violation.
    pub fn decode(bytes: &[u8]) -> Result<Self, ProtocolError> {
        if bytes.is_empty() || bytes.len() > MAX_SCREEN_SHARE_MESSAGE {
            return Err(ProtocolError::Malformed(
                "invalid screen-share message length".into(),
            ));
        }
        let message: Self =
            postcard::from_bytes(bytes).map_err(|e| ProtocolError::Malformed(e.to_string()))?;
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

/// One validated audio unit forwarded from an inbound connection to the app
/// (BORU-SS-37). The capability gate is enforced by the protocol before the
/// unit is forwarded: audio is only delivered when the session holds an
/// explicit `Capability::Audio` grant.
#[derive(Debug, Clone)]
pub struct InboundAudio {
    /// Session the audio belongs to; the app's audio worker filters on this.
    pub session_id: ScreenShareSessionId,
    /// Validated audio header (sample rate / channels / sequence).
    pub header: super::transport::AudioHeader,
    /// Encoded Opus access-unit bytes (bounded by transport validation).
    pub payload: Vec<u8>,
}

/// Iroh protocol handler for `boru/screen-share/1`.
#[derive(Debug, Clone)]
pub struct ScreenShareProtocol {
    manager: Arc<Mutex<SessionManager>>,
    negotiations: Arc<Mutex<NegotiationManager>>,
    events: mpsc::Sender<SessionEvent>,
    media_tx: mpsc::Sender<InboundMedia>,
    /// Audio units (BORU-SS-37), delivered only after the `Audio` capability
    /// is granted. Bounded; a full queue drops the newest audio (audio is
    /// real-time and drop-tolerant, never blocks video).
    audio_tx: mpsc::Sender<InboundAudio>,
    /// Inbound connections per session so the app can respond (Accept/Reject/
    /// EndSession) on the same connection the invitation arrived on.
    connections: Arc<Mutex<HashMap<ScreenShareSessionId, (usize, iroh::endpoint::Connection)>>>,
}

impl ScreenShareProtocol {
    /// Create a handler and its session state store. Media units are dropped.
    pub fn new(events: mpsc::Sender<SessionEvent>) -> Self {
        let (media_tx, _dropped_rx) = mpsc::channel(1);
        let (audio_tx, _dropped_audio_rx) = mpsc::channel(1);
        Self::with_channels(events, media_tx, audio_tx)
    }

    /// Create a handler that forwards inbound media to `media_tx` and
    /// inbound audio to `audio_tx`.
    pub fn with_channels(
        events: mpsc::Sender<SessionEvent>,
        media_tx: mpsc::Sender<InboundMedia>,
        audio_tx: mpsc::Sender<InboundAudio>,
    ) -> Self {
        Self {
            manager: Arc::new(Mutex::new(SessionManager::default())),
            negotiations: Arc::new(Mutex::new(NegotiationManager::default())),
            events,
            media_tx,
            audio_tx,
            connections: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    /// Access the state machine for locally initiated sessions.
    pub fn manager(&self) -> Arc<Mutex<SessionManager>> {
        Arc::clone(&self.manager)
    }

    /// Access the versioned negotiation state machine (PDF Task 3.1).
    pub fn negotiations(&self) -> Arc<Mutex<NegotiationManager>> {
        Arc::clone(&self.negotiations)
    }

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
            connections
                .get(&session_id)
                .map(|(_, connection)| connection.clone())
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
            connections
                .get(&session_id)
                .map(|(_, connection)| connection.clone())
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
    async fn accept(
        &self,
        connection: iroh::endpoint::Connection,
    ) -> Result<(), iroh::protocol::AcceptError> {
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
                        Ok(ReadUnit::Audio(header, payload)) => {
                            // BORU-SS-37: audio is a SEPARATE optional
                            // capability. A packet is only forwarded to the
                            // app when the session holds an explicit Audio
                            // grant (the host grants it via GrantControl
                            // before streaming; the viewer's permissions are
                            // the consent record). Unauthorized audio is
                            // dropped without applying or logging payload
                            // contents.
                            let session_id = ScreenShareSessionId::from_bytes(header.session_id);
                            let authorized = self
                                .manager
                                .lock()
                                .await
                                .permissions(session_id)
                                .is_some_and(|permissions| {
                                    permissions.allows(session_id, remote_id, Capability::Audio)
                                });
                            if !authorized {
                                tracing::warn!(session = ?session_id, "screen-share: viewer dropped unauthorized audio packet");
                                continue;
                            }
                            let sequence = header.sequence;
                            let dropped = self
                                .audio_tx
                                .try_send(InboundAudio {
                                    session_id,
                                    header,
                                    payload,
                                })
                                .is_err();
                            if dropped {
                                tracing::warn!(sequence, "screen-share: viewer audio dropped (channel full)");
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
                    self.negotiations.lock().await.receive_offer(
                        remote_id,
                        message,
                        NEGOTIATION_TIMEOUT,
                        &self.events,
                    )
                };
                match result {
                    Ok(()) => {
                        // Keep the inbound connection so the app can answer the
                        // offer (Accept/Reject) on the same connection.
                        self.connections
                            .lock()
                            .await
                            .insert(session_id, (stable_id, connection.clone()));
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
                        )
                        .await;
                    }
                }
            }
            ScreenShareMessage::ScreenShareAccept { session_id, .. } => {
                let result = {
                    self.negotiations
                        .lock()
                        .await
                        .handle_accept(remote_id, message, &self.events)
                };
                if let Err(error) = result {
                    tracing::warn!(
                        ?session_id,
                        ?error,
                        "screen-share: versioned Accept rejected"
                    );
                    let reason = negotiation_reject_reason(&error);
                    let _ = write_screen_share_new_stream(
                        connection,
                        &ScreenShareMessage::ScreenShareReject {
                            version: SCREEN_SHARE_PROTOCOL_VERSION,
                            session_id,
                            reason,
                        },
                    )
                    .await;
                }
                self.connections.lock().await.remove(&session_id);
            }
            ScreenShareMessage::ScreenShareReject { session_id, .. } => {
                let _ = {
                    self.negotiations
                        .lock()
                        .await
                        .handle_reject(remote_id, message, &self.events)
                };
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
            ScreenShareMessage::Clipboard {
                session_id,
                nonce,
                text,
                ..
            } => {
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
                    let _ = self
                        .events
                        .try_send(SessionEvent::ClipboardReceived { session_id, text });
                }
            }
            // PDF Phase 10: the host switched the shared source (monitor) or
            // the platform renegotiated geometry. Surface the change to the
            // app BEFORE the following media units carry the new dimensions
            // so the viewer can update its UI / decoder state in time.
            ScreenShareMessage::SourceChanged {
                session_id,
                source_id,
                title,
                width,
                height,
                frame_rate,
                source_mode,
                ..
            } => {
                tracing::info!(session = ?session_id, source_id, title = %title, width, height, frame_rate, mode = ?source_mode, "screen-share: viewer source change announced");
                let _ = self.events.try_send(SessionEvent::SourceChanged {
                    session_id,
                    source_id,
                    title,
                    width: width as u32,
                    height: height as u32,
                    source_mode,
                });
            }
            // BORU-SS-33: metadata cursor mode (PDF Task 5.3). The host
            // delivers the cursor as shape-on-change + position-per-move
            // control messages; the viewer re-composites the cached sprite
            // at the reported position instead of receiving it baked into
            // the video frames. Surface both to the app so the viewer can
            // render the remote cursor as an overlay.
            ScreenShareMessage::CursorShape {
                session_id,
                width,
                height,
                hotspot_x,
                hotspot_y,
                pixels,
                ..
            } => {
                if let Ok(sprite) = crate::screen_share::coords::CursorSprite::new(
                    width as u32,
                    height as u32,
                    hotspot_x as u32,
                    hotspot_y as u32,
                    pixels,
                ) {
                    let _ = self
                        .events
                        .try_send(SessionEvent::CursorShape { session_id, sprite });
                }
            }
            ScreenShareMessage::CursorPosition {
                session_id,
                x,
                y,
                visible,
                ..
            } => {
                let _ = self.events.try_send(SessionEvent::CursorPosition {
                    session_id,
                    x,
                    y,
                    visible,
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
        E::UnsupportedConfig(detail) => {
            format!("selected configuration is not mutually supported: {detail}")
        }
        E::EmptySessionId => "empty session id".into(),
    }
}

async fn write_message(
    send: &mut iroh::endpoint::SendStream,
    message: &ControlMessage,
) -> Result<(), ProtocolError> {
    let bytes = encode(message)?;
    send.write_u8(0x01)
        .await
        .map_err(|e| ProtocolError::Io(e.to_string()))?;
    send.write_u32(bytes.len() as u32)
        .await
        .map_err(|e| ProtocolError::Io(e.to_string()))?;
    send.write_all(&bytes)
        .await
        .map_err(|e| ProtocolError::Io(e.to_string()))?;
    send.finish()
        .map_err(|e| ProtocolError::Io(e.to_string()))?;
    Ok(())
}

/// Write one versioned protocol message on an accepted stream, mirroring the
/// transport's `SCREEN_SHARE_KIND` framing.
async fn write_screen_share_message(
    send: &mut iroh::endpoint::SendStream,
    message: &ScreenShareMessage,
) -> Result<(), ProtocolError> {
    let bytes = message.encode()?;
    send.write_u8(0x03)
        .await
        .map_err(|e| ProtocolError::Io(e.to_string()))?;
    send.write_u32(bytes.len() as u32)
        .await
        .map_err(|e| ProtocolError::Io(e.to_string()))?;
    send.write_all(&bytes)
        .await
        .map_err(|e| ProtocolError::Io(e.to_string()))?;
    send.finish()
        .map_err(|e| ProtocolError::Io(e.to_string()))?;
    Ok(())
}

/// Open a fresh stream on `connection` and write one versioned protocol
/// message on it. Protocol-level replies (a Reject for a bad offer) must go
/// on a NEW stream: the peer opened the stream the request arrived on and is
/// reading replies via `accept_bi()`, so writing on the request's own stream
/// would strand the reply where nobody reads it.
async fn write_screen_share_new_stream(
    connection: &iroh::endpoint::Connection,
    message: &ScreenShareMessage,
) -> Result<(), ProtocolError> {
    let (mut send, _) = connection
        .open_bi()
        .await
        .map_err(|e| ProtocolError::Io(e.to_string()))?;
    write_screen_share_message(&mut send, message).await
}

/// A conservative timeout for negotiation streams.
pub const NEGOTIATION_TIMEOUT: Duration = Duration::from_secs(30);

#[cfg(test)]
mod tests {
    use super::*;
    use crate::screen_share::session::SessionState;
    use crate::screen_share::{
        capture::{PixelFormat, ScreenCapture},
        codec::{
            Av1Decoder, Av1Encoder, CodecConfig, OpenH264Decoder, OpenH264Encoder, VideoEncoder,
            DEFAULT_QUEUE_CAPACITY,
        },
        transport::{read_unit, AudioHeader, QuicScreenTransport, ReadUnit},
        viewer::ViewerPipeline,
        TestPatternCapture,
    };
    use iroh::endpoint::presets;
    use iroh::protocol::Router;

    fn hello() -> Hello {
        Hello {
            version: 1,
            session_id: ScreenShareSessionId::from_bytes([1; 16]),
            host_id: iroh::SecretKey::generate().public(),
            conversation_id: 7,
            codecs: vec!["h264".into()],
            width: 1920,
            height: 1080,
            frame_rate: 30,
            permission: Permission::ViewOnly,
        }
    }
    #[test]
    fn round_trip() {
        let message = ControlMessage::Hello(hello());
        assert_eq!(decode(&encode(&message).unwrap()).unwrap(), message);
    }
    #[test]
    fn input_wire_round_trip_carries_pointer_state() {
        let message = ControlMessage::Input {
            version: SCREEN_SHARE_PROTOCOL_VERSION,
            session_id: ScreenShareSessionId::from_bytes([7; 16]),
            nonce: [3; 16],
            kind: InputEventKind::PointerButton,
            code: 1,
            x: 0.5,
            y: 0.25,
            pressed: true,
            modifiers: MOD_SHIFT,
        };
        assert_eq!(decode(&encode(&message).unwrap()).unwrap(), message);
        let bad_x = ControlMessage::Input {
            version: SCREEN_SHARE_PROTOCOL_VERSION,
            session_id: ScreenShareSessionId::from_bytes([7; 16]),
            nonce: [3; 16],
            kind: InputEventKind::PointerButton,
            code: 1,
            x: 1.5,
            y: 0.25,
            pressed: true,
            modifiers: 0,
        };
        assert!(encode(&bad_x).is_err());
        // A wheel tick with a valid direction round-trips too.
        let wheel = ControlMessage::Input {
            version: SCREEN_SHARE_PROTOCOL_VERSION,
            session_id: ScreenShareSessionId::from_bytes([7; 16]),
            nonce: [3; 16],
            kind: InputEventKind::Wheel,
            code: 4,
            x: 0.5,
            y: 0.25,
            pressed: true,
            modifiers: 0,
        };
        assert_eq!(decode(&encode(&wheel).unwrap()).unwrap(), wheel);
    }
    #[test]
    fn input_kind_validation_is_explicit() {
        let sid = ScreenShareSessionId::from_bytes([7; 16]);
        // Pointer move must carry code 0 and normalized coordinates.
        let move_ok = ControlMessage::Input {
            version: SCREEN_SHARE_PROTOCOL_VERSION,
            session_id: sid,
            nonce: [0; 16],
            kind: InputEventKind::PointerMove,
            code: 0,
            x: 0.5,
            y: 0.5,
            pressed: false,
            modifiers: 0,
        };
        assert!(encode(&move_ok).is_ok());
        let move_bad_code = ControlMessage::Input {
            version: SCREEN_SHARE_PROTOCOL_VERSION,
            session_id: sid,
            nonce: [0; 16],
            kind: InputEventKind::PointerMove,
            code: 1,
            x: 0.5,
            y: 0.5,
            pressed: false,
            modifiers: 0,
        };
        assert!(encode(&move_bad_code).is_err());
        // Pointer buttons are 1-3.
        let button_bad = ControlMessage::Input {
            version: SCREEN_SHARE_PROTOCOL_VERSION,
            session_id: sid,
            nonce: [0; 16],
            kind: InputEventKind::PointerButton,
            code: 9,
            x: 0.5,
            y: 0.5,
            pressed: false,
            modifiers: 0,
        };
        assert!(encode(&button_bad).is_err());
        // Wheel is 4-7.
        let wheel_bad = ControlMessage::Input {
            version: SCREEN_SHARE_PROTOCOL_VERSION,
            session_id: sid,
            nonce: [0; 16],
            kind: InputEventKind::Wheel,
            code: 1,
            x: 0.5,
            y: 0.5,
            pressed: false,
            modifiers: 0,
        };
        assert!(encode(&wheel_bad).is_err());
        // Keyboard events carry no coordinates.
        let key_with_coords = ControlMessage::Input {
            version: SCREEN_SHARE_PROTOCOL_VERSION,
            session_id: sid,
            nonce: [0; 16],
            kind: InputEventKind::Key,
            code: 0x61,
            x: 0.5,
            y: 0.0,
            pressed: false,
            modifiers: 0,
        };
        assert!(encode(&key_with_coords).is_err());
        // Modifier mask is bounded to the known bits.
        let bad_mods = ControlMessage::Input {
            version: SCREEN_SHARE_PROTOCOL_VERSION,
            session_id: sid,
            nonce: [0; 16],
            kind: InputEventKind::Key,
            code: 0x61,
            x: 0.0,
            y: 0.0,
            pressed: false,
            modifiers: 1 << 20,
        };
        assert!(encode(&bad_mods).is_err());
        // A modifier change with a valid mask round-trips.
        let mod_change = ControlMessage::Input {
            version: SCREEN_SHARE_PROTOCOL_VERSION,
            session_id: sid,
            nonce: [0; 16],
            kind: InputEventKind::ModifierChange,
            code: MOD_SHIFT | MOD_CTRL,
            x: 0.0,
            y: 0.0,
            pressed: false,
            modifiers: MOD_SHIFT | MOD_CTRL,
        };
        assert_eq!(decode(&encode(&mod_change).unwrap()).unwrap(), mod_change);
    }
    #[test]
    fn input_kind_derives_control_capability() {
        assert_eq!(
            InputEventKind::PointerMove.capability(),
            Capability::ControlPointer
        );
        assert_eq!(
            InputEventKind::PointerButton.capability(),
            Capability::ControlPointer
        );
        assert_eq!(
            InputEventKind::Wheel.capability(),
            Capability::ControlPointer
        );
        assert_eq!(
            InputEventKind::Key.capability(),
            Capability::ControlKeyboard
        );
        assert_eq!(
            InputEventKind::ModifierChange.capability(),
            Capability::ControlKeyboard
        );
        assert!(InputEventKind::PointerMove.is_pointer());
        assert!(!InputEventKind::Key.is_pointer());
    }
    #[test]
    fn malformed_and_unsupported_are_rejected() {
        assert!(decode(&[0xff]).is_err());
        let mut message = hello();
        message.version = 2;
        assert!(matches!(
            encode(&ControlMessage::Hello(message)),
            Err(ProtocolError::UnsupportedVersion { .. })
        ));
    }
    #[test]
    fn accept_is_explicit() {
        let mut manager = SessionManager::default();
        let id = ScreenShareSessionId::from_bytes([2; 16]);
        let host = hello().host_id;
        let viewer = iroh::SecretKey::generate().public();
        manager.start_invitation(id, host, viewer, 7);
        assert_eq!(manager.state(id), Some(SessionState::AwaitingAcceptance));
    }

    fn sid() -> ScreenShareSessionId {
        ScreenShareSessionId::from_bytes([7; 16])
    }
    fn offer() -> ScreenShareMessage {
        ScreenShareMessage::ScreenShareOffer {
            version: SCREEN_SHARE_PROTOCOL_VERSION,
            session_id: sid(),
            host_id: iroh::SecretKey::generate().public(),
            conversation_id: 7,
            codecs: vec!["h264".into()],
            resolutions: vec![(1920, 1080), (1280, 720)],
            frame_rate_min: 15,
            frame_rate_max: 30,
            target_bitrate_bps: 2_000_000,
            remote_control: false,
        }
    }
    fn accept() -> ScreenShareMessage {
        ScreenShareMessage::ScreenShareAccept {
            version: SCREEN_SHARE_PROTOCOL_VERSION,
            session_id: sid(),
            codec: "h264".into(),
            width: 1280,
            height: 720,
            frame_rate: 30,
        }
    }
    fn reject() -> ScreenShareMessage {
        ScreenShareMessage::ScreenShareReject {
            version: SCREEN_SHARE_PROTOCOL_VERSION,
            session_id: sid(),
            reason: "user declined".into(),
        }
    }
    fn started() -> ScreenShareMessage {
        ScreenShareMessage::ScreenShareStarted {
            version: SCREEN_SHARE_PROTOCOL_VERSION,
            session_id: sid(),
        }
    }
    fn stopped() -> ScreenShareMessage {
        ScreenShareMessage::ScreenShareStopped {
            version: SCREEN_SHARE_PROTOCOL_VERSION,
            session_id: sid(),
            reason: "host ended".into(),
        }
    }
    fn stream_config() -> ScreenShareMessage {
        ScreenShareMessage::StreamConfig {
            version: SCREEN_SHARE_PROTOCOL_VERSION,
            session_id: sid(),
            width: 1280,
            height: 720,
            frame_rate: 30,
            target_bitrate_bps: 1_500_000,
            codec: "h264".into(),
            keyframe_interval: 120,
            quality_profile: QualityProfile::Balanced.as_u8(),
            source_mode: SourceMode::Single,
        }
    }
    fn video_packet() -> ScreenShareMessage {
        ScreenShareMessage::VideoPacket {
            version: SCREEN_SHARE_PROTOCOL_VERSION,
            session_id: sid(),
            sequence: 1,
            timestamp_us: 1_000,
            keyframe: true,
            config_generation: 0,
            width: 640,
            height: 360,
            payload: vec![0xAB; 32],
        }
    }
    fn keyframe_request() -> ScreenShareMessage {
        ScreenShareMessage::KeyframeRequest {
            version: SCREEN_SHARE_PROTOCOL_VERSION,
            session_id: sid(),
        }
    }
    fn request_source() -> ScreenShareMessage {
        ScreenShareMessage::RequestSource {
            version: SCREEN_SHARE_PROTOCOL_VERSION,
            session_id: sid(),
            source_id: 7,
        }
    }
    fn quality_update() -> ScreenShareMessage {
        ScreenShareMessage::QualityUpdate {
            version: SCREEN_SHARE_PROTOCOL_VERSION,
            session_id: sid(),
            target_bitrate_bps: 1_000_000,
            max_frame_rate: 30,
            scale_factor: 100,
        }
    }
    fn protocol_error() -> ScreenShareMessage {
        ScreenShareMessage::Error {
            version: SCREEN_SHARE_PROTOCOL_VERSION,
            session_id: sid(),
            code: 1,
            message: "encode failure".into(),
        }
    }
    fn clipboard() -> ScreenShareMessage {
        ScreenShareMessage::Clipboard {
            version: SCREEN_SHARE_PROTOCOL_VERSION,
            session_id: sid(),
            nonce: [0xAB; 16],
            text: RedactedText::new("hello clipboard".into()),
        }
    }
    fn audio_packet() -> ScreenShareMessage {
        ScreenShareMessage::AudioPacket {
            version: SCREEN_SHARE_PROTOCOL_VERSION,
            session_id: sid(),
            sequence: 1,
            timestamp_us: 1_000,
            sample_rate: 48_000,
            channels: 2,
            payload: vec![0xAA; 32],
        }
    }
    fn source_changed() -> ScreenShareMessage {
        ScreenShareMessage::SourceChanged {
            version: SCREEN_SHARE_PROTOCOL_VERSION,
            session_id: sid(),
            source_id: 7,
            title: "DP-1: 1920x1080".into(),
            width: 1920,
            height: 1080,
            frame_rate: 30,
            source_mode: SourceMode::PerDisplay,
        }
    }
    fn cursor_shape() -> ScreenShareMessage {
        ScreenShareMessage::CursorShape {
            version: SCREEN_SHARE_PROTOCOL_VERSION,
            session_id: sid(),
            shape_id: 1,
            width: 32,
            height: 32,
            hotspot_x: 16,
            hotspot_y: 16,
            pixels: vec![0xCD; 32 * 32 * 4],
        }
    }
    fn cursor_position() -> ScreenShareMessage {
        ScreenShareMessage::CursorPosition {
            version: SCREEN_SHARE_PROTOCOL_VERSION,
            session_id: sid(),
            x: 0.5,
            y: 0.25,
            visible: true,
        }
    }

    /// Every one of the Task 2.3 message types plus the Task 9.3 Clipboard,
    /// Task 10 SourceChanged, Task 14 RequestSource, Task 5.3 cursor
    /// messages, and the BORU-SS-37 AudioPacket message must survive a
    /// postcard encode → decode round trip unchanged.
    #[test]
    fn round_trip_all_screen_share_messages() {
        let messages = [
            offer(),
            accept(),
            reject(),
            started(),
            stopped(),
            stream_config(),
            video_packet(),
            keyframe_request(),
            request_source(),
            quality_update(),
            protocol_error(),
            clipboard(),
            audio_packet(),
            source_changed(),
            cursor_shape(),
            cursor_position(),
        ];
        assert_eq!(messages.len(), 16, "the Task 2.3 message set (ten) plus Clipboard, SourceChanged, RequestSource, CursorShape, CursorPosition and BORU-SS-37 AudioPacket must have sixteen types");
        for message in messages {
            let bytes = message.encode().expect("encode should succeed");
            assert_eq!(
                ScreenShareMessage::decode(&bytes).expect("decode should succeed"),
                message
            );
        }
    }

    /// BORU-SS-38: the `source_mode` field on `StreamConfig` is wire-encoded
    /// as a single byte and survives a round trip for every mode.
    #[test]
    fn source_mode_round_trips_on_stream_config() {
        for mode in [
            SourceMode::Single,
            SourceMode::PerDisplay,
            SourceMode::Spanning,
        ] {
            let mut message = stream_config();
            if let ScreenShareMessage::StreamConfig { source_mode, .. } = &mut message {
                *source_mode = mode;
            }
            let bytes = message.encode().expect("encode should succeed");
            let decoded = ScreenShareMessage::decode(&bytes).expect("decode should succeed");
            match decoded {
                ScreenShareMessage::StreamConfig {
                    source_mode: got, ..
                } => assert_eq!(got, mode),
                other => panic!("expected StreamConfig, got {other:?}"),
            }
        }
    }

    /// BORU-SS-38 backward compatibility: a `StreamConfig` from an OLD peer
    /// that predates the `source_mode` field (no trailing byte) decodes as
    /// [`SourceMode::Single`]. This mirrors the `SignedMessage::compression`
    /// pattern: postcard reports the exhausted buffer as `Err(EOF)`, which
    /// the custom `SourceMode` deserializer maps to the legacy default.
    #[test]
    fn stream_config_without_source_mode_decodes_as_single() {
        // Serialize the full message, then drop the trailing source_mode byte
        // to simulate an old peer's encoding. The enum discriminant, session
        // id and every other field stay in place.
        let mut message = stream_config();
        if let ScreenShareMessage::StreamConfig { source_mode, .. } = &mut message {
            *source_mode = SourceMode::Spanning;
        }
        let bytes = message.encode().expect("encode should succeed");
        // The last byte is the source_mode byte (u8 serializer writes 1 byte).
        let legacy = &bytes[..bytes.len() - 1];
        let decoded = ScreenShareMessage::decode(legacy).expect("legacy stream config must decode");
        match decoded {
            ScreenShareMessage::StreamConfig { source_mode, .. } => {
                assert_eq!(
                    source_mode,
                    SourceMode::Single,
                    "missing source_mode must default to Single"
                );
            }
            other => panic!("expected StreamConfig, got {other:?}"),
        }
    }

    /// BORU-SS-38: `RequestSource` is bounded and must reference a live
    /// session and a non-zero source id.
    #[test]
    fn request_source_validation_bounds_session_and_source() {
        let base = request_source();
        assert_eq!(
            ScreenShareMessage::decode(&base.encode().unwrap()).unwrap(),
            base
        );
        // Empty session id is rejected.
        let mut empty_session = base.clone();
        if let ScreenShareMessage::RequestSource { session_id, .. } = &mut empty_session {
            *session_id = ScreenShareSessionId::zero();
        }
        assert!(matches!(
            empty_session.encode(),
            Err(ProtocolError::Malformed(_))
        ));
        // Zero source id is rejected.
        let mut zero_source = base.clone();
        if let ScreenShareMessage::RequestSource { source_id, .. } = &mut zero_source {
            *source_id = 0;
        }
        assert!(matches!(
            zero_source.encode(),
            Err(ProtocolError::Malformed(_))
        ));
    }

    /// Truncated wire input must be rejected with an error, never a panic.
    #[test]
    fn truncated_message_is_rejected_without_panicking() {
        let bytes = video_packet().encode().unwrap();
        for cut in [0usize, 1, 2, bytes.len() / 2, bytes.len() - 1] {
            let result = ScreenShareMessage::decode(&bytes[..cut]);
            assert!(
                result.is_err(),
                "truncated input at byte {cut} must be rejected, got {result:?}"
            );
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
        if let ScreenShareMessage::Clipboard { text, .. } = &mut empty {
            text.0.clear();
        }
        assert!(matches!(empty.encode(), Err(ProtocolError::Malformed(_))));
        // Oversized text is rejected.
        let mut huge = base.clone();
        if let ScreenShareMessage::Clipboard { text, .. } = &mut huge {
            *text = RedactedText::new("x".repeat(MAX_CLIPBOARD_TEXT + 1));
        }
        assert!(matches!(huge.encode(), Err(ProtocolError::Malformed(_))));
        // An empty session id is rejected.
        let mut empty_session = base.clone();
        if let ScreenShareMessage::Clipboard { session_id, .. } = &mut empty_session {
            *session_id = ScreenShareSessionId::zero();
        }
        assert!(matches!(
            empty_session.encode(),
            Err(ProtocolError::Malformed(_))
        ));
        // The valid fixture still round-trips.
        assert_eq!(
            ScreenShareMessage::decode(&base.encode().unwrap()).unwrap(),
            base
        );
    }

    /// A message carrying a non-current protocol version is rejected cleanly on
    /// both the encode and decode paths (no panic, no state mutation).
    #[test]
    fn bad_version_is_rejected_cleanly() {
        let mut message = offer();
        {
            let ScreenShareMessage::ScreenShareOffer { version, .. } = &mut message else {
                panic!("wrong variant")
            };
            *version = 2;
        }
        assert!(matches!(
            message.encode(),
            Err(ProtocolError::UnsupportedVersion {
                received: 2,
                supported: 1
            })
        ));
        // Decode path: serialize the bad version directly (bypassing validate)
        // and confirm decode rejects it with UnsupportedVersion, not a panic.
        let bytes = postcard::to_stdvec(&message).unwrap();
        assert!(matches!(
            ScreenShareMessage::decode(&bytes),
            Err(ProtocolError::UnsupportedVersion {
                received: 2,
                supported: 1
            })
        ));
    }

    /// Unknown enum discriminants (postcard varints that map to no variant)
    /// are rejected cleanly.
    #[test]
    fn unknown_discriminant_is_rejected_cleanly() {
        // The enum has fifteen variants → postcard discriminants 0..=14.
        assert!(
            ScreenShareMessage::decode(&[15, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0])
                .is_err()
        );
        // A multi-byte varint far outside the variant range.
        assert!(ScreenShareMessage::decode(&[0xff, 0xff, 0xff, 0xff]).is_err());
    }

    /// BORU-SS-37: the audio packet is bounded and must reference a live
    /// session. A packet with no session, zero sequence, an out-of-range
    /// sample rate/channel count, or an empty/oversized payload is rejected.
    #[test]
    fn audio_packet_validation_bounds_fields() {
        let base = audio_packet();
        // Empty session id is rejected.
        let mut empty_session = base.clone();
        if let ScreenShareMessage::AudioPacket { session_id, .. } = &mut empty_session {
            *session_id = ScreenShareSessionId::zero();
        }
        assert!(matches!(
            empty_session.encode(),
            Err(ProtocolError::Malformed(_))
        ));
        // Zero sequence is rejected.
        let mut zero_seq = base.clone();
        if let ScreenShareMessage::AudioPacket { sequence, .. } = &mut zero_seq {
            *sequence = 0;
        }
        assert!(matches!(
            zero_seq.encode(),
            Err(ProtocolError::Malformed(_))
        ));
        // Out-of-range sample rate is rejected.
        let mut bad_rate = base.clone();
        if let ScreenShareMessage::AudioPacket { sample_rate, .. } = &mut bad_rate {
            *sample_rate = 96_000;
        }
        assert!(matches!(
            bad_rate.encode(),
            Err(ProtocolError::Malformed(_))
        ));
        // Zero / >2 channels are rejected.
        let mut zero_channels = base.clone();
        if let ScreenShareMessage::AudioPacket { channels, .. } = &mut zero_channels {
            *channels = 0;
        }
        assert!(matches!(
            zero_channels.encode(),
            Err(ProtocolError::Malformed(_))
        ));
        let mut many_channels = base.clone();
        if let ScreenShareMessage::AudioPacket { channels, .. } = &mut many_channels {
            *channels = 3;
        }
        assert!(matches!(
            many_channels.encode(),
            Err(ProtocolError::Malformed(_))
        ));
        // Empty payload is rejected.
        let mut empty_payload = base.clone();
        if let ScreenShareMessage::AudioPacket { payload, .. } = &mut empty_payload {
            payload.clear();
        }
        assert!(matches!(
            empty_payload.encode(),
            Err(ProtocolError::Malformed(_))
        ));
        // Oversized payload is rejected.
        let mut huge_payload = base.clone();
        if let ScreenShareMessage::AudioPacket { payload, .. } = &mut huge_payload {
            *payload = vec![0; MAX_AUDIO_FRAME + 1];
        }
        assert!(matches!(
            huge_payload.encode(),
            Err(ProtocolError::Malformed(_))
        ));
        // The valid fixture still round-trips.
        assert_eq!(
            ScreenShareMessage::decode(&base.encode().unwrap()).unwrap(),
            base
        );
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
        if let ScreenShareMessage::SourceChanged { source_id, .. } = &mut empty_id {
            *source_id = 0;
        }
        assert!(matches!(
            empty_id.encode(),
            Err(ProtocolError::Malformed(_))
        ));
        // Empty title is rejected.
        let mut empty_title = base.clone();
        if let ScreenShareMessage::SourceChanged { title, .. } = &mut empty_title {
            title.clear();
        }
        assert!(matches!(
            empty_title.encode(),
            Err(ProtocolError::Malformed(_))
        ));
        // Oversized title is rejected.
        let mut huge_title = base.clone();
        if let ScreenShareMessage::SourceChanged { title, .. } = &mut huge_title {
            *title = "x".repeat(MAX_SOURCE_NAME + 1);
        }
        assert!(matches!(
            huge_title.encode(),
            Err(ProtocolError::Malformed(_))
        ));
        // Non-ASCII titles are rejected (untrusted peer text stays ASCII).
        let mut bad_title = base.clone();
        if let ScreenShareMessage::SourceChanged { title, .. } = &mut bad_title {
            *title = "モニター".into();
        }
        assert!(matches!(
            bad_title.encode(),
            Err(ProtocolError::Malformed(_))
        ));
        // Zero dimensions are rejected.
        let mut zero_dims = base.clone();
        if let ScreenShareMessage::SourceChanged { width, .. } = &mut zero_dims {
            *width = 0;
        }
        assert!(matches!(
            zero_dims.encode(),
            Err(ProtocolError::Malformed(_))
        ));
        // Zero frame rate is rejected.
        let mut zero_fps = base.clone();
        if let ScreenShareMessage::SourceChanged { frame_rate, .. } = &mut zero_fps {
            *frame_rate = 0;
        }
        assert!(matches!(
            zero_fps.encode(),
            Err(ProtocolError::Malformed(_))
        ));
        // An empty session id is rejected.
        let mut empty_session = base.clone();
        if let ScreenShareMessage::SourceChanged { session_id, .. } = &mut empty_session {
            *session_id = ScreenShareSessionId::zero();
        }
        assert!(matches!(
            empty_session.encode(),
            Err(ProtocolError::Malformed(_))
        ));
        // The valid fixture still round-trips.
        assert_eq!(
            ScreenShareMessage::decode(&base.encode().unwrap()).unwrap(),
            base
        );
    }

    /// BORU-SS-33: cursor shape/position messages are bounded — oversized
    /// sprites, malformed hotspots, wrong-size pixel buffers, out-of-range
    /// normalized positions, and empty session ids are all rejected cleanly.
    #[test]
    fn cursor_validation_bounds_shape_and_position() {
        let base = cursor_shape();
        // Zero or oversized dimensions are rejected.
        let mut zero_dim = base.clone();
        if let ScreenShareMessage::CursorShape { width, .. } = &mut zero_dim {
            *width = 0;
        }
        assert!(matches!(
            zero_dim.encode(),
            Err(ProtocolError::Malformed(_))
        ));
        let mut huge_dim = base.clone();
        if let ScreenShareMessage::CursorShape { width, .. } = &mut huge_dim {
            *width = MAX_CURSOR_DIM + 1;
        }
        assert!(matches!(
            huge_dim.encode(),
            Err(ProtocolError::Malformed(_))
        ));
        // A hotspot at/outside the sprite edge is rejected.
        let mut bad_hotspot = base.clone();
        if let ScreenShareMessage::CursorShape {
            hotspot_x, width, ..
        } = &mut bad_hotspot
        {
            *hotspot_x = *width;
        }
        assert!(matches!(
            bad_hotspot.encode(),
            Err(ProtocolError::Malformed(_))
        ));
        // A pixel buffer that does not match width*height*4 is rejected.
        let mut short_pixels = base.clone();
        if let ScreenShareMessage::CursorShape { pixels, .. } = &mut short_pixels {
            pixels.pop();
        }
        assert!(matches!(
            short_pixels.encode(),
            Err(ProtocolError::Malformed(_))
        ));
        // A pixel buffer over the bounded shape size is rejected.
        let mut huge_pixels = base.clone();
        if let ScreenShareMessage::CursorShape {
            width,
            height,
            pixels,
            ..
        } = &mut huge_pixels
        {
            *width = MAX_CURSOR_DIM;
            *height = MAX_CURSOR_DIM;
            pixels.extend_from_slice(&[0u8; 4]);
        }
        assert!(matches!(
            huge_pixels.encode(),
            Err(ProtocolError::Malformed(_))
        ));
        // An empty session id is rejected.
        let mut empty_session = base.clone();
        if let ScreenShareMessage::CursorShape { session_id, .. } = &mut empty_session {
            *session_id = ScreenShareSessionId::zero();
        }
        assert!(matches!(
            empty_session.encode(),
            Err(ProtocolError::Malformed(_))
        ));

        let pos = cursor_position();
        // Out-of-range or non-finite normalized positions are rejected.
        let mut out_of_range = pos.clone();
        if let ScreenShareMessage::CursorPosition { x, .. } = &mut out_of_range {
            *x = 1.5;
        }
        assert!(matches!(
            out_of_range.encode(),
            Err(ProtocolError::Malformed(_))
        ));
        let mut negative = pos.clone();
        if let ScreenShareMessage::CursorPosition { y, .. } = &mut negative {
            *y = -0.1;
        }
        assert!(matches!(
            negative.encode(),
            Err(ProtocolError::Malformed(_))
        ));
        let mut nan = pos.clone();
        if let ScreenShareMessage::CursorPosition { x, .. } = &mut nan {
            *x = f32::NAN;
        }
        assert!(matches!(nan.encode(), Err(ProtocolError::Malformed(_))));
        let mut empty_session = pos.clone();
        if let ScreenShareMessage::CursorPosition { session_id, .. } = &mut empty_session {
            *session_id = ScreenShareSessionId::zero();
        }
        assert!(matches!(
            empty_session.encode(),
            Err(ProtocolError::Malformed(_))
        ));
        // The valid fixtures still round-trip.
        assert_eq!(
            ScreenShareMessage::decode(&base.encode().unwrap()).unwrap(),
            base
        );
        assert_eq!(
            ScreenShareMessage::decode(&pos.encode().unwrap()).unwrap(),
            pos
        );
    }

    /// Semantic invariants are enforced by validate() on both encode and
    /// decode; violations are clean errors.
    #[test]
    fn semantic_validation_rejects_invalid_fields() {
        let mut m = offer();
        {
            let ScreenShareMessage::ScreenShareOffer { session_id, .. } = &mut m else {
                panic!("wrong variant")
            };
            *session_id = ScreenShareSessionId::zero();
        }
        assert!(matches!(m.encode(), Err(ProtocolError::Malformed(_))));
        {
            let ScreenShareMessage::ScreenShareOffer { resolutions, .. } = &mut m else {
                panic!("wrong variant")
            };
            resolutions.clear();
        }
        assert!(matches!(m.encode(), Err(ProtocolError::Malformed(_))));
        {
            let ScreenShareMessage::ScreenShareOffer { frame_rate_min, .. } = &mut m else {
                panic!("wrong variant")
            };
            *frame_rate_min = 0;
        }
        assert!(matches!(m.encode(), Err(ProtocolError::Malformed(_))));
        {
            let ScreenShareMessage::ScreenShareOffer { frame_rate_max, .. } = &mut m else {
                panic!("wrong variant")
            };
            *frame_rate_max = 10; // below the (restored) minimum of 15
        }
        assert!(matches!(m.encode(), Err(ProtocolError::Malformed(_))));
        {
            let ScreenShareMessage::ScreenShareOffer { codecs, .. } = &mut m else {
                panic!("wrong variant")
            };
            *codecs = vec!["not ascii ☃".into()];
        }
        assert!(matches!(m.encode(), Err(ProtocolError::Malformed(_))));
        // Restore every mutated field so the offer encodes cleanly again.
        {
            let ScreenShareMessage::ScreenShareOffer {
                session_id,
                resolutions,
                frame_rate_min,
                frame_rate_max,
                codecs,
                ..
            } = &mut m
            else {
                panic!("wrong variant")
            };
            *session_id = sid();
            *resolutions = vec![(1920, 1080), (1280, 720)];
            *frame_rate_min = 15;
            *frame_rate_max = 30;
            *codecs = vec!["h264".into()];
        }
        assert!(m.encode().is_ok(), "restored offer must encode cleanly");

        let mut m = reject();
        {
            let ScreenShareMessage::ScreenShareReject { reason, .. } = &mut m else {
                panic!("wrong variant")
            };
            reason.clear();
        }
        assert!(matches!(m.encode(), Err(ProtocolError::Malformed(_))));

        let mut m = video_packet();
        {
            let ScreenShareMessage::VideoPacket { payload, .. } = &mut m else {
                panic!("wrong variant")
            };
            payload.clear();
        }
        assert!(matches!(m.encode(), Err(ProtocolError::Malformed(_))));
        {
            let ScreenShareMessage::VideoPacket { sequence, .. } = &mut m else {
                panic!("wrong variant")
            };
            *sequence = 0;
        }
        assert!(matches!(m.encode(), Err(ProtocolError::Malformed(_))));
        // Oversized video payloads are rejected by the size bound.
        {
            let ScreenShareMessage::VideoPacket { payload, .. } = &mut m else {
                panic!("wrong variant")
            };
            *payload = vec![0; MAX_MEDIA_FRAME + 1];
        }
        assert!(matches!(m.encode(), Err(ProtocolError::Malformed(_))));

        let mut m = stream_config();
        {
            let ScreenShareMessage::StreamConfig {
                keyframe_interval, ..
            } = &mut m
            else {
                panic!("wrong variant")
            };
            *keyframe_interval = 0;
        }
        assert!(matches!(m.encode(), Err(ProtocolError::Malformed(_))));

        let mut m = quality_update();
        {
            let ScreenShareMessage::QualityUpdate { scale_factor, .. } = &mut m else {
                panic!("wrong variant")
            };
            *scale_factor = 0;
        }
        assert!(matches!(m.encode(), Err(ProtocolError::Malformed(_))));

        let mut m = protocol_error();
        {
            let ScreenShareMessage::Error { code, .. } = &mut m else {
                panic!("wrong variant")
            };
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
            let ScreenShareMessage::ScreenShareAccept { codec, .. } = &mut m else {
                panic!("wrong variant")
            };
            codec.clear();
        }
        assert!(matches!(m.encode(), Err(ProtocolError::Malformed(_))));
        {
            let ScreenShareMessage::ScreenShareAccept { width, .. } = &mut m else {
                panic!("wrong variant")
            };
            *width = 0;
        }
        assert!(matches!(m.encode(), Err(ProtocolError::Malformed(_))));
        {
            let ScreenShareMessage::ScreenShareAccept { frame_rate, .. } = &mut m else {
                panic!("wrong variant")
            };
            *frame_rate = 0;
        }
        assert!(matches!(m.encode(), Err(ProtocolError::Malformed(_))));
        // Restore so the accept round-trips again.
        {
            let ScreenShareMessage::ScreenShareAccept {
                codec,
                width,
                frame_rate,
                ..
            } = &mut m
            else {
                panic!("wrong variant")
            };
            *codec = "h264".into();
            *width = 1280;
            *frame_rate = 30;
        }
        assert!(m.encode().is_ok(), "restored accept must encode cleanly");

        let mut m = offer();
        {
            let ScreenShareMessage::ScreenShareOffer { resolutions, .. } = &mut m else {
                panic!("wrong variant")
            };
            resolutions.push((0, 0));
        }
        assert!(matches!(m.encode(), Err(ProtocolError::Malformed(_))));
        {
            let ScreenShareMessage::ScreenShareOffer {
                resolutions,
                frame_rate_min,
                frame_rate_max,
                ..
            } = &mut m
            else {
                panic!("wrong variant")
            };
            resolutions.pop();
            *frame_rate_min = 30;
            *frame_rate_max = 15; // inverted range
        }
        assert!(matches!(m.encode(), Err(ProtocolError::Malformed(_))));
        {
            let ScreenShareMessage::ScreenShareOffer {
                frame_rate_min,
                frame_rate_max,
                ..
            } = &mut m
            else {
                panic!("wrong variant")
            };
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
        let (audio_tx, _audio_rx) = mpsc::channel(8);
        let protocol = ScreenShareProtocol::with_channels(events_tx, media_tx, audio_tx);
        let router = Router::builder(viewer.clone())
            .accept(SCREEN_SHARE_ALPN, protocol.clone())
            .spawn();

        // Host endpoint dials the viewer with the screen-share ALPN.
        let host = iroh::Endpoint::bind(presets::Minimal).await.unwrap();
        let host_pk = host.secret_key().public();
        let connection = host
            .connect(viewer.addr(), SCREEN_SHARE_ALPN)
            .await
            .unwrap();
        let session_id = ScreenShareSessionId::generate();
        let transport =
            QuicScreenTransport::new(connection.clone(), *session_id.as_bytes()).unwrap();

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
        transport
            .send_control(&ControlMessage::Hello(hello))
            .await
            .unwrap();
        let event = events_rx.recv().await.unwrap();
        let SessionEvent::Invitation {
            session_id: got_id,
            host_id,
            ..
        } = event
        else {
            panic!("expected Invitation, got {event:?}");
        };
        assert_eq!(got_id, session_id);
        assert_eq!(host_id, host_pk);

        // Viewer explicitly accepts on the same inbound connection.
        protocol
            .send_control(
                session_id,
                ControlMessage::Accept {
                    version: SCREEN_SHARE_PROTOCOL_VERSION,
                    session_id,
                },
            )
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

    /// Full negotiation + media round trip for the AV1 codec path
    /// (BORU-SS-35): the viewer advertises AV1 support, the host selects it,
    /// and a rav1e-encoded / rav1d-decoded frame survives the wire.
    #[tokio::test]
    async fn end_to_end_invite_accept_av1_media_decode() {
        let viewer = iroh::Endpoint::bind(presets::Minimal).await.unwrap();
        let (events_tx, mut events_rx) = mpsc::channel(32);
        let (media_tx, mut media_rx) = mpsc::channel(64);
        let (audio_tx, _audio_rx) = mpsc::channel(8);
        let protocol = ScreenShareProtocol::with_channels(events_tx, media_tx, audio_tx);
        let router = Router::builder(viewer.clone())
            .accept(SCREEN_SHARE_ALPN, protocol.clone())
            .spawn();

        let host = iroh::Endpoint::bind(presets::Minimal).await.unwrap();
        let host_pk = host.secret_key().public();
        let connection = host
            .connect(viewer.addr(), SCREEN_SHARE_ALPN)
            .await
            .unwrap();
        let session_id = ScreenShareSessionId::generate();
        let transport =
            QuicScreenTransport::new(connection.clone(), *session_id.as_bytes()).unwrap();

        // Host advertises AV1 alongside H.264; the viewer must select AV1.
        let hello = Hello {
            version: SCREEN_SHARE_PROTOCOL_VERSION,
            session_id,
            host_id: host_pk,
            conversation_id: 8,
            codecs: vec!["h264".into(), "av1".into()],
            width: 640,
            height: 360,
            frame_rate: 15,
            permission: Permission::ViewOnly,
        };
        transport
            .send_control(&ControlMessage::Hello(hello))
            .await
            .unwrap();
        let event = events_rx.recv().await.unwrap();
        let SessionEvent::Invitation {
            session_id: got_id,
            host_id,
            ..
        } = event
        else {
            panic!("expected Invitation, got {event:?}");
        };
        assert_eq!(got_id, session_id);
        assert_eq!(host_id, host_pk);

        // Viewer accepts with the mutually supported codec (av1).
        protocol
            .send_control(
                session_id,
                ControlMessage::Accept {
                    version: SCREEN_SHARE_PROTOCOL_VERSION,
                    session_id,
                },
            )
            .await
            .unwrap();
        let (mut send, recv) = connection.accept_bi().await.unwrap();
        match read_unit(recv).await.unwrap() {
            ReadUnit::Control(ControlMessage::Accept { session_id: id, .. }) => {
                assert_eq!(id, session_id);
            }
            other => panic!("expected Accept control, got {other:?}"),
        }
        drop(send);

        // Host captures + encodes one synthetic frame with the AV1 encoder and
        // streams it. rav1e's low-latency lookahead swallows the first few
        // frames, so feed until a packet emerges.
        let config = CodecConfig {
            width: 640,
            height: 360,
            target_fps: 15,
            ..CodecConfig::default()
        };
        let mut capture = TestPatternCapture::new(640, 360).unwrap();
        let mut encoder = Av1Encoder::new(config).unwrap();
        let mut encoded = None;
        for _ in 0..8 {
            let frame = capture.capture().unwrap().unwrap();
            if let Ok(packet) = encoder.encode(&frame) {
                encoded = Some(packet);
                break;
            }
        }
        let encoded = encoded.expect("av1 packet after lookahead warm-up");
        assert!(
            encoded.keyframe,
            "first emitted av1 unit must be a keyframe"
        );
        transport.send_frame(&encoded).await.unwrap();

        // Viewer protocol forwards the media unit to the app-facing channel.
        let media = media_rx.recv().await.unwrap();
        assert_eq!(media.session_id, session_id);
        assert_eq!(media.header.sequence, encoded.sequence);
        assert_eq!(media.header.width as u32, 640);
        assert_eq!(media.header.height as u32, 360);

        // Viewer decodes through the production pipeline into an RGBA frame.
        let mut pipeline = ViewerPipeline::new(
            Av1Decoder::default_profile().unwrap(),
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
        let (audio_tx, _audio_rx) = mpsc::channel(8);
        let protocol = ScreenShareProtocol::with_channels(events_tx.clone(), media_tx, audio_tx);
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
        let connection = host
            .connect(viewer.addr(), SCREEN_SHARE_ALPN)
            .await
            .unwrap();
        let transport =
            QuicScreenTransport::new(connection.clone(), *session_id.as_bytes()).unwrap();
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
        transport
            .send_control(&ControlMessage::Hello(hello.clone()))
            .await
            .unwrap();
        let event = events_rx.recv().await.unwrap();
        let SessionEvent::Invitation {
            session_id: got_id,
            host_id,
            ..
        } = event
        else {
            panic!("expected Invitation, got {event:?}");
        };
        assert_eq!(got_id, session_id);
        assert_eq!(host_id, host_pk);

        // Viewer accepts on the inbound connection; host applies the Accept.
        protocol
            .send_control(
                session_id,
                ControlMessage::Accept {
                    version: SCREEN_SHARE_PROTOCOL_VERSION,
                    session_id,
                },
            )
            .await
            .unwrap();
        host_manager.apply_remote(
            viewer_pk,
            ControlMessage::Accept {
                version: SCREEN_SHARE_PROTOCOL_VERSION,
                session_id,
            },
            &host_events_tx,
        );
        assert_eq!(
            host_manager.state(session_id),
            Some(SessionState::Streaming)
        );

        // Viewer had remote control granted; the reconnect must drop it.
        protocol.manager().lock().await.grant_control(
            session_id,
            vec![Capability::ControlPointer],
            &events_tx,
        );
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
        assert_eq!(
            host_manager.state(session_id),
            Some(SessionState::Reconnecting)
        );
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
        let reconnect_connection = host
            .connect(viewer.addr(), SCREEN_SHARE_ALPN)
            .await
            .unwrap();
        let reconnect_transport =
            QuicScreenTransport::new(reconnect_connection.clone(), *session_id.as_bytes()).unwrap();
        reconnect_transport
            .send_control(&ControlMessage::Hello(hello))
            .await
            .unwrap();

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
            protocol
                .manager()
                .lock()
                .await
                .permissions(session_id)
                .unwrap()
                .capabilities(),
            &[Capability::ViewScreen],
            "reconnect must reset to view-only — control is not silently resumed"
        );

        // ---- Viewer re-accepts on the NEW connection and requests a fresh
        // keyframe (REC-1).
        protocol
            .send_control(
                session_id,
                ControlMessage::Accept {
                    version: SCREEN_SHARE_PROTOCOL_VERSION,
                    session_id,
                },
            )
            .await
            .unwrap();
        protocol
            .send_screen_share(
                session_id,
                ScreenShareMessage::KeyframeRequest {
                    version: SCREEN_SHARE_PROTOCOL_VERSION,
                    session_id,
                },
            )
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
        host_manager.apply_remote(
            viewer_pk,
            ControlMessage::Accept {
                version: SCREEN_SHARE_PROTOCOL_VERSION,
                session_id,
            },
            &host_events_tx,
        );
        assert_eq!(
            host_manager.state(session_id),
            Some(SessionState::Streaming)
        );
        let event = host_events_rx.recv().await.unwrap();
        assert!(
            matches!(event, SessionEvent::Reconnected { session_id: id } if id == session_id),
            "expected Reconnected, got {event:?}"
        );

        // Host also receives the viewer's fresh-keyframe request.
        let (mut send, recv) = reconnect_connection.accept_bi().await.unwrap();
        match read_unit(recv).await.unwrap() {
            ReadUnit::ScreenShare(ScreenShareMessage::KeyframeRequest {
                session_id: id, ..
            }) => {
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

    /// BORU-SS-37: the host streams encoded Opus audio on the dedicated audio
    /// stream kind; the viewer's protocol forwards it to the app-facing audio
    /// channel ONLY when the session holds an explicit Audio grant, and drops
    /// it otherwise (audio is a separate optional capability, like clipboard).
    #[tokio::test]
    async fn end_to_end_audio_packet_delivery_is_grant_gated() {
        // Viewer endpoint with the protocol handler registered on the router.
        let viewer = iroh::Endpoint::bind(presets::Minimal).await.unwrap();
        let (events_tx, mut events_rx) = mpsc::channel(32);
        let (media_tx, _media_rx) = mpsc::channel(64);
        let (audio_tx, mut audio_rx) = mpsc::channel(8);
        let protocol = ScreenShareProtocol::with_channels(events_tx.clone(), media_tx, audio_tx);
        let router = Router::builder(viewer.clone())
            .accept(SCREEN_SHARE_ALPN, protocol.clone())
            .spawn();

        // Host dials the viewer and negotiates view-only like the real driver.
        let host = iroh::Endpoint::bind(presets::Minimal).await.unwrap();
        let host_pk = host.secret_key().public();
        let connection = host
            .connect(viewer.addr(), SCREEN_SHARE_ALPN)
            .await
            .unwrap();
        let session_id = ScreenShareSessionId::generate();
        let transport =
            QuicScreenTransport::new(connection.clone(), *session_id.as_bytes()).unwrap();
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
        transport
            .send_control(&ControlMessage::Hello(hello))
            .await
            .unwrap();
        let event = events_rx.recv().await.unwrap();
        let SessionEvent::Invitation {
            session_id: got_id, ..
        } = event
        else {
            panic!("expected Invitation, got {event:?}");
        };
        assert_eq!(got_id, session_id);
        protocol
            .send_control(
                session_id,
                ControlMessage::Accept {
                    version: SCREEN_SHARE_PROTOCOL_VERSION,
                    session_id,
                },
            )
            .await
            .unwrap();
        let (mut send, recv) = connection.accept_bi().await.unwrap();
        match read_unit(recv).await.unwrap() {
            ReadUnit::Control(ControlMessage::Accept { session_id: id, .. }) => {
                assert_eq!(id, session_id);
            }
            other => panic!("expected Accept control, got {other:?}"),
        }
        drop(send);

        // WITHOUT the Audio grant, an audio unit must be dropped (capability
        // gate): the viewer never forwards unauthorized audio to the app.
        let header = AudioHeader {
            version: SCREEN_SHARE_PROTOCOL_VERSION,
            session_id: *session_id.as_bytes(),
            sequence: 1,
            timestamp_us: 1_000,
            sample_rate: 48_000,
            channels: 2,
            payload_len: 3,
        };
        transport.send_audio(&header, &[1, 2, 3]).await.unwrap();
        assert!(
            tokio::time::timeout(Duration::from_millis(300), audio_rx.recv())
                .await
                .is_err(),
            "audio must be dropped without an Audio grant"
        );

        // Grant the Audio capability (separate optional capability).
        protocol.manager().lock().await.grant_control(
            session_id,
            vec![Capability::Audio],
            &events_tx,
        );
        let event = events_rx.recv().await.unwrap();
        assert!(
            matches!(event, SessionEvent::ControlChanged { session_id: id, active: true, .. } if id == session_id),
            "expected ControlChanged(active:true), got {event:?}"
        );

        // With the grant, the same unit is forwarded to the app-facing channel.
        transport.send_audio(&header, &[9, 8, 7]).await.unwrap();
        let audio = tokio::time::timeout(Duration::from_secs(2), audio_rx.recv())
            .await
            .expect("audio delivered within timeout")
            .expect("audio channel open");
        assert_eq!(audio.session_id, session_id);
        assert_eq!(audio.header.sequence, 1);
        assert_eq!(audio.header.sample_rate, 48_000);
        assert_eq!(audio.header.channels, 2);
        assert_eq!(audio.payload, vec![9, 8, 7]);

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
        let (audio_tx, _audio_rx) = mpsc::channel(8);
        let protocol = ScreenShareProtocol::with_channels(events_tx, media_tx, audio_tx);
        let router = Router::builder(viewer.clone())
            .accept(SCREEN_SHARE_ALPN, protocol.clone())
            .spawn();

        let host = iroh::Endpoint::bind(presets::Minimal).await.unwrap();
        let host_pk = host.secret_key().public();
        let connection = host
            .connect(viewer.addr(), SCREEN_SHARE_ALPN)
            .await
            .unwrap();
        let session_id = ScreenShareSessionId::generate();
        let transport =
            QuicScreenTransport::new(connection.clone(), *session_id.as_bytes()).unwrap();

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
        host_negotiations
            .start_offer(
                offer.clone(),
                viewer.secret_key().public(),
                NEGOTIATION_TIMEOUT,
            )
            .unwrap();
        transport.send_screen_share(&offer).await.unwrap();

        // Recipient: protocol handler emits a NegotiationInvitation.
        let event = events_rx.recv().await.unwrap();
        let SessionEvent::NegotiationInvitation {
            session_id: got_id,
            host_id,
            offer: got_offer,
            ..
        } = event
        else {
            panic!("expected NegotiationInvitation, got {event:?}");
        };
        assert_eq!(got_id, session_id);
        assert_eq!(host_id, host_pk);
        let ScreenShareMessage::ScreenShareOffer { resolutions, .. } = &got_offer else {
            panic!("offer")
        };
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
        protocol
            .send_screen_share(session_id, accept.clone())
            .await
            .unwrap();

        // Initiator reads the Accept and applies it; capture is then allowed.
        let (mut send, recv) = connection.accept_bi().await.unwrap();
        match read_unit(recv).await.unwrap() {
            ReadUnit::ScreenShare(ScreenShareMessage::ScreenShareAccept {
                session_id: got,
                codec,
                width,
                height,
                frame_rate,
                ..
            }) => {
                assert_eq!(got, session_id);
                assert_eq!(codec, "h264");
                assert_eq!((width, height), (1920, 1080));
                assert_eq!(frame_rate, 30);
            }
            other => panic!("expected ScreenShareAccept, got {other:?}"),
        }
        drop(send);
        {
            host_negotiations
                .handle_accept(viewer.secret_key().public(), accept, &negotiation_events_tx)
                .unwrap();
        }
        assert!(
            host_negotiations.can_start_capture(session_id),
            "capture allowed after explicit accept"
        );

        // Duplicate offer: the same session id is refused with an explicit
        // ScreenShareReject rather than a silent state change.
        transport.send_screen_share(&offer).await.unwrap();
        let (mut send, recv) = connection.accept_bi().await.unwrap();
        match read_unit(recv).await.unwrap() {
            ReadUnit::ScreenShare(ScreenShareMessage::ScreenShareReject {
                session_id: id,
                reason,
                ..
            }) => {
                assert_eq!(id, session_id);
                assert_eq!(reason, "duplicate offer");
            }
            other => panic!("expected ScreenShareReject for duplicate, got {other:?}"),
        }
        drop(send);

        router.shutdown().await.unwrap();
    }
}
