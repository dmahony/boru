//! Fault injection + restart helpers (BORU-TEST-010).
//!
//! For rich, deterministic fault-injection scenarios (drop acks, duplicate
//! envelopes, delayed/lost delivery, protocol errors, event plans, repro
//! guards) reuse [`test_deterministic_harness`](crate::fault) — specifically
//! the `FaultConfig`, `EventPlan` and `ReproGuard` types there — rather than
//! duplicating them here. This module only adds lightweight, scope-limited
//! helpers that the smaller integration tests want without pulling in the full
//! harness.

use n0_error::Result;

/// A counter that guards against accidental restart loops in restart-oriented
/// tests.
///
/// Call [`Self::bump`] before restarting a peer. If more than `max` restarts
/// are attempted in one test, it returns an error naming the peer so the test
/// fails fast instead of spinning forever. This is a deliberate, explicit
/// version of the "restart loops are a debugging antipattern" rule.
#[derive(Debug, Default, Clone)]
pub struct RestartGuard {
    max: usize,
    count: usize,
}

impl RestartGuard {
    /// Create a guard allowing up to `max` restarts per test.
    pub fn new(max: usize) -> Self {
        Self { max, count: 0 }
    }

    /// Record one more restart for `peer`; errors if the budget is exceeded.
    pub fn bump(&mut self, peer: &str) -> Result<()> {
        self.count += 1;
        if self.count > self.max {
            n0_error::bail_any!(
                "peer {peer} restarted {}/{} times; refusing to restart again (restart loop guard)",
                self.count,
                self.max
            );
        }
        Ok(())
    }

    /// Number of restarts recorded so far.
    pub fn count(&self) -> usize {
        self.count
    }
}
