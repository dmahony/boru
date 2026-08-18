//! Deterministic clock + timeout helpers (BORU-TEST-010).
//!
//! As an alternative to fixed `sleep()` calls, tests can poll with a bounded
//! deadline and get a failure message that names the peer/state/event context
//! that was still missing when the deadline elapsed.

use std::time::Duration;

use n0_error::{bail_any, Result};

/// Default bounded wait for single gossip handshakes / message hops.
pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(30);
/// Poll interval used by the bounded-wait helpers.
pub const TICK: Duration = Duration::from_millis(100);

/// Poll `cond` every [`TICK`] until it returns `true` or `timeout` elapses.
///
/// * `what` — a short noun identifying what is being awaited (e.g. `"peer B to
///   join the mesh"`). Included verbatim in the failure message.
/// * `state` — a closure returning the *current* state/event context (e.g.
///   `format!("joined={} neighbors={}", sub.is_joined(), sub.neighbors().count())`).
///   This is evaluated only on timeout, so the error carries exactly what was
///   still wrong, satisfying the "peer id, state, event context" requirement.
pub async fn wait_until<F, S>(
    what: &str,
    timeout: Duration,
    mut cond: F,
    mut state: S,
) -> Result<()>
where
    F: FnMut() -> bool,
    S: FnMut() -> String,
{
    let deadline = tokio::time::Instant::now() + timeout;
    loop {
        if cond() {
            return Ok(());
        }
        if tokio::time::Instant::now() >= deadline {
            bail_any!(
                "timed out after {timeout:?} waiting for {what}; state: {}",
                state()
            );
        }
        tokio::time::sleep(TICK).await;
    }
}
