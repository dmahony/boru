//! Discovery-ordered broadcast gate for conversation senders.
//!
//! Call [`DiscoveryOrderedSender::note_discovery`] synchronously when discovery is
//! applied, then use [`DiscoveryOrderedSender::broadcast`] for sends. The gate
//! joins every pending discovery batch before forwarding a broadcast; join
//! failures are returned and the broadcast is not attempted.

use std::{future::Future, pin::Pin, sync::{Arc, Mutex}};

use bytes::Bytes;
use iroh_base::EndpointId;

use boru_core::api::GossipSender;

type BoxFuture<'a> = Pin<Box<dyn Future<Output = Result<(), String>> + Send + 'a>>;

trait SenderOps: Send + Sync {
    fn join<'a>(&'a self, peers: Vec<EndpointId>) -> BoxFuture<'a>;
    fn broadcast<'a>(&'a self, message: Bytes) -> BoxFuture<'a>;
}

struct GossipOps(GossipSender);

impl SenderOps for GossipOps {
    fn join<'a>(&'a self, peers: Vec<EndpointId>) -> BoxFuture<'a> {
        Box::pin(async move { self.0.join_peers(peers).await.map_err(|error| error.to_string()) })
    }

    fn broadcast<'a>(&'a self, message: Bytes) -> BoxFuture<'a> {
        Box::pin(async move { self.0.broadcast(message).await.map_err(|error| error.to_string()) })
    }
}

#[derive(Default)]
struct DiscoveryState {
    generation: u64,
    joined_generation: u64,
    pending: Vec<EndpointId>,
}

struct Inner {
    ops: Arc<dyn SenderOps>,
    state: Mutex<DiscoveryState>,
    gate: tokio::sync::Mutex<()>,
}

/// Cloneable conversation sender that serializes pending joins before broadcasts.
///
/// Create with [`new`](Self::new), call [`note_discovery`](Self::note_discovery)
/// synchronously for each newly discovered peer batch, and replace direct
/// broadcasts with [`broadcast`](Self::broadcast). Concurrent broadcasts share
/// one gate, so a discovery batch is joined once before any gated send proceeds.
#[derive(Clone)]
pub(crate) struct DiscoveryOrderedSender(Arc<Inner>);

impl DiscoveryOrderedSender {
    /// Wrap an existing gossip sender with the discovery ordering gate.
    pub(crate) fn new(sender: GossipSender) -> Self {
        Self::with_ops(Arc::new(GossipOps(sender)))
    }

    fn with_ops(ops: Arc<dyn SenderOps>) -> Self {
        Self(Arc::new(Inner { ops, state: Mutex::new(DiscoveryState::default()), gate: tokio::sync::Mutex::new(()) }))
    }

    /// Record peers synchronously when the discovery event is applied.
    /// The next broadcast must complete their join before it can be sent.
    pub(crate) fn note_discovery(&self, peers: impl IntoIterator<Item = EndpointId>) {
        let mut state = self.0.state.lock().expect("discovery state poisoned");
        let mut changed = false;
        for peer in peers {
            if !state.pending.contains(&peer) {
                state.pending.push(peer);
                changed = true;
            }
        }
        if changed {
            state.generation = state.generation.wrapping_add(1);
        }
    }

    /// Join all discoveries registered so far, then broadcast; fails closed on join error.
    pub(crate) async fn broadcast(&self, message: Bytes) -> Result<(), String> {
        let _gate = self.0.gate.lock().await;
        let (generation, peers) = {
            let state = self.0.state.lock().expect("discovery state poisoned");
            if state.joined_generation == state.generation {
                (state.generation, Vec::new())
            } else {
                (state.generation, state.pending.clone())
            }
        };
        if !peers.is_empty() {
            self.0.ops.join(peers.clone()).await?;
            let mut state = self.0.state.lock().expect("discovery state poisoned");
            state.pending.retain(|peer| !peers.contains(peer));
            state.joined_generation = generation;
        }
        self.0.ops.broadcast(message).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{collections::HashSet, sync::atomic::{AtomicUsize, Ordering}};

    struct MockOps {
        joins: AtomicUsize,
        sent: AtomicUsize,
        events: Mutex<Vec<&'static str>>,
        fail_join: bool,
    }

    impl SenderOps for MockOps {
        fn join<'a>(&'a self, _peers: Vec<EndpointId>) -> BoxFuture<'a> {
            Box::pin(async move {
                self.joins.fetch_add(1, Ordering::SeqCst);
                self.events.lock().unwrap().push("join");
                if self.fail_join { Err("join failed".to_owned()) } else { Ok(()) }
            })
        }
        fn broadcast<'a>(&'a self, _message: Bytes) -> BoxFuture<'a> {
            Box::pin(async move {
                self.sent.fetch_add(1, Ordering::SeqCst);
                self.events.lock().unwrap().push("broadcast");
                Ok(())
            })
        }
    }

    fn mock(fail_join: bool) -> (DiscoveryOrderedSender, Arc<MockOps>) {
        let ops = Arc::new(MockOps { joins: AtomicUsize::new(0), sent: AtomicUsize::new(0), events: Mutex::new(Vec::new()), fail_join });
        (DiscoveryOrderedSender::with_ops(ops.clone()), ops)
    }

    fn peer() -> EndpointId { iroh::SecretKey::generate().public() }

    #[tokio::test]
    async fn discovery_after_handler_return_is_joined_before_broadcast() {
        let (sender, ops) = mock(false);
        async {}.await; // Represents an awaited handler returning before discovery is applied.
        sender.note_discovery([peer()]);
        sender.broadcast(Bytes::new()).await.unwrap();
        assert_eq!(*ops.events.lock().unwrap(), ["join", "broadcast"]);
    }

    #[tokio::test]
    async fn concurrent_sends_join_one_discovery_batch_once() {
        let (sender, ops) = mock(false);
        sender.note_discovery([peer()]);
        let a = sender.clone();
        let b = sender.clone();
        let (ra, rb) = tokio::join!(a.broadcast(Bytes::new()), b.broadcast(Bytes::new()));
        ra.unwrap(); rb.unwrap();
        assert_eq!(ops.joins.load(Ordering::SeqCst), 1);
        assert_eq!(ops.sent.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn failed_join_fails_closed_without_broadcast() {
        let (sender, ops) = mock(true);
        sender.note_discovery([peer()]);
        assert!(sender.broadcast(Bytes::new()).await.is_err());
        assert_eq!(ops.sent.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn no_discovery_does_not_join_and_discovery_batches_are_deduplicated() {
        let (sender, ops) = mock(false);
        sender.broadcast(Bytes::new()).await.unwrap();
        sender.broadcast(Bytes::new()).await.unwrap();
        assert_eq!(ops.joins.load(Ordering::SeqCst), 0);
        sender.note_discovery([peer(), peer()]);
        sender.broadcast(Bytes::new()).await.unwrap();
        sender.broadcast(Bytes::new()).await.unwrap();
        assert_eq!(ops.joins.load(Ordering::SeqCst), 1);
        let unique: HashSet<_> = [peer()].into_iter().collect();
        assert_eq!(unique.len(), 1);
    }
}
