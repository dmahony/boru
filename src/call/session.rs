//! Shared real-time media-session state.
//!
//! Voice and screen sharing use different media codecs and transports, but
//! they belong to the same user-visible peer session.  This small state
//! machine is the common lifecycle boundary: tracks can start and stop
//! independently, while reconnecting the session keeps the other track alive.

use std::collections::BTreeMap;

use super::CallId;
use iroh::PublicKey;

/// A media track owned by a real-time session.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum MediaTrack {
    /// Bidirectional voice audio.
    Voice,
    /// Native screen-share video.
    Screen,
}

/// Lifecycle of one independently controllable track.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrackState {
    /// The track is not active.
    Stopped,
    /// Negotiation or capture is starting.
    Starting,
    /// The track is sending/receiving media.
    Active,
    /// The track is temporarily reconnecting.
    Reconnecting,
}

/// A projection used by presence cards and direct-chat controls.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MediaPresence {
    /// Whether the shared session exists for this peer.
    pub session_active: bool,
    /// Current voice track state.
    pub voice: TrackState,
    /// Current screen track state.
    pub screen: TrackState,
}

/// Shared lifecycle state for voice and screen tracks.
#[derive(Debug, Clone)]
pub struct RealtimeMediaSession {
    id: Option<CallId>,
    peer: Option<PublicKey>,
    tracks: BTreeMap<MediaTrack, TrackState>,
}

impl Default for RealtimeMediaSession {
    fn default() -> Self {
        Self::new()
    }
}

impl RealtimeMediaSession {
    /// Create an empty session.
    pub fn new() -> Self {
        Self {
            id: None,
            peer: None,
            tracks: BTreeMap::from([
                (MediaTrack::Voice, TrackState::Stopped),
                (MediaTrack::Screen, TrackState::Stopped),
            ]),
        }
    }

    /// Attach a peer/session identity without starting either track.
    pub fn begin(&mut self, id: CallId, peer: PublicKey) {
        self.id = Some(id);
        self.peer = Some(peer);
    }

    /// Return the session identity, if a track/session is present.
    pub fn id(&self) -> Option<CallId> {
        self.id
    }

    /// Return the peer associated with this session.
    pub fn peer(&self) -> Option<PublicKey> {
        self.peer
    }

    /// Set one track's state. This deliberately does not affect its sibling.
    pub fn set_track(&mut self, track: MediaTrack, state: TrackState) {
        self.tracks.insert(track, state);
    }

    /// Read one track's state.
    pub fn track(&self, track: MediaTrack) -> TrackState {
        self.tracks
            .get(&track)
            .copied()
            .unwrap_or(TrackState::Stopped)
    }

    /// Move only one active track into reconnecting.
    pub fn reconnect_track(&mut self, track: MediaTrack) {
        if self.track(track) == TrackState::Active {
            self.set_track(track, TrackState::Reconnecting);
        }
    }

    /// Stop one track while preserving the other track and session identity.
    pub fn stop_track(&mut self, track: MediaTrack) {
        self.set_track(track, TrackState::Stopped);
        if self
            .tracks
            .values()
            .all(|state| *state == TrackState::Stopped)
        {
            self.id = None;
            self.peer = None;
        }
    }

    /// Clear the whole session and both tracks.
    pub fn stop_all(&mut self) {
        self.tracks.insert(MediaTrack::Voice, TrackState::Stopped);
        self.tracks.insert(MediaTrack::Screen, TrackState::Stopped);
        self.id = None;
        self.peer = None;
    }

    /// Build the stable presence projection for UI cards.
    pub fn presence(&self) -> MediaPresence {
        MediaPresence {
            session_active: self.id.is_some(),
            voice: self.track(MediaTrack::Voice),
            screen: self.track(MediaTrack::Screen),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use iroh::SecretKey;

    #[test]
    fn tracks_start_and_stop_independently() {
        let key = SecretKey::generate().public();
        let mut session = RealtimeMediaSession::new();
        session.begin(CallId::new(), key);
        session.set_track(MediaTrack::Voice, TrackState::Active);
        session.set_track(MediaTrack::Screen, TrackState::Active);
        session.stop_track(MediaTrack::Screen);
        assert_eq!(session.track(MediaTrack::Voice), TrackState::Active);
        assert_eq!(session.track(MediaTrack::Screen), TrackState::Stopped);
        assert!(session.presence().session_active);
    }

    #[test]
    fn reconnecting_screen_does_not_reconnect_voice() {
        let key = SecretKey::generate().public();
        let mut session = RealtimeMediaSession::new();
        session.begin(CallId::new(), key);
        session.set_track(MediaTrack::Voice, TrackState::Active);
        session.set_track(MediaTrack::Screen, TrackState::Active);
        session.reconnect_track(MediaTrack::Screen);
        assert_eq!(session.track(MediaTrack::Voice), TrackState::Active);
        assert_eq!(session.track(MediaTrack::Screen), TrackState::Reconnecting);
    }

    #[test]
    fn stopping_last_track_clears_presence() {
        let key = SecretKey::generate().public();
        let mut session = RealtimeMediaSession::new();
        session.begin(CallId::new(), key);
        session.set_track(MediaTrack::Screen, TrackState::Active);
        session.stop_track(MediaTrack::Screen);
        assert!(!session.presence().session_active);
        assert_eq!(session.peer(), None);
    }

    #[test]
    fn stopping_screen_keeps_voice_active() {
        let key = SecretKey::generate().public();
        let mut session = RealtimeMediaSession::new();
        session.begin(CallId::new(), key);
        session.set_track(MediaTrack::Voice, TrackState::Active);
        session.set_track(MediaTrack::Screen, TrackState::Active);
        session.stop_track(MediaTrack::Screen);
        assert_eq!(session.presence().voice, TrackState::Active);
        assert_eq!(session.presence().screen, TrackState::Stopped);
        assert!(session.presence().session_active);
    }

    #[test]
    fn reconnect_failure_can_cleanly_end_only_screen_track() {
        let key = SecretKey::generate().public();
        let mut session = RealtimeMediaSession::new();
        session.begin(CallId::new(), key);
        session.set_track(MediaTrack::Voice, TrackState::Active);
        session.set_track(MediaTrack::Screen, TrackState::Active);
        session.reconnect_track(MediaTrack::Screen);
        session.stop_track(MediaTrack::Screen);
        assert_eq!(session.track(MediaTrack::Voice), TrackState::Active);
        assert_eq!(session.track(MediaTrack::Screen), TrackState::Stopped);
        assert!(session.presence().session_active);
    }
}

// Authenticated, idempotent call-session lifecycle state.
//
// This module contains no media payloads and no gossip integration. The call
// actor uses it as the single decision boundary for reliable signalling on an
// authenticated Iroh connection.

use std::time::{Duration, Instant};

use super::CallKind;

/// Lifecycle of one authenticated call session.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionState {
    /// No offer has been exchanged yet.
    Idle,
    /// An offer is awaiting the peer's response.
    Offered,
    /// An incoming offer is awaiting local user consent.
    Ringing,
    /// Both sides accepted and media may be started by the actor.
    Active,
    /// The media path is being re-established without creating a new call id.
    Reconnecting,
    /// The session was declined.
    Declined,
    /// The session ended, including timeout and transport teardown.
    Ended,
}

impl SessionState {
    /// Whether no further signal can change this session.
    pub const fn is_terminal(self) -> bool {
        matches!(self, Self::Declined | Self::Ended)
    }
}

/// Reliable lifecycle signal carried by the authenticated call-control stream.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionSignal {
    /// Start or re-offer a call.
    Offer {
        /// Call identity.
        call_id: CallId,
        /// Requested media kind.
        kind: CallKind,
    },
    /// Explicitly accept an offer or finish a reconnect.
    Accept {
        /// Call identity.
        call_id: CallId,
    },
    /// Explicitly reject an offer.
    Decline {
        /// Call identity.
        call_id: CallId,
    },
    /// End an established or pending call.
    Leave {
        /// Call identity.
        call_id: CallId,
    },
    /// Re-establish the media path for the existing call.
    Reconnect {
        /// Call identity.
        call_id: CallId,
        /// Monotonic reconnect generation.
        generation: u64,
    },
}

impl SessionSignal {
    fn call_id(self) -> CallId {
        match self {
            Self::Offer { call_id, .. }
            | Self::Accept { call_id }
            | Self::Decline { call_id }
            | Self::Leave { call_id }
            | Self::Reconnect { call_id, .. } => call_id,
        }
    }
}

/// Why a signal was not applied.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionError {
    /// The signal came from a different authenticated peer.
    UnauthorizedPeer,
    /// The signal belongs to another call.
    WrongCall,
    /// The signal cannot be applied in the current state.
    WrongState,
    /// The session deadline has elapsed.
    TimedOut,
}

/// Result of applying a signal. Duplicate terminal/idempotent signals report
/// `changed == false` rather than producing an error.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SessionUpdate {
    /// Whether the state changed as a result of the signal.
    pub changed: bool,
    /// State after processing the signal.
    pub state: SessionState,
}

/// Local-only counters for troubleshooting signalling without logging payloads.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct SessionDiagnostics {
    /// Number of accepted transitions.
    pub accepted: u64,
    /// Number of declined transitions.
    pub declined: u64,
    /// Number of leave transitions.
    pub leaves: u64,
    /// Number of reconnect transitions.
    pub reconnects: u64,
    /// Signals rejected because their authenticated peer was unexpected.
    pub unauthorized: u64,
    /// Number of deadline expirations.
    pub timeouts: u64,
}

/// One authenticated call session.
#[derive(Debug, Clone)]
pub struct CallSession {
    call_id: CallId,
    kind: CallKind,
    local_peer: PublicKey,
    remote_peer: PublicKey,
    state: SessionState,
    deadline: Option<Instant>,
    generation: u64,
    diagnostics: SessionDiagnostics,
}

impl CallSession {
    /// Create a session pinned to the authenticated Iroh peer identity.
    pub fn new(
        call_id: CallId,
        kind: CallKind,
        local_peer: PublicKey,
        remote_peer: PublicKey,
    ) -> Self {
        Self {
            call_id,
            kind,
            local_peer,
            remote_peer,
            state: SessionState::Idle,
            deadline: None,
            generation: 0,
            diagnostics: SessionDiagnostics::default(),
        }
    }

    /// Call identity.
    pub const fn call_id(&self) -> CallId {
        self.call_id
    }
    /// Requested media kind.
    pub const fn kind(&self) -> CallKind {
        self.kind
    }
    /// Local authenticated identity.
    pub const fn local_peer(&self) -> PublicKey {
        self.local_peer
    }
    /// Expected remote authenticated identity.
    pub const fn remote_peer(&self) -> PublicKey {
        self.remote_peer
    }
    /// Current lifecycle state.
    pub const fn state(&self) -> SessionState {
        self.state
    }
    /// Current reconnect generation.
    pub const fn generation(&self) -> u64 {
        self.generation
    }
    /// Local diagnostics snapshot.
    pub const fn diagnostics(&self) -> SessionDiagnostics {
        self.diagnostics
    }

    /// Record a locally initiated offer.
    pub fn start_offer(&mut self, timeout: Duration) -> Result<SessionUpdate, SessionError> {
        if self.state != SessionState::Idle {
            return Err(SessionError::WrongState);
        }
        self.deadline = Some(Instant::now() + timeout);
        self.state = SessionState::Offered;
        Ok(self.update(true))
    }

    /// Record an incoming offer from the authenticated connection peer.
    pub fn receive_offer(
        &mut self,
        peer: PublicKey,
        kind: CallKind,
        timeout: Duration,
    ) -> Result<SessionUpdate, SessionError> {
        self.authorize(peer)?;
        if self.state != SessionState::Idle {
            return Err(SessionError::WrongState);
        }
        self.kind = kind;
        self.deadline = Some(Instant::now() + timeout);
        self.state = SessionState::Ringing;
        Ok(self.update(true))
    }

    /// Apply a lifecycle signal after verifying its authenticated sender.
    pub fn apply(
        &mut self,
        peer: PublicKey,
        signal: SessionSignal,
    ) -> Result<SessionUpdate, SessionError> {
        self.authorize(peer)?;
        if signal.call_id() != self.call_id {
            return Err(SessionError::WrongCall);
        }
        match signal {
            SessionSignal::Offer { kind, .. } => {
                self.receive_offer(peer, kind, Duration::from_secs(5))
            }
            SessionSignal::Accept { .. } => match self.state {
                SessionState::Offered | SessionState::Ringing | SessionState::Reconnecting => {
                    self.deadline = None;
                    self.state = SessionState::Active;
                    self.diagnostics.accepted = self.diagnostics.accepted.saturating_add(1);
                    Ok(self.update(true))
                }
                SessionState::Active => Ok(self.update(false)),
                _ => Err(SessionError::WrongState),
            },
            SessionSignal::Decline { .. } => match self.state {
                SessionState::Offered | SessionState::Ringing => {
                    self.deadline = None;
                    self.state = SessionState::Declined;
                    self.diagnostics.declined = self.diagnostics.declined.saturating_add(1);
                    Ok(self.update(true))
                }
                SessionState::Declined => Ok(self.update(false)),
                _ => Err(SessionError::WrongState),
            },
            SessionSignal::Leave { .. } => {
                if self.state.is_terminal() {
                    return Ok(self.update(false));
                }
                self.deadline = None;
                self.state = SessionState::Ended;
                self.diagnostics.leaves = self.diagnostics.leaves.saturating_add(1);
                Ok(self.update(true))
            }
            SessionSignal::Reconnect { generation, .. } => match self.state {
                SessionState::Active if generation > self.generation => {
                    self.generation = generation;
                    self.deadline = Some(Instant::now() + Duration::from_secs(5));
                    self.state = SessionState::Reconnecting;
                    self.diagnostics.reconnects = self.diagnostics.reconnects.saturating_add(1);
                    Ok(self.update(true))
                }
                SessionState::Reconnecting if generation == self.generation => {
                    Ok(self.update(false))
                }
                _ => Err(SessionError::WrongState),
            },
        }
    }

    /// End an overdue pending or reconnecting session at `now`.
    pub fn expire(&mut self, now: Instant) -> Option<SessionUpdate> {
        if self.deadline.is_some_and(|deadline| deadline <= now)
            && matches!(
                self.state,
                SessionState::Offered | SessionState::Ringing | SessionState::Reconnecting
            )
        {
            self.deadline = None;
            self.state = SessionState::Ended;
            self.diagnostics.timeouts = self.diagnostics.timeouts.saturating_add(1);
            Some(self.update(true))
        } else {
            None
        }
    }

    fn authorize(&mut self, peer: PublicKey) -> Result<(), SessionError> {
        if peer != self.remote_peer {
            self.diagnostics.unauthorized = self.diagnostics.unauthorized.saturating_add(1);
            Err(SessionError::UnauthorizedPeer)
        } else {
            Ok(())
        }
    }

    const fn update(&self, changed: bool) -> SessionUpdate {
        SessionUpdate {
            changed,
            state: self.state,
        }
    }
}

#[cfg(test)]
mod signalling_tests {
    use super::*;

    fn peers() -> (PublicKey, PublicKey, PublicKey) {
        fn key() -> PublicKey {
            iroh::SecretKey::generate().public()
        }
        (key(), key(), key())
    }

    fn make_session() -> (CallSession, PublicKey, PublicKey, PublicKey) {
        let (local, remote, stranger) = peers();
        (
            CallSession::new(CallId::generate(), CallKind::Voice, local, remote),
            local,
            remote,
            stranger,
        )
    }

    #[test]
    fn duplicate_and_reordered_signals_are_safe() {
        let (mut session, _, remote, _) = make_session();
        assert_eq!(
            session.apply(
                remote,
                SessionSignal::Accept {
                    call_id: session.call_id()
                }
            ),
            Err(SessionError::WrongState)
        );
        session.start_offer(Duration::from_secs(5)).unwrap();
        assert_eq!(
            session
                .apply(
                    remote,
                    SessionSignal::Accept {
                        call_id: session.call_id()
                    }
                )
                .unwrap()
                .state,
            SessionState::Active
        );
        assert!(
            !session
                .apply(
                    remote,
                    SessionSignal::Accept {
                        call_id: session.call_id()
                    }
                )
                .unwrap()
                .changed
        );
    }

    #[test]
    fn rejection_and_leave_are_idempotent() {
        let (mut session, _, remote, _) = make_session();
        session.start_offer(Duration::from_secs(5)).unwrap();
        session
            .apply(
                remote,
                SessionSignal::Decline {
                    call_id: session.call_id(),
                },
            )
            .unwrap();
        assert!(
            !session
                .apply(
                    remote,
                    SessionSignal::Decline {
                        call_id: session.call_id()
                    }
                )
                .unwrap()
                .changed
        );
        assert!(
            !session
                .apply(
                    remote,
                    SessionSignal::Leave {
                        call_id: session.call_id()
                    }
                )
                .unwrap()
                .changed
        );
    }

    #[test]
    fn timeout_reconnect_and_unauthorized_peer_are_tracked() {
        let (mut session, _, _remote, stranger) = make_session();
        assert_eq!(
            session.apply(
                stranger,
                SessionSignal::Leave {
                    call_id: session.call_id()
                }
            ),
            Err(SessionError::UnauthorizedPeer)
        );
        session.start_offer(Duration::ZERO).unwrap();
        assert_eq!(
            session.expire(Instant::now()).unwrap().state,
            SessionState::Ended
        );
        assert_eq!(session.diagnostics().unauthorized, 1);
        assert_eq!(session.diagnostics().timeouts, 1);

        let (mut session, _, remote, _) = make_session();
        session.start_offer(Duration::from_secs(5)).unwrap();
        session
            .apply(
                remote,
                SessionSignal::Accept {
                    call_id: session.call_id(),
                },
            )
            .unwrap();
        session
            .apply(
                remote,
                SessionSignal::Reconnect {
                    call_id: session.call_id(),
                    generation: 1,
                },
            )
            .unwrap();
        assert_eq!(
            session
                .apply(
                    remote,
                    SessionSignal::Accept {
                        call_id: session.call_id()
                    }
                )
                .unwrap()
                .state,
            SessionState::Active
        );
    }
}
