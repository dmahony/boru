//! Session-scoped permission policy for screen capture and remote input.
//!
//! This module implements the PDF Task 9.1 remote-control permission model:
//!
//! - Every share defaults to **view-only** ([`SessionPermissions::view_only`]).
//! - Remote control is a **separate, explicit grant** — it is never implied by
//!   accepting a share, and every grant issues a fresh session nonce that the
//!   viewer must echo back in every input message.
//! - Control can be **revoked with one action** ([`revoke_control`]) or dropped
//!   by a security-significant reconnect ([`reset_for_reconnect`]), returning
//!   the session to view-only.
//! - Input stops immediately when sharing ends ([`end`] makes the record
//!   inactive and clears every capability/token), when the peer disconnects
//!   (reconnect resets to view-only), or when consent is revoked.
//!
//! [`revoke_control`]: SessionPermissions::revoke_control
//! [`reset_for_reconnect`]: SessionPermissions::reset_for_reconnect
//! [`end`]: SessionPermissions::end
#![allow(missing_docs)]

use super::session::ScreenShareSessionId;
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, VecDeque};
use std::time::{Duration, Instant};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Capability { ViewScreen, ControlPointer, ControlKeyboard, Clipboard }
pub const MAX_CAPABILITIES: usize = 4;
pub const REQUEST_WINDOW: Duration = Duration::from_secs(10);
pub const MAX_REQUESTS_PER_WINDOW: u32 = 4;
pub const CONTROL_GRANT_TTL: Duration = Duration::from_secs(15 * 60);

/// Input rate-limit window (PDF Task 9.2): pathological input streams are
/// bounded per second. The viewer throttles pointer moves to ~30/s and key
/// repeats to the OS repeat rate, so a sustained stream above this budget is
/// by definition a flood (buggy or malicious) and is dropped by the host.
pub const INPUT_RATE_WINDOW: Duration = Duration::from_secs(1);
/// Maximum input events accepted per [`INPUT_RATE_WINDOW`] per session.
pub const MAX_INPUT_EVENTS_PER_WINDOW: u32 = 200;

impl Capability {
    /// True for the remote-control capabilities that require explicit consent
    /// and ride the grant nonce. `ViewScreen` is the view-only baseline and is
    /// never granted through the control path; `Clipboard` is a separate
    /// optional capability (PDF Task 9.3, BORU-SS-25).
    pub fn is_control(self) -> bool {
        matches!(self, Capability::ControlPointer | Capability::ControlKeyboard)
    }
}

#[derive(Debug, Default)]
pub struct RequestRateLimiter { requests: HashMap<iroh::PublicKey, (Instant, u32)> }
impl RequestRateLimiter {
    pub fn allow(&mut self, peer_id: iroh::PublicKey, now: Instant) -> bool {
        let entry = self.requests.entry(peer_id).or_insert((now, 0));
        if now.duration_since(entry.0) >= REQUEST_WINDOW { *entry = (now, 0); }
        if entry.1 >= MAX_REQUESTS_PER_WINDOW { return false; }
        entry.1 += 1;
        true
    }
}

/// Sliding-window rate limiter for remote-control input streams (PDF Task 9.2).
///
/// The host drops input messages that exceed [`MAX_INPUT_EVENTS_PER_WINDOW`]
/// within [`INPUT_RATE_WINDOW`], so a pathological viewer (buggy or malicious)
/// cannot flood the platform injection backend. The window slides: an event
/// older than the window is forgotten the next time [`allow`] is called, so a
/// burst is bounded but a sustained low-rate stream passes.
///
/// [`allow`]: SlidingWindowRateLimiter::allow
#[derive(Debug)]
pub struct SlidingWindowRateLimiter {
    window: Duration,
    max_events: u32,
    events: VecDeque<Instant>,
}
impl Default for SlidingWindowRateLimiter {
    fn default() -> Self {
        Self::new(INPUT_RATE_WINDOW, MAX_INPUT_EVENTS_PER_WINDOW)
    }
}
impl SlidingWindowRateLimiter {
    pub fn new(window: Duration, max_events: u32) -> Self {
        Self { window, max_events, events: VecDeque::new() }
    }
    /// Record `now` and return whether the event is within budget. Events
    /// older than the window are evicted first; when the window is full the
    /// event is rejected without being recorded.
    pub fn allow(&mut self, now: Instant) -> bool {
        while self.events.front().is_some_and(|&t| now.saturating_duration_since(t) >= self.window) {
            self.events.pop_front();
        }
        if self.events.len() as u32 >= self.max_events {
            return false;
        }
        self.events.push_back(now);
        true
    }
    /// Number of events currently inside the window (test/diagnostics helper).
    pub fn len(&self) -> usize { self.events.len() }
    pub fn is_empty(&self) -> bool { self.events.is_empty() }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ControlToken { nonce: [u8; 16], issued_at: Instant }
impl ControlToken {
    pub fn nonce(&self) -> &[u8; 16] { &self.nonce }
    pub fn is_valid_at(&self, now: Instant) -> bool { now.saturating_duration_since(self.issued_at) < CONTROL_GRANT_TTL }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionPermissions {
    session_id: ScreenShareSessionId,
    peer_id: iroh::PublicKey,
    granted: Vec<Capability>,
    active: bool,
    token: Option<ControlToken>,
}
impl SessionPermissions {
    /// The view-only default: the peer may receive frames but never inject
    /// input. No token is issued.
    pub fn view_only(session_id: ScreenShareSessionId, peer_id: iroh::PublicKey) -> Self {
        Self { session_id, peer_id, granted: vec![Capability::ViewScreen], active: true, token: None }
    }
    pub fn session_id(&self) -> ScreenShareSessionId { self.session_id }
    pub fn peer_id(&self) -> iroh::PublicKey { self.peer_id }
    pub fn is_active(&self) -> bool { self.active }
    pub fn capabilities(&self) -> &[Capability] { &self.granted }
    pub fn token(&self) -> Option<ControlToken> { self.token }
    /// True when the record is active, the token matches the wire nonce, and
    /// the token has not expired.
    pub fn nonce_matches(&self, nonce: [u8; 16], now: Instant) -> bool {
        self.token.is_some_and(|token| token.nonce == nonce && token.is_valid_at(now))
    }
    /// True only when every condition holds: active session, matching session
    /// id and peer, and the capability is present.
    pub fn allows(&self, session_id: ScreenShareSessionId, peer_id: iroh::PublicKey, capability: Capability) -> bool {
        self.active && self.session_id == session_id && self.peer_id == peer_id && self.granted.contains(&capability)
    }
    pub fn allows_token(&self, session_id: ScreenShareSessionId, peer_id: iroh::PublicKey, token: ControlToken, capability: Capability, now: Instant) -> bool {
        self.allows(session_id, peer_id, capability) && self.token == Some(token) && token.is_valid_at(now)
    }
    /// True when the session is active and has at least one remote-control
    /// capability granted (pointer/keyboard).
    pub fn has_control(&self) -> bool {
        self.active && self.granted.iter().any(|capability| capability.is_control())
    }
    /// True when the session is active and only the view-only baseline is
    /// granted (no control, no clipboard).
    pub fn is_view_only(&self) -> bool {
        self.active && self.granted.iter().all(|capability| *capability == Capability::ViewScreen)
    }
    /// Host-side explicit consent: add the given capabilities (control or
    /// clipboard) and issue a fresh nonce token when any control capability is
    /// granted. `ViewScreen` is never granted through this path — it is the
    /// implied baseline. Returns false when the session is no longer active.
    pub fn grant(&mut self, capabilities: impl IntoIterator<Item = Capability>) -> bool {
        if !self.active { return false; }
        let mut granted_control = false;
        for capability in capabilities {
            if capability == Capability::ViewScreen { continue; }
            if !self.granted.contains(&capability) && self.granted.len() < MAX_CAPABILITIES {
                self.granted.push(capability);
                granted_control |= capability.is_control();
            }
        }
        if granted_control {
            let mut nonce = [0; 16];
            if getrandom::fill(&mut nonce).is_err() { return false; }
            self.token = Some(ControlToken { nonce, issued_at: Instant::now() });
        }
        true
    }
    /// Grant control capabilities carrying a peer-provided nonce (the viewer
    /// echoes the host's nonce back in every input message). Used on the
    /// viewer side when the host's `GrantControl` arrives. `ViewScreen` is
    /// never granted through this path.
    pub fn grant_with_nonce(&mut self, capabilities: impl IntoIterator<Item = Capability>, nonce: [u8; 16]) -> bool {
        if !self.active { return false; }
        let mut granted_control = false;
        for capability in capabilities {
            if capability == Capability::ViewScreen { continue; }
            if !self.granted.contains(&capability) && self.granted.len() < MAX_CAPABILITIES {
                self.granted.push(capability);
                granted_control |= capability.is_control();
            }
        }
        if granted_control {
            self.token = Some(ControlToken { nonce, issued_at: Instant::now() });
        }
        true
    }
    /// One-click revoke: drop every control/clipboard capability and the
    /// nonce token. The session stays active and view-only, so streaming
    /// continues without input.
    pub fn revoke_control(&mut self) {
        self.granted.retain(|capability| *capability == Capability::ViewScreen);
        self.token = None;
    }
    /// Security-significant reconnect: reset to view-only (PDF Task 3.3 /
    /// REC-2). Control capabilities and the nonce token are dropped; the
    /// session stays view-only until fresh explicit consent grants control.
    pub fn reset_for_reconnect(&mut self) {
        self.revoke_control();
    }
    /// Stop condition: the session is over. The record becomes inactive and
    /// every capability/token is cleared, so any late input or view attempt
    /// fails authorization immediately.
    pub fn end(&mut self) { self.active = false; self.granted.clear(); self.token = None; }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PermissionState { Unknown, Granted, Denied }

#[cfg(test)]
mod tests {
    use super::*;

    fn session() -> ScreenShareSessionId { ScreenShareSessionId::from_bytes([7; 16]) }
    fn peer() -> iroh::PublicKey { iroh::SecretKey::generate().public() }
    fn other_peer() -> iroh::PublicKey { iroh::SecretKey::generate().public() }

    #[test]
    fn view_only_does_not_authorize_input() {
        let session = session();
        let peer = peer();
        let permissions = SessionPermissions::view_only(session, peer);
        assert!(permissions.is_active());
        assert!(permissions.is_view_only());
        assert!(!permissions.has_control());
        assert!(permissions.allows(session, peer, Capability::ViewScreen));
        assert!(!permissions.allows(session, peer, Capability::ControlPointer));
        assert!(!permissions.allows(session, peer, Capability::ControlKeyboard));
        assert!(permissions.token().is_none());
    }

    #[test]
    fn grant_issues_token_and_authorizes_control() {
        let session = session();
        let peer = peer();
        let mut permissions = SessionPermissions::view_only(session, peer);
        assert!(permissions.grant([Capability::ControlPointer]));
        assert!(permissions.has_control());
        assert!(!permissions.is_view_only());
        let token = permissions.token().unwrap();
        assert!(permissions.allows(session, peer, Capability::ControlPointer));
        assert!(permissions.allows_token(session, peer, token, Capability::ControlPointer, Instant::now()));
        assert!(permissions.nonce_matches(*token.nonce(), Instant::now()));
    }

    #[test]
    fn revoke_invalidates_token_and_returns_to_view_only() {
        let session = session();
        let peer = peer();
        let mut permissions = SessionPermissions::view_only(session, peer);
        permissions.grant([Capability::ControlPointer, Capability::ControlKeyboard]);
        let token = permissions.token().unwrap();
        assert!(permissions.allows_token(session, peer, token, Capability::ControlPointer, Instant::now()));
        permissions.revoke_control();
        assert!(!permissions.allows_token(session, peer, token, Capability::ControlPointer, Instant::now()));
        assert!(!permissions.allows(session, peer, Capability::ControlPointer));
        assert!(!permissions.allows(session, peer, Capability::ControlKeyboard));
        assert!(permissions.is_view_only());
        assert!(permissions.token().is_none());
        // Viewing continues after revoke.
        assert!(permissions.allows(session, peer, Capability::ViewScreen));
    }

    #[test]
    fn end_stops_all_input_and_viewing() {
        let session = session();
        let peer = peer();
        let mut permissions = SessionPermissions::view_only(session, peer);
        permissions.grant([Capability::ControlPointer]);
        let token = permissions.token().unwrap();
        permissions.end();
        assert!(!permissions.is_active());
        assert!(!permissions.allows(session, peer, Capability::ViewScreen));
        assert!(!permissions.allows(session, peer, Capability::ControlPointer));
        assert!(!permissions.allows_token(session, peer, token, Capability::ControlPointer, Instant::now()));
        assert!(!permissions.nonce_matches(*token.nonce(), Instant::now()));
        assert!(!permissions.has_control());
        // Late grants on an ended session are refused.
        assert!(!permissions.grant([Capability::ControlPointer]));
    }

    #[test]
    fn reset_for_reconnect_drops_control_keeps_viewing() {
        let session = session();
        let peer = peer();
        let mut permissions = SessionPermissions::view_only(session, peer);
        permissions.grant([Capability::ControlPointer]);
        let token = permissions.token().unwrap();
        permissions.reset_for_reconnect();
        assert!(permissions.is_active());
        assert!(permissions.is_view_only());
        assert!(!permissions.has_control());
        assert!(!permissions.allows(session, peer, Capability::ControlPointer));
        assert!(!permissions.nonce_matches(*token.nonce(), Instant::now()));
        assert!(permissions.allows(session, peer, Capability::ViewScreen));
    }

    #[test]
    fn expired_token_is_rejected() {
        let session = session();
        let peer = peer();
        let mut permissions = SessionPermissions::view_only(session, peer);
        permissions.grant([Capability::ControlPointer]);
        let token = permissions.token().unwrap();
        let expired_at = token.issued_at + CONTROL_GRANT_TTL + Duration::from_secs(1);
        assert!(!permissions.allows_token(session, peer, token, Capability::ControlPointer, expired_at));
        assert!(!permissions.nonce_matches(*token.nonce(), expired_at));
        // The capability itself still authorizes at any time — the nonce is
        // the freshness gate.
        assert!(permissions.allows(session, peer, Capability::ControlPointer));
    }

    #[test]
    fn session_and_peer_mismatch_are_rejected() {
        let session = session();
        let peer = peer();
        let permissions = SessionPermissions::view_only(session, peer);
        let other_session = ScreenShareSessionId::from_bytes([8; 16]);
        assert!(!permissions.allows(other_session, peer, Capability::ViewScreen));
        assert!(!permissions.allows(session, other_peer(), Capability::ViewScreen));
        assert!(!permissions.allows(other_session, other_peer(), Capability::ViewScreen));
    }

    #[test]
    fn grant_ignores_view_screen_and_duplicates() {
        let session = session();
        let peer = peer();
        let mut permissions = SessionPermissions::view_only(session, peer);
        // ViewScreen is the implied baseline; granting it through the control
        // path must not duplicate it or mint a token.
        assert!(permissions.grant([Capability::ViewScreen, Capability::ControlPointer, Capability::ControlPointer]));
        assert_eq!(permissions.capabilities().iter().filter(|c| **c == Capability::ViewScreen).count(), 1);
        assert_eq!(permissions.capabilities().iter().filter(|c| **c == Capability::ControlPointer).count(), 1);
        assert!(permissions.token().is_some());
    }

    #[test]
    fn grant_with_nonce_accepts_host_nonce() {
        let session = session();
        let peer = peer();
        let mut permissions = SessionPermissions::view_only(session, peer);
        let nonce = [0xAB; 16];
        assert!(permissions.grant_with_nonce([Capability::ControlKeyboard], nonce));
        assert!(permissions.nonce_matches(nonce, Instant::now()));
        assert!(!permissions.nonce_matches([0x00; 16], Instant::now()));
        assert!(permissions.allows(session, peer, Capability::ControlKeyboard));
    }

    #[test]
    fn rate_limiter_blocks_bursts_and_recovers() {
        let peer = peer();
        let mut limiter = RequestRateLimiter::default();
        let start = Instant::now();
        for _ in 0..MAX_REQUESTS_PER_WINDOW {
            assert!(limiter.allow(peer, start));
        }
        assert!(!limiter.allow(peer, start));
        assert!(!limiter.allow(peer, start + Duration::from_secs(1)));
        // A fresh window allows requests again.
        assert!(limiter.allow(peer, start + REQUEST_WINDOW));
    }

    /// PDF Task 9.2: pathological input streams are bounded by a sliding
    /// window. A burst beyond the budget is dropped; events that age out of
    /// the window free budget again; a sustained stream within the budget
    /// passes.
    #[test]
    fn sliding_window_rate_limiter_bounds_input_streams() {
        let window = Duration::from_secs(1);
        let mut limiter = SlidingWindowRateLimiter::new(window, 3);
        let start = Instant::now();
        assert!(limiter.allow(start));
        assert!(limiter.allow(start));
        assert!(limiter.allow(start));
        // Window full: the pathological 4th event is dropped.
        assert!(!limiter.allow(start));
        assert!(!limiter.allow(start + Duration::from_millis(500)));
        assert_eq!(limiter.len(), 3);
        // The window slides: the first events age out and budget frees.
        assert!(limiter.allow(start + window));
        assert_eq!(limiter.len(), 1);
        assert!(limiter.allow(start + window + Duration::from_millis(1)));
        assert!(limiter.allow(start + window + Duration::from_millis(2)));
        assert!(!limiter.allow(start + window + Duration::from_millis(3)));
    }

    /// A long-running but bounded stream passes: the limiter only rejects
    /// sustained floods, not normal human-rate input.
    #[test]
    fn sliding_window_rate_limiter_passes_sustained_low_rate() {
        let mut limiter = SlidingWindowRateLimiter::default();
        let start = Instant::now();
        // 60 events spread over 2 seconds (well under the 200/s budget).
        for i in 0..60 {
            let t = start + Duration::from_millis(i * 33);
            assert!(limiter.allow(t), "event {i} must pass at {t:?}");
        }
    }
}
