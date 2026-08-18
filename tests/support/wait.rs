//! Message wait / assertion helpers (BORU-TEST-010).
//!
//! These replace the hand-rolled drain loops that were duplicated across test
//! files, and produce assertion failures that name the peer/state/event
//! context rather than a bare `assert!(...)`.

use std::time::Duration;

use boru_core::api::{Event as GossipEvent, GossipTopic};
use n0_future::StreamExt;

/// Non-blocking drain of events from a topic subscription.
///
/// Reads until the stream yields no new event within `timeout`. Returns all
/// events collected (typically `Received`, `NeighborUp`/`NeighborDown`
/// lifecycle events). Does not assert anything — use [`expect_received`] or
/// inspect the returned slice when a message must have arrived.
pub async fn drain_events(sub: &mut GossipTopic, timeout: Duration) -> Vec<GossipEvent> {
    let mut events = Vec::new();
    loop {
        let item = tokio::time::timeout(timeout, sub.next()).await;
        match item {
            Ok(Some(Ok(ev))) => events.push(ev),
            Ok(Some(Err(e))) => {
                eprintln!("  drain error: {e}");
                break;
            }
            _ => break, // timeout or stream ended
        }
    }
    events
}

/// Drain events until one matches `pred`, or `timeout` elapses.
///
/// Returns the matched event. Failure names `what` and the most recent events
/// observed, so the error carries the event context that was (and was not)
/// seen.
pub async fn wait_until_event<F>(
    sub: &mut GossipTopic,
    what: &str,
    timeout: Duration,
    mut pred: F,
) -> Result<GossipEvent, String>
where
    F: FnMut(&GossipEvent) -> bool,
{
    let deadline = tokio::time::Instant::now() + timeout;
    // Most recently observed events, capped for the failure message.
    let mut recent: std::collections::VecDeque<GossipEvent> = Default::default();
    loop {
        let evs = drain_events(sub, Duration::from_millis(30)).await;
        for ev in &evs {
            if pred(ev) {
                return Ok(ev.clone());
            }
        }
        for ev in evs {
            recent.push_back(ev);
            if recent.len() > 8 {
                recent.pop_front();
            }
        }
        if tokio::time::Instant::now() >= deadline {
            return Err(format!(
                "timed out after {timeout:?} waiting for {what}; recent events: {recent:?}"
            ));
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

/// Wait for the next `Received` event carrying the given content on `sub`.
///
/// This is the message-arrival assertion: it hangs until a gossip `Received`
/// carrying `expected` content arrives or `timeout` elapses, in which case it
/// returns an `Err` describing the wait target and recent events.
pub async fn expect_received(
    sub: &mut GossipTopic,
    what: &str,
    expected: &[u8],
    timeout: Duration,
) -> Result<(), String> {
    let _ = wait_until_event(
        sub,
        what,
        timeout,
        |ev| matches!(ev, GossipEvent::Received(m) if m.content.as_ref() == expected),
    )
    .await?;
    Ok(())
}
