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
    fn wrong_call_is_rejected_without_mutating_lifecycle() {
        let (mut session, _, remote, _) = make_session();
        let wrong_call = CallId::generate();
        assert_eq!(
            session.apply(remote, SessionSignal::Leave { call_id: wrong_call }),
            Err(SessionError::WrongCall)
        );
        assert_eq!(session.state(), SessionState::Idle);
        assert_eq!(session.diagnostics().leaves, 0);
    }
}
