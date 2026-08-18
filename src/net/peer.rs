//! Peer and topic state: connection selection, redundant-connection tracking
//! and connection-loop error classification (peer/address management).

use std::collections::{BTreeSet, HashSet};

use futures_concurrency::stream::stream_group;
use iroh::EndpointId;
use n0_error::{e, stack_error};
use tokio::sync::{broadcast, mpsc};

use super::protocol::{ProtoEvent, ProtoMessage};
use super::util::{ReadError, WriteError};

const TOPIC_EVENT_CAP: usize = 256;

pub(super) type ConnId = usize;

#[derive(Debug)]
pub(super) enum PeerState {
    Pending {
        queue: Vec<ProtoMessage>,
    },
    Active {
        active_send_tx: mpsc::Sender<ProtoMessage>,
        active_conn_id: ConnId,
        active_conn_origin: ConnOrigin,
        /// Redundant connections kept alive so the QUIC connection is not
        /// torn down on the remote peer's side.  Each entry stores the
        /// connection ID and the message sender — keeping the sender alive
        /// prevents the old connection_loop (which holds the receiver) from
        /// terminating, which would close the QUIC connection and disconnect
        /// the remote peer.
        other_conns: Vec<(ConnId, mpsc::Sender<ProtoMessage>)>,
    },
}

impl PeerState {
    /// The heuristic to decide whether a new connection should replace an existing active one.
    ///
    /// Returns `true` when the new session should replace the old one (current behavior),
    /// `false` when the old session should stay and the new one should be rejected.
    ///
    /// The heuristic prefers Dial connections (our outgoing connections) over Accept
    /// connections (incoming). A Dial connection is one we intentionally initiated;
    /// an Accept connection is the remote peer doing the same thing. When both are the
    /// same origin, prefer the newer connection (the peer simply reconnected).
    fn should_keep_new_session(old_origin: ConnOrigin, new_origin: ConnOrigin) -> bool {
        match (old_origin, new_origin) {
            // Both sides initiated by the same party: prefer the newer connection.
            // The remote peer likely simply reconnected.
            (ConnOrigin::Accept, ConnOrigin::Accept) => true,
            (ConnOrigin::Dial, ConnOrigin::Dial) => true,
            // Our outgoing (Dial) connection is more intentional than an incoming (Accept):
            // we decided to connect to this peer, so replace the passive Accept with
            // our active Dial.
            (ConnOrigin::Accept, ConnOrigin::Dial) => true,
            // Simultaneous dial: keep the incoming Accept connection and demote our
            // own outgoing Dial.  The old Dial connection's sender is kept alive
            // (stored in other_conns) so its connection_loop does NOT exit and does
            // NOT close the QUIC connection — that would kill the remote peer's
            // active connection (they promoted our Dial to their Accept).
            (ConnOrigin::Dial, ConnOrigin::Accept) => true,
        }
    }

    pub(super) fn accept_conn(
        &mut self,
        send_tx: mpsc::Sender<ProtoMessage>,
        conn_id: ConnId,
        origin: ConnOrigin,
    ) -> Option<Vec<ProtoMessage>> {
        match self {
            PeerState::Pending { queue } => {
                let queue = std::mem::take(queue);
                *self = PeerState::Active {
                    active_send_tx: send_tx,
                    active_conn_id: conn_id,
                    active_conn_origin: origin,
                    other_conns: Vec::new(),
                };
                Some(queue)
            }
            PeerState::Active {
                active_send_tx,
                active_conn_id,
                active_conn_origin: old_origin,
                other_conns,
            } => {
                if Self::should_keep_new_session(*old_origin, origin) {
                    // Keep the new connection, demote the old one.
                    // Move the old send_tx into other_conns alongside the old
                    // conn_id so it is NOT dropped — dropping it would cause the
                    // old connection_loop (which holds the receiver) to exit and
                    // close the QUIC connection, disconnecting the remote peer.
                    let old_tx = std::mem::replace(active_send_tx, send_tx);
                    other_conns.push((*active_conn_id, old_tx));
                    tracing::debug!(
                        conn_id,
                        previous_active = *active_conn_id,
                        ?origin,
                        "CONN_REGISTER: replacing active connection",
                    );
                    *active_conn_id = conn_id;
                    *old_origin = origin;
                    Some(Vec::new())
                } else {
                    // Keep the old connection, reject the new one.
                    // Dropping `send_tx` closes the channel so the caller's connection_loop
                    // won't be spawned meaningfully — the caller is responsible for closing
                    // the connection.
                    tracing::debug!(
                        conn_id,
                        active = *active_conn_id,
                        ?origin,
                        "CONN_REGISTER: rejecting new connection, keeping existing active",
                    );
                    None
                }
            }
        }
    }
}

impl Default for PeerState {
    fn default() -> Self {
        PeerState::Pending { queue: Vec::new() }
    }
}

#[derive(Debug)]
pub(super) struct TopicState {
    pub(super) neighbors: BTreeSet<EndpointId>,
    pub(super) event_sender: broadcast::Sender<ProtoEvent>,
    /// Keys identifying command receivers in [`Actor::command_rx`].
    ///
    /// This represents the receiver side of gossip's publish public API.
    pub(super) command_rx_keys: HashSet<stream_group::Key>,
}

impl Default for TopicState {
    fn default() -> Self {
        let (event_sender, _) = broadcast::channel(TOPIC_EVENT_CAP);
        Self {
            neighbors: Default::default(),
            command_rx_keys: Default::default(),
            event_sender,
        }
    }
}

impl TopicState {
    /// Check if the topic still has any publisher or subscriber.
    pub(super) fn still_needed(&self) -> bool {
        // Keep topic alive if either senders or receivers exist.
        // Using || prevents topic closure when senders are dropped while receivers listen.
        !self.command_rx_keys.is_empty() || self.event_sender.receiver_count() > 0
    }

    #[cfg(test)]
    pub(super) fn joined(&self) -> bool {
        !self.neighbors.is_empty()
    }
}

/// Whether a connection is initiated by us (Dial) or by the remote peer (Accept)
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ConnOrigin {
    Accept,
    Dial,
}

#[allow(missing_docs)]
#[stack_error(derive, add_meta, from_sources, std_sources)]
#[non_exhaustive]
pub(super) enum ConnectionLoopError {
    #[error(transparent)]
    Write {
        source: WriteError,
    },
    #[error(transparent)]
    Read {
        source: ReadError,
    },
    #[error(transparent)]
    Connection {
        #[error(std_err)]
        source: iroh::endpoint::ConnectionError,
    },
    ActorDropped {},
}

impl<T> From<mpsc::error::SendError<T>> for ConnectionLoopError {
    fn from(_value: mpsc::error::SendError<T>) -> Self {
        e!(ConnectionLoopError::ActorDropped)
    }
}
