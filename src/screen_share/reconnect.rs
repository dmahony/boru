//! Reconnect behaviour for the screen-share media path (PDF Task 3.3).
//!
//! Three guarantees implemented here and in the session state machine:
//!
//! 1. **Chat/friend session survives transient media failure.** Chat traffic
//!    lives on a separate QUIC connection (gossip), so a screen-share media
//!    failure cannot tear it down. The screen-share session itself enters
//!    [`SessionState::Reconnecting`] and the host driver re-establishes the
//!    media path with bounded retries ([`ReconnectPolicy`]).
//!
//! 2. **Fresh keyframe after reconnection.** After the media path is
//!    re-established the host forces the encoder to emit a keyframe
//!    (`VideoEncoder::force_keyframe`), so the viewer can resynchronise
//!    without waiting for the next periodic keyframe. The viewer can also
//!    request one explicitly with [`keyframe_request`].
//!
//! 3. **Remote control is never silently resumed.** A security-significant
//!    reconnect resets the session to view-only (`SessionPermissions`), and
//!    [`ReconnectPolicy::may_resume_control`] is the ONLY gate that would
//!    permit control to come back — it defaults to `false` (REC-2).
//!
//! The retry loop itself is intentionally transport-agnostic so it can be
//! unit-tested without a live QUIC connection.

use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use super::{
    protocol::SCREEN_SHARE_PROTOCOL_VERSION, session::ScreenShareSessionId, ScreenShareError,
};

/// Bounded retry policy for re-establishing the media path after a transient
/// failure.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ReconnectPolicy {
    /// Maximum number of re-establishment attempts before the session is
    /// failed (`fail_reconnect`).
    pub max_attempts: u32,
    /// Base delay between attempts; doubled on each retry (exponential
    /// backoff) up to [`Self::max_delay`].
    pub base_delay: Duration,
    /// Upper bound for the per-attempt backoff delay.
    pub max_delay: Duration,
    /// Whether remote-control capabilities may be resumed after a
    /// security-significant reconnect. **Must stay `false` unless a future
    /// policy explicitly opts in** — a reconnected session starts view-only
    /// and control requires fresh explicit consent (PDF Task 3.3 / REC-2).
    pub regrant_control_on_reconnect: bool,
}

impl Default for ReconnectPolicy {
    fn default() -> Self {
        Self {
            max_attempts: 3,
            base_delay: Duration::from_millis(250),
            max_delay: Duration::from_secs(2),
            regrant_control_on_reconnect: false,
        }
    }
}

impl ReconnectPolicy {
    /// The ONLY gate that permits control to resume after a
    /// security-significant reconnect. Defaults to `false` — the reconnected
    /// session starts view-only and control requires fresh consent.
    pub fn may_resume_control(&self) -> bool {
        self.regrant_control_on_reconnect
    }

    /// Exponential backoff delay before the `attempt`-th retry (0-based).
    /// The first attempt (0) has no delay.
    pub fn backoff(&self, attempt: u32) -> Duration {
        if attempt == 0 {
            return Duration::ZERO;
        }
        let shift = attempt.saturating_sub(1).min(6);
        let delay = self.base_delay.saturating_mul(1u32 << shift);
        delay.min(self.max_delay)
    }
}

/// Outcome of a bounded re-establishment loop.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReconnectOutcome {
    /// The media path was re-established and the session may resume streaming.
    Reconnected,
    /// Every attempt failed; the caller should fail the session.
    Exhausted,
    /// The stop flag was set; the caller should end the session.
    Stopped,
}

/// Run `establish` up to `policy.max_attempts` times with exponential backoff,
/// honouring the `stop` flag. `establish` performs one full media-path
/// re-establishment (dial + negotiate + channels) and returns `Ok(value)` on
/// success. Returns `Err(ReconnectOutcome)` when the budget was exhausted or
/// `stop` was set. Transport-agnostic so it is unit-testable without QUIC.
pub async fn retry_reconnect<F, Fut, T>(
    policy: &ReconnectPolicy,
    stop: &AtomicBool,
    mut establish: F,
) -> Result<T, ReconnectOutcome>
where
    F: FnMut(u32) -> Fut,
    Fut: std::future::Future<Output = Result<T, ScreenShareError>>,
{
    for attempt in 0..policy.max_attempts {
        if stop.load(Ordering::Relaxed) {
            return Err(ReconnectOutcome::Stopped);
        }
        if attempt > 0 {
            tokio::time::sleep(policy.backoff(attempt)).await;
            if stop.load(Ordering::Relaxed) {
                return Err(ReconnectOutcome::Stopped);
            }
        }
        match establish(attempt).await {
            Ok(value) => return Ok(value),
            Err(error) => {
                tracing::warn!(
                    attempt,
                    error = %error,
                    "screen-share: reconnect attempt failed"
                );
            }
        }
    }
    Err(ReconnectOutcome::Exhausted)
}

/// Build the versioned keyframe request the viewer sends after a media
/// reconnection so the host forces the next frame to be a keyframe.
pub fn keyframe_request(session_id: ScreenShareSessionId) -> super::protocol::ScreenShareMessage {
    super::protocol::ScreenShareMessage::KeyframeRequest {
        version: SCREEN_SHARE_PROTOCOL_VERSION,
        session_id,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::screen_share::session::ScreenShareSessionId;

    fn sid() -> ScreenShareSessionId {
        ScreenShareSessionId::from_bytes([7; 16])
    }

    #[test]
    fn default_policy_denies_control_resume() {
        let policy = ReconnectPolicy::default();
        assert!(
            !policy.may_resume_control(),
            "reconnect must not silently resume control"
        );
    }

    #[test]
    fn policy_can_explicitly_allow_control_resume() {
        let policy = ReconnectPolicy {
            regrant_control_on_reconnect: true,
            ..ReconnectPolicy::default()
        };
        assert!(policy.may_resume_control());
    }

    #[test]
    fn backoff_is_exponential_and_bounded() {
        let policy = ReconnectPolicy {
            base_delay: Duration::from_millis(100),
            max_delay: Duration::from_secs(1),
            ..ReconnectPolicy::default()
        };
        assert_eq!(policy.backoff(0), Duration::ZERO);
        assert_eq!(policy.backoff(1), Duration::from_millis(100));
        assert_eq!(policy.backoff(2), Duration::from_millis(200));
        assert_eq!(policy.backoff(3), Duration::from_millis(400));
        // Capped at max_delay for large attempts.
        assert_eq!(policy.backoff(10), Duration::from_secs(1));
    }

    #[tokio::test]
    async fn retry_reconnects_after_transient_failures() {
        let policy = ReconnectPolicy {
            max_attempts: 3,
            base_delay: Duration::from_millis(1),
            max_delay: Duration::from_millis(5),
            regrant_control_on_reconnect: false,
        };
        let stop = AtomicBool::new(false);
        let failures = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let attempts = failures.clone();
        let outcome = retry_reconnect(&policy, &stop, move |_attempt| {
            let failures = failures.clone();
            async move {
                if failures.load(std::sync::atomic::Ordering::SeqCst) < 2 {
                    failures.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                    Err(ScreenShareError::new("transient"))
                } else {
                    Ok(42u32)
                }
            }
        })
        .await;
        assert_eq!(outcome, Ok(42));
        assert_eq!(attempts.load(std::sync::atomic::Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn retry_exhausts_after_max_attempts() {
        let policy = ReconnectPolicy {
            max_attempts: 2,
            base_delay: Duration::from_millis(1),
            max_delay: Duration::from_millis(5),
            regrant_control_on_reconnect: false,
        };
        let stop = AtomicBool::new(false);
        let mut calls = 0;
        let outcome = retry_reconnect(&policy, &stop, |_attempt| {
            calls += 1;
            async move { Err::<(), _>(ScreenShareError::new("always fails")) }
        })
        .await;
        assert_eq!(outcome, Err(ReconnectOutcome::Exhausted));
        assert_eq!(calls, 2);
    }

    #[tokio::test]
    async fn retry_stops_when_flag_set() {
        let policy = ReconnectPolicy::default();
        let stop = AtomicBool::new(true);
        let mut calls = 0;
        let outcome = retry_reconnect(&policy, &stop, |_attempt| {
            calls += 1;
            async move { Ok(()) }
        })
        .await;
        assert_eq!(outcome, Err(ReconnectOutcome::Stopped));
        assert_eq!(calls, 0, "no attempt may run when stop is already set");
    }

    #[test]
    fn keyframe_request_message_round_trips() {
        let request = keyframe_request(sid());
        let bytes = request.encode().unwrap();
        let decoded = super::super::protocol::ScreenShareMessage::decode(&bytes).unwrap();
        assert_eq!(decoded, request);
    }
}
