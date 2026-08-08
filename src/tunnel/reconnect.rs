//! Automatic tunnel link reconnection with exponential backoff.
//!
//! This module provides a small, transport-agnostic reconnect loop
//! ([`run_reconnect_loop`]) plus the [`ReconnectPolicy`] backoff schedule. The
//! loop is used by the recipient-side [`LocalTunnelListener`](super::LocalTunnelListener)
//! so a dropped tunnel link re-establishes itself instead of relying on the
//! next local application connection to re-dial.

use std::{
    sync::Arc,
    time::Duration,
};

use tokio::sync::watch;
use tokio_util::sync::CancellationToken;

use super::service::{ReconnectInfo, ReconnectPolicy};

/// A handle to an established tunnel link.
///
/// The loop holds the handle after a successful connect and awaits
/// [`Self::closed`] to learn when the link drops.
pub trait TunnelLinkHandle: Send + Sync + 'static {
    /// Resolve when the tunnel link closes for any reason.
    fn closed(&self) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send + '_>>;
}

impl TunnelLinkHandle for iroh::endpoint::Connection {
    fn closed(
        &self,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send + '_>> {
        let connection = self.clone();
        Box::pin(async move {
            let _ = connection.closed().await;
        })
    }
}

/// Reconnect-loop status, surfaced to callers (and ultimately the GUI) so the
/// UI reflects the reconnecting state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TunnelLinkStatus {
    /// No link is currently established and no retry is scheduled.
    Idle,
    /// The loop is attempting to establish the link.
    Connecting,
    /// The link is established.
    Connected,
    /// The link dropped; the loop is waiting `next_delay` before retrying.
    Reconnecting {
        /// Zero-based retry counter.
        attempt: u32,
        /// Delay before the next attempt.
        next_delay: Duration,
    },
    /// The loop stopped (expired, revoked, or explicitly cancelled).
    Stopped,
}

impl TunnelLinkStatus {
    /// Human-readable label for the GUI.
    pub fn label(self) -> &'static str {
        match self {
            Self::Idle => "Idle",
            Self::Connecting => "Connecting",
            Self::Connected => "Connected",
            Self::Reconnecting { .. } => "Reconnecting",
            Self::Stopped => "Stopped",
        }
    }
}

/// Run the reconnect loop until the link is established, the link is closed,
/// the tunnel expires, or the caller cancels.
///
/// The loop calls `connect` to establish the link. On success it reports
/// [`TunnelLinkStatus::Connected`] and waits for the returned handle's
/// `closed` future; when the link drops it reports
/// [`TunnelLinkStatus::Reconnecting`] and waits `policy.delay_for(attempt)`
/// before retrying. `is_expired` is consulted before every retry — an expired
/// tunnel never auto-reconnects.
///
/// Status changes are delivered through `status_tx` (a watch channel so the
/// GUI can subscribe cheaply) and through `info_tx` when the service's
/// reconnect counter should be updated (optional).
pub async fn run_reconnect_loop<C, E>(
    mut connect: C,
    mut is_expired: E,
    policy: ReconnectPolicy,
    cancellation: CancellationToken,
    status_tx: watch::Sender<TunnelLinkStatus>,
    info_tx: Option<tokio::sync::mpsc::UnboundedSender<ReconnectInfo>>,
) where
    C: FnMut() -> std::pin::Pin<
        Box<dyn std::future::Future<Output = anyhow::Result<Arc<dyn TunnelLinkHandle>>> + Send>,
    > + Send,
    E: FnMut() -> bool + Send,
{
    let mut attempt: u32 = 0;
    loop {
        if cancellation.is_cancelled() {
            let _ = status_tx.send(TunnelLinkStatus::Stopped);
            return;
        }
        if is_expired() {
            let _ = status_tx.send(TunnelLinkStatus::Stopped);
            return;
        }

        let _ = status_tx.send(TunnelLinkStatus::Connecting);
        let connect_result = connect().await;
        match connect_result {
            Ok(handle) => {
                attempt = 0;
                let _ = status_tx.send(TunnelLinkStatus::Connected);
                // Recompute the next delay from a clean slate.
                if let Some(info_tx) = &info_tx {
                    let _ = info_tx.send(ReconnectInfo {
                        attempt: 0,
                        next_delay: Duration::ZERO,
                    });
                }
                // Wait for the link to drop, but remain cancellable so the
                // caller can stop the loop while a link is up.
                tokio::select! {
                    _ = handle.closed() => {}
                    _ = cancellation.cancelled() => {
                        let _ = status_tx.send(TunnelLinkStatus::Stopped);
                        return;
                    }
                }
            }
            Err(_) => {
                // Fall through to the backoff wait; the link is not up.
            }
        }

        if cancellation.is_cancelled() {
            let _ = status_tx.send(TunnelLinkStatus::Stopped);
            return;
        }
        if is_expired() {
            let _ = status_tx.send(TunnelLinkStatus::Stopped);
            return;
        }

        let next_delay = policy.delay_for(attempt);
        let _ = status_tx.send(TunnelLinkStatus::Reconnecting {
            attempt,
            next_delay,
        });
        if let Some(info_tx) = &info_tx {
            let _ = info_tx.send(ReconnectInfo {
                attempt,
                next_delay,
            });
        }
        attempt = attempt.saturating_add(1);
        tokio::select! {
            _ = tokio::time::sleep(next_delay) => {}
            _ = cancellation.cancelled() => {
                let _ = status_tx.send(TunnelLinkStatus::Stopped);
                return;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::{
        sync::{
            atomic::{AtomicU32, Ordering},
            Arc,
        },
        time::Duration,
    };

    use tokio::sync::watch;
    use tokio_util::sync::CancellationToken;

    use super::{run_reconnect_loop, TunnelLinkHandle, TunnelLinkStatus};
    use crate::tunnel::service::ReconnectPolicy;

    /// A link handle whose `closed` future resolves when the test closes it.
    struct FakeLink {
        closed: tokio::sync::watch::Receiver<bool>,
    }

    impl TunnelLinkHandle for FakeLink {
        fn closed(
            &self,
        ) -> std::pin::Pin<
            Box<dyn std::future::Future<Output = ()> + Send + '_>,
        > {
        let mut closed = self.closed.clone();
            Box::pin(async move {
                let _ = closed.changed().await;
            })
        }
    }

    fn policy() -> ReconnectPolicy {
        ReconnectPolicy {
            initial_delay: Duration::from_millis(10),
            max_delay: Duration::from_millis(80),
            factor: 2,
            jitter: 0.0,
        }
    }

    /// A connect closure that fails `failures` times, then returns a link the
    /// test can close at will.
    fn failing_connect(
        failures: u32,
        attempts: Arc<AtomicU32>,
        close_tx: tokio::sync::watch::Sender<bool>,
    ) -> impl FnMut(
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = anyhow::Result<Arc<dyn TunnelLinkHandle>>> + Send>,
    > + Send {
        move || {
            let attempts = Arc::clone(&attempts);
            let close_tx = close_tx.clone();
            Box::pin(async move {
                let n = attempts.fetch_add(1, Ordering::SeqCst);
                if n < failures {
                    anyhow::bail!("connect failed (attempt {n})");
                }
                Ok(Arc::new(FakeLink {
                    closed: close_tx.subscribe(),
                }) as Arc<dyn TunnelLinkHandle>)
            })
        }
    }

    #[tokio::test]
    async fn retries_with_backoff_then_connects() {
        let attempts = Arc::new(AtomicU32::new(0));
        let (close_tx, _close_rx) = watch::channel(false);
        let connect = failing_connect(3, Arc::clone(&attempts), close_tx.clone());
        let (status_tx, mut status_rx) = watch::channel(TunnelLinkStatus::Idle);
        let cancellation = CancellationToken::new();
        let mut seen = Vec::new();
        let task = tokio::spawn(run_reconnect_loop(
            connect,
            || false,
            policy(),
            cancellation.clone(),
            status_tx,
            None,
        ));

        // The loop must surface Reconnecting with growing delays before the
        // eventual Connected.
        while seen.last().copied() != Some(TunnelLinkStatus::Connected) {
            tokio::select! {
                _ = status_rx.changed() => {
                    let status = *status_rx.borrow();
                    seen.push(status);
                }
                _ = tokio::time::sleep(Duration::from_secs(5)) => break,
            }
        }

        cancellation.cancel();
        let _ = task.await;
        assert_eq!(attempts.load(Ordering::SeqCst), 4);
        assert!(seen
            .iter()
            .any(|s| matches!(s, TunnelLinkStatus::Reconnecting { attempt: 0, .. })));
        assert!(seen
            .iter()
            .any(|s| matches!(s, TunnelLinkStatus::Reconnecting { attempt: 1, .. })));
        assert!(seen.iter().any(|s| *s == TunnelLinkStatus::Connected));
    }

    #[tokio::test]
    async fn link_drop_triggers_reconnect_and_eventual_recovery() {
        let attempts = Arc::new(AtomicU32::new(0));
        let (close_tx, _close_rx) = watch::channel(false);
        let connect = failing_connect(0, Arc::clone(&attempts), close_tx.clone());
        let (status_tx, mut status_rx) = watch::channel(TunnelLinkStatus::Idle);
        let cancellation = CancellationToken::new();
        let task = tokio::spawn(run_reconnect_loop(
            connect,
            || false,
            policy(),
            cancellation.clone(),
            status_tx,
            None,
        ));

        // Wait for the first Connected.
        loop {
            tokio::select! {
                _ = status_rx.changed() => {
                    if *status_rx.borrow() == TunnelLinkStatus::Connected { break; }
                }
                _ = tokio::time::sleep(Duration::from_secs(5)) => panic!("never connected"),
            }
        }

        // Drop the link; the loop must report Reconnecting and then reconnect.
        let _ = close_tx.send(true);
        let mut reconnected = false;
        for _ in 0..20 {
            tokio::select! {
                _ = status_rx.changed() => {
                    if *status_rx.borrow() == TunnelLinkStatus::Connected {
                        reconnected = true;
                        break;
                    }
                }
                _ = tokio::time::sleep(Duration::from_secs(5)) => break,
            }
        }

        cancellation.cancel();
        let _ = task.await;
        assert!(reconnected, "link must reconnect after drop");
        assert!(attempts.load(Ordering::SeqCst) >= 2);
    }

    #[tokio::test]
    async fn expired_tunnel_never_reconnects() {
        let attempts = Arc::new(AtomicU32::new(0));
        let (close_tx, _close_rx) = watch::channel(false);
        let connect = failing_connect(u32::MAX, Arc::clone(&attempts), close_tx.clone());
        let (status_tx, mut status_rx) = watch::channel(TunnelLinkStatus::Idle);
        let cancellation = CancellationToken::new();
        let task = tokio::spawn(run_reconnect_loop(
            connect,
            || true, // always expired
            policy(),
            cancellation.clone(),
            status_tx,
            None,
        ));

        let mut stopped = false;
        for _ in 0..10 {
            tokio::select! {
                _ = status_rx.changed() => {
                    if *status_rx.borrow() == TunnelLinkStatus::Stopped {
                        stopped = true;
                        break;
                    }
                }
                _ = tokio::time::sleep(Duration::from_secs(5)) => break,
            }
        }

        cancellation.cancel();
        let _ = task.await;
        assert!(stopped, "expired tunnel must stop, not retry");
        assert_eq!(attempts.load(Ordering::SeqCst), 0, "no connect attempted");
    }

    #[tokio::test]
    async fn cancellation_stops_the_loop() {
        let attempts = Arc::new(AtomicU32::new(0));
        let (close_tx, _close_rx) = watch::channel(false);
        let connect = failing_connect(u32::MAX, Arc::clone(&attempts), close_tx.clone());
        let (status_tx, mut status_rx) = watch::channel(TunnelLinkStatus::Idle);
        let cancellation = CancellationToken::new();
        let task = tokio::spawn(run_reconnect_loop(
            connect,
            || false,
            policy(),
            cancellation.clone(),
            status_tx,
            None,
        ));

        cancellation.cancel();
        let _ = task.await;
        let mut stopped = false;
        for _ in 0..10 {
            tokio::select! {
                _ = status_rx.changed() => {
                    if *status_rx.borrow() == TunnelLinkStatus::Stopped {
                        stopped = true;
                        break;
                    }
                }
                _ = tokio::time::sleep(Duration::from_secs(2)) => break,
            }
        }
        assert!(stopped, "cancellation must produce Stopped");
    }
}
