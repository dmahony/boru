//! Networking for the `Boru` protocol.
//!
//! This module is a facade over the gossip engine:
//! - [`protocol`]    – protocol registration (ALPNs, message/event aliases)
//! - [`actor`]       – the gossip actor lifecycle and event loop
//! - [`peer`]        – peer & topic state management
//! - [`connectivity`]– relay/direct per-connection transport loops
//! - [`dialer`]      – peer dialing and stale-dial cleanup
//! - [`topic`]       – topic subscription forwarding / command streams
//!
//! The public surface is [`Gossip`] (with its [`Builder`]) and the ALPN
//! constants [`GOSSIP_ALPN`] / [`FILE_ACCESS_ALPN`].

mod actor;
mod address_lookup;
mod address_resolution;
mod connectivity;
mod dialer;
mod peer;
mod protocol;
mod topic;
mod util;

#[cfg(test)]
pub(crate) mod tests;

pub use protocol::{ProtoCommand, ProtoEvent, FILE_ACCESS_ALPN, GOSSIP_ALPN};

use std::sync::{Arc, Mutex};

use bytes::Bytes;
use iroh::{
    endpoint::Connection,
    protocol::{AcceptError, ProtocolHandler},
    Endpoint, EndpointAddr,
};
use n0_error::{e, stack_error};
use n0_future::task::{self, AbortOnDropHandle};
use tokio::sync::{mpsc, oneshot};
use tracing::{error_span, warn, Instrument};

use self::actor::Actor;
use self::address_lookup::GossipAddressLookup;
use crate::{
    api::GossipApi,
    friends::FriendsStore,
    metrics::Metrics,
    proto::{self, HyparviewConfig, PlumtreeConfig},
};

/// Publish and subscribe on gossiping topics.
///
/// Each topic is a separate broadcast tree with separate memberships.
///
/// A topic has to be joined before you can publish or subscribe on the topic.
/// To join the swarm for a topic, you have to know the [`PublicKey`] of at least one peer that also joined the topic.
///
/// Messages published on the swarm will be delivered to all peers that joined the swarm for that
/// topic. You will also be relaying (gossiping) messages published by other peers.
///
/// With the default settings, the protocol will maintain up to 5 peer connections per topic.
///
/// Even though the [`Gossip`] is created from a [`Endpoint`], it does not accept connections
/// itself. You should run an accept loop on the [`Endpoint`] yourself, check the ALPN protocol of incoming
/// connections, and if the ALPN protocol equals [`GOSSIP_ALPN`], forward the connection to the
/// gossip actor through [Self::handle_connection].
///
/// The gossip actor will, however, initiate new connections to other peers by itself.
#[derive(Debug, Clone)]
pub struct Gossip {
    pub(crate) inner: Arc<Inner>,
}

impl std::ops::Deref for Gossip {
    type Target = GossipApi;
    fn deref(&self) -> &Self::Target {
        &self.inner.api
    }
}
#[derive(Debug)]
enum LocalActorMessage {
    Shutdown {
        reply: oneshot::Sender<()>,
    },
    HandleConnection(Connection),
    RetryDial(EndpointAddr, Bytes),
    /// Periodic stale-dial cleanup trigger from the spawned timer task.
    CleanupStaleDials,
}

#[allow(missing_docs)]
#[stack_error(derive, add_meta)]
#[non_exhaustive]
pub enum Error {
    ActorDropped {},
}

impl<T> From<mpsc::error::SendError<T>> for Error {
    fn from(_value: mpsc::error::SendError<T>) -> Self {
        e!(Error::ActorDropped)
    }
}
impl From<oneshot::error::RecvError> for Error {
    fn from(_value: oneshot::error::RecvError) -> Self {
        e!(Error::ActorDropped)
    }
}

#[derive(Debug)]
pub(crate) struct Inner {
    api: GossipApi,
    local_tx: mpsc::Sender<LocalActorMessage>,
    _actor_handle: AbortOnDropHandle<()>,
    max_message_size: usize,
    metrics: Arc<Metrics>,
}

impl ProtocolHandler for Gossip {
    async fn accept(&self, connection: Connection) -> Result<(), AcceptError> {
        self.handle_connection(connection)
            .await
            .map_err(AcceptError::from_err)?;
        Ok(())
    }

    async fn shutdown(&self) {
        if let Err(err) = self.shutdown().await {
            warn!("error while shutting down gossip: {err:#}");
        }
    }
}

/// Builder to configure and construct [`Gossip`].
#[derive(Debug, Clone)]
pub struct Builder {
    config: proto::Config,
    alpn: Option<Bytes>,
    friends: Option<Arc<Mutex<FriendsStore>>>,
}

impl Builder {
    /// Sets the maximum message size in bytes.
    /// By default this is `4096` bytes.
    pub fn max_message_size(mut self, size: usize) -> Self {
        self.config.max_message_size = size;
        self
    }

    /// Set the membership configuration.
    pub fn membership_config(mut self, config: HyparviewConfig) -> Self {
        self.config.membership = config;
        self
    }

    /// Set the broadcast configuration.
    pub fn broadcast_config(mut self, config: PlumtreeConfig) -> Self {
        self.config.broadcast = config;
        self
    }

    /// Set the ALPN this gossip instance uses.
    ///
    /// It has to be the same for all peers in the network. If you set a custom ALPN,
    /// you have to use the same ALPN when registering the [`Gossip`] in on a iroh
    /// router with [`RouterBuilder::accept`].
    ///
    /// [`RouterBuilder::accept`]: iroh::protocol::RouterBuilder::accept
    pub fn alpn(mut self, alpn: impl AsRef<[u8]>) -> Self {
        self.alpn = Some(alpn.as_ref().to_vec().into());
        self
    }

    /// Persist addresses learned from gossip for known friends.
    pub fn friends_store(mut self, friends: Arc<Mutex<FriendsStore>>) -> Self {
        self.friends = Some(friends);
        self
    }

    /// Spawn a gossip actor and get a handle for it
    pub fn spawn(self, endpoint: Endpoint) -> Gossip {
        let metrics = Arc::new(Metrics::default());
        let address_lookup = self
            .friends
            .map(|friends| GossipAddressLookup::with_friends(Default::default(), friends))
            .unwrap_or_default();

        // `Endpoint::address_lookup` returns `Err` when the endpoint is closed.
        // In that case, the gossip actor will close too very soon for other reasons,
        // so it's fine if we only add our `GossipAddressLookup` for the non-closed
        // case. The alternative would be to return a `Result` from `spawn`,
        // but as long as this is the only direct error case, it seem unwarranted.
        if let Ok(endpoint_addr_lookup) = endpoint.address_lookup().as_ref() {
            endpoint_addr_lookup.add(address_lookup.clone());
        }
        let (actor, rpc_tx, local_tx) = Actor::new(
            endpoint,
            self.config,
            metrics.clone(),
            self.alpn,
            address_lookup,
        );
        let me = actor.endpoint.id().fmt_short().to_string();
        let max_message_size = actor.state.max_message_size();

        // Initialise gossip debug tracing (reads `BORU_DEBUG` env var).
        crate::gossip_debug::init(&me);

        let actor_handle = task::spawn(actor.run().instrument(error_span!("gossip", %me)));

        let api = GossipApi::local(rpc_tx);

        Gossip {
            inner: Inner {
                api,
                local_tx,
                _actor_handle: AbortOnDropHandle::new(actor_handle),
                max_message_size,
                metrics,
            }
            .into(),
        }
    }
}

impl Gossip {
    /// Creates a default `Builder`, with the endpoint set.
    pub fn builder() -> Builder {
        Builder {
            config: Default::default(),
            alpn: None,
            friends: None,
        }
    }

    /// Create a minimal Gossip for testing (requires `test-utils` feature).
    ///
    /// Creates a real endpoint bound to port 0 (auto-assign) with relays
    /// disabled.  The gossip actor runs on a background tokio thread.
    /// Safe to clone — the underlying endpoint and actor are reference-counted.
    #[cfg(feature = "test-utils")]
    pub fn test_dummy() -> Self {
        use std::sync::OnceLock;
        static DUMMY: OnceLock<(Gossip, iroh::Endpoint)> = OnceLock::new();
        let (gossip, _endpoint) = DUMMY.get_or_init(|| {
            let rt = tokio::runtime::Runtime::new().expect("create tokio runtime for test gossip");
            let _guard = rt.enter();
            let endpoint = rt.block_on(async {
                iroh::Endpoint::builder(iroh::endpoint::presets::N0)
                    .secret_key(iroh::SecretKey::generate())
                    .address_lookup(iroh::address_lookup::memory::MemoryLookup::new())
                    .relay_mode(iroh::RelayMode::Disabled)
                    .bind()
                    .await
                    .expect("bind test endpoint")
            });
            let gossip = Gossip::builder().spawn(endpoint.clone());
            std::mem::forget(rt); // Keep the runtime alive for the test process lifetime
            (gossip, endpoint)
        });
        gossip.clone()
    }

    /// Get the maximum message size configured for this gossip actor.
    pub fn max_message_size(&self) -> usize {
        self.inner.max_message_size
    }

    /// Handle an incoming [`Connection`].
    ///
    /// Make sure to check the ALPN protocol yourself before passing the connection.
    pub async fn handle_connection(&self, conn: Connection) -> Result<(), Error> {
        self.inner
            .local_tx
            .send(LocalActorMessage::HandleConnection(conn))
            .await?;
        Ok(())
    }

    /// Shutdown the gossip instance.
    ///
    /// This leaves all topics, sending `Disconnect` messages to peers, and then
    /// stops the gossip actor loop and drops all state and connections.
    pub async fn shutdown(&self) -> Result<(), Error> {
        let (reply, reply_rx) = oneshot::channel();
        self.inner
            .local_tx
            .send(LocalActorMessage::Shutdown { reply })
            .await?;
        reply_rx.await?;
        Ok(())
    }

    /// Returns the metrics tracked for this gossip instance.
    pub fn metrics(&self) -> &Arc<Metrics> {
        &self.inner.metrics
    }
}
