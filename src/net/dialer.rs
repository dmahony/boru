//! Dialing: the dial machine (per-peer dial state, stale-dial cleanup) and
//! the transport-selected connect used to establish peer connections.

use std::collections::{HashMap, VecDeque};

use bytes::Bytes;
use iroh::{endpoint::Connection, Endpoint, EndpointAddr, EndpointId};
use n0_future::{
    task::JoinSet,
    time::{Duration, Instant},
};
use tokio_util::sync::CancellationToken;
use tracing::{debug, error, info, warn, Instrument};

use super::connectivity::select_transport;

const STALE_DIAL_THRESHOLD_S: u64 = 15;

#[derive(Debug)]
pub(super) struct Dialer {
    endpoint: Endpoint,
    pending: JoinSet<(
        EndpointId,
        Option<Result<Connection, iroh::endpoint::ConnectError>>,
    )>,
    /// In-flight dials keyed by peer id. Each entry stores the cancellation
    /// token used to abort the dial and the original address we dialed, so
    /// retries can preserve relay/direct addresses instead of falling back to
    /// a bare peer id.
    pending_dials: HashMap<EndpointId, (CancellationToken, EndpointAddr)>,
    /// Peers whose dial tasks were aborted due to JoinSet timeout or
    /// exhaustion.  They are drained one-by-one as `None` (disconnected)
    /// so the caller can trigger retry logic for each.
    pub(super) aborted_peers: VecDeque<EndpointId>,
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
            aborted_peers: VecDeque::new(),
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

    /// Return the original address used for a pending dial, if any.
    pub(super) fn pending_addr(&self, endpoint_id: EndpointId) -> Option<EndpointAddr> {
        self.pending_dials
            .get(&endpoint_id)
            .map(|(_, addr)| addr.clone())
    }

    /// Aborts dials that have been pending longer than STALE_DIAL_THRESHOLD_S.
    /// Returns true if any stale dials were found and aborted.
    pub(super) fn cleanup_stale_dials(&mut self) -> bool {
        let now = Instant::now();
        let threshold = Duration::from_secs(STALE_DIAL_THRESHOLD_S);
        let stale: Vec<EndpointId> = self
            .dial_start_times
            .iter()
            .filter(|(_, &start)| now.duration_since(start) > threshold)
            .map(|(k, _)| *k)
            .collect();
        if stale.is_empty() {
            return false;
        }
        warn!(
            "found {} stale dials (>{:?}s), aborting",
            stale.len(),
            threshold
        );
        for peer_id in &stale {
            self.pending_dials.remove(peer_id);
            self.dial_start_times.remove(peer_id);
        }
        // Abort all tasks — the stale ones can't be selectively aborted
        // from a JoinSet, so we abort everything and queue all pending
        // peers as aborted.
        self.pending.abort_all();
        let remaining: Vec<_> = self.dial_start_times.drain().map(|(k, _)| k).collect();
        self.aborted_peers.extend(stale);
        self.aborted_peers.extend(remaining);
        true
    }

    /// Waits for the next dial operation to complete.
    /// `None` means disconnected
    pub(super) async fn next_conn(
        &mut self,
    ) -> (
        EndpointId,
        Option<Result<Connection, iroh::endpoint::ConnectError>>,
    ) {
        // First, drain any peers that were aborted by a JoinSet timeout.
        // Returning them as `None` (disconnected) lets the caller fire
        // retry logic instead of blocking forever.
        if let Some(peer_id) = self.aborted_peers.pop_front() {
            return (peer_id, None);
        }
        match self.pending_dials.is_empty() {
            false => {
                let (endpoint_id, res) = loop {
                    // Timeout join_next so a hung dial task doesn't block the
                    // gossip actor's event loop forever.
                    match n0_future::time::timeout(
                        Duration::from_secs(20),
                        self.pending.join_next(),
                    )
                    .await
                    {
                        Ok(Some(Ok((endpoint_id, res)))) => {
                            self.pending_dials.remove(&endpoint_id);
                            self.dial_start_times.remove(&endpoint_id);
                            break (endpoint_id, res);
                        }
                        Ok(Some(Err(e))) => {
                            error!("next conn error: {:?}", e);
                            continue;
                        }
                        Ok(None) => {
                            // JoinSet is empty but pending_dials wasn't —
                            // this means tasks were aborted externally.
                            // Collect all pending peer IDs and return them
                            // as disconnected so retries fire.
                            let aborted: Vec<_> =
                                self.pending_dials.drain().map(|(k, _)| k).collect();
                            if aborted.is_empty() {
                                // Nothing to recover – wait for new dials.
                                std::future::pending().await
                            } else {
                                let first = aborted[0];
                                self.aborted_peers.extend(aborted.into_iter().skip(1));
                                break (first, None);
                            }
                        }
                        Err(_) => {
                            // join_next timed out – the hung dial tasks are
                            // still in the JoinSet.  Abort them all and
                            // return each pending peer as disconnected.
                            warn!("dial join_next timed out, abandoning stale dials");
                            self.pending.abort_all();
                            let aborted: Vec<_> =
                                self.pending_dials.drain().map(|(k, _)| k).collect();
                            if aborted.is_empty() {
                                // No pending dials to recover – wait for new ones.
                                std::future::pending().await
                            } else {
                                let first = aborted[0];
                                self.aborted_peers.extend(aborted.into_iter().skip(1));
                                break (first, None);
                            }
                        }
                    }
                };

                (endpoint_id, res)
            }
            true => std::future::pending().await,
        }
    }
}

/// Perform a single endpoint connection attempt with hard timeouts and
/// explicit task-per-path cancellation.
///
/// `endpoint.connect()` can block the async task internally if the QUIC
/// handshake or relay setup stalls. By spawning each path attempt as its own
/// `tokio::task` and awaiting the `JoinHandle` with `tokio::time::timeout`,
/// we can abandon a hung connect and either fall back to the next path or
/// return `None` so the caller retries.
async fn dial_endpoint(
    endpoint: Endpoint,
    endpoint_addr: EndpointAddr,
    alpn: Bytes,
    cancel: CancellationToken,
) -> Option<Result<Connection, iroh::endpoint::ConnectError>> {
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

    // Helper to await a spawned connect task with a timeout and cancellation.
    async fn await_connect_task(
        task: tokio::task::JoinHandle<Result<Connection, iroh::endpoint::ConnectError>>,
        cancel: &CancellationToken,
        timeout: Duration,
        peer_id: EndpointId,
        path: &str,
    ) -> Option<Result<Connection, iroh::endpoint::ConnectError>> {
        let mut task = Some(task);
        tokio::select! {
            biased;
            _ = cancel.cancelled() => {
                if let Some(t) = task.take() {
                    t.abort();
                }
                None
            }
            res = tokio::time::timeout(timeout, task.take().expect("task is always Some here")) => {
                match res {
                    // Timeout fired: task is still stuck.
                    Err(_) => {
                        info!(peer = %peer_id.fmt_short(), path, "connect timed out");
                        None
                    }
                    // Task completed; inspect its result.
                    Ok(Ok(connect_result)) => Some(connect_result),
                    Ok(Err(join_err)) if join_err.is_cancelled() => None,
                    Ok(Err(join_err)) => {
                        warn!(peer = %peer_id.fmt_short(), path, "connect task panicked: {join_err}");
                        None
                    }
                }
            }
        }
    }

    if let Some(addr) = direct_addr {
        info!(peer = %peer_id.fmt_short(), "connecting via direct transport");
        let direct_task = tokio::spawn({
            let endpoint = endpoint.clone();
            let alpn = alpn.clone();
            async move { endpoint.connect(addr, &alpn).await }
        });
        if let Some(result) = await_connect_task(
            direct_task,
            &cancel,
            Duration::from_secs(5),
            peer_id,
            "direct",
        )
        .await
        {
            match &result {
                Ok(_) => info!(peer = %peer_id.fmt_short(), "direct connect succeeded"),
                Err(err) if relay_addr.is_some() => {
                    info!(peer = %peer_id.fmt_short(), "direct connect failed; falling back to relay: {err}");
                }
                Err(_) => return Some(result),
            }
            if result.is_ok() {
                return Some(result);
            }
        }
    }

    if let Some(addr) = relay_addr {
        info!(peer = %peer_id.fmt_short(), "connecting via relay transport");
        let relay_task = tokio::spawn({
            let endpoint = endpoint.clone();
            async move { endpoint.connect(addr, &alpn).await }
        });
        await_connect_task(
            relay_task,
            &cancel,
            Duration::from_secs(10),
            peer_id,
            "relay",
        )
        .await
    } else {
        info!(peer = %peer_id.fmt_short(), "no usable address; connecting by peer id only");
        let bare_task = tokio::spawn(async move { endpoint.connect(peer_id, &alpn).await });
        await_connect_task(
            bare_task,
            &cancel,
            Duration::from_secs(10),
            peer_id,
            "bare-id",
        )
        .await
    }
}
