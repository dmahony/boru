//! Dialing: the dial machine (per-peer dial state, stale-dial cleanup) and
//! the transport-selected connect used to establish peer connections.

use std::collections::HashMap;

use bytes::Bytes;
use iroh::{endpoint::Connection, Endpoint, EndpointAddr, EndpointId};
use n0_future::{
    task::JoinSet,
    time::{Duration, Instant},
};
use tokio_util::sync::CancellationToken;
use tracing::{debug, error, info, warn, Instrument};

use super::connectivity::select_transport;

// Direct (5s) then relay (10s) must be allowed to finish normally.
const STALE_DIAL_THRESHOLD_S: u64 = 30;

#[derive(Debug)]
pub(super) enum DialOutcome {
    Connected(Connection),
    Failed(iroh::endpoint::ConnectError),
    TimedOut,
    Cancelled,
    TaskFailed,
}

/// Only absence of a route or an unanswered handshake is normal discovery
/// unavailability. Authentication, protocol, endpoint and task failures stay
/// actionable; do not classify them by matching error text.
pub(super) fn is_discovery_unavailable(err: &iroh::endpoint::ConnectError) -> bool {
    use iroh::endpoint::{ConnectError, ConnectWithOptsError, ConnectingError, ConnectionError};
    matches!(err,
        ConnectError::Connect { source: ConnectWithOptsError::NoAddress { .. }, .. }
        | ConnectError::Connection { source: ConnectionError::TimedOut, .. }
        | ConnectError::Connecting {
            source: ConnectingError::ConnectionError { source: ConnectionError::TimedOut, .. }, ..
        }
    )
}

#[derive(Debug)]
pub(super) struct Dialer {
    endpoint: Endpoint,
    pending: JoinSet<(EndpointId, DialOutcome)>,
    /// In-flight dials keyed by peer id. Each entry stores the cancellation
    /// token used to abort the dial and the original address we dialed, so
    /// retries can preserve relay/direct addresses instead of falling back to
    /// a bare peer id.
    pending_dials: HashMap<EndpointId, (CancellationToken, EndpointAddr)>,

    /// When each dial was started, for stale-dial detection.
    dial_start_times: HashMap<EndpointId, Instant>,
}

impl Dialer {
    /// Create a new dialer for a [`Endpoint`]
    pub(super) fn new(endpoint: Endpoint) -> Self {
        Self {
            endpoint,
            pending: Default::default(),
            pending_dials: Default::default(),

            dial_start_times: Default::default(),
        }
    }

    /// Starts to dial a endpoint using direct addresses first, then relay.
    pub(super) fn queue_dial(&mut self, endpoint_addr: EndpointAddr, alpn: Bytes) {
        let endpoint_id = endpoint_addr.id;
        if self.is_pending(endpoint_id) {
            return;
        }
        let cancel = CancellationToken::new();
        self.pending_dials
            .insert(endpoint_id, (cancel.clone(), endpoint_addr.clone()));
        self.dial_start_times.insert(endpoint_id, Instant::now());
        let endpoint = self.endpoint.clone();
        info!(peer = %endpoint_id.fmt_short(), "queue dial");
        self.pending.spawn(
            async move {
                let result = dial_endpoint(endpoint, endpoint_addr, alpn, cancel).await;
                (endpoint_id, result)
            }
            .instrument(tracing::Span::current()),
        );
    }

    /// Checks if a endpoint is currently being dialed.
    pub(super) fn is_pending(&self, endpoint: EndpointId) -> bool {
        self.pending_dials.contains_key(&endpoint)
    }

    /// Keep cancelled entries until consumed: old completions must not retire
    /// replacement dials, and callers need the original address for retries.
    pub(super) fn cancel(&self, peer: EndpointId) {
        if let Some((cancel, _)) = self.pending_dials.get(&peer) {
            cancel.cancel();
        }
    }

    /// Cancel only stale peers, not unrelated in-flight connections.
    pub(super) fn cleanup_stale_dials(&mut self) {
        let now = Instant::now();
        let threshold = Duration::from_secs(STALE_DIAL_THRESHOLD_S);
        let stale: Vec<EndpointId> = self
            .dial_start_times
            .iter()
            .filter(|(_, &start)| now.duration_since(start) > threshold)
            .map(|(k, _)| *k)
            .collect();
        for peer_id in &stale {
            warn!(peer = %peer_id.fmt_short(), "dial exceeded path timeout budget; cancelling stalled operation");
            self.cancel(*peer_id);
            self.dial_start_times.remove(peer_id);
        }
    }

    /// Return the original address with the outcome before retiring state.
    pub(super) async fn next_conn(&mut self) -> (EndpointAddr, DialOutcome) {
        loop {
            match self.pending.join_next().await {
                Some(Ok((peer, outcome))) => {
                    self.dial_start_times.remove(&peer);
                    if let Some((cancel, addr)) = self.pending_dials.remove(&peer) {
                        if cancel.is_cancelled() {
                            if let DialOutcome::Connected(conn) = outcome {
                                conn.close(0u32.into(), b"dial no longer needed");
                            }
                            return (addr, DialOutcome::Cancelled);
                        }
                        return (addr, outcome);
                    }
                }
                Some(Err(err)) => error!(%err, "gossip dial task failed"),
                None => {
                    // Task panics/aborts can leave orphaned entries. Recover
                    // them as operational failures, never as disconnects.
                    if let Some(peer) = self.pending_dials.keys().next().copied() {
                        self.dial_start_times.remove(&peer);
                        let (_, addr) = self.pending_dials.remove(&peer).unwrap();
                        return (addr, DialOutcome::TaskFailed);
                    }
                    std::future::pending::<()>().await;
                }
            }
        }
    }
}

/// Bounded connection attempts own their futures: cancellation and timeout
/// cannot leave detached tasks dialing in the background.
async fn dial_endpoint(
    endpoint: Endpoint,
    endpoint_addr: EndpointAddr,
    alpn: Bytes,
    cancel: CancellationToken,
) -> DialOutcome {
    let peer_id = endpoint_addr.id;
    let selected = select_transport(&endpoint_addr);
    debug!(peer = %peer_id.fmt_short(), ?selected, "select transport");

    let direct_addr = endpoint_addr.ip_addrs().next().map(|_| {
        endpoint_addr
            .ip_addrs()
            .fold(EndpointAddr::new(peer_id), |addr, ip| {
                addr.with_ip_addr(*ip)
            })
    });
    let relay_addr = endpoint_addr.relay_urls().next().map(|_| {
        endpoint_addr
            .relay_urls()
            .fold(EndpointAddr::new(peer_id), |addr, relay| {
                addr.with_relay_url(relay.clone())
            })
    });

    if let Some(addr) = direct_addr {
        info!(peer = %peer_id.fmt_short(), "connecting via direct transport");
        let result = await_connect(
            endpoint.connect(addr, &alpn),
            &cancel,
            Duration::from_secs(5),
        )
        .await;
        match result {
            DialOutcome::Failed(_) | DialOutcome::TimedOut if relay_addr.is_some() => {
                debug!(peer = %peer_id.fmt_short(), ?result, "direct attempt failed; trying relay");
            }
            _ => return result,
        }
    }

    if let Some(addr) = relay_addr {
        info!(peer = %peer_id.fmt_short(), "connecting via relay transport");
        await_connect(
            endpoint.connect(addr, &alpn),
            &cancel,
            Duration::from_secs(10),
        )
        .await
    } else {
        info!(peer = %peer_id.fmt_short(), "no usable address; connecting by peer id only");
        await_connect(
            endpoint.connect(peer_id, &alpn),
            &cancel,
            Duration::from_secs(10),
        )
        .await
    }
}

async fn await_connect(
    connect: impl std::future::Future<Output = Result<Connection, iroh::endpoint::ConnectError>>,
    cancel: &CancellationToken,
    timeout: Duration,
) -> DialOutcome {
    tokio::select! {
        biased;
        _ = cancel.cancelled() => DialOutcome::Cancelled,
        result = tokio::time::timeout(timeout, connect) => match result {
            Ok(Ok(conn)) => DialOutcome::Connected(conn),
            Ok(Err(err)) => DialOutcome::Failed(err),
            Err(_) => DialOutcome::TimedOut,
        }
    }
}

#[cfg(test)]
mod regression_tests {
    use super::*;
    use std::sync::{Arc, atomic::{AtomicBool, Ordering}};

    struct PendingConnect(Arc<AtomicBool>);

    impl std::future::Future for PendingConnect {
        type Output = Result<Connection, iroh::endpoint::ConnectError>;
        fn poll(self: std::pin::Pin<&mut Self>, _: &mut std::task::Context<'_>) -> std::task::Poll<Self::Output> {
            std::task::Poll::Pending
        }
    }

    impl Drop for PendingConnect {
        fn drop(&mut self) {
            self.0.store(true, Ordering::SeqCst);
        }
    }

    #[tokio::test]
    async fn timeout_drops_connect_future_instead_of_detaching() {
        let dropped = Arc::new(AtomicBool::new(false));
        let outcome = await_connect(PendingConnect(dropped.clone()), &CancellationToken::new(), Duration::from_millis(1)).await;
        assert!(matches!(outcome, DialOutcome::TimedOut));
        assert!(dropped.load(Ordering::SeqCst));
    }

    #[tokio::test]
    async fn cancellation_drops_connect_future_without_failure() {
        let dropped = Arc::new(AtomicBool::new(false));
        let cancel = CancellationToken::new();
        cancel.cancel();
        let outcome = await_connect(PendingConnect(dropped.clone()), &cancel, Duration::from_secs(60)).await;
        assert!(matches!(outcome, DialOutcome::Cancelled));
        assert!(dropped.load(Ordering::SeqCst));
    }

    #[test]
    fn offline_timeout_is_distinct_from_operational_error() {
        use iroh::endpoint::{ConnectError, ConnectWithOptsError, ConnectionError};
        let timeout = ConnectError::from(ConnectionError::TimedOut);
        assert!(is_discovery_unavailable(&timeout));
        let closed = ConnectError::from(n0_error::e!(ConnectWithOptsError::EndpointClosed));
        assert!(!is_discovery_unavailable(&closed));
    }

    #[tokio::test]
    async fn cancellation_keeps_slot_until_completion_then_allows_new_dial() {
        let endpoint = Endpoint::builder(iroh::endpoint::presets::Minimal).bind().await.unwrap();
        let mut dialer = Dialer::new(endpoint.clone());
        let peer = iroh::SecretKey::generate().public();
        let addr = EndpointAddr::new(peer);
        dialer.queue_dial(addr.clone(), Bytes::from_static(b"test"));
        dialer.cancel(peer);
        // A second request cannot replace state while an old completion exists.
        dialer.queue_dial(addr.clone(), Bytes::from_static(b"test"));
        assert_eq!(dialer.pending.len(), 1);
        let (returned, outcome) = tokio::time::timeout(Duration::from_secs(2), dialer.next_conn()).await.unwrap();
        assert_eq!(returned, addr);
        assert!(matches!(outcome, DialOutcome::Cancelled));
        assert!(!dialer.is_pending(peer));
        dialer.queue_dial(addr, Bytes::from_static(b"test"));
        assert!(dialer.is_pending(peer));
        dialer.cancel(peer);
        let _ = dialer.next_conn().await;
        endpoint.close().await;
    }

    #[tokio::test]
    async fn task_failure_recovers_address_without_reporting_disconnect() {
        let endpoint = Endpoint::builder(iroh::endpoint::presets::Minimal).bind().await.unwrap();
        let mut dialer = Dialer::new(endpoint.clone());
        let peer = iroh::SecretKey::generate().public();
        let addr = EndpointAddr::new(peer).with_ip_addr("127.0.0.1:12345".parse().unwrap());
        dialer.pending_dials.insert(peer, (CancellationToken::new(), addr.clone()));
        dialer.pending.spawn(async { panic!("injected dial task failure") });
        let (returned, outcome) = tokio::time::timeout(Duration::from_secs(2), dialer.next_conn()).await.unwrap();
        assert_eq!(returned, addr);
        assert!(matches!(outcome, DialOutcome::TaskFailed));
        assert!(!dialer.is_pending(peer));
        endpoint.close().await;
    }

    #[tokio::test]
    async fn completion_preserves_retry_address_and_retires_pending() {
        let endpoint = Endpoint::builder(iroh::endpoint::presets::Minimal).bind().await.unwrap();
        let mut dialer = Dialer::new(endpoint.clone());
        let peer = iroh::SecretKey::generate().public();
        let addr = EndpointAddr::new(peer)
            .with_ip_addr("127.0.0.1:12345".parse().unwrap())
            .with_relay_url("https://relay.example.test".parse().unwrap());
        dialer.pending_dials.insert(peer, (CancellationToken::new(), addr.clone()));
        dialer.dial_start_times.insert(peer, Instant::now());
        dialer.pending.spawn(async move { (peer, DialOutcome::TimedOut) });
        let (returned, outcome) = dialer.next_conn().await;
        assert_eq!(returned, addr);
        assert!(matches!(outcome, DialOutcome::TimedOut));
        assert!(!dialer.is_pending(peer));
        assert!(!dialer.dial_start_times.contains_key(&peer));
        endpoint.close().await;
    }

    #[tokio::test]
    async fn stale_cleanup_only_cancels_stale_peer_and_keeps_addresses() {
        let endpoint = Endpoint::builder(iroh::endpoint::presets::Minimal).bind().await.unwrap();
        let mut dialer = Dialer::new(endpoint.clone());
        let stale = iroh::SecretKey::generate().public();
        let fresh = iroh::SecretKey::generate().public();
        for peer in [stale, fresh] {
            dialer.pending_dials.insert(peer, (CancellationToken::new(), EndpointAddr::new(peer)));
            dialer.dial_start_times.insert(peer, Instant::now());
        }
        dialer.dial_start_times.insert(stale, Instant::now() - Duration::from_secs(31));
        dialer.cleanup_stale_dials();
        assert!(dialer.pending_dials[&stale].0.is_cancelled());
        assert!(!dialer.pending_dials[&fresh].0.is_cancelled());
        assert!(dialer.is_pending(stale));
        assert!(dialer.is_pending(fresh));
        assert!(dialer.dial_start_times.contains_key(&fresh));
        endpoint.close().await;
    }
}
