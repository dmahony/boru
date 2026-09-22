//! Gossip actor: lifecycle, event loop and peer/topic orchestration.

use std::{
    collections::{hash_map::Entry, HashMap, VecDeque},
    sync::Arc,
};

use bytes::Bytes;
use futures_concurrency::stream::{stream_group, StreamGroup};
use iroh::{endpoint::Connection, Endpoint, EndpointAddr, EndpointId, PublicKey, Watcher};
use irpc::WithChannels;
use n0_future::{
    task::JoinSet,
    time::{Duration, Instant},
    Stream, StreamExt as _,
};
use rand::{rngs::StdRng, SeedableRng};
use tokio::sync::mpsc;
use tracing::{debug, error_span, info, trace, warn, Instrument};

use super::{
    address_lookup::GossipAddressLookup,
    connectivity::{connection_loop, decode_peer_data, encode_peer_data},
    dialer::{is_discovery_unavailable, DialOutcome, Dialer},
    peer::{ConnOrigin, ConnectionLoopError, PeerState, TopicState},
    protocol::{event_kind_tag, InEvent, OutEvent, ProtoCommand, ProtoEvent, Timer},
    topic::{topic_subscriber_loop, TopicCommandStream},
    util::Timers,
    LocalActorMessage, GOSSIP_ALPN,
};
use crate::{
    api::{self, Command, Event, RpcMessage},
    chat_core::DIAGNOSTICS,
    diagnostics::{DiagnosticEventKind, DiscoverySource},
    metrics::Metrics,
    proto::{self, Scope, TopicId},
};

const SEND_QUEUE_CAP: usize = 64;
const TO_ACTOR_CAP: usize = 64;
const IN_EVENT_CAP: usize = 1024;
const MAX_DIAL_RETRIES: usize = 3;
const RETRY_BASE_DELAY_S: u64 = 5;
const RETRY_MAX_DELAY_S: u64 = 60;
const RETRY_COOLDOWN_S: u64 = 60;
const DIAL_MAINTENANCE_INTERVAL_S: u64 = 1;

/// Actor that sends and handles messages between the connection and main state loops
pub(super) struct Actor {
    alpn: Bytes,
    /// Protocol state
    pub(super) state: proto::State<PublicKey, StdRng>,
    /// The endpoint through which we dial peers
    pub(super) endpoint: Endpoint,
    /// Dial machine to connect to peers
    dialer: Dialer,
    /// Input messages to the actor
    rpc_rx: mpsc::Receiver<RpcMessage>,
    local_rx: mpsc::Receiver<LocalActorMessage>,
    /// Sender for the state input (cloned into the connection loops)
    in_event_tx: mpsc::Sender<InEvent>,
    /// Input events to the state (emitted from the connection loops)
    in_event_rx: mpsc::Receiver<InEvent>,
    /// Queued timers
    timers: Timers<Timer>,
    /// Map of topics to their state.
    pub(super) topics: HashMap<TopicId, TopicState>,
    /// Map of peers to their state.
    peers: HashMap<EndpointId, PeerState>,
    /// Stream of commands from topic handles.
    command_rx: stream_group::Keyed<TopicCommandStream>,
    /// Internal queue of topic to close because all handles were dropped.
    quit_queue: VecDeque<TopicId>,
    /// Tasks for the connection loops, to keep track of panics.
    connection_tasks: JoinSet<(EndpointId, Connection, Result<(), ConnectionLoopError>)>,
    metrics: Arc<Metrics>,
    topic_event_forwarders: JoinSet<TopicId>,
    address_lookup: GossipAddressLookup,
    /// Track retry attempts per peer for dial failures.
    retry_map: HashMap<EndpointId, usize>,
    /// Actor-owned deadlines; one per peer, cancelled on connection/quit.
    retry_schedule: HashMap<EndpointId, (Instant, EndpointAddr)>,
    cooldowns: HashMap<EndpointId, Instant>,
    maintenance_due: Instant,
}

impl Actor {
    pub(super) fn new(
        endpoint: Endpoint,
        config: proto::Config,
        metrics: Arc<Metrics>,
        alpn: Option<Bytes>,
        address_lookup: GossipAddressLookup,
    ) -> (
        Self,
        mpsc::Sender<RpcMessage>,
        mpsc::Sender<LocalActorMessage>,
    ) {
        let peer_id = endpoint.id();
        let dialer = Dialer::new(endpoint.clone());
        let state = proto::State::new(
            peer_id,
            Default::default(),
            config,
            rand::rngs::StdRng::from_rng(&mut rand::rng()),
        );
        let (rpc_tx, rpc_rx) = mpsc::channel(TO_ACTOR_CAP);
        let (local_tx, local_rx) = mpsc::channel(16);
        let (in_event_tx, in_event_rx) = mpsc::channel(IN_EVENT_CAP);

        let actor = Actor {
            alpn: alpn.unwrap_or_else(|| GOSSIP_ALPN.to_vec().into()),
            endpoint,
            state,
            dialer,
            rpc_rx,
            in_event_rx,
            in_event_tx,
            timers: Timers::new(),
            command_rx: StreamGroup::new().keyed(),
            peers: Default::default(),
            topics: Default::default(),
            quit_queue: Default::default(),
            connection_tasks: Default::default(),
            metrics,
            local_rx,
            topic_event_forwarders: Default::default(),
            address_lookup,
            retry_map: Default::default(),
            retry_schedule: Default::default(),
            cooldowns: Default::default(),
            maintenance_due: Instant::now(),
        };

        (actor, rpc_tx, local_tx)
    }

    pub(super) async fn run(mut self) {
        let mut addr_update_stream = self.setup().await;


        let mut i = 0;
        while self.event_loop(&mut addr_update_stream, i).await {
            i += 1;
        }
    }

    /// Performs the initial actor setup to run the [`Actor::event_loop`].
    ///
    /// This updates our current address and return it. It also returns the home relay stream and
    /// direct addr stream.
    pub(super) async fn setup(
        &mut self,
    ) -> impl Stream<Item = EndpointAddr> + Send + Unpin + use<> {
        let addr_update_stream = self.endpoint.watch_addr().stream();
        let initial_addr = self.endpoint.addr();
        self.handle_addr_update(initial_addr).await;
        addr_update_stream
    }

    /// One event loop processing step.
    ///
    /// None is returned when no further processing should be performed.
    pub(super) async fn event_loop(
        &mut self,
        addr_updates: &mut (impl Stream<Item = EndpointAddr> + Send + Unpin),
        i: usize,
    ) -> bool {
        self.metrics.actor_tick_main.inc();
        tokio::select! {
            biased;
            conn = self.local_rx.recv() => {
                match conn {
                    Some(LocalActorMessage::Shutdown { reply }) => {
                        debug!("received shutdown message, quit all topics");
                        self.quit_queue.extend(self.topics.keys().copied());
                        self.process_quit_queue().await;
                        debug!("all topics quit, stop gossip actor");
                        reply.send(()).ok();
                        return false;
                    },
                    Some(LocalActorMessage::HandleConnection(conn)) => {
                        self.handle_connection(conn.remote_id(), ConnOrigin::Accept, conn);
                    }

                    None => {
                        debug!("all gossip handles dropped, stop gossip actor");
                        return false;
                    }
                }
            }
            msg = self.rpc_rx.recv() => {
                trace!(?i, "tick: to_actor_rx");
                self.metrics.actor_tick_rx.inc();
                match msg {
                    Some(msg) => {
                        self.handle_rpc_msg(msg, Instant::now()).await;
                    }
                    None => {
                        debug!("all gossip handles dropped, stop gossip actor");
                        return false;
                    }
                }
            },
            Some((key, (topic, command))) = self.command_rx.next(), if !self.command_rx.is_empty() => {
                trace!(?i, "tick: command_rx");
                self.handle_command(topic, key, command).await;
            },
            Some(new_address) = addr_updates.next() => {
                trace!(?i, "tick: new_address");
                self.metrics.actor_tick_endpoint.inc();
                self.handle_addr_update(new_address).await;
            }
            _ = n0_future::time::sleep_until(self.maintenance_due) => {
                self.maintain_dials();
            }
            (addr, outcome) = self.dialer.next_conn() => {
                let peer_id = addr.id;
                self.metrics.actor_tick_dialer.inc();
                match outcome {
                    DialOutcome::Connected(conn) => {
                        debug!(peer = %peer_id.fmt_short(), "dial successful");
                        self.metrics.actor_tick_dialer_success.inc();
                        self.handle_connection(peer_id, ConnOrigin::Dial, conn);
                    }
                    DialOutcome::Cancelled => {
                        debug!(peer = %peer_id.fmt_short(), "dial cancelled");
                        if matches!(self.peers.get(&peer_id), Some(PeerState::Pending { .. })) {
                            self.schedule_retry(peer_id, addr).await;
                        }
                    }
                    failure => {
                        match failure {
                            DialOutcome::Failed(err) if is_discovery_unavailable(&err) => {
                                debug!(peer = %peer_id.fmt_short(), error = %err, "discovery candidate unavailable");
                            }
                            DialOutcome::Failed(err) => warn!(peer = %peer_id.fmt_short(), "dial failed: {err:#}"),
                            DialOutcome::TaskFailed => warn!(peer = %peer_id.fmt_short(), "dial task failed; retrying operation"),
                            // Offline discovery candidates never established a
                            // connection: a timeout is not a disconnect.
                            DialOutcome::TimedOut => debug!(peer = %peer_id.fmt_short(), "peer did not answer within dial timeout"),
                            _ => unreachable!(),
                        }
                        self.metrics.actor_tick_dialer_failure.inc();
                        if matches!(self.peers.get(&peer_id), Some(PeerState::Pending { .. })) {
                            self.schedule_retry(peer_id, addr).await;
                        }
                    }
                }
            }
            event = self.in_event_rx.recv() => {
                trace!(?i, "tick: in_event_rx");
                self.metrics.actor_tick_in_event_rx.inc();
                let event = event.expect("unreachable: in_event_tx is never dropped before receiver");
                self.handle_in_event(event, Instant::now()).await;
            }
            _ = self.timers.wait_next() => {
                trace!(?i, "tick: timers");
                self.metrics.actor_tick_timers.inc();
                let now = Instant::now();
                while let Some((_instant, timer)) = self.timers.pop_before(now) {
                    self.handle_in_event(InEvent::TimerExpired(timer), now).await;
                }
            }
            Some(res) = self.connection_tasks.join_next(), if !self.connection_tasks.is_empty() => {
                trace!(?i, "tick: connection_tasks");
                let (peer_id, conn, result) = res.expect("connection task panicked");
                self.handle_connection_task_finished(peer_id, conn, result).await;
            }
            Some(res) = self.topic_event_forwarders.join_next(), if !self.topic_event_forwarders.is_empty() => {
                let topic_id = res.expect("topic event forwarder panicked");
                if let Some(state) = self.topics.get_mut(&topic_id) {
                    if !state.still_needed() {
                        self.quit_queue.push_back(topic_id);
                        self.process_quit_queue().await;
                    }
                }
            }
        }

        true
    }

    async fn handle_addr_update(&mut self, endpoint_addr: EndpointAddr) {
        debug!(
            peer = %endpoint_addr.id.fmt_short(),
            relay = ?endpoint_addr.relay_urls().next(),
            "gossip address update",
        );
        // let peer_data = our_peer_data(&self.endpoint, current_addresses);
        let peer_data = encode_peer_data(&endpoint_addr.into());
        self.handle_in_event(InEvent::UpdatePeerData(peer_data), Instant::now())
            .await
    }

    async fn handle_command(
        &mut self,
        topic: TopicId,
        key: stream_group::Key,
        command: Option<Command>,
    ) {
        debug!(?topic, ?key, ?command, "handle command");
        let Some(state) = self.topics.get_mut(&topic) else {
            // TODO: unreachable?
            warn!("received command for unknown topic");
            return;
        };
        match command {
            Some(command) => {
                let command = match command {
                    Command::Broadcast(message) => ProtoCommand::Broadcast(message, Scope::Swarm),
                    Command::BroadcastNeighbors(message) => {
                        ProtoCommand::Broadcast(message, Scope::Neighbors)
                    }
                    Command::JoinPeers(peers) => ProtoCommand::Join(peers),
                };
                self.handle_in_event(proto::InEvent::Command(topic, command), Instant::now())
                    .await;
            }
            None => {
                state.command_rx_keys.remove(&key);
                if !state.still_needed() {
                    self.quit_queue.push_back(topic);
                    self.process_quit_queue().await;
                }
            }
        }
    }

    /// Schedule a retry for a peer, preserving the last-known address.
    async fn schedule_retry(&mut self, peer_id: EndpointId, addr: EndpointAddr) {
        if self.retry_schedule.contains_key(&peer_id)
            || matches!(self.peers.get(&peer_id), Some(PeerState::Active { .. }))
            || self.cooldowns.get(&peer_id).is_some_and(|until| *until > Instant::now())
        {
            return;
        }
        let attempts = self.retry_map.entry(peer_id).or_insert(0);
        if *attempts < MAX_DIAL_RETRIES {
            *attempts += 1;
            let delay = std::cmp::min(
                RETRY_BASE_DELAY_S * (1u64 << (*attempts - 1)),
                RETRY_MAX_DELAY_S,
            );
            info!(
                peer = %peer_id.fmt_short(),
                "will retry dial in {delay}s (attempt {} / {MAX_DIAL_RETRIES})",
                *attempts,
            );
            self.retry_schedule.insert(peer_id, (Instant::now() + Duration::from_secs(delay), addr));
        } else {
            // Historical discovery candidates need a bounded burst, not an
            // endless cooldown timer. Fresh protocol demand may retry later.
            info!(
                peer = %peer_id.fmt_short(),
                "peer unavailable after {MAX_DIAL_RETRIES} retries; waiting for fresh demand after {RETRY_COOLDOWN_S}s cooldown",
            );
            // Install before processing protocol outputs, which may themselves
            // request another dial for this candidate.
            self.cooldowns.insert(peer_id, Instant::now() + Duration::from_secs(RETRY_COOLDOWN_S));
            self.handle_in_event(InEvent::PeerDisconnected(peer_id), Instant::now())
                .await;
            self.retry_map.remove(&peer_id);
            self.retry_schedule.remove(&peer_id);
            self.peers.remove(&peer_id);
            self.cooldowns.insert(peer_id, Instant::now() + Duration::from_secs(RETRY_COOLDOWN_S));
        }
    }

    fn maintain_dials(&mut self) {
        let now = Instant::now();
        self.maintenance_due = now + Duration::from_secs(DIAL_MAINTENANCE_INTERVAL_S);
        self.dialer.cleanup_stale_dials();
        self.cooldowns.retain(|_, until| *until > now);
        let due: Vec<_> = self.retry_schedule.iter()
            .filter(|(_, (when, _))| *when <= now)
            .map(|(peer, _)| *peer).collect();
        for peer in due {
            let (_, addr) = self.retry_schedule.remove(&peer).unwrap();
            if matches!(self.peers.get(&peer), Some(PeerState::Pending { .. })) {
                self.dialer.queue_dial(addr, self.alpn.clone());
            } else {
                self.retry_map.remove(&peer);
            }
        }
    }

    fn clear_dial_state(&mut self, peer: EndpointId) {
        self.retry_map.remove(&peer);
        self.retry_schedule.remove(&peer);
        self.cooldowns.remove(&peer);
        self.dialer.cancel(peer);
    }

    fn handle_connection(&mut self, peer_id: EndpointId, origin: ConnOrigin, conn: Connection) {
        self.clear_dial_state(peer_id);
        let (send_tx, send_rx) = mpsc::channel(SEND_QUEUE_CAP);
        let conn_id = conn.stable_id();

        let queue = match self.peers.entry(peer_id) {
            Entry::Occupied(mut entry) => entry.get_mut().accept_conn(send_tx, conn_id, origin),
            Entry::Vacant(entry) => {
                entry.insert(PeerState::Active {
                    active_send_tx: send_tx,
                    active_conn_id: conn_id,
                    active_conn_origin: origin,
                    other_conns: Vec::new(),
                });
                Some(Vec::new())
            }
        };

        let Some(queue) = queue else {
            debug!(
                peer = %peer_id.fmt_short(),
                ?origin,
                conn_id,
                "session collision: rejecting new connection, keeping existing one",
            );
            // Close the rejected connection so the remote peer gets a signal.
            conn.close(0u32.into(), b"redundant connection");
            return;
        };

        let max_message_size = self.state.max_message_size();
        let in_event_tx = self.in_event_tx.clone();

        // Spawn a task for this connection
        self.connection_tasks.spawn(
            async move {
                let res = connection_loop(
                    peer_id,
                    conn.clone(),
                    origin,
                    send_rx,
                    in_event_tx,
                    max_message_size,
                    queue,
                )
                .await;
                (peer_id, conn, res)
            }
            .instrument(error_span!("conn", peer = %peer_id.fmt_short(), conn_id)),
        );
    }

    #[tracing::instrument(name = "conn", skip_all, fields(peer = %peer_id.fmt_short()))]
    async fn handle_connection_task_finished(
        &mut self,
        peer_id: EndpointId,
        conn: Connection,
        task_result: Result<(), ConnectionLoopError>,
    ) {
        // Log the failure reason so connection deaths (especially WriteError::TooLarge
        // from oversized gossip messages) are visible in logs.
        if let Err(ref err) = task_result {
            warn!(peer = %peer_id.fmt_short(), "connection loop ended: {err:#}");
        }

        // Extract the backup connection before mutating self.peers further.
        // This avoids a double mutable borrow when we need to update the peer
        // state after popping from other_conns.
        let backup_conn = match self.peers.get_mut(&peer_id) {
            Some(PeerState::Active {
                active_conn_id,
                other_conns,
                ..
            }) => {
                if conn.stable_id() == *active_conn_id {
                    // Active connection died — pop a backup if available.
                    other_conns.pop()
                } else {
                    // Backup connection finished — just remove it from tracking.
                    other_conns.retain(|(id, _)| *id != conn.stable_id());
                    debug!(
                        "backup connection task finished, {} backup(s) remaining",
                        other_conns.len()
                    );
                    return;
                }
            }
            _ => {
                debug!("peer already marked as disconnected");
                if conn.close_reason().is_none() {
                    conn.close(0u32.into(), b"close from disconnect");
                }
                return;
            }
        };

        // If we get here, the active connection died.
        debug!("active send connection closed");
        if conn.close_reason().is_none() {
            conn.close(0u32.into(), b"close from disconnect");
        }

        if let Some((backup_conn_id, backup_tx)) = backup_conn {
            // Promote the backup connection to active.  This keeps the peer
            // reachable and prevents the "dead sender" stuck state where
            // PeerState::Active holds a closed active_send_tx but no new
            // dial is triggered.
            info!(
                peer = %peer_id.fmt_short(),
                "promoting backup connection to active after active failure"
            );
            if let Some(PeerState::Active {
                active_send_tx,
                active_conn_id,
                active_conn_origin,
                ..
            }) = self.peers.get_mut(&peer_id)
            {
                *active_send_tx = backup_tx;
                *active_conn_id = backup_conn_id;
                *active_conn_origin = ConnOrigin::Accept; // backup was an Accept
            }
            // Do NOT fire PeerDisconnected — the peer is still
            // reachable via the promoted backup connection.
        } else {
            self.handle_in_event(InEvent::PeerDisconnected(peer_id), Instant::now())
                .await;
            // Reset PeerState to Pending so the dialer will re-dial.
            // Without this, the dead active_send_tx stays in
            // PeerState::Active and all future sends silently fail.
            self.peers
                .insert(peer_id, PeerState::Pending { queue: Vec::new() });
        }
    }

    async fn handle_rpc_msg(&mut self, msg: RpcMessage, now: Instant) {
        trace!("handle to_actor  {msg:?}");
        match msg {
            RpcMessage::Join(msg) => {
                let WithChannels {
                    inner,
                    rx,
                    tx,
                    // TODO(frando): make use of span?
                    span: _,
                } = msg;
                let api::JoinRequest {
                    topic_id,
                    bootstrap,
                } = inner;
                let TopicState {
                    neighbors,
                    event_sender,
                    command_rx_keys,
                } = self.topics.entry(topic_id).or_default();
                // Always spawn the permanent subscriber loop.  The initial
                // NeighborUp replay is sent inside the loop task so that a
                // failed replay (channel already closed/full at join) cannot
                // permanently suppress event forwarding for a topic the
                // application has successfully joined.  If the replay send
                // fails, the app-side receiver is gone and the task exits —
                // the loop itself never starts, which is correct in that
                // case, but the decision is no longer made eagerly on the
                // actor's Join path.
                let initial_neighbors = neighbors.iter().copied().collect::<Vec<_>>();
                let subscriber_rx = event_sender.subscribe();
                let fut = async move {
                    for neighbor in initial_neighbors {
                        if tx.send(Event::NeighborUp(neighbor)).await.is_err() {
                            warn!(
                                topic = %topic_id.fmt_short(),
                                "gossip: subscriber loop exiting — initial NeighborUp replay failed (app-side receiver dropped)"
                            );
                            return topic_id;
                        }
                    }
                    topic_subscriber_loop(topic_id, tx, subscriber_rx).await;
                    topic_id
                };
                self.topic_event_forwarders
                    .spawn(fut.instrument(tracing::Span::current()));
                debug!(
                    topic = %topic_id.fmt_short(),
                    neighbors = neighbors.len(),
                    "gossip: subscriber loop spawned for topic"
                );
                let command_rx = TopicCommandStream::new(topic_id, Box::pin(rx.into_stream()));
                let key = self.command_rx.insert(command_rx);
                command_rx_keys.insert(key);

                self.handle_in_event(
                    InEvent::Command(
                        topic_id,
                        ProtoCommand::Join(bootstrap.into_iter().collect()),
                    ),
                    now,
                )
                .await;
            }
        }
    }

    async fn handle_in_event(&mut self, event: InEvent, now: Instant) {
        self.handle_in_event_inner(event, now).await;
        self.process_quit_queue().await;
    }

    async fn process_quit_queue(&mut self) {
        while let Some(topic_id) = self.quit_queue.pop_front() {
            self.handle_in_event_inner(
                InEvent::Command(topic_id, ProtoCommand::Quit),
                Instant::now(),
            )
            .await;
            if self.topics.remove(&topic_id).is_some() {
                tracing::debug!(%topic_id, "publishers and subscribers gone; unsubscribing");
            }
        }
    }

    async fn handle_in_event_inner(&mut self, event: InEvent, now: Instant) {
        if matches!(event, InEvent::TimerExpired(_)) {
            trace!(?event, "handle in_event");
        } else {
            debug!(?event, "handle in_event");
        };
        let out = self.state.handle(event, now, Some(&self.metrics));
        for event in out {
            if matches!(event, OutEvent::ScheduleTimer(_, _)) {
                trace!(?event, "handle out_event");
            } else {
                debug!(?event, "handle out_event");
            };
            match event {
                OutEvent::SendMessage(peer_id, message) => {
                    // A malformed discovery record must never make us dial
                    // ourselves. This also prevents a self NeighborUp from
                    // falsely making a room appear ready.
                    if peer_id == self.endpoint.id() {
                        debug!(peer = %peer_id.fmt_short(), "ignoring self peer in gossip dial");
                        continue;
                    }
                    if self.cooldowns.get(&peer_id).is_some_and(|until| *until > now) {
                        continue;
                    }
                    let state = self.peers.entry(peer_id).or_default();
                    match state {
                        PeerState::Active {
                            active_send_tx,
                            active_conn_id,
                            ..
                        } => {
                            debug!(
                                peer = %peer_id.fmt_short(),
                                conn_id = *active_conn_id,
                                topic = %message.topic.fmt_short(),
                                "SEND_ROUTE: routing message to active connection",
                            );
                            if let Err(_err) = active_send_tx.send(message).await {
                                // Removing the peer is handled by the in_event PeerDisconnected sent
                                // in [`Self::handle_connection_task_finished`].
                                warn!(
                                    peer = %peer_id.fmt_short(),
                                    "failed to send: connection task send loop terminated",
                                );
                            }
                        }
                        PeerState::Pending { queue } => {
                            if queue.is_empty() && !self.retry_schedule.contains_key(&peer_id) {
                                info!(peer = %peer_id.fmt_short(), "start to dial");
                                DIAGNOSTICS.record_with_peer(
                                    None,
                                    Some(peer_id.to_string()),
                                    DiagnosticEventKind::AddressLookupStarted {
                                        source: DiscoverySource::Gossip,
                                    },
                                );
                                let endpoint_addr = match self.address_lookup.endpoint_addr(peer_id)
                                {
                                    Some(mut addr) => {
                                        info!(
                                            peer = %peer_id.fmt_short(),
                                            "dial from gossip lookup: relay={:?} ips={:?}",
                                            addr.relay_urls().next(),
                                            addr.ip_addrs().cloned().collect::<Vec<_>>(),
                                        );
                                        // Gossip doesn't always include the relay URL.
                                        // If the resolved address has no usable transport,
                                        // add the relay URL from our own published address.
                                        let has_relay = addr.relay_urls().next().is_some();
                                        let has_ips = addr.ip_addrs().next().is_some();
                                        if !has_relay && !has_ips {
                                            let relay_url =
                                                self.endpoint.addr().relay_urls().next().cloned();
                                            if let Some(relay) = relay_url {
                                                addr = addr.with_relay_url(relay.clone());
                                                info!(
                                                    peer = %peer_id.fmt_short(),
                                                    "added relay URL from endpoint addr: {}",
                                                    relay,
                                                );
                                            }
                                        }
                                        addr
                                    }
                                    None => {
                                        let mut found = EndpointAddr::new(peer_id);
                                        if let Ok(services) = self.endpoint.address_lookup() {
                                            let mut stream = services.resolve(peer_id);
                                            match n0_future::time::timeout(
                                                Duration::from_secs(5),
                                                stream.next(),
                                            )
                                            .await
                                            {
                                                Ok(Some(Ok(Ok(item)))) => {
                                                    found = item.into_endpoint_addr();
                                                    // mDNS doesn't reliably include the
                                                    // relay URL (TXT records arrive later).
                                                    // Add it from our own published address.
                                                    let our_addr = self.endpoint.addr();
                                                    if found.relay_urls().next().is_none() {
                                                        if let Some(relay) =
                                                            our_addr.relay_urls().next()
                                                        {
                                                            found =
                                                                found.with_relay_url(relay.clone());
                                                            info!(
                                                                peer = %peer_id.fmt_short(),
                                                                "added relay URL from endpoint addr: {}",
                                                                relay,
                                                            );
                                                        }
                                                    }
                                                    info!(
                                                        peer = %peer_id.fmt_short(),
                                                        "dial from endpoint lookup: relay={:?} ips={:?}",
                                                        found.relay_urls().next(),
                                                        found.ip_addrs().cloned().collect::<Vec<_>>(),
                                                    );
                                                }
                                                _ => {
                                                    // No addresses from any lookup service.
                                                    // Fall back to the relay URL we're already
                                                    // connected to — the peer can be reached
                                                    // through the same relay.
                                                    let relay_url = self
                                                        .endpoint
                                                        .addr()
                                                        .relay_urls()
                                                        .next()
                                                        .cloned();
                                                    if let Some(relay) = relay_url {
                                                        found = found.with_relay_url(relay.clone());
                                                        info!(
                                                            peer = %peer_id.fmt_short(),
                                                            "dial from endpoint lookup: TIMEOUT/EMPTY, using relay URL: {}",
                                                            relay,
                                                        );
                                                    } else {
                                                        info!(
                                                            peer = %peer_id.fmt_short(),
                                                            "dial from endpoint lookup: TIMEOUT/EMPTY, using bare ID (no relay configured)",
                                                        );
                                                    }
                                                }
                                            }
                                        }
                                        found
                                    }
                                };
                                DIAGNOSTICS.record_with_peer(
                                    None,
                                    Some(peer_id.to_string()),
                                    DiagnosticEventKind::AddressResolved {
                                        source: DiscoverySource::Gossip,
                                        // Do not persist relay URLs or IPs in
                                        // diagnostics; the transition itself
                                        // is sufficient to classify failures.
                                        addresses: Vec::new(),
                                    },
                                );
                                DIAGNOSTICS.record_with_peer(
                                    None,
                                    Some(peer_id.to_string()),
                                    DiagnosticEventKind::ConnectionAttemptStarted {
                                        addresses: Vec::new(),
                                    },
                                );
                                self.dialer.queue_dial(endpoint_addr, self.alpn.clone());
                            }
                            queue.push(message);
                        }
                    }
                }
                OutEvent::EmitEvent(topic_id, event) => {
                    // Log gossip debug trace for protocol-level events.
                    if crate::gossip_debug::is_enabled() {
                        let topic_short = topic_id.fmt_short();
                        match &event {
                            crate::proto::Event::NeighborUp(p) => {
                                let p_str = p.fmt_short().to_string();
                                crate::gossip_debug::log_event(
                                    "NeighborUp",
                                    Some(&topic_short),
                                    Some(&p_str),
                                    None,
                                );
                            }
                            crate::proto::Event::NeighborDown(p) => {
                                let p_str = p.fmt_short().to_string();
                                crate::gossip_debug::log_event(
                                    "NeighborDown",
                                    Some(&topic_short),
                                    Some(&p_str),
                                    None,
                                );
                            }
                            crate::proto::Event::Received(msg) => {
                                let len = msg.content.len();
                                let from_str = msg.delivered_from.fmt_short().to_string();
                                crate::gossip_debug::log_event(
                                    "Received",
                                    Some(&topic_short),
                                    Some(&from_str),
                                    Some(len),
                                );
                            }
                            crate::proto::Event::MissingMessages {
                                since_round,
                                from_peer,
                            } => {
                                let peer_str = from_peer.fmt_short().to_string();
                                crate::gossip_debug::log_event(
                                    "MissingMessages",
                                    Some(&topic_short),
                                    Some(&peer_str),
                                    Some(since_round.get() as usize),
                                );
                            }
                        }
                    }

                    let Some(state) = self.topics.get_mut(&topic_id) else {
                        // TODO: unreachable?
                        warn!(?topic_id, "gossip state emitted event for unknown topic");
                        continue;
                    };
                    let TopicState {
                        neighbors,
                        event_sender,
                        ..
                    } = state;
                    match &event {
                        ProtoEvent::NeighborUp(neighbor) => {
                            neighbors.insert(*neighbor);
                        }
                        ProtoEvent::NeighborDown(neighbor) => {
                            neighbors.remove(neighbor);
                        }
                        _ => {}
                    }
                    let event_kind = event_kind_tag(&event);
                    if let Err(err) = event_sender.send(event) {
                        warn!(
                            topic = %topic_id.fmt_short(),
                            error = %err,
                            "gossip: event_sender.send failed — event dropped (broadcast channel closed or no receivers)"
                        );
                    } else {
                        debug!(
                            topic = %topic_id.fmt_short(),
                            event_kind,
                            "ACTOR_EMIT: broadcast sent to subscriber loops",
                        );
                    }
                    if !state.still_needed() {
                        self.quit_queue.push_back(topic_id);
                    }
                }
                OutEvent::ScheduleTimer(delay, timer) => {
                    self.timers.insert(now + delay, timer);
                }
                OutEvent::DisconnectPeer(peer_id) => {
                    // signal disconnection by dropping the senders to the connection
                    debug!(peer=%peer_id.fmt_short(), "gossip state indicates disconnect: drop peer");
                    self.peers.remove(&peer_id);
                    self.retry_map.remove(&peer_id);
                    self.retry_schedule.remove(&peer_id);
                    self.dialer.cancel(peer_id);
                }
                OutEvent::PeerData(endpoint_id, data) => match decode_peer_data(&data) {
                    Err(err) => warn!("Failed to decode {data:?} from {endpoint_id}: {err}"),
                    Ok(info) => {
                        debug!(peer = ?endpoint_id, "add known addrs: {info:?}");
                        let mut endpoint_addr = EndpointAddr::new(endpoint_id);
                        for addr in info.direct_addresses {
                            endpoint_addr = endpoint_addr.with_ip_addr(addr);
                        }
                        if let Some(relay_url) = info.relay_url {
                            endpoint_addr = endpoint_addr.with_relay_url(relay_url);
                        }

                        self.address_lookup.add(endpoint_addr);
                    }
                },
            }
        }
    }
}

#[cfg(test)]
mod regression_tests {
    use super::*;

    async fn actor() -> Actor {
        let endpoint = Endpoint::builder(iroh::endpoint::presets::Minimal).bind().await.unwrap();
        Actor::new(endpoint, proto::Config::default(), Arc::new(Metrics::default()), None, GossipAddressLookup::new()).0
    }

    #[tokio::test]
    async fn duplicate_failures_schedule_one_retry_with_original_address() {
        let mut actor = actor().await;
        let peer = iroh::SecretKey::generate().public();
        let addr = EndpointAddr::new(peer).with_ip_addr("127.0.0.1:12345".parse().unwrap());
        actor.peers.insert(peer, PeerState::default());
        actor.schedule_retry(peer, addr.clone()).await;
        actor.schedule_retry(peer, EndpointAddr::new(peer)).await;
        assert_eq!(actor.retry_map[&peer], 1);
        assert_eq!(actor.retry_schedule.len(), 1);
        assert_eq!(actor.retry_schedule[&peer].1, addr);
        actor.endpoint.close().await;
    }

    #[tokio::test]
    async fn connection_cleanup_removes_due_retries_and_cooldown() {
        let mut actor = actor().await;
        let peer = iroh::SecretKey::generate().public();
        actor.peers.insert(peer, PeerState::default());
        actor.retry_map.insert(peer, 2);
        actor.retry_schedule.insert(peer, (Instant::now(), EndpointAddr::new(peer)));
        actor.cooldowns.insert(peer, Instant::now());
        actor.clear_dial_state(peer);
        actor.maintain_dials();
        assert!(!actor.dialer.is_pending(peer));
        assert!(actor.retry_map.is_empty());
        assert!(actor.retry_schedule.is_empty());
        assert!(actor.cooldowns.is_empty());
        actor.endpoint.close().await;
    }

    #[tokio::test]
    async fn exhausted_offline_peer_does_not_schedule_perpetual_cooldowns() {
        let mut actor = actor().await;
        let peer = iroh::SecretKey::generate().public();
        actor.peers.insert(peer, PeerState::default());
        actor.retry_map.insert(peer, MAX_DIAL_RETRIES);
        actor.schedule_retry(peer, EndpointAddr::new(peer)).await;
        assert!(actor.retry_schedule.is_empty());
        assert!(!actor.peers.contains_key(&peer));
        actor.schedule_retry(peer, EndpointAddr::new(peer)).await;
        assert!(actor.retry_schedule.is_empty());
        actor.cooldowns.insert(peer, Instant::now() - Duration::from_secs(1));
        actor.maintain_dials();
        assert!(!actor.dialer.is_pending(peer));
        assert!(actor.cooldowns.is_empty());
        actor.endpoint.close().await;
    }
}
