//! Rolling candidate admission policy for DHT discovery.
//!
//! Replaces the old "hard lifetime per-session candidate cap" with a bounded,
//! rolling admission scheme (PDF Task 3):
//!
//! * **No lifetime dead-end.** A candidate admitted once is *not* permanently
//!   exhausted.  After a cooldown / stale TTL it becomes admissible again, so
//!   long-running sessions keep recovering new peers instead of permanently
//!   running dry after `max_candidates_per_session`.
//! * **Short-term abuse bound.** Admissions are still rate-limited to
//!   `max_per_window` per rolling `window` (default 10 per 60 s), which bounds
//!   burst throughput regardless of how many distinct candidates the DHT
//!   returns.
//! * **Per-cycle cap.** At most `max_per_cycle` (default 20) candidates are
//!   admitted from a single discovery result set.
//! * **Bounded remembered set.** Peers remembered as "recently admitted" are
//!   kept in an LRU-bounded set (`128..=256`), so memory stays fixed no matter
//!   how many distinct peers appear over a session.
//! * **Counting at handoff.** A candidate is counted when it is admitted
//!   (i.e. handed to the joiner / forwarded onward), not merely because it was
//!   returned by the DHT.
//!
//! The policy is pure and unit-testable: it takes wall-clock [`Instant`]s as
//! arguments and holds no I/O or timers.  The discovery loops feed it the
//! validated candidates from each lookup and forward the admitted subset.

use std::collections::{HashMap, VecDeque};
use std::time::{Duration, Instant};

use iroh::EndpointId;

/// Default cooldown / stale TTL before a peer can be re-admitted.
pub const DEFAULT_CANDIDATE_COOLDOWN: Duration = Duration::from_secs(600); // 10 min
/// Default per-cycle candidate cap.
pub const DEFAULT_MAX_PER_CYCLE: usize = 20;
/// Default rolling window abuse bound.
pub const DEFAULT_MAX_PER_WINDOW: usize = 10;
/// Default rolling window.
pub const DEFAULT_WINDOW: Duration = Duration::from_secs(60);
/// Lower bound for the remembered-set capacity (clamped in [`CandidateAdmission::new`]).
pub const REMEMBERED_MIN: usize = 128;
/// Upper bound for the remembered-set capacity (clamped in [`CandidateAdmission::new`]).
pub const REMEMBERED_MAX: usize = 256;
/// Default remembered-set bound.
pub const DEFAULT_MAX_REMEMBERED: usize = 192;

/// Tuning for [`CandidateAdmission`].
#[derive(Debug, Clone)]
pub struct CandidateAdmissionConfig {
    /// How long a peer stays "recently admitted" before it may be tried again.
    /// Default: 10 minutes.
    pub cooldown: Duration,
    /// Max candidates admitted per discovery cycle.  Default: 20.
    pub max_per_cycle: usize,
    /// Max candidates admitted per rolling window (short-term abuse bound).
    /// Default: 10.
    pub max_per_window: usize,
    /// Rolling window duration.  Default: 60 s.
    pub window: Duration,
    /// Bounded remembered-set capacity, clamped to `[128, 256]`.  Default: 192.
    pub max_remembered: usize,
}

impl Default for CandidateAdmissionConfig {
    fn default() -> Self {
        Self {
            cooldown: DEFAULT_CANDIDATE_COOLDOWN,
            max_per_cycle: DEFAULT_MAX_PER_CYCLE,
            max_per_window: DEFAULT_MAX_PER_WINDOW,
            window: DEFAULT_WINDOW,
            max_remembered: DEFAULT_MAX_REMEMBERED,
        }
    }
}

/// A single admission decision.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AdmissionResult {
    /// Candidate admitted and counted at handoff.
    Admitted,
    /// Candidate skipped because it was admitted within the cooldown window.
    WithinCooldown,
    /// Rolling-window abuse bound reached.
    WindowFull,
}

/// Rolling, bounded candidate admission policy.
///
/// Holds only bookkeeping state; caller passes the current [`Instant`].
#[derive(Debug, Clone)]
pub struct CandidateAdmission {
    /// Recently-admitted peers with their admission instant (LRU order, oldest
    /// at the front).
    remembered: VecDeque<EndpointId>,
    /// Admission time per remembered peer, for exact cooldown staleness.
    remembered_at: HashMap<EndpointId, Instant>,
    /// Wall-clock instants of recent admissions (for the rolling window).
    /// Oldest at the front.
    attempt_times: VecDeque<Instant>,
    /// Configuration.
    config: CandidateAdmissionConfig,
}

impl CandidateAdmission {
    /// Create a fresh, empty admission policy.
    pub fn new(config: CandidateAdmissionConfig) -> Self {
        let max_remembered = config.max_remembered.clamp(REMEMBERED_MIN, REMEMBERED_MAX);
        Self {
            remembered: VecDeque::with_capacity(max_remembered),
            remembered_at: HashMap::with_capacity(max_remembered),
            attempt_times: VecDeque::new(),
            config: CandidateAdmissionConfig {
                max_remembered,
                ..config
            },
        }
    }

    /// Attempt to admit a single candidate at `now`.
    ///
    /// Returns [`AdmissionResult::Admitted`] and records the admission if the
    /// candidate passes all bounds, otherwise one of the rejection reasons.
    pub fn admit_candidate(&mut self, peer: &EndpointId, now: Instant) -> AdmissionResult {
        self.prune(now);

        if self.remembered_at.contains_key(peer) {
            return AdmissionResult::WithinCooldown;
        }
        if self.attempt_times.len() >= self.config.max_per_window {
            return AdmissionResult::WindowFull;
        }

        self.remembered_at.insert(*peer, now);
        self.remembered.push_back(*peer);
        self.attempt_times.push_back(now);
        self.enforce_bound();
        AdmissionResult::Admitted
    }

    /// Admit a batch of candidates, returning the subset that were admitted.
    ///
    /// Iterates candidates in order; stops once the per-cycle cap is reached.
    /// Each admitted candidate is counted at handoff.
    pub fn admit_batch(&mut self, candidates: &[EndpointId], now: Instant) -> Vec<EndpointId> {
        self.prune(now);
        let mut out = Vec::with_capacity(candidates.len().min(self.config.max_per_cycle));
        for peer in candidates {
            if out.len() >= self.config.max_per_cycle {
                break;
            }
            if self.admit_candidate(peer, now) == AdmissionResult::Admitted {
                out.push(*peer);
            }
        }
        out
    }

    /// Number of candidates currently remembered (recently admitted).
    pub fn remembered_len(&self) -> usize {
        self.remembered.len()
    }

    /// Whether `peer` is currently within its cooldown (remembered).
    pub fn is_remembered(&self, peer: &EndpointId) -> bool {
        self.remembered_at.contains_key(peer)
    }

    /// Number of admissions recorded within the current rolling window.
    pub fn attempts_in_window(&mut self, now: Instant) -> usize {
        self.prune(now);
        self.attempt_times.len()
    }

    /// Drop remembered entries older than cooldown and window timestamps that
    /// have rolled out of the window.
    fn prune(&mut self, now: Instant) {
        let cooldown = self.config.cooldown;
        // Evict remembered peers whose cooldown has elapsed.
        let mut keep: VecDeque<EndpointId> = VecDeque::with_capacity(self.remembered.len());
        while let Some(peer) = self.remembered.pop_front() {
            let admitted = self.remembered_at.get(&peer).copied();
            let expired = admitted.is_some_and(|at| now.duration_since(at) >= cooldown);
            if expired {
                self.remembered_at.remove(&peer);
            } else {
                keep.push_back(peer);
            }
        }
        self.remembered = keep;

        // Prune window timestamps older than `now - window`.
        if let Some(cutoff) = now.checked_sub(self.config.window) {
            while self.attempt_times.front().is_some_and(|t| *t <= cutoff) {
                self.attempt_times.pop_front();
            }
        }
    }

    /// Evict the oldest remembered entries once the set exceeds capacity.
    fn enforce_bound(&mut self) {
        while self.remembered.len() > self.config.max_remembered {
            if let Some(oldest) = self.remembered.pop_front() {
                self.remembered_at.remove(&oldest);
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn ep(id: u8) -> EndpointId {
        let mut bytes = [0u8; 32];
        bytes[0] = id;
        let sk = iroh::SecretKey::from_bytes(&bytes);
        sk.public()
    }

    /// A freshly admitted candidate is remembered (within cooldown).
    #[test]
    fn admitted_candidate_is_remembered() {
        let mut a = CandidateAdmission::new(CandidateAdmissionConfig::default());
        let now = Instant::now();
        let p = ep(1);
        let r = a.admit_candidate(&p, now);
        assert_eq!(r, AdmissionResult::Admitted);
        assert!(a.is_remembered(&p));
        assert_eq!(a.remembered_len(), 1);
    }

    /// Re-admitting the same candidate within cooldown is rejected.
    #[test]
    fn candidate_within_cooldown_is_not_readmitted() {
        let mut a = CandidateAdmission::new(CandidateAdmissionConfig::default());
        let now = Instant::now();
        let p = ep(1);
        assert_eq!(a.admit_candidate(&p, now), AdmissionResult::Admitted);
        assert_eq!(a.admit_candidate(&p, now), AdmissionResult::WithinCooldown);
    }

    /// A peer whose cooldown has elapsed becomes admissible again (no lifetime
    /// dead-end).
    #[test]
    fn candidate_reusable_after_cooldown() {
        let mut a = CandidateAdmission::new(CandidateAdmissionConfig {
            cooldown: Duration::from_secs(30),
            window: Duration::from_secs(60),
            max_per_window: 100,
            ..Default::default()
        });
        let t0 = Instant::now();
        let p = ep(1);
        assert_eq!(a.admit_candidate(&p, t0), AdmissionResult::Admitted);
        // Within cooldown -> rejected.
        assert_eq!(
            a.admit_candidate(&p, t0 + Duration::from_secs(5)),
            AdmissionResult::WithinCooldown
        );
        // After cooldown -> re-admissible.
        assert_eq!(
            a.admit_candidate(&p, t0 + Duration::from_secs(31)),
            AdmissionResult::Admitted
        );
    }

    /// The rolling window limits admissions per period.
    #[test]
    fn window_bounds_admissions() {
        let mut a = CandidateAdmission::new(CandidateAdmissionConfig {
            max_per_window: 3,
            window: Duration::from_secs(60),
            ..Default::default()
        });
        let now = Instant::now();
        for id in 1..=3u8 {
            assert_eq!(a.admit_candidate(&ep(id), now), AdmissionResult::Admitted);
        }
        // 4th distinct candidate within the same window is rejected.
        assert_eq!(a.admit_candidate(&ep(4), now), AdmissionResult::WindowFull);
        assert_eq!(a.attempts_in_window(now), 3);
    }

    /// After the window rolls over, new candidates are admitted again.
    #[test]
    fn window_rolls_over_resets_rate() {
        let mut a = CandidateAdmission::new(CandidateAdmissionConfig {
            max_per_window: 1,
            window: Duration::from_secs(10),
            ..Default::default()
        });
        let t0 = Instant::now();
        assert_eq!(a.admit_candidate(&ep(1), t0), AdmissionResult::Admitted);
        assert_eq!(a.admit_candidate(&ep(2), t0), AdmissionResult::WindowFull);
        // After the window elapses, the rate frees up.
        let later = t0 + Duration::from_secs(11);
        assert_eq!(a.admit_candidate(&ep(2), later), AdmissionResult::Admitted);
    }

    /// The remembered set is bounded: beyond capacity the oldest are evicted.
    #[test]
    fn remembered_set_is_bounded() {
        let mut a = CandidateAdmission::new(CandidateAdmissionConfig {
            cooldown: Duration::from_secs(3600),
            window: Duration::from_secs(3600),
            max_per_window: 10_000,
            max_remembered: 128,
            ..Default::default()
        });
        let now = Instant::now();
        for id in 0..200u8 {
            a.admit_candidate(&ep(id), now);
        }
        // Bound enforced at configured capacity.
        assert!(a.remembered_len() <= 128);
        // The oldest evicted peer is no longer remembered.
        assert!(!a.is_remembered(&ep(0)));
    }

    /// batch admission respects the per-cycle cap and dedups.
    #[test]
    fn batch_admission_respects_cycle_cap() {
        let mut a = CandidateAdmission::new(CandidateAdmissionConfig {
            max_per_cycle: 5,
            max_per_window: 100,
            window: Duration::from_secs(60),
            cooldown: Duration::from_secs(600),
            ..Default::default()
        });
        let now = Instant::now();
        let peers: Vec<EndpointId> = (1..=10).map(ep).collect();
        let admitted = a.admit_batch(&peers, now);
        assert_eq!(admitted.len(), 5, "per-cycle cap of 5");
    }

    /// Batch admission deduplicates against previously admitted peers.
    #[test]
    fn batch_admission_dedups_against_remembered() {
        let mut a = CandidateAdmission::new(CandidateAdmissionConfig::default());
        let now = Instant::now();
        let p1 = ep(1);
        assert_eq!(a.admit_candidate(&p1, now), AdmissionResult::Admitted);
        // Re-discovering the same peer yields no new admission.
        let admitted = a.admit_batch(&[p1, ep(2)], now);
        assert_eq!(admitted, vec![ep(2)]);
    }
}
