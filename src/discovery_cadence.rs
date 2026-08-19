//! Adaptive DHT discovery cadence (BORU-DHT-05).
//!
//! `DiscoveryCadencePolicy` decides how long to wait before the next DHT
//! discovery lookup, based on mesh-health signals (known/connected neighbour
//! counts, recent successful join, recent DHT success/failure).  It is a pure,
//! unit-testable, UI-independent state machine: no wall-clock, no I/O, no
//! tokio.  Callers feed it [`CadenceSignals`] each cycle and receive the next
//! base wait; the caller is responsible for applying jitter (the periodic
//! loops already do so via `public_room_continuous::apply_jitter`).
//!
//! Behaviour (matching the Boru DHT Discovery Implementation Plan, PDF Task 5):
//! - **Explicit join/create** → look up immediately (min delay).
//! - **Recent DHT failure** → bounded exponential backoff (base → max), never
//!   an endless tight loop.
//! - **Healthy mesh** (connected neighbours present) → slow cadence, 2–5 min.
//! - **Zero known neighbours** → immediate lookup (fastest, so an isolated
//!   node keeps probing until it finds anyone).
//! - **Isolated / startup** (some known, none connected) → fast ramp
//!   `2s / 5s / 10s / 20s / 30s`, then holds at the last stage.
//! - Always floored at `min_delay` to avoid tight loops.

use std::time::Duration;

/// Every interval the cadence policy is allowed to return: the *base* value
/// before jitter.  Jitter is applied by the calling loop, not inside this
/// module (keeps it deterministic and unit-testable).
const STARTUP_RAMP_DEFAULT: [Duration; 5] = [
    Duration::from_secs(2),
    Duration::from_secs(5),
    Duration::from_secs(10),
    Duration::from_secs(20),
    Duration::from_secs(30),
];

/// Configuration for [`DiscoveryCadencePolicy`].
#[derive(Debug, Clone)]
pub struct CadencePolicyConfig {
    /// Fast lookup ramp used while the node is isolated/starting up (some
    /// known neighbours but none connected yet).  Defaults to
    /// `2s / 5s / 10s / 20s / 30s`; the policy advances through it and holds
    /// at the last stage.
    pub startup_ramp: [Duration; 5],
    /// Cadence once the mesh is healthy (connected neighbours present).
    /// Default 3 minutes (within the 2–5 min target band).
    pub healthy_interval: Duration,
    /// Absolute floor for any returned wait — prevents tight loops.
    /// Default 250 ms.
    pub min_delay: Duration,
    /// Cadence when there are zero known neighbours (fully isolated): the
    /// node keeps probing immediately until it finds anyone.  Defaults to
    /// `min_delay`.
    pub zero_neighbour_interval: Duration,
    /// Base delay for the first DHT failure; doubles per consecutive failure.
    /// Default 2 s.
    pub failure_backoff_base: Duration,
    /// Hard cap on failure backoff.  Default 60 s.
    pub failure_backoff_max: Duration,
}

impl Default for CadencePolicyConfig {
    fn default() -> Self {
        let min_delay = Duration::from_millis(250);
        Self {
            startup_ramp: STARTUP_RAMP_DEFAULT,
            healthy_interval: Duration::from_secs(180), // 3 minutes
            min_delay,
            zero_neighbour_interval: min_delay,
            failure_backoff_base: Duration::from_secs(2),
            failure_backoff_max: Duration::from_secs(60),
        }
    }
}

/// Mesh-health signals fed into the policy each discovery cycle.
#[derive(Debug, Clone, Copy, Default)]
pub struct CadenceSignals {
    /// Total known neighbours (candidates seen / remembered).
    pub known_neighbours: usize,
    /// Currently connected neighbours.  `> 0` ⇒ mesh healthy ⇒ slow cadence.
    pub connected_neighbours: usize,
    /// A peer joined successfully since the last decision point.
    pub recent_successful_join: bool,
    /// The most recent DHT lookup returned Ok (possibly empty).
    pub recent_dht_success: bool,
    /// The most recent DHT lookup returned Err.
    pub recent_dht_failure: bool,
    /// True when the user explicitly created or joined a room — forces an
    /// immediate lookup on the next cycle.
    pub explicit_join_or_create: bool,
}

/// A pure, unit-testable policy that returns the base wait before the next
/// DHT discovery lookup given current mesh-health signals.
///
/// The policy keeps two small pieces of internal state: the number of
/// consecutive DHT failures (drives bounded backoff) and the current stage of
/// the startup ramp (so an isolated node escalates `2s→30s` instead of being
/// stuck at 2 s forever).  All decisions are computed from the supplied
/// signals + state; there is no `Instant::now()` inside.
#[derive(Debug)]
pub struct DiscoveryCadencePolicy {
    config: CadencePolicyConfig,
    consecutive_failures: u32,
    ramp_stage: usize,
}

impl DiscoveryCadencePolicy {
    /// Create a policy with the given configuration.
    pub fn new(config: CadencePolicyConfig) -> Self {
        Self {
            config,
            consecutive_failures: 0,
            ramp_stage: 0,
        }
    }

    /// Compute the base wait (pre-jitter) before the next lookup.
    pub fn next_wait(&mut self, signals: &CadenceSignals) -> Duration {
        // 1. Explicit user join/create → immediate.
        if signals.explicit_join_or_create {
            self.consecutive_failures = 0;
            self.ramp_stage = 0;
            return self.config.min_delay;
        }

        // 2. Recent DHT failure → bounded exponential backoff.  This is the
        //    only branch that leaves the failure counter above zero; a success
        //    resets it below, so the backoff cannot spiral forever on a merely
        //    flaky lookup.
        if signals.recent_dht_failure {
            self.consecutive_failures = self.consecutive_failures.saturating_add(1);
            let exponent = self.consecutive_failures.saturating_sub(1);
            let factor = 1u32 << exponent.min(20);
            let backoff = self
                .config
                .failure_backoff_base
                .checked_mul(factor)
                .unwrap_or(self.config.failure_backoff_max)
                .min(self.config.failure_backoff_max);
            // A failure is never an excuse to tight-loop; floor at min_delay.
            return backoff.max(self.config.min_delay);
        }

        // Healthy / success path: reset the failure counter and treat a
        // successful join as a sign the mesh is forming.
        self.consecutive_failures = 0;

        // 3. Healthy mesh → slow cadence (2–5 min).
        if signals.connected_neighbours > 0
            || (signals.known_neighbours > 0 && signals.recent_successful_join)
        {
            self.ramp_stage = 0;
            return self.config.healthy_interval;
        }

        // 4. Zero known neighbours → immediate lookup (keep probing).
        if signals.known_neighbours == 0 {
            self.ramp_stage = 0;
            return self.config.zero_neighbour_interval;
        }

        // 5. Isolated / startup: some known, none connected → fast ramp, then
        //    hold at the last (slowest) ramp stage.
        let stage = self.ramp_stage.min(self.config.startup_ramp.len() - 1);
        let wait = self.config.startup_ramp[stage];
        if self.ramp_stage < self.config.startup_ramp.len() - 1 {
            self.ramp_stage += 1;
        }
        wait
    }

    /// Number of consecutive DHT failures tracked so far (diagnostics).
    pub fn consecutive_failures(&self) -> u32 {
        self.consecutive_failures
    }

    /// Current startup-ramp stage (diagnostics).
    pub fn ramp_stage(&self) -> usize {
        self.ramp_stage
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn signals() -> CadenceSignals {
        CadenceSignals::default()
    }

    #[test]
    fn isolated_zero_neighbour_is_immediate() {
        let mut p = DiscoveryCadencePolicy::new(CadencePolicyConfig::default());
        let s = signals(); // known = 0
        assert_eq!(
            p.next_wait(&s),
            Duration::from_millis(250),
            "zero-neighbour cadence defaults to min_delay"
        );
        assert_eq!(p.consecutive_failures(), 0);
    }

    #[test]
    fn isolated_ramp_escalates_and_holds() {
        let mut p = DiscoveryCadencePolicy::new(CadencePolicyConfig::default());
        let s = CadenceSignals {
            known_neighbours: 1,
            connected_neighbours: 0,
            recent_successful_join: false,
            ..Default::default()
        };
        let expects = [2, 5, 10, 20, 30, 30, 30];
        for e in expects {
            assert_eq!(p.next_wait(&s).as_secs(), e, "ramp stage");
        }
        // Stage caps at the last entry.
        assert_eq!(p.ramp_stage(), 4);
    }

    #[test]
    fn connected_neighbours_means_healthy() {
        let mut p = DiscoveryCadencePolicy::new(CadencePolicyConfig::default());
        let s = CadenceSignals {
            known_neighbours: 5,
            connected_neighbours: 2,
            ..Default::default()
        };
        assert_eq!(p.next_wait(&s).as_secs(), 180);
        // Healthy resets the ramp.
        assert_eq!(p.ramp_stage(), 0);
    }

    #[test]
    fn recent_successful_join_means_healthy() {
        let mut p = DiscoveryCadencePolicy::new(CadencePolicyConfig::default());
        let s = CadenceSignals {
            known_neighbours: 3,
            connected_neighbours: 0,
            recent_successful_join: true,
            ..Default::default()
        };
        assert_eq!(p.next_wait(&s).as_secs(), 180);
    }

    #[test]
    fn dht_failure_backs_off_exponentially_bounded() {
        let mut p = DiscoveryCadencePolicy::new(CadencePolicyConfig::default());
        let fail = CadenceSignals {
            known_neighbours: 1,
            recent_dht_failure: true,
            ..Default::default()
        };
        // 1st failure → base (2s)
        assert_eq!(p.next_wait(&fail).as_secs(), 2);
        // 2nd → 4s
        assert_eq!(p.next_wait(&fail).as_secs(), 4);
        // 3rd → 8s
        assert_eq!(p.next_wait(&fail).as_secs(), 8);
        assert_eq!(p.consecutive_failures(), 3);
        // Once max is reached it stays capped, never > max.
        let mut p2 = DiscoveryCadencePolicy::new(CadencePolicyConfig {
            failure_backoff_base: Duration::from_secs(40),
            failure_backoff_max: Duration::from_secs(100),
            ..Default::default()
        });
        // 1st failure: 40s
        assert_eq!(p2.next_wait(&fail).as_secs(), 40);
        // 2nd failure: 80s
        assert_eq!(p2.next_wait(&fail).as_secs(), 80);
        // 3rd failure would be 160s → capped at 100s.
        assert_eq!(p2.next_wait(&fail).as_secs(), 100);
        // 4th failure stays capped at 100s.
        assert_eq!(p2.next_wait(&fail).as_secs(), 100);
    }

    #[test]
    fn failure_never_tight_loops_below_min_delay() {
        let mut p = DiscoveryCadencePolicy::new(CadencePolicyConfig {
            failure_backoff_base: Duration::from_millis(100),
            min_delay: Duration::from_secs(1),
            ..Default::default()
        });
        let fail = CadenceSignals {
            recent_dht_failure: true,
            ..Default::default()
        };
        // Even a tiny base is floored at min_delay (1s).
        assert_eq!(p.next_wait(&fail).as_secs(), 1);
    }

    #[test]
    fn success_resets_failure_counter() {
        let mut p = DiscoveryCadencePolicy::new(CadencePolicyConfig::default());
        let fail = CadenceSignals {
            known_neighbours: 1,
            recent_dht_failure: true,
            ..Default::default()
        };
        p.next_wait(&fail);
        p.next_wait(&fail);
        assert_eq!(p.consecutive_failures(), 2);
        // Success cycle resets.
        let ok = CadenceSignals {
            known_neighbours: 1,
            recent_dht_success: true,
            ..Default::default()
        };
        p.next_wait(&ok);
        assert_eq!(p.consecutive_failures(), 0);
    }

    #[test]
    fn explicit_join_create_is_immediate() {
        let mut p = DiscoveryCadencePolicy::new(CadencePolicyConfig::default());
        // Even with a healthy-looking mesh, explicit action forces immediate.
        let s = CadenceSignals {
            known_neighbours: 10,
            connected_neighbours: 4,
            explicit_join_or_create: true,
            ..Default::default()
        };
        assert_eq!(p.next_wait(&s).as_secs(), 0);
        assert_eq!(p.next_wait(&s).as_secs(), 0);
    }

    #[test]
    fn explicit_join_resets_ramp_and_failures() {
        let mut p = DiscoveryCadencePolicy::new(CadencePolicyConfig::default());
        let fail = CadenceSignals {
            known_neighbours: 1,
            recent_dht_failure: true,
            ..Default::default()
        };
        p.next_wait(&fail);
        let s = CadenceSignals {
            known_neighbours: 1,
            explicit_join_or_create: true,
            ..Default::default()
        };
        p.next_wait(&s);
        assert_eq!(p.consecutive_failures(), 0);
        assert_eq!(p.ramp_stage(), 0);
    }

    #[test]
    fn isolated_ramp_prefers_known_neighbours_over_join_flag() {
        // A node with known neighbours but none connected and no recent join
        // stays on the fast ramp, not the healthy interval.
        let mut p = DiscoveryCadencePolicy::new(CadencePolicyConfig::default());
        let s = CadenceSignals {
            known_neighbours: 2,
            connected_neighbours: 0,
            recent_successful_join: false,
            ..Default::default()
        };
        assert_eq!(p.next_wait(&s).as_secs(), 2);
    }
}
