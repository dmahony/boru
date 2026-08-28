//! Screen-share invitation and session state machine.
#![allow(missing_docs)]

use std::collections::HashMap;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

use super::capture::CaptureSource;
use super::coords::CursorSprite;
use super::permissions::{Capability, SessionPermissions};
use super::protocol::ScreenShareMessage;
use super::protocol::{
    ControlMessage, Hello, Permission, RedactedText, SCREEN_SHARE_PROTOCOL_VERSION,
};
use super::stats::ScreenShareSessionMetrics;

/// Opaque identifier for one negotiation, independent of a conversation.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ScreenShareSessionId([u8; 16]);

impl ScreenShareSessionId {
    /// Generate a fresh identifier using the OS CSPRNG.
    pub fn generate() -> Self {
        let mut bytes = [0; 16];
        getrandom::fill(&mut bytes).expect("OS CSPRNG unavailable");
        Self(bytes)
    }
    /// Construct the all-zero identifier, useful only as a test sentinel.
    pub const fn zero() -> Self {
        Self([0; 16])
    }
    /// Construct an identifier from raw wire bytes.
    pub const fn from_bytes(bytes: [u8; 16]) -> Self {
        Self(bytes)
    }
    /// Return the wire representation.
    pub const fn as_bytes(&self) -> &[u8; 16] {
        &self.0
    }
}
impl std::fmt::Debug for ScreenShareSessionId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_tuple("ScreenShareSessionId")
            .field(&hex::encode(self.0))
            .finish()
    }
}

/// Lifecycle states. Streaming is only reachable after explicit Accept.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SessionState {
    Idle,
    Inviting,
    AwaitingAcceptance,
    Connecting,
    Streaming,
    /// The media path failed transiently and is being re-established. The
    /// session (and the chat/friend session it belongs to) survives; only
    /// the media stream reconnects. Permissions are reset to view-only.
    Reconnecting,
    Paused,
    Ending,
    Ended,
    Failed,
}

/// Events exposed to the conversation/UI layer. They contain no media data.
#[derive(Debug, Clone, PartialEq)]
pub enum SessionEvent {
    /// A recipient-visible invitation that requires an explicit action.
    Invitation {
        session_id: ScreenShareSessionId,
        host_id: iroh::PublicKey,
        conversation_id: u64,
        hello: Hello,
    },
    /// Versioned negotiation invitation (PDF Task 3.1): carries the full offer
    /// so the UI can present codecs/resolutions/fps/remote-control before the
    /// recipient decides. The recipient must accept before capture begins.
    NegotiationInvitation {
        session_id: ScreenShareSessionId,
        host_id: iroh::PublicKey,
        conversation_id: u64,
        offer: ScreenShareMessage,
    },
    /// A session entered streaming after consent.
    Accepted {
        session_id: ScreenShareSessionId,
        peer_id: iroh::PublicKey,
    },
    /// A peer declined or the protocol rejected the session.
    Rejected {
        session_id: ScreenShareSessionId,
        reason: String,
    },
    /// A session ended or its connection disappeared.
    Ended { session_id: ScreenShareSessionId },
    /// The media path failed transiently; the session is being re-established.
    /// The chat/friend session is unaffected — only the media stream is
    /// reconnecting. Remote-control permissions were reset to view-only.
    Reconnecting { session_id: ScreenShareSessionId },
    /// The media path was re-established after a transient failure. The
    /// session is streaming again, but view-only: control requires fresh
    /// consent (PDF Task 3.3 / REC-2).
    Reconnected { session_id: ScreenShareSessionId },
    /// Viewer requested explicit control capabilities; host UI must decide.
    ControlRequest {
        session_id: ScreenShareSessionId,
        peer_id: iroh::PublicKey,
        capabilities: Vec<Capability>,
    },
    /// Control became active or was revoked while viewing continues.
    ControlChanged {
        session_id: ScreenShareSessionId,
        active: bool,
        capabilities: Vec<Capability>,
    },
    /// A peer sent a text-only clipboard payload (PDF Task 9.3 / BORU-SS-25).
    /// Emitted only after the payload was authorized against the explicitly
    /// granted `Clipboard` capability — clipboard sync is never implied by
    /// remote control. The app places the text on the local clipboard.
    /// The text is wrapped in [`RedactedText`] so Debug formatting can never
    /// leak clipboard contents into logs (PDF Phase 12 guardrail).
    ClipboardReceived {
        session_id: ScreenShareSessionId,
        text: RedactedText,
    },
    /// The capture sources (monitors) available to the host, emitted before
    /// the share starts (PDF Phase 10: "enumerate available monitors before
    /// starting a share"). The app may present the list so the sharer can
    /// choose the initial source; monitor switching UX is BORU-SS-29.
    SourcesEnumerated {
        session_id: ScreenShareSessionId,
        sources: Vec<CaptureSource>,
    },
    /// The shared source changed (the host switched monitor, or the platform
    /// renegotiated the capture geometry). Carries the NEW source identity
    /// and dimensions; the wire `SourceChanged` message is sent BEFORE any
    /// frame with the new geometry, and this event surfaces the same change
    /// to the app UI. `source_mode` tells the viewer how the desktop maps
    /// onto the stream (Single / PerDisplay / Spanning, PDF Phase 14 /
    /// BORU-SS-38).
    SourceChanged {
        session_id: ScreenShareSessionId,
        source_id: u64,
        title: String,
        width: u32,
        height: u32,
        source_mode: super::protocol::SourceMode,
    },
    /// BORU-SS-33: the host delivered a new cursor SHAPE (PDF Task 5.3
    /// `Metadata` cursor mode). The viewer caches the sprite and composites
    /// it over the decoded frame at the latest reported position. Contains
    /// no screen content — only the bounded cursor sprite pixels.
    CursorShape {
        session_id: ScreenShareSessionId,
        sprite: CursorSprite,
    },
    /// BORU-SS-33: the host delivered a cursor POSITION move (PDF Task 5.3
    /// `Metadata` cursor mode). Position is normalized against the shared
    /// source (`0..=1`), matching the input coordinate contract; the viewer
    /// re-composites the cached sprite at this position.
    CursorPosition {
        session_id: ScreenShareSessionId,
        x: f32,
        y: f32,
        visible: bool,
    },
    /// The shared source disappeared (monitor unplug / laptop dock-undock).
    /// The host re-enumerated; `fallback` names the source it fell back to,
    /// or `None` when no source remains and the stream is paused (the chat
    /// session and the screen-share session itself survive — PDF Phase 10
    /// requires graceful handling, not a crash or forced end).
    SourceUnavailable {
        session_id: ScreenShareSessionId,
        reason: String,
        fallback: Option<String>,
    },
    /// Periodic developer metrics for the diagnostics overlay (PDF Phase 12).
    /// Emitted ~1 Hz from the host streaming loop; carries negotiated codec /
    /// dimensions / bitrate / frame rate / backend plus a live pipeline
    /// snapshot. Local-only — never sent on the wire, contains no media
    /// payloads. Consumers that don't render an overlay ignore it.
    Metrics {
        session_id: ScreenShareSessionId,
        metrics: ScreenShareSessionMetrics,
    },
    /// System-audio sharing state changed (BORU-SS-37). Emitted when the host
    /// enables/disables shared audio (audio is opt-in) or the capture backend
    /// fails. `error` carries a typed, user-safe reason when capture could
    /// not start (e.g. no PipeWire runtime). Contains no media data.
    AudioState {
        session_id: ScreenShareSessionId,
        enabled: bool,
        error: Option<String>,
    },
}

#[derive(Debug, Clone)]
struct Record {
    state: SessionState,
    host_id: iroh::PublicKey,
    peer_id: Option<iroh::PublicKey>,
    conversation_id: u64,
}

/// Bounded in-memory state for active sessions.
#[derive(Debug, Default)]
pub struct SessionManager {
    sessions: HashMap<ScreenShareSessionId, Record>,
    permissions: HashMap<ScreenShareSessionId, SessionPermissions>,
}

pub const MAX_ACTIVE_SESSIONS: usize = 8;

impl SessionManager {
    /// Start a local invitation. The caller must send the corresponding Hello.
    /// `peer` is the invitee (the node the Hello will be dialed to); it is
    /// recorded so the eventual remote Accept can be attributed to the invitee.
    pub fn start_invitation(
        &mut self,
        id: ScreenShareSessionId,
        host_id: iroh::PublicKey,
        peer: iroh::PublicKey,
        conversation_id: u64,
    ) {
        if id == ScreenShareSessionId::zero() || self.sessions.len() >= MAX_ACTIVE_SESSIONS {
            return;
        }
        self.sessions.insert(
            id,
            Record {
                state: SessionState::AwaitingAcceptance,
                host_id,
                peer_id: Some(peer),
                conversation_id,
            },
        );
    }
    /// Return a session state, if the session is known.
    pub fn state(&self, id: ScreenShareSessionId) -> Option<SessionState> {
        self.sessions.get(&id).map(|record| record.state)
    }
    /// Return the permission record for a session, if known.
    pub fn permissions(&self, id: ScreenShareSessionId) -> Option<&SessionPermissions> {
        self.permissions.get(&id)
    }
    /// Build a default, view-only Hello for a locally initiated session.
    pub fn hello(
        &self,
        id: ScreenShareSessionId,
        codecs: Vec<String>,
        width: u16,
        height: u16,
        frame_rate: u16,
    ) -> Option<Hello> {
        let record = self.sessions.get(&id)?;
        Some(Hello {
            version: SCREEN_SHARE_PROTOCOL_VERSION,
            session_id: id,
            host_id: record.host_id,
            conversation_id: record.conversation_id,
            codecs,
            width,
            height,
            frame_rate,
            permission: Permission::ViewOnly,
        })
    }
    /// Explicitly accept a pending invitation. This is the only transition to Streaming.
    pub fn accept_invitation(
        &mut self,
        id: ScreenShareSessionId,
        host_id: iroh::PublicKey,
    ) -> Option<ControlMessage> {
        let record = self.sessions.get_mut(&id)?;
        if record.host_id != host_id
            || !matches!(
                record.state,
                SessionState::Connecting | SessionState::AwaitingAcceptance
            )
        {
            return None;
        }
        record.peer_id = Some(host_id);
        record.state = SessionState::Streaming;
        Some(ControlMessage::Accept {
            version: SCREEN_SHARE_PROTOCOL_VERSION,
            session_id: id,
        })
    }
    /// Explicitly decline an invitation and remove all state/resources.
    pub fn reject_invitation(
        &mut self,
        id: ScreenShareSessionId,
        reason: impl Into<String>,
    ) -> Option<ControlMessage> {
        self.permissions.remove(&id);
        if self.sessions.remove(&id).is_some() {
            Some(ControlMessage::Reject {
                version: SCREEN_SHARE_PROTOCOL_VERSION,
                session_id: id,
                reason: reason.into(),
            })
        } else {
            None
        }
    }
    /// End a session idempotently; unknown/already-ended sessions produce no wire message.
    /// The permission record is ended too, so any late input/view attempt fails
    /// authorization immediately (PDF Task 9.1 stop condition).
    pub fn end(&mut self, id: ScreenShareSessionId) -> Option<ControlMessage> {
        let record = self.sessions.get_mut(&id)?;
        if record.state == SessionState::Ended {
            return None;
        }
        record.state = SessionState::Ended;
        if let Some(permissions) = self.permissions.get_mut(&id) {
            permissions.end();
        }
        Some(ControlMessage::EndSession {
            version: SCREEN_SHARE_PROTOCOL_VERSION,
            session_id: id,
        })
    }
    /// Enter the reconnecting state after a transient media failure (PDF Task
    /// 3.3 / REC-1). The session record survives — the chat/friend session it
    /// belongs to is unaffected because chat lives on a separate QUIC
    /// connection — and remote-control permissions are reset to view-only
    /// (REC-2: control is never silently resumed after a security-significant
    /// reconnect). Emits `Reconnecting` and `ControlChanged(active: false)`.
    /// Returns false when the session is not streaming (nothing to reconnect).
    pub fn begin_reconnect(
        &mut self,
        id: ScreenShareSessionId,
        events: &tokio::sync::mpsc::Sender<SessionEvent>,
    ) -> bool {
        let Some(record) = self.sessions.get_mut(&id) else {
            return false;
        };
        if record.state != SessionState::Streaming {
            return false;
        }
        record.state = SessionState::Reconnecting;
        if let Some(permissions) = self.permissions.get_mut(&id) {
            permissions.reset_for_reconnect();
        }
        let _ = events.try_send(SessionEvent::Reconnecting { session_id: id });
        let _ = events.try_send(SessionEvent::ControlChanged {
            session_id: id,
            active: false,
            capabilities: vec![Capability::ViewScreen],
        });
        tracing::info!(session = ?id, "screen-share: session reconnecting");
        true
    }
    /// Mark the media path re-established: Reconnecting → Streaming. Permissions
    /// remain view-only (a reconnect never re-grants control by itself). Emits
    /// `Reconnected`. Returns false when the session is not reconnecting.
    pub fn complete_reconnect(
        &mut self,
        id: ScreenShareSessionId,
        events: &tokio::sync::mpsc::Sender<SessionEvent>,
    ) -> bool {
        let Some(record) = self.sessions.get_mut(&id) else {
            return false;
        };
        if record.state != SessionState::Reconnecting {
            return false;
        }
        record.state = SessionState::Streaming;
        let _ = events.try_send(SessionEvent::Reconnected { session_id: id });
        tracing::info!(session = ?id, "screen-share: session reconnected");
        true
    }
    /// Abandon a reconnect attempt: Reconnecting → Ended. Emits `Ended`.
    /// Returns false when the session is not reconnecting.
    pub fn fail_reconnect(
        &mut self,
        id: ScreenShareSessionId,
        events: &tokio::sync::mpsc::Sender<SessionEvent>,
    ) -> bool {
        let Some(record) = self.sessions.get_mut(&id) else {
            return false;
        };
        if record.state != SessionState::Reconnecting {
            return false;
        }
        record.state = SessionState::Ended;
        if let Some(permissions) = self.permissions.get_mut(&id) {
            permissions.end();
        }
        let _ = events.try_send(SessionEvent::Ended { session_id: id });
        tracing::warn!(session = ?id, "screen-share: reconnect failed, session ended");
        true
    }
    /// Host-side grant of control capabilities. Generates the fresh nonce,
    /// emits a local `ControlChanged` so the host UI shows the indicator, and
    /// returns the wire GrantControl message to send to the viewer.
    pub fn grant_control(
        &mut self,
        id: ScreenShareSessionId,
        capabilities: Vec<Capability>,
        events: &tokio::sync::mpsc::Sender<SessionEvent>,
    ) -> Option<ControlMessage> {
        self.grant_control_with_policy(
            id,
            capabilities,
            events,
            &super::permissions::UnmanagedRoomPermissionHook,
        )
    }

    pub fn grant_control_with_policy<P: super::permissions::ScreenSharePermissionHook + ?Sized>(
        &mut self,
        id: ScreenShareSessionId,
        capabilities: Vec<Capability>,
        events: &tokio::sync::mpsc::Sender<SessionEvent>,
        policy: &P,
    ) -> Option<ControlMessage> {
        let permissions = self.permissions.get_mut(&id)?;
        if !permissions.grant_with_policy(capabilities.clone(), policy) {
            return None;
        }
        let nonce = *permissions.token()?.nonce();
        let _ = events.try_send(SessionEvent::ControlChanged {
            session_id: id,
            active: true,
            capabilities: capabilities.clone(),
        });
        Some(ControlMessage::GrantControl {
            version: SCREEN_SHARE_PROTOCOL_VERSION,
            session_id: id,
            capabilities,
            nonce,
        })
    }
    /// Host-side revocation of control. Emits a local `ControlChanged` and
    /// returns the wire RevokeControl message to send to the viewer.
    pub fn revoke_control(
        &mut self,
        id: ScreenShareSessionId,
        events: &tokio::sync::mpsc::Sender<SessionEvent>,
    ) -> Option<ControlMessage> {
        let permissions = self.permissions.get_mut(&id)?;
        permissions.revoke_control();
        let _ = events.try_send(SessionEvent::ControlChanged {
            session_id: id,
            active: false,
            capabilities: vec![Capability::ViewScreen],
        });
        Some(ControlMessage::RevokeControl {
            version: SCREEN_SHARE_PROTOCOL_VERSION,
            session_id: id,
        })
    }
    /// Apply one validated remote control message. Hello never grants consent.
    pub fn apply_remote(
        &mut self,
        peer_id: iroh::PublicKey,
        message: ControlMessage,
        events: &tokio::sync::mpsc::Sender<SessionEvent>,
    ) -> Option<ControlMessage> {
        match message {
            ControlMessage::Hello(hello) => {
                tracing::info!(session = ?hello.session_id, "screen-share: Hello received");
                if hello.session_id == ScreenShareSessionId::zero()
                    || self.sessions.len() >= MAX_ACTIVE_SESSIONS
                {
                    return Some(ControlMessage::Reject {
                        version: SCREEN_SHARE_PROTOCOL_VERSION,
                        session_id: hello.session_id,
                        reason: "session is not available".into(),
                    });
                }
                if hello.host_id != peer_id {
                    tracing::warn!(session = ?hello.session_id, "screen-share: Hello host_id does not match connected peer, rejecting");
                    return Some(ControlMessage::Reject {
                        version: SCREEN_SHARE_PROTOCOL_VERSION,
                        session_id: hello.session_id,
                        reason: "invitation identity does not match the connected peer".into(),
                    });
                }
                if hello.permission != Permission::ViewOnly {
                    return Some(ControlMessage::Reject {
                        version: SCREEN_SHARE_PROTOCOL_VERSION,
                        session_id: hello.session_id,
                        reason: "unsupported permission".into(),
                    });
                }
                // Reconnect (PDF Task 3.3 / REC-1): the SAME host re-offers a
                // session that is already active (or already reconnecting)
                // after a transient media failure. This is not a duplicate
                // offer — it is the media path re-establishing after the
                // connection dropped. Keep the session record (the chat/friend
                // session is unaffected), reset remote-control permissions to
                // view-only (REC-2), and surface Reconnecting so the app can
                // re-accept on the new connection. A fresh Hello for a pending
                // invite (Connecting) is also treated as a re-offer, not a
                // rejection, so a host that re-sends before the user decides
                // still works.
                if let Some(existing) = self.sessions.get(&hello.session_id) {
                    if existing.host_id == peer_id
                        && matches!(
                            existing.state,
                            SessionState::Connecting
                                | SessionState::Streaming
                                | SessionState::Reconnecting
                        )
                    {
                        if let Some(record) = self.sessions.get_mut(&hello.session_id) {
                            record.state = SessionState::Reconnecting;
                            record.peer_id = Some(peer_id);
                        }
                        if let Some(permissions) = self.permissions.get_mut(&hello.session_id) {
                            permissions.reset_for_reconnect();
                        }
                        let _ = events.try_send(SessionEvent::Reconnecting {
                            session_id: hello.session_id,
                        });
                        let _ = events.try_send(SessionEvent::ControlChanged {
                            session_id: hello.session_id,
                            active: false,
                            capabilities: vec![Capability::ViewScreen],
                        });
                        return None;
                    }
                    return Some(ControlMessage::Reject {
                        version: SCREEN_SHARE_PROTOCOL_VERSION,
                        session_id: hello.session_id,
                        reason: "session already exists".into(),
                    });
                }
                self.sessions.insert(
                    hello.session_id,
                    Record {
                        state: SessionState::Connecting,
                        host_id: hello.host_id,
                        peer_id: Some(peer_id),
                        conversation_id: hello.conversation_id,
                    },
                );
                self.permissions.insert(
                    hello.session_id,
                    SessionPermissions::view_only(hello.session_id, peer_id),
                );
                emit_event(
                    events,
                    SessionEvent::Invitation {
                        session_id: hello.session_id,
                        host_id: hello.host_id,
                        conversation_id: hello.conversation_id,
                        hello,
                    },
                );
                None
            }
            ControlMessage::Accept { session_id, .. } => {
                // The Accept always comes from the INVITEE (the peer the host
                // dialed), never from the host itself. Validate against the
                // recorded invitee: on the host record.peer_id holds the
                // viewer; on the viewer record.peer_id holds the host (set
                // when the Hello was applied). Checking host_id here would
                // make the host's own session unreachable — the check can
                // never pass because host_id is the host's own key.
                if let Some(record) = self.sessions.get_mut(&session_id) {
                    if record.peer_id == Some(peer_id)
                        && matches!(
                            record.state,
                            SessionState::AwaitingAcceptance
                                | SessionState::Connecting
                                | SessionState::Reconnecting
                        )
                    {
                        let was_reconnecting = record.state == SessionState::Reconnecting;
                        record.state = SessionState::Streaming;
                        self.permissions.insert(
                            session_id,
                            SessionPermissions::view_only(session_id, peer_id),
                        );
                        tracing::info!(session = ?session_id, "screen-share: session entered Streaming");
                        if was_reconnecting {
                            // A fresh Accept on a reconnecting session completes
                            // the reconnect (PDF Task 3.3).
                            emit_event(events, SessionEvent::Reconnected { session_id });
                        } else {
                            emit_event(
                                events,
                                SessionEvent::Accepted {
                                    session_id,
                                    peer_id,
                                },
                            );
                        }
                    } else {
                        tracing::warn!(session = ?session_id, "screen-share: Accept ignored (peer or state mismatch)");
                    }
                }
                None
            }
            ControlMessage::RequestControl {
                session_id,
                capabilities,
                ..
            } => {
                // RequestControl is a viewer → host message. Only the
                // INVITEE (record.peer_id, and never the host itself) may
                // request control; the host UI decides with an explicit
                // grant. A RequestControl from the host (e.g. on the viewer
                // side) is ignored.
                if let Some(record) = self.sessions.get(&session_id) {
                    if record.peer_id == Some(peer_id) && record.host_id != peer_id {
                        let _ = events.try_send(SessionEvent::ControlRequest {
                            session_id,
                            peer_id,
                            capabilities,
                        });
                    }
                }
                None
            }
            ControlMessage::GrantControl {
                session_id,
                capabilities,
                nonce,
                ..
            } => {
                // GrantControl is a host → viewer message. Only the HOST
                // (record.host_id) may grant control; the viewer stores the
                // host's nonce so it can echo it back in every Input message,
                // and host-side validation uses that nonce. A GrantControl
                // from the viewer (a forged self-grant attempt on the host)
                // is ignored — remote control is never granted by the peer
                // that would receive it.
                let from_host = self
                    .sessions
                    .get(&session_id)
                    .is_some_and(|record| record.host_id == peer_id);
                if from_host {
                    if let Some(permissions) = self.permissions.get_mut(&session_id) {
                        if permissions.peer_id() == peer_id {
                            permissions.grant_with_nonce(capabilities.clone(), nonce);
                            let _ = events.try_send(SessionEvent::ControlChanged {
                                session_id,
                                active: true,
                                capabilities,
                            });
                        }
                    }
                }
                None
            }
            ControlMessage::RevokeControl { session_id, .. } => {
                // RevokeControl is a host → viewer message; only the HOST may
                // revoke. A forged RevokeControl from the viewer is ignored.
                let from_host = self
                    .sessions
                    .get(&session_id)
                    .is_some_and(|record| record.host_id == peer_id);
                if from_host {
                    if let Some(permissions) = self.permissions.get_mut(&session_id) {
                        if permissions.peer_id() == peer_id {
                            permissions.revoke_control();
                            let _ = events.try_send(SessionEvent::ControlChanged {
                                session_id,
                                active: false,
                                capabilities: vec![Capability::ViewScreen],
                            });
                        }
                    }
                }
                None
            }
            ControlMessage::Input { .. } => None,
            ControlMessage::Reject {
                session_id, reason, ..
            } => {
                if let Some(record) = self.sessions.get(&session_id) {
                    if record.host_id != peer_id && record.peer_id != Some(peer_id) {
                        return None;
                    }
                }
                self.permissions.remove(&session_id);
                if self.sessions.remove(&session_id).is_some() {
                    let _ = events.try_send(SessionEvent::Rejected { session_id, reason });
                }
                None
            }
            ControlMessage::EndSession { session_id, .. } => {
                if let Some(record) = self.sessions.get_mut(&session_id) {
                    if record.host_id != peer_id && record.peer_id != Some(peer_id) {
                        return None;
                    }
                    if record.state != SessionState::Ended {
                        record.state = SessionState::Ended;
                        if let Some(permissions) = self.permissions.get_mut(&session_id) {
                            permissions.end();
                        }
                        let _ = events.try_send(SessionEvent::Ended { session_id });
                    }
                }
                None
            }
        }
    }
}

/// Minimal session record retained for compatibility with the subsystem boundary.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ScreenShareSession {
    id: ScreenShareSessionId,
    conversation_id: u64,
}
impl Default for ScreenShareSession {
    fn default() -> Self {
        Self::new()
    }
}

impl ScreenShareSession {
    pub fn new() -> Self {
        Self {
            id: ScreenShareSessionId::generate(),
            conversation_id: 0,
        }
    }
    pub fn for_conversation(conversation_id: u64) -> Self {
        Self {
            id: ScreenShareSessionId::generate(),
            conversation_id,
        }
    }
    pub const fn id(&self) -> ScreenShareSessionId {
        self.id
    }
    pub const fn conversation_id(&self) -> u64 {
        self.conversation_id
    }
}

/// Role a node plays in one negotiation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum NegotiationRole {
    /// The node that sends the offer.
    Initiator,
    /// The node that receives the offer and decides to accept or reject.
    Recipient,
}

/// Lifecycle of a negotiation before streaming starts.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum NegotiationState {
    /// Offer sent (initiator) or received (recipient); awaiting a decision.
    Pending,
    /// The recipient explicitly accepted; capture may begin.
    Accepted,
    /// The negotiation ended without streaming (reject, cancel, timeout,
    /// peer disconnect, or protocol error).
    Closed,
}

/// The mutually supported configuration the recipient selected.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NegotiatedConfig {
    /// Selected codec (one of the offer's `codecs`).
    pub codec: String,
    /// Selected capture width.
    pub width: u16,
    /// Selected capture height.
    pub height: u16,
    /// Selected frame rate.
    pub frame_rate: u16,
}

impl NegotiatedConfig {
    /// Select a mutually supported configuration from the offer given the
    /// recipient's locally supported codecs. Picks the first codec both sides
    /// support, the first offered resolution, and the highest offered frame
    /// rate. Returns `None` when there is no shared codec or the message is
    /// not a `ScreenShareOffer`.
    pub fn select(offer: &ScreenShareMessage, local_codecs: &[String]) -> Option<Self> {
        let ScreenShareMessage::ScreenShareOffer {
            codecs,
            resolutions,
            frame_rate_max,
            ..
        } = offer
        else {
            return None;
        };
        let codec = codecs
            .iter()
            .find(|offered| {
                local_codecs
                    .iter()
                    .any(|local| local.eq_ignore_ascii_case(offered))
            })?
            .clone();
        let (width, height) = resolutions.first().copied()?;
        Some(Self {
            codec,
            width,
            height,
            frame_rate: *frame_rate_max,
        })
    }

    /// True when this configuration lies within the offer's advertised
    /// capabilities (codec list, resolution list, frame-rate range).
    pub fn within(&self, offer: &ScreenShareMessage) -> bool {
        let ScreenShareMessage::ScreenShareOffer {
            codecs,
            resolutions,
            frame_rate_min,
            frame_rate_max,
            ..
        } = offer
        else {
            return false;
        };
        self.codecs_contains(codecs)
            && resolutions
                .iter()
                .any(|(w, h)| *w == self.width && *h == self.height)
            && self.frame_rate >= *frame_rate_min
            && self.frame_rate <= *frame_rate_max
    }

    fn codecs_contains(&self, offered: &[String]) -> bool {
        offered
            .iter()
            .any(|codec| codec.eq_ignore_ascii_case(&self.codec))
    }
}

/// Stable failure for a negotiation operation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NegotiationError {
    /// No negotiation is known for the session id.
    UnknownSession,
    /// A duplicate offer arrived for an already-pending session.
    DuplicateOffer,
    /// Too many concurrent negotiations.
    Capacity,
    /// The operation is not valid in the current state.
    WrongState,
    /// The remote peer is not the recorded counterparty.
    PeerMismatch,
    /// The selected configuration is not mutually supported.
    UnsupportedConfig(String),
    /// The session id is the all-zero sentinel.
    EmptySessionId,
}

#[derive(Debug, Clone)]
struct NegotiationRecord {
    role: NegotiationRole,
    state: NegotiationState,
    host_id: iroh::PublicKey,
    peer_id: iroh::PublicKey,
    conversation_id: u64,
    offer: ScreenShareMessage,
    selected: Option<NegotiatedConfig>,
    deadline: Instant,
}

/// Bounded in-memory state for negotiations (PDF Task 3.1).
///
/// The initiator records an offer before sending it; the recipient records an
/// offer when it arrives. Only an explicit accept transitions a negotiation to
/// [`NegotiationState::Accepted`], and capture is only permitted in that state
/// ([`NegotiationManager::can_start_capture`]). Duplicate offers are rejected
/// explicitly (matching the existing Hello convention), and pending
/// negotiations expire at their deadline.
#[derive(Debug, Default)]
pub struct NegotiationManager {
    negotiations: HashMap<ScreenShareSessionId, NegotiationRecord>,
}

/// Maximum concurrent negotiations tracked by one manager.
pub const MAX_ACTIVE_NEGOTIATIONS: usize = 8;

/// Extract the offer fields from a `ScreenShareMessage`, returning `None`
/// when the message is not a `ScreenShareOffer`.
fn as_offer(message: &ScreenShareMessage) -> Option<(ScreenShareSessionId, iroh::PublicKey, u64)> {
    match message {
        ScreenShareMessage::ScreenShareOffer {
            session_id,
            host_id,
            conversation_id,
            ..
        } => Some((*session_id, *host_id, *conversation_id)),
        _ => None,
    }
}

impl NegotiationManager {
    /// Create an empty negotiation store.
    pub fn new() -> Self {
        Self::default()
    }

    /// Current state of a negotiation, if known.
    pub fn state(&self, id: ScreenShareSessionId) -> Option<NegotiationState> {
        self.negotiations.get(&id).map(|record| record.state)
    }

    /// The configuration selected by the recipient, once accepted.
    pub fn selected(&self, id: ScreenShareSessionId) -> Option<&NegotiatedConfig> {
        self.negotiations
            .get(&id)
            .and_then(|record| record.selected.as_ref())
    }

    /// The offer being negotiated, if known.
    pub fn offer(&self, id: ScreenShareSessionId) -> Option<&ScreenShareMessage> {
        self.negotiations.get(&id).map(|record| &record.offer)
    }

    /// The role this node plays in the negotiation, if known.
    pub fn role(&self, id: ScreenShareSessionId) -> Option<NegotiationRole> {
        self.negotiations.get(&id).map(|record| record.role)
    }

    /// Number of negotiations in a state that still occupies a slot
    /// (Pending or Accepted). Closed records no longer consume capacity.
    fn active_count(&self) -> usize {
        self.negotiations
            .values()
            .filter(|record| record.state != NegotiationState::Closed)
            .count()
    }

    /// Record an offer the local node is about to send (initiator side).
    pub fn start_offer(
        &mut self,
        offer: ScreenShareMessage,
        peer: iroh::PublicKey,
        timeout: Duration,
    ) -> Result<(), NegotiationError> {
        let Some((id, host_id, conversation_id)) = as_offer(&offer) else {
            return Err(NegotiationError::WrongState);
        };
        if id == ScreenShareSessionId::zero() {
            return Err(NegotiationError::EmptySessionId);
        }
        if self.negotiations.contains_key(&id) {
            return Err(NegotiationError::DuplicateOffer);
        }
        if self.active_count() >= MAX_ACTIVE_NEGOTIATIONS {
            return Err(NegotiationError::Capacity);
        }
        self.negotiations.insert(
            id,
            NegotiationRecord {
                role: NegotiationRole::Initiator,
                state: NegotiationState::Pending,
                host_id,
                peer_id: peer,
                conversation_id,
                offer,
                selected: None,
                deadline: Instant::now() + timeout,
            },
        );
        Ok(())
    }

    /// Record an offer that arrived from `peer` (recipient side). A duplicate
    /// offer for an already-pending session is an explicit error, matching the
    /// existing Hello convention. Emits [`SessionEvent::NegotiationInvitation`]
    /// so the UI can present the capabilities.
    pub fn receive_offer(
        &mut self,
        peer: iroh::PublicKey,
        offer: ScreenShareMessage,
        timeout: Duration,
        events: &tokio::sync::mpsc::Sender<SessionEvent>,
    ) -> Result<(), NegotiationError> {
        let Some((id, host_id, conversation_id)) = as_offer(&offer) else {
            return Err(NegotiationError::WrongState);
        };
        if id == ScreenShareSessionId::zero() {
            return Err(NegotiationError::EmptySessionId);
        }
        if self.negotiations.contains_key(&id) {
            return Err(NegotiationError::DuplicateOffer);
        }
        if self.active_count() >= MAX_ACTIVE_NEGOTIATIONS {
            return Err(NegotiationError::Capacity);
        }
        if host_id != peer {
            return Err(NegotiationError::PeerMismatch);
        }
        self.negotiations.insert(
            id,
            NegotiationRecord {
                role: NegotiationRole::Recipient,
                state: NegotiationState::Pending,
                host_id,
                peer_id: peer,
                conversation_id,
                offer: offer.clone(),
                selected: None,
                deadline: Instant::now() + timeout,
            },
        );
        emit_event(
            events,
            SessionEvent::NegotiationInvitation {
                session_id: id,
                host_id,
                conversation_id,
                offer,
            },
        );
        Ok(())
    }

    /// Recipient-side: accept a pending negotiation with the selected
    /// configuration. Returns the wire `ScreenShareAccept` to send back and
    /// transitions the record to `Accepted` (the only state that permits
    /// capture).
    pub fn accept(
        &mut self,
        id: ScreenShareSessionId,
        config: NegotiatedConfig,
        events: &tokio::sync::mpsc::Sender<SessionEvent>,
    ) -> Result<ScreenShareMessage, NegotiationError> {
        let record = self
            .negotiations
            .get_mut(&id)
            .ok_or(NegotiationError::UnknownSession)?;
        if record.role != NegotiationRole::Recipient {
            return Err(NegotiationError::WrongState);
        }
        if record.state != NegotiationState::Pending {
            return Err(NegotiationError::WrongState);
        }
        if !config.within(&record.offer) {
            return Err(NegotiationError::UnsupportedConfig(format!(
                "codec {}, {}x{} @ {}fps not offered",
                config.codec, config.width, config.height, config.frame_rate
            )));
        }
        record.selected = Some(config.clone());
        record.state = NegotiationState::Accepted;
        emit_event(
            events,
            SessionEvent::Accepted {
                session_id: id,
                peer_id: record.peer_id,
            },
        );
        Ok(ScreenShareMessage::ScreenShareAccept {
            version: SCREEN_SHARE_PROTOCOL_VERSION,
            session_id: id,
            codec: config.codec,
            width: config.width,
            height: config.height,
            frame_rate: config.frame_rate,
        })
    }

    /// Recipient-side: reject a pending negotiation. Returns the wire
    /// `ScreenShareReject` to send back.
    pub fn reject(
        &mut self,
        id: ScreenShareSessionId,
        reason: impl Into<String>,
        events: &tokio::sync::mpsc::Sender<SessionEvent>,
    ) -> Result<ScreenShareMessage, NegotiationError> {
        let record = self
            .negotiations
            .get_mut(&id)
            .ok_or(NegotiationError::UnknownSession)?;
        if record.role != NegotiationRole::Recipient {
            return Err(NegotiationError::WrongState);
        }
        if record.state != NegotiationState::Pending {
            return Err(NegotiationError::WrongState);
        }
        let reason = reason.into();
        record.state = NegotiationState::Closed;
        emit_event(
            events,
            SessionEvent::Rejected {
                session_id: id,
                reason: reason.clone(),
            },
        );
        Ok(ScreenShareMessage::ScreenShareReject {
            version: SCREEN_SHARE_PROTOCOL_VERSION,
            session_id: id,
            reason,
        })
    }

    /// Initiator-side: cancel a pending offer before the recipient responds.
    /// Returns the wire `ScreenShareReject` to inform the peer.
    pub fn cancel(
        &mut self,
        id: ScreenShareSessionId,
        reason: impl Into<String>,
        events: &tokio::sync::mpsc::Sender<SessionEvent>,
    ) -> Result<ScreenShareMessage, NegotiationError> {
        let record = self
            .negotiations
            .get_mut(&id)
            .ok_or(NegotiationError::UnknownSession)?;
        if record.role != NegotiationRole::Initiator {
            return Err(NegotiationError::WrongState);
        }
        if record.state != NegotiationState::Pending {
            return Err(NegotiationError::WrongState);
        }
        let reason = reason.into();
        record.state = NegotiationState::Closed;
        emit_event(
            events,
            SessionEvent::Rejected {
                session_id: id,
                reason: reason.clone(),
            },
        );
        Ok(ScreenShareMessage::ScreenShareReject {
            version: SCREEN_SHARE_PROTOCOL_VERSION,
            session_id: id,
            reason,
        })
    }

    /// Initiator-side: the remote `Accept` arrived. Validates the selected
    /// configuration is mutually supported (within the offered capabilities),
    /// then transitions to `Accepted`. Capture may begin after this.
    pub fn handle_accept(
        &mut self,
        peer: iroh::PublicKey,
        accept: ScreenShareMessage,
        events: &tokio::sync::mpsc::Sender<SessionEvent>,
    ) -> Result<(), NegotiationError> {
        let ScreenShareMessage::ScreenShareAccept {
            session_id,
            codec,
            width,
            height,
            frame_rate,
            ..
        } = &accept
        else {
            return Err(NegotiationError::WrongState);
        };
        let id = *session_id;
        let record = self
            .negotiations
            .get_mut(&id)
            .ok_or(NegotiationError::UnknownSession)?;
        if record.role != NegotiationRole::Initiator {
            return Err(NegotiationError::WrongState);
        }
        if record.state != NegotiationState::Pending {
            return Err(NegotiationError::WrongState);
        }
        if record.peer_id != peer {
            return Err(NegotiationError::PeerMismatch);
        }
        let config = NegotiatedConfig {
            codec: codec.clone(),
            width: *width,
            height: *height,
            frame_rate: *frame_rate,
        };
        if !config.within(&record.offer) {
            return Err(NegotiationError::UnsupportedConfig(format!(
                "codec {}, {}x{} @ {}fps not offered",
                config.codec, config.width, config.height, config.frame_rate
            )));
        }
        record.selected = Some(config);
        record.state = NegotiationState::Accepted;
        emit_event(
            events,
            SessionEvent::Accepted {
                session_id: id,
                peer_id: peer,
            },
        );
        Ok(())
    }

    /// Either side: the remote `Reject` arrived; the negotiation is closed.
    pub fn handle_reject(
        &mut self,
        peer: iroh::PublicKey,
        reject: ScreenShareMessage,
        events: &tokio::sync::mpsc::Sender<SessionEvent>,
    ) -> Result<(), NegotiationError> {
        let ScreenShareMessage::ScreenShareReject {
            session_id, reason, ..
        } = &reject
        else {
            return Err(NegotiationError::WrongState);
        };
        let id = *session_id;
        let record = self
            .negotiations
            .get_mut(&id)
            .ok_or(NegotiationError::UnknownSession)?;
        if record.peer_id != peer {
            return Err(NegotiationError::PeerMismatch);
        }
        if record.state == NegotiationState::Closed {
            return Err(NegotiationError::WrongState);
        }
        record.state = NegotiationState::Closed;
        emit_event(
            events,
            SessionEvent::Rejected {
                session_id: id,
                reason: reason.clone(),
            },
        );
        Ok(())
    }

    /// Close every pending negotiation whose deadline has passed. Returns the
    /// closed session ids so the caller can notify the wire layer.
    pub fn expire_pending(
        &mut self,
        now: Instant,
        events: &tokio::sync::mpsc::Sender<SessionEvent>,
    ) -> Vec<ScreenShareSessionId> {
        let expired: Vec<ScreenShareSessionId> = self
            .negotiations
            .iter()
            .filter(|(_, record)| {
                record.state == NegotiationState::Pending && record.deadline <= now
            })
            .map(|(id, _)| *id)
            .collect();
        for id in &expired {
            if let Some(record) = self.negotiations.get_mut(id) {
                record.state = NegotiationState::Closed;
                emit_event(
                    events,
                    SessionEvent::Rejected {
                        session_id: *id,
                        reason: "negotiation timed out".into(),
                    },
                );
            }
        }
        expired
    }

    /// Close every open negotiation involving `peer` (the peer disconnected).
    /// Returns the closed session ids.
    pub fn peer_disconnected(
        &mut self,
        peer: iroh::PublicKey,
        events: &tokio::sync::mpsc::Sender<SessionEvent>,
    ) -> Vec<ScreenShareSessionId> {
        let affected: Vec<ScreenShareSessionId> = self
            .negotiations
            .iter()
            .filter(|(_, record)| {
                record.state != NegotiationState::Closed
                    && (record.host_id == peer || record.peer_id == peer)
            })
            .map(|(id, _)| *id)
            .collect();
        for id in &affected {
            if let Some(record) = self.negotiations.get_mut(id) {
                record.state = NegotiationState::Closed;
                emit_event(events, SessionEvent::Ended { session_id: *id });
            }
        }
        affected
    }

    /// Capture gate: capture must NOT begin before explicit acceptance.
    /// Returns true only for negotiations in the `Accepted` state.
    pub fn can_start_capture(&self, id: ScreenShareSessionId) -> bool {
        self.negotiations
            .get(&id)
            .is_some_and(|record| record.state == NegotiationState::Accepted)
    }

    /// Number of tracked negotiations (including closed records until
    /// [`Self::prune_closed`] runs). Bounded by [`MAX_ACTIVE_NEGOTIATIONS`]
    /// for pending ones.
    pub fn len(&self) -> usize {
        self.negotiations.len()
    }
    /// True when no negotiations are tracked.
    pub fn is_empty(&self) -> bool {
        self.negotiations.is_empty()
    }
    /// Drop closed records so a long-lived manager does not grow without
    /// bound. Pending and accepted records are kept.
    pub fn prune_closed(&mut self) {
        self.negotiations
            .retain(|_, record| record.state != NegotiationState::Closed);
    }
}

/// Push a session event to the app-facing channel, logging a warning if the
/// bounded channel is full (a dropped Invitation/Accepted is a silent
/// negotiation failure otherwise).
fn emit_event(events: &tokio::sync::mpsc::Sender<SessionEvent>, event: SessionEvent) {
    if let Err(
        tokio::sync::mpsc::error::TrySendError::Full(ev)
        | tokio::sync::mpsc::error::TrySendError::Closed(ev),
    ) = events.try_send(event)
    {
        tracing::warn!(
            ?ev,
            "screen-share: session event dropped (receiver full or closed)"
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn accept_requires_pending_invitation() {
        let key = iroh::SecretKey::generate().public();
        let mut manager = SessionManager::default();
        let id = ScreenShareSessionId::generate();
        assert!(manager.accept_invitation(id, key).is_none());
    }
    #[test]
    fn end_is_idempotent() {
        let key = iroh::SecretKey::generate().public();
        let peer = iroh::SecretKey::generate().public();
        let id = ScreenShareSessionId::generate();
        let mut manager = SessionManager::default();
        manager.start_invitation(id, key, peer, 1);
        assert!(manager.end(id).is_some());
        assert!(manager.end(id).is_none());
        assert_eq!(manager.state(id), Some(SessionState::Ended));
    }

    /// PDF Phase 11 Security matrix — "no capture before consent". Before
    /// the viewer's explicit Accept there is NO permission record, so no
    /// capability (view, input, clipboard) can be authorized; the manager
    /// ignores anything that is not the Accept. Only the explicit Accept
    /// opens streaming, and it opens to view-only — remote input still
    /// requires a separate explicit grant.
    #[test]
    fn no_capture_before_consent() {
        let host = iroh::SecretKey::generate().public();
        let viewer = iroh::SecretKey::generate().public();
        let mut manager = SessionManager::default();
        let (tx, mut rx) = tokio::sync::mpsc::channel(8);
        let id = ScreenShareSessionId::generate();
        manager.start_invitation(id, host, viewer, 42);
        assert_eq!(manager.state(id), Some(SessionState::AwaitingAcceptance));
        assert!(
            manager.permissions(id).is_none(),
            "no permission record before consent — nothing can be authorized"
        );
        // A forged Accept from the HOST itself (not the invitee) is ignored.
        assert!(manager
            .apply_remote(
                host,
                ControlMessage::Accept {
                    version: SCREEN_SHARE_PROTOCOL_VERSION,
                    session_id: id
                },
                &tx
            )
            .is_none());
        assert_eq!(manager.state(id), Some(SessionState::AwaitingAcceptance));
        // The INVITEE's explicit Accept is the only way into Streaming.
        assert!(manager
            .apply_remote(
                viewer,
                ControlMessage::Accept {
                    version: SCREEN_SHARE_PROTOCOL_VERSION,
                    session_id: id
                },
                &tx
            )
            .is_none());
        assert_eq!(manager.state(id), Some(SessionState::Streaming));
        let permissions = manager.permissions(id).expect("permissions after consent");
        assert!(
            permissions.is_view_only(),
            "every share defaults to view-only"
        );
        assert!(
            !permissions.allows(id, viewer, Capability::ControlPointer),
            "no input without an explicit permission grant"
        );
        assert!(
            !permissions.allows(id, viewer, Capability::ControlKeyboard),
            "no keyboard without an explicit permission grant"
        );
        assert!(permissions.allows(id, viewer, Capability::ViewScreen));
        assert_eq!(
            rx.try_recv().unwrap(),
            SessionEvent::Accepted {
                session_id: id,
                peer_id: viewer
            }
        );
    }

    /// PDF Phase 11 Security matrix — "peer disconnect cleanup". When the
    /// peer disconnects (EndSession from the viewer, or the host loop's
    /// connection-error exit path), the session ends and the permission
    /// record becomes inactive, so any late input/view attempt fails
    /// authorization immediately — even if control was granted before the
    /// disconnect.
    #[test]
    fn peer_disconnect_during_streaming_cleans_permissions() {
        let host = iroh::SecretKey::generate().public();
        let viewer = iroh::SecretKey::generate().public();
        let mut manager = SessionManager::default();
        let (tx, mut rx) = tokio::sync::mpsc::channel(8);
        let id = ScreenShareSessionId::generate();
        manager.start_invitation(id, host, viewer, 7);
        // Stream with control granted (the strongest pre-disconnect state).
        manager.apply_remote(
            viewer,
            ControlMessage::Accept {
                version: SCREEN_SHARE_PROTOCOL_VERSION,
                session_id: id,
            },
            &tx,
        );
        assert!(manager
            .grant_control(id, vec![Capability::ControlPointer], &tx)
            .is_some());
        assert!(manager
            .permissions(id)
            .unwrap()
            .allows(id, viewer, Capability::ControlPointer));
        let _ = rx.try_recv(); // Accepted
        let _ = rx.try_recv(); // ControlChanged(active:true)
                               // The peer disconnects: its EndSession arrives (the host loop's
                               // connection-error path ends the session the same way).
        manager.apply_remote(
            viewer,
            ControlMessage::EndSession {
                version: SCREEN_SHARE_PROTOCOL_VERSION,
                session_id: id,
            },
            &tx,
        );
        assert_eq!(manager.state(id), Some(SessionState::Ended));
        let permissions = manager.permissions(id).expect("permission record retained");
        assert!(!permissions.is_active(), "ended session is inactive");
        assert!(
            !permissions.allows(id, viewer, Capability::ControlPointer),
            "late input after disconnect fails authorization"
        );
        assert!(
            !permissions.allows(id, viewer, Capability::ViewScreen),
            "no late view after disconnect either"
        );
        assert_eq!(
            rx.try_recv().unwrap(),
            SessionEvent::Ended { session_id: id }
        );
    }

    /// REC-1: begin_reconnect keeps the session record alive (the chat/friend
    /// session survives a transient media failure) but transitions to
    /// Reconnecting and emits the event.
    #[test]
    fn begin_reconnect_preserves_session_and_emits_event() {
        let host = iroh::SecretKey::generate().public();
        let viewer = iroh::SecretKey::generate().public();
        let mut manager = SessionManager::default();
        let id = ScreenShareSessionId::generate();
        manager.start_invitation(id, host, viewer, 42);
        let (tx, mut rx) = tokio::sync::mpsc::channel(8);
        manager.apply_remote(
            viewer,
            ControlMessage::Accept {
                version: SCREEN_SHARE_PROTOCOL_VERSION,
                session_id: id,
            },
            &tx,
        );
        assert_eq!(manager.state(id), Some(SessionState::Streaming));
        // Drain the Accepted emitted by the Accept before the reconnect.
        assert!(
            matches!(rx.try_recv(), Ok(SessionEvent::Accepted { session_id, .. }) if session_id == id)
        );

        assert!(manager.begin_reconnect(id, &tx));
        assert_eq!(manager.state(id), Some(SessionState::Reconnecting));
        assert_eq!(
            rx.try_recv().unwrap(),
            SessionEvent::Reconnecting { session_id: id }
        );
        // The session is still tracked — the chat/friend session survives.
        assert!(manager.permissions(id).is_some());
        // Control was reset: only ViewScreen remains.
        assert_eq!(
            manager.permissions(id).unwrap().capabilities(),
            &[Capability::ViewScreen]
        );
    }

    /// begin_reconnect on a session that is not streaming is a no-op.
    #[test]
    fn begin_reconnect_requires_streaming() {
        let host = iroh::SecretKey::generate().public();
        let viewer = iroh::SecretKey::generate().public();
        let mut manager = SessionManager::default();
        let id = ScreenShareSessionId::generate();
        manager.start_invitation(id, host, viewer, 1);
        let (tx, _rx) = tokio::sync::mpsc::channel(8);
        assert!(!manager.begin_reconnect(id, &tx));
        assert_eq!(manager.state(id), Some(SessionState::AwaitingAcceptance));
    }

    /// REC-2: complete_reconnect returns to Streaming but does NOT silently
    /// re-grant control capabilities that were active before the reconnect.
    #[test]
    fn complete_reconnect_returns_streaming_without_control_resume() {
        let host = iroh::SecretKey::generate().public();
        let viewer = iroh::SecretKey::generate().public();
        let mut manager = SessionManager::default();
        let id = ScreenShareSessionId::generate();
        manager.start_invitation(id, host, viewer, 7);
        let (tx, mut rx) = tokio::sync::mpsc::channel(8);
        manager.apply_remote(
            viewer,
            ControlMessage::Accept {
                version: SCREEN_SHARE_PROTOCOL_VERSION,
                session_id: id,
            },
            &tx,
        );
        // Grant control BEFORE the failure; the reconnect must not resume it.
        assert!(manager
            .grant_control(
                id,
                vec![Capability::ControlPointer, Capability::ControlKeyboard],
                &tx
            )
            .is_some());
        assert!(manager
            .permissions(id)
            .unwrap()
            .allows(id, viewer, Capability::ControlPointer));
        // Drain events emitted so far (Accepted, ControlChanged(active:true)).
        let _ = rx.try_recv();
        let _ = rx.try_recv();

        assert!(manager.begin_reconnect(id, &tx));
        assert_eq!(
            manager.permissions(id).unwrap().capabilities(),
            &[Capability::ViewScreen]
        );
        let _ = rx.try_recv(); // Reconnecting
        let _ = rx.try_recv(); // ControlChanged(active:false)
        assert!(manager.complete_reconnect(id, &tx));
        assert_eq!(manager.state(id), Some(SessionState::Streaming));
        assert_eq!(
            rx.try_recv().unwrap(),
            SessionEvent::Reconnected { session_id: id }
        );
        // Control was NOT silently resumed after the reconnect.
        assert_eq!(
            manager.permissions(id).unwrap().capabilities(),
            &[Capability::ViewScreen]
        );
        assert!(!manager
            .permissions(id)
            .unwrap()
            .allows(id, viewer, Capability::ControlPointer));
    }

    /// fail_reconnect abandons the reconnect and ends the session.
    #[test]
    fn fail_reconnect_ends_session() {
        let host = iroh::SecretKey::generate().public();
        let viewer = iroh::SecretKey::generate().public();
        let mut manager = SessionManager::default();
        let id = ScreenShareSessionId::generate();
        manager.start_invitation(id, host, viewer, 1);
        let (tx, mut rx) = tokio::sync::mpsc::channel(8);
        manager.apply_remote(
            viewer,
            ControlMessage::Accept {
                version: SCREEN_SHARE_PROTOCOL_VERSION,
                session_id: id,
            },
            &tx,
        );
        let _ = rx.try_recv(); // Accepted
        assert!(manager.begin_reconnect(id, &tx));
        assert!(manager.fail_reconnect(id, &tx));
        assert_eq!(manager.state(id), Some(SessionState::Ended));
        assert_eq!(
            rx.try_recv().unwrap(),
            SessionEvent::Reconnecting { session_id: id }
        );
        assert_eq!(
            rx.try_recv().unwrap(),
            SessionEvent::ControlChanged {
                session_id: id,
                active: false,
                capabilities: vec![Capability::ViewScreen]
            }
        );
        assert_eq!(
            rx.try_recv().unwrap(),
            SessionEvent::Ended { session_id: id }
        );
    }

    /// A re-Hello for the same session from the same host is treated as a
    /// reconnect (not a duplicate-offer rejection): the session survives,
    /// permissions reset to view-only, and Reconnecting is emitted.
    #[test]
    fn rehello_from_same_host_reconnects_active_session() {
        let host = iroh::SecretKey::generate().public();
        let viewer = iroh::SecretKey::generate().public();
        let mut manager = SessionManager::default();
        let id = ScreenShareSessionId::generate();
        manager.start_invitation(id, host, viewer, 7);
        let (tx, mut rx) = tokio::sync::mpsc::channel(8);
        manager.apply_remote(
            viewer,
            ControlMessage::Accept {
                version: SCREEN_SHARE_PROTOCOL_VERSION,
                session_id: id,
            },
            &tx,
        );
        assert!(manager
            .grant_control(id, vec![Capability::ControlPointer], &tx)
            .is_some());
        let _ = rx.try_recv(); // Accepted
        let _ = rx.try_recv(); // ControlChanged(active:true)

        let hello = Hello {
            version: SCREEN_SHARE_PROTOCOL_VERSION,
            session_id: id,
            host_id: host,
            conversation_id: 7,
            codecs: vec!["h264".into()],
            width: 640,
            height: 360,
            frame_rate: 15,
            permission: Permission::ViewOnly,
        };
        // The re-Hello arrives from the HOST's connection, exactly like the
        // original Hello did.
        let response = manager.apply_remote(host, ControlMessage::Hello(hello), &tx);
        assert!(response.is_none(), "a reconnect Hello must not be rejected");
        assert_eq!(manager.state(id), Some(SessionState::Reconnecting));
        assert_eq!(
            rx.try_recv().unwrap(),
            SessionEvent::Reconnecting { session_id: id }
        );
        assert_eq!(
            manager.permissions(id).unwrap().capabilities(),
            &[Capability::ViewScreen]
        );
    }

    /// A re-Hello from a DIFFERENT host for an existing session is still a
    /// duplicate-offer rejection (strangers must not hijack a session).
    #[test]
    fn rehello_from_stranger_is_rejected() {
        let host = iroh::SecretKey::generate().public();
        let stranger = iroh::SecretKey::generate().public();
        let viewer = iroh::SecretKey::generate().public();
        let mut manager = SessionManager::default();
        let id = ScreenShareSessionId::generate();
        manager.start_invitation(id, host, viewer, 7);
        let (tx, mut rx) = tokio::sync::mpsc::channel(8);
        manager.apply_remote(
            viewer,
            ControlMessage::Accept {
                version: SCREEN_SHARE_PROTOCOL_VERSION,
                session_id: id,
            },
            &tx,
        );

        let hello = Hello {
            version: SCREEN_SHARE_PROTOCOL_VERSION,
            session_id: id,
            host_id: stranger,
            conversation_id: 7,
            codecs: vec!["h264".into()],
            width: 640,
            height: 360,
            frame_rate: 15,
            permission: Permission::ViewOnly,
        };
        let response = manager.apply_remote(viewer, ControlMessage::Hello(hello), &tx);
        assert!(
            matches!(response, Some(ControlMessage::Reject { reason, .. }) if reason == "invitation identity does not match the connected peer")
        );
        assert_eq!(manager.state(id), Some(SessionState::Streaming));
        let _ = rx.try_recv(); // drain any stray event
    }
    /// Regression: the HOST's manager records the invitee and must transition
    /// to Streaming when the INVITEE's Accept arrives. (The old check compared
    /// against record.host_id — the host's own key — so the Accept was
    /// silently ignored and the host never streamed: "waiting for viewer to
    /// accept" forever, viewer sees nothing.)
    #[test]
    fn remote_accept_from_invitee_transitions_host_to_streaming() {
        let host = iroh::SecretKey::generate().public();
        let viewer = iroh::SecretKey::generate().public();
        let mut manager = SessionManager::default();
        let id = ScreenShareSessionId::generate();
        manager.start_invitation(id, host, viewer, 7);
        let (tx, mut rx) = tokio::sync::mpsc::channel(8);
        let response = manager.apply_remote(
            viewer,
            ControlMessage::Accept {
                version: SCREEN_SHARE_PROTOCOL_VERSION,
                session_id: id,
            },
            &tx,
        );
        assert!(response.is_none());
        assert_eq!(manager.state(id), Some(SessionState::Streaming));
        match rx.try_recv() {
            Ok(SessionEvent::Accepted {
                session_id,
                peer_id,
            }) => {
                assert_eq!(session_id, id);
                assert_eq!(peer_id, viewer);
            }
            other => panic!("expected Accepted event, got {other:?}"),
        }
    }
    /// Mirror case: the viewer's manager (record built from the remote Hello)
    /// also accepts via the invitee check.
    #[test]
    fn remote_accept_from_invitee_transitions_viewer_to_streaming() {
        let host = iroh::SecretKey::generate().public();
        let mut manager = SessionManager::default();
        let id = ScreenShareSessionId::generate();
        let hello = crate::screen_share::protocol::Hello {
            version: SCREEN_SHARE_PROTOCOL_VERSION,
            session_id: id,
            host_id: host,
            conversation_id: 7,
            codecs: vec!["h264".into()],
            width: 640,
            height: 360,
            frame_rate: 15,
            permission: Permission::ViewOnly,
        };
        let (tx, _rx) = tokio::sync::mpsc::channel(8);
        manager.apply_remote(host, ControlMessage::Hello(hello), &tx);
        assert_eq!(manager.state(id), Some(SessionState::Connecting));
        manager.apply_remote(
            host,
            ControlMessage::Accept {
                version: SCREEN_SHARE_PROTOCOL_VERSION,
                session_id: id,
            },
            &tx,
        );
        assert_eq!(manager.state(id), Some(SessionState::Streaming));
    }
    /// PDF Task 9.1 / T5.3: a share defaults to view-only even after the
    /// viewer accepts; remote control requires a separate explicit host grant.
    #[test]
    fn accept_never_grants_control_without_explicit_grant() {
        let host = iroh::SecretKey::generate().public();
        let viewer = iroh::SecretKey::generate().public();
        let mut manager = SessionManager::default();
        let id = ScreenShareSessionId::generate();
        manager.start_invitation(id, host, viewer, 7);
        let (tx, _rx) = tokio::sync::mpsc::channel(8);
        manager.apply_remote(
            viewer,
            ControlMessage::Accept {
                version: SCREEN_SHARE_PROTOCOL_VERSION,
                session_id: id,
            },
            &tx,
        );
        assert_eq!(manager.state(id), Some(SessionState::Streaming));
        let permissions = manager.permissions(id).unwrap();
        // View-only: the viewer can see the screen but cannot inject input.
        assert!(permissions.allows(id, viewer, Capability::ViewScreen));
        assert!(!permissions.allows(id, viewer, Capability::ControlPointer));
        assert!(!permissions.allows(id, viewer, Capability::ControlKeyboard));
        assert!(permissions.token().is_none());
        // Only an explicit host-side grant adds control capabilities.
        assert!(manager
            .grant_control(id, vec![Capability::ControlPointer], &tx)
            .is_some());
        assert!(manager
            .permissions(id)
            .unwrap()
            .allows(id, viewer, Capability::ControlPointer));
        assert!(!manager
            .permissions(id)
            .unwrap()
            .allows(id, viewer, Capability::ControlKeyboard));
    }

    /// A ControlRequest from the peer is surfaced as an event; it never
    /// changes permissions by itself (the host UI must grant explicitly).
    #[test]
    fn control_request_requires_host_grant() {
        let host = iroh::SecretKey::generate().public();
        let viewer = iroh::SecretKey::generate().public();
        let mut manager = SessionManager::default();
        let id = ScreenShareSessionId::generate();
        manager.start_invitation(id, host, viewer, 7);
        let (tx, mut rx) = tokio::sync::mpsc::channel(8);
        manager.apply_remote(
            viewer,
            ControlMessage::Accept {
                version: SCREEN_SHARE_PROTOCOL_VERSION,
                session_id: id,
            },
            &tx,
        );
        // Drain the Accepted emitted by the Accept before the RequestControl.
        assert!(
            matches!(rx.try_recv(), Ok(SessionEvent::Accepted { session_id, .. }) if session_id == id)
        );
        assert!(manager
            .apply_remote(
                viewer,
                ControlMessage::RequestControl {
                    version: SCREEN_SHARE_PROTOCOL_VERSION,
                    session_id: id,
                    capabilities: vec![Capability::ControlPointer, Capability::ControlKeyboard],
                },
                &tx,
            )
            .is_none());
        // The event is emitted for the host UI, but permission state is
        // unchanged — still view-only.
        assert!(
            matches!(rx.try_recv(), Ok(SessionEvent::ControlRequest { session_id, .. }) if session_id == id)
        );
        assert!(!manager
            .permissions(id)
            .unwrap()
            .allows(id, viewer, Capability::ControlPointer));
        assert!(!manager
            .permissions(id)
            .unwrap()
            .allows(id, viewer, Capability::ControlKeyboard));
    }

    /// PDF Task 9.1 hardening: a viewer must not be able to grant ITSELF
    /// control by sending a forged GrantControl to the host. Only the HOST
    /// (record.host_id) may grant; on the host side a GrantControl from the
    /// viewer is ignored, so the session stays view-only.
    #[test]
    fn forged_grant_control_from_viewer_is_ignored_on_host() {
        let host = iroh::SecretKey::generate().public();
        let viewer = iroh::SecretKey::generate().public();
        let mut manager = SessionManager::default();
        let id = ScreenShareSessionId::generate();
        manager.start_invitation(id, host, viewer, 7);
        let (tx, mut rx) = tokio::sync::mpsc::channel(8);
        manager.apply_remote(
            viewer,
            ControlMessage::Accept {
                version: SCREEN_SHARE_PROTOCOL_VERSION,
                session_id: id,
            },
            &tx,
        );
        assert!(
            matches!(rx.try_recv(), Ok(SessionEvent::Accepted { session_id, .. }) if session_id == id)
        );
        // The viewer tries to grant itself control with a self-chosen nonce.
        manager.apply_remote(
            viewer,
            ControlMessage::GrantControl {
                version: SCREEN_SHARE_PROTOCOL_VERSION,
                session_id: id,
                capabilities: vec![Capability::ControlPointer],
                nonce: [0x42; 16],
            },
            &tx,
        );
        // The forged grant must NOT change permission state.
        let permissions = manager.permissions(id).unwrap();
        assert!(!permissions.allows(id, viewer, Capability::ControlPointer));
        assert!(permissions.token().is_none());
        assert!(!permissions.nonce_matches([0x42; 16], std::time::Instant::now()));
        // No ControlChanged(active:true) event was emitted for the forgery.
        assert!(matches!(rx.try_recv(), Err(_)));
        // And a forged RevokeControl from the viewer is equally ignored.
        manager.grant_control(id, vec![Capability::ControlPointer], &tx);
        assert!(manager.permissions(id).unwrap().has_control());
        manager.apply_remote(
            viewer,
            ControlMessage::RevokeControl {
                version: SCREEN_SHARE_PROTOCOL_VERSION,
                session_id: id,
            },
            &tx,
        );
        assert!(
            manager.permissions(id).unwrap().has_control(),
            "viewer cannot revoke the host's grant"
        );
    }

    /// The reverse direction: on the VIEWER side, a GrantControl arriving from
    /// the HOST is applied — the viewer stores the host's nonce and may now
    /// inject input (echoing the nonce back in every Input message).
    #[test]
    fn host_grant_control_is_applied_on_viewer() {
        let host = iroh::SecretKey::generate().public();
        let mut manager = SessionManager::default();
        let id = ScreenShareSessionId::generate();
        let hello = crate::screen_share::protocol::Hello {
            version: SCREEN_SHARE_PROTOCOL_VERSION,
            session_id: id,
            host_id: host,
            conversation_id: 7,
            codecs: vec!["h264".into()],
            width: 640,
            height: 360,
            frame_rate: 15,
            permission: Permission::ViewOnly,
        };
        let (tx, mut rx) = tokio::sync::mpsc::channel(8);
        manager.apply_remote(host, ControlMessage::Hello(hello), &tx);
        manager.apply_remote(
            host,
            ControlMessage::Accept {
                version: SCREEN_SHARE_PROTOCOL_VERSION,
                session_id: id,
            },
            &tx,
        );
        // Drain the Invitation + Accepted emitted by the Hello/Accept before
        // the GrantControl.
        let _ = rx.try_recv();
        let _ = rx.try_recv();
        let nonce = [0x77; 16];
        manager.apply_remote(
            host,
            ControlMessage::GrantControl {
                version: SCREEN_SHARE_PROTOCOL_VERSION,
                session_id: id,
                capabilities: vec![Capability::ControlPointer, Capability::ControlKeyboard],
                nonce,
            },
            &tx,
        );
        let permissions = manager.permissions(id).unwrap();
        assert!(permissions.allows(id, host, Capability::ControlPointer));
        assert!(permissions.allows(id, host, Capability::ControlKeyboard));
        assert!(permissions.nonce_matches(nonce, std::time::Instant::now()));
        // ControlChanged(active:true) surfaced so the viewer UI shows the
        // persistent indicator.
        assert!(matches!(
            rx.try_recv(),
            Ok(SessionEvent::ControlChanged { active: true, .. })
        ));
    }

    /// A RequestControl from the HOST is a viewer → host message; on the
    /// viewer side it must not surface a consent prompt (which would confuse
    /// the UI and could be abused to spam prompts).
    #[test]
    fn request_control_from_host_is_ignored_on_viewer() {
        let host = iroh::SecretKey::generate().public();
        let mut manager = SessionManager::default();
        let id = ScreenShareSessionId::generate();
        let hello = crate::screen_share::protocol::Hello {
            version: SCREEN_SHARE_PROTOCOL_VERSION,
            session_id: id,
            host_id: host,
            conversation_id: 7,
            codecs: vec!["h264".into()],
            width: 640,
            height: 360,
            frame_rate: 15,
            permission: Permission::ViewOnly,
        };
        let (tx, mut rx) = tokio::sync::mpsc::channel(8);
        manager.apply_remote(host, ControlMessage::Hello(hello), &tx);
        manager.apply_remote(
            host,
            ControlMessage::Accept {
                version: SCREEN_SHARE_PROTOCOL_VERSION,
                session_id: id,
            },
            &tx,
        );
        // Drain the invitation + accepted events before the RequestControl.
        let _ = rx.try_recv();
        let _ = rx.try_recv();
        manager.apply_remote(
            host,
            ControlMessage::RequestControl {
                version: SCREEN_SHARE_PROTOCOL_VERSION,
                session_id: id,
                capabilities: vec![Capability::ControlPointer],
            },
            &tx,
        );
        assert!(
            matches!(rx.try_recv(), Err(_)),
            "no ControlRequest prompt on the viewer side"
        );
        assert!(!manager.permissions(id).unwrap().has_control());
    }

    /// PDF Task 9.1 stop condition: when the session ends (peer EndSession),
    /// the permission record is ended immediately so any late input/view
    /// attempt fails authorization.
    #[test]
    fn end_session_ends_permissions() {
        let host = iroh::SecretKey::generate().public();
        let viewer = iroh::SecretKey::generate().public();
        let mut manager = SessionManager::default();
        let id = ScreenShareSessionId::generate();
        manager.start_invitation(id, host, viewer, 7);
        let (tx, mut rx) = tokio::sync::mpsc::channel(8);
        manager.apply_remote(
            viewer,
            ControlMessage::Accept {
                version: SCREEN_SHARE_PROTOCOL_VERSION,
                session_id: id,
            },
            &tx,
        );
        assert!(manager
            .grant_control(id, vec![Capability::ControlPointer], &tx)
            .is_some());
        let _ = rx.try_recv(); // Accepted
        let _ = rx.try_recv(); // ControlChanged(active:true)
        let token = manager.permissions(id).unwrap().token().unwrap();
        manager.apply_remote(
            viewer,
            ControlMessage::EndSession {
                version: SCREEN_SHARE_PROTOCOL_VERSION,
                session_id: id,
            },
            &tx,
        );
        let permissions = manager.permissions(id).unwrap();
        assert!(!permissions.is_active());
        assert!(!permissions.allows(id, viewer, Capability::ViewScreen));
        assert!(!permissions.allows_token(
            id,
            viewer,
            token,
            Capability::ControlPointer,
            std::time::Instant::now()
        ));
        // A late input with the (now-ended) token is rejected by the same
        // authorization gate the host loop uses.
        assert!(!permissions.nonce_matches(*token.nonce(), std::time::Instant::now()));
        assert!(
            matches!(rx.try_recv(), Ok(SessionEvent::Ended { session_id }) if session_id == id)
        );
    }

    /// A stranger's Accept must never transition the session.
    #[test]
    fn remote_accept_from_wrong_peer_is_ignored() {
        let host = iroh::SecretKey::generate().public();
        let viewer = iroh::SecretKey::generate().public();
        let stranger = iroh::SecretKey::generate().public();
        let mut manager = SessionManager::default();
        let id = ScreenShareSessionId::generate();
        manager.start_invitation(id, host, viewer, 7);
        let (tx, _rx) = tokio::sync::mpsc::channel(8);
        manager.apply_remote(
            stranger,
            ControlMessage::Accept {
                version: SCREEN_SHARE_PROTOCOL_VERSION,
                session_id: id,
            },
            &tx,
        );
        assert_eq!(manager.state(id), Some(SessionState::AwaitingAcceptance));
    }

    // ------------------------------------------------------------------
    // Versioned negotiation (PDF Task 3.1) tests.
    // ------------------------------------------------------------------

    fn test_offer(host: iroh::PublicKey) -> ScreenShareMessage {
        ScreenShareMessage::ScreenShareOffer {
            version: SCREEN_SHARE_PROTOCOL_VERSION,
            session_id: ScreenShareSessionId::from_bytes([9; 16]),
            host_id: host,
            conversation_id: 7,
            codecs: vec!["h264".into(), "vp8".into()],
            resolutions: vec![(1920, 1080), (1280, 720)],
            frame_rate_min: 15,
            frame_rate_max: 30,
            target_bitrate_bps: 2_000_000,
            remote_control: false,
        }
    }

    fn test_offer_with_id(host: iroh::PublicKey, id: ScreenShareSessionId) -> ScreenShareMessage {
        let mut offer = test_offer(host);
        let ScreenShareMessage::ScreenShareOffer { session_id, .. } = &mut offer else {
            panic!("offer")
        };
        *session_id = id;
        offer
    }

    fn channel() -> (
        tokio::sync::mpsc::Sender<SessionEvent>,
        tokio::sync::mpsc::Receiver<SessionEvent>,
    ) {
        tokio::sync::mpsc::channel(8)
    }

    /// Extract the codec list from an offer message (test helper).
    fn offer_codecs(offer: &ScreenShareMessage) -> Vec<String> {
        let ScreenShareMessage::ScreenShareOffer { codecs, .. } = offer else {
            panic!("offer")
        };
        codecs.clone()
    }

    /// The full offer → accept → accepted round trip on one manager pair:
    /// initiator records the offer, the recipient accepts with a mutually
    /// supported configuration, and capture is only permitted after the
    /// explicit accept on both sides.
    #[test]
    fn negotiation_offer_accept_roundtrip() {
        let host = iroh::SecretKey::generate().public();
        let viewer = iroh::SecretKey::generate().public();
        let mut initiator = NegotiationManager::new();
        let mut recipient = NegotiationManager::new();
        let (tx, mut rx) = channel();
        let id = ScreenShareSessionId::from_bytes([9; 16]);

        initiator
            .start_offer(test_offer(host), viewer, Duration::from_secs(30))
            .unwrap();
        assert_eq!(initiator.state(id), Some(NegotiationState::Pending));
        assert!(
            !initiator.can_start_capture(id),
            "no capture before acceptance"
        );

        recipient
            .receive_offer(host, test_offer(host), Duration::from_secs(30), &tx)
            .unwrap();
        assert_eq!(recipient.state(id), Some(NegotiationState::Pending));
        assert!(
            !recipient.can_start_capture(id),
            "no capture before acceptance"
        );
        match rx.try_recv().unwrap() {
            SessionEvent::NegotiationInvitation {
                session_id,
                host_id,
                offer,
                ..
            } => {
                assert_eq!(session_id, id);
                assert_eq!(host_id, host);
                assert_eq!(
                    offer_codecs(&offer),
                    vec!["h264".to_string(), "vp8".to_string()]
                );
            }
            other => panic!("expected NegotiationInvitation, got {other:?}"),
        }

        // Recipient selects a mutually supported configuration and accepts.
        let selected =
            NegotiatedConfig::select(recipient.offer(id).unwrap(), &["h264".to_string()]).unwrap();
        assert_eq!(selected.codec, "h264");
        assert_eq!((selected.width, selected.height), (1920, 1080));
        assert_eq!(selected.frame_rate, 30);
        let accept_message = recipient.accept(id, selected, &tx).unwrap();
        assert_eq!(recipient.state(id), Some(NegotiationState::Accepted));
        assert!(
            recipient.can_start_capture(id),
            "capture allowed after explicit accept"
        );
        // The recipient's own Accept emits Accepted naming the host.
        assert_eq!(
            rx.try_recv().unwrap(),
            SessionEvent::Accepted {
                session_id: id,
                peer_id: host
            }
        );

        // Initiator applies the remote accept (the manager takes the message).
        initiator
            .handle_accept(viewer, accept_message, &tx)
            .unwrap();
        assert_eq!(initiator.state(id), Some(NegotiationState::Accepted));
        assert!(
            initiator.can_start_capture(id),
            "capture allowed after remote accept"
        );
        assert_eq!(initiator.selected(id).unwrap().codec, "h264");
        assert_eq!(
            rx.try_recv().unwrap(),
            SessionEvent::Accepted {
                session_id: id,
                peer_id: viewer
            }
        );
    }

    /// A stranger cannot accept a negotiation it does not own.
    #[test]
    fn negotiation_accept_from_wrong_peer_is_rejected() {
        let host = iroh::SecretKey::generate().public();
        let viewer = iroh::SecretKey::generate().public();
        let stranger = iroh::SecretKey::generate().public();
        let mut initiator = NegotiationManager::new();
        let (tx, _rx) = channel();
        let id = ScreenShareSessionId::from_bytes([9; 16]);
        initiator
            .start_offer(test_offer(host), viewer, Duration::from_secs(30))
            .unwrap();
        let accept = ScreenShareMessage::ScreenShareAccept {
            version: SCREEN_SHARE_PROTOCOL_VERSION,
            session_id: id,
            codec: "h264".into(),
            width: 1920,
            height: 1080,
            frame_rate: 30,
        };
        assert_eq!(
            initiator.handle_accept(stranger, accept, &tx),
            Err(NegotiationError::PeerMismatch)
        );
        assert_eq!(initiator.state(id), Some(NegotiationState::Pending));
    }

    /// The recipient may only accept a configuration that is mutually
    /// supported (inside the offered capabilities).
    #[test]
    fn negotiation_accept_rejects_unsupported_config() {
        let host = iroh::SecretKey::generate().public();
        let mut recipient = NegotiationManager::new();
        let (tx, _rx) = channel();
        let id = ScreenShareSessionId::from_bytes([9; 16]);
        recipient
            .receive_offer(host, test_offer(host), Duration::from_secs(30), &tx)
            .unwrap();
        let bad = NegotiatedConfig {
            codec: "av1".into(),
            width: 1920,
            height: 1080,
            frame_rate: 30,
        };
        assert!(matches!(
            recipient.accept(id, bad, &tx),
            Err(NegotiationError::UnsupportedConfig(_))
        ));
        assert_eq!(recipient.state(id), Some(NegotiationState::Pending));
    }

    /// Explicit reject closes the negotiation on both sides and never permits
    /// capture.
    #[test]
    fn negotiation_reject_roundtrip() {
        let host = iroh::SecretKey::generate().public();
        let viewer = iroh::SecretKey::generate().public();
        let mut initiator = NegotiationManager::new();
        let mut recipient = NegotiationManager::new();
        let (tx, mut rx) = channel();
        let id = ScreenShareSessionId::from_bytes([9; 16]);
        initiator
            .start_offer(test_offer(host), viewer, Duration::from_secs(30))
            .unwrap();
        recipient
            .receive_offer(host, test_offer(host), Duration::from_secs(30), &tx)
            .unwrap();
        // Drain the invitation emitted by receive_offer before rejecting.
        assert!(
            matches!(rx.try_recv().unwrap(), SessionEvent::NegotiationInvitation { session_id, .. } if session_id == id)
        );
        let reject_message = recipient.reject(id, "user declined", &tx).unwrap();
        assert_eq!(recipient.state(id), Some(NegotiationState::Closed));
        assert!(!recipient.can_start_capture(id));
        initiator
            .handle_reject(viewer, reject_message, &tx)
            .unwrap();
        assert_eq!(initiator.state(id), Some(NegotiationState::Closed));
        assert!(!initiator.can_start_capture(id));
        assert_eq!(
            rx.try_recv().unwrap(),
            SessionEvent::Rejected {
                session_id: id,
                reason: "user declined".into()
            }
        );
    }

    /// Initiator cancel withdraws a pending offer before the recipient
    /// responds; the peer is informed with a Reject message.
    #[test]
    fn negotiation_cancel_withdraws_offer() {
        let host = iroh::SecretKey::generate().public();
        let viewer = iroh::SecretKey::generate().public();
        let mut initiator = NegotiationManager::new();
        let (tx, mut rx) = channel();
        let id = ScreenShareSessionId::from_bytes([9; 16]);
        initiator
            .start_offer(test_offer(host), viewer, Duration::from_secs(30))
            .unwrap();
        let message = initiator.cancel(id, "cancelled by initiator", &tx).unwrap();
        assert_eq!(initiator.state(id), Some(NegotiationState::Closed));
        assert!(!initiator.can_start_capture(id));
        assert!(
            matches!(message, ScreenShareMessage::ScreenShareReject { reason, .. } if reason == "cancelled by initiator")
        );
        assert_eq!(
            rx.try_recv().unwrap(),
            SessionEvent::Rejected {
                session_id: id,
                reason: "cancelled by initiator".into()
            }
        );
    }

    /// A duplicate offer for an already-pending session is an explicit error,
    /// matching the existing Hello convention.
    #[test]
    fn negotiation_duplicate_offer_is_rejected() {
        let host = iroh::SecretKey::generate().public();
        let mut recipient = NegotiationManager::new();
        let (tx, _rx) = channel();
        let id = ScreenShareSessionId::from_bytes([9; 16]);
        recipient
            .receive_offer(
                host,
                test_offer_with_id(host, id),
                Duration::from_secs(30),
                &tx,
            )
            .unwrap();
        let duplicate = test_offer_with_id(host, id);
        assert_eq!(
            recipient.receive_offer(host, duplicate, Duration::from_secs(30), &tx),
            Err(NegotiationError::DuplicateOffer)
        );
        assert_eq!(recipient.state(id), Some(NegotiationState::Pending));
    }

    /// Pending negotiations expire at their deadline and never permit capture.
    #[test]
    fn negotiation_timeout_expires_pending() {
        let host = iroh::SecretKey::generate().public();
        let viewer = iroh::SecretKey::generate().public();
        let mut initiator = NegotiationManager::new();
        let (tx, mut rx) = channel();
        let id = ScreenShareSessionId::from_bytes([9; 16]);
        let started = std::time::Instant::now();
        initiator
            .start_offer(test_offer(host), viewer, Duration::from_secs(30))
            .unwrap();
        let before_deadline = started + Duration::from_secs(29);
        assert!(
            initiator.expire_pending(before_deadline, &tx).is_empty(),
            "not yet expired"
        );
        assert_eq!(initiator.state(id), Some(NegotiationState::Pending));
        let after_deadline = started + Duration::from_secs(31);
        let expired = initiator.expire_pending(after_deadline, &tx);
        assert_eq!(expired, vec![id]);
        assert_eq!(initiator.state(id), Some(NegotiationState::Closed));
        assert!(!initiator.can_start_capture(id));
        assert_eq!(
            rx.try_recv().unwrap(),
            SessionEvent::Rejected {
                session_id: id,
                reason: "negotiation timed out".into()
            }
        );
    }

    /// Peer disconnect closes every negotiation involving that peer.
    #[test]
    fn negotiation_peer_disconnect_closes_sessions() {
        let host = iroh::SecretKey::generate().public();
        let viewer = iroh::SecretKey::generate().public();
        let mut initiator = NegotiationManager::new();
        let (tx, mut rx) = channel();
        let id = ScreenShareSessionId::from_bytes([9; 16]);
        let other_id = ScreenShareSessionId::from_bytes([10; 16]);
        initiator
            .start_offer(
                test_offer_with_id(host, id),
                viewer,
                Duration::from_secs(30),
            )
            .unwrap();
        initiator
            .start_offer(
                test_offer_with_id(host, other_id),
                viewer,
                Duration::from_secs(30),
            )
            .unwrap();
        let closed = initiator.peer_disconnected(viewer, &tx);
        assert_eq!(closed.len(), 2);
        assert_eq!(initiator.state(id), Some(NegotiationState::Closed));
        assert_eq!(initiator.state(other_id), Some(NegotiationState::Closed));
        assert!(!initiator.can_start_capture(id));
        // HashMap iteration order is unspecified: assert the set of Ended ids.
        let mut ended: Vec<ScreenShareSessionId> = (0..2)
            .map(|_| match rx.try_recv().unwrap() {
                SessionEvent::Ended { session_id } => session_id,
                other => panic!("expected Ended, got {other:?}"),
            })
            .collect();
        ended.sort_by_key(|sid| *sid.as_bytes());
        let mut expected = vec![id, other_id];
        expected.sort_by_key(|sid| *sid.as_bytes());
        assert_eq!(ended, expected);
    }

    /// An offer whose host_id does not match the connected peer is refused.
    #[test]
    fn negotiation_offer_identity_mismatch_is_rejected() {
        let host = iroh::SecretKey::generate().public();
        let stranger = iroh::SecretKey::generate().public();
        let mut recipient = NegotiationManager::new();
        let (tx, _rx) = channel();
        // The offer claims host but arrives from stranger.
        let result =
            recipient.receive_offer(stranger, test_offer(host), Duration::from_secs(30), &tx);
        assert_eq!(result, Err(NegotiationError::PeerMismatch));
        assert!(recipient.is_empty());
    }

    /// Closing a negotiation frees the capacity slot and a later offer with a
    /// fresh session id is accepted.
    #[test]
    fn negotiation_capacity_is_bounded_and_reusable() {
        let host = iroh::SecretKey::generate().public();
        let viewer = iroh::SecretKey::generate().public();
        let mut initiator = NegotiationManager::new();
        let (tx, _rx) = channel();
        for i in 0..MAX_ACTIVE_NEGOTIATIONS {
            // Ids start at 1: the all-zero id is the empty-session sentinel.
            let offer =
                test_offer_with_id(host, ScreenShareSessionId::from_bytes([(i as u8) + 1; 16]));
            initiator
                .start_offer(offer, viewer, Duration::from_secs(30))
                .unwrap();
        }
        let overflow = test_offer_with_id(host, ScreenShareSessionId::from_bytes([0xEE; 16]));
        assert_eq!(
            initiator.start_offer(overflow.clone(), viewer, Duration::from_secs(30)),
            Err(NegotiationError::Capacity)
        );
        // Cancel one and retry: the slot is reusable.
        initiator
            .cancel(ScreenShareSessionId::from_bytes([1; 16]), "cancel", &tx)
            .unwrap();
        assert!(initiator
            .start_offer(overflow, viewer, Duration::from_secs(30))
            .is_ok());
    }

    /// The empty-session sentinel is refused by the manager before any state
    /// is created.
    #[test]
    fn negotiation_empty_session_id_is_rejected() {
        let host = iroh::SecretKey::generate().public();
        let viewer = iroh::SecretKey::generate().public();
        let mut initiator = NegotiationManager::new();
        let mut offer = test_offer(host);
        let ScreenShareMessage::ScreenShareOffer { session_id, .. } = &mut offer else {
            panic!("offer")
        };
        *session_id = ScreenShareSessionId::zero();
        assert_eq!(
            initiator.start_offer(offer, viewer, Duration::from_secs(30)),
            Err(NegotiationError::EmptySessionId)
        );
    }
}
