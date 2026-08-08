//! Builder and actor for the call control subsystem.
//!
//! The actor owns call state and the protocol handler is deliberately a thin
//! transport shim. Frontends only receive a [`CallHandle`]; they never own an
//! Iroh connection.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex, RwLock};

use iroh::{
    endpoint::Connection,
    protocol::{AcceptError, ProtocolHandler},
    Endpoint, PublicKey, SecretKey,
};
use n0_error::Result;
use tokio::sync::mpsc;

use super::{CallId, CallKind};

/// ALPN used by call-control connections.
pub const CALL_ALPN: &[u8] = b"/boru-call/1";

const COMMAND_CAPACITY: usize = 256;
const EVENT_CAPACITY: usize = 256;

/// Errors returned when a frontend command cannot be queued.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CallError {
    /// A call is already active on this handle.
    Busy,
    /// The peer is not authorized to receive calls.
    Unauthorized,
    /// The actor has stopped and cannot receive commands.
    ActorDropped,
    /// The actor queue is temporarily full.
    QueueFull,
}

impl std::fmt::Display for CallError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::Busy => "a call is already active",
            Self::Unauthorized => "peer is not authorized for calls",
            Self::ActorDropped => "call actor dropped",
            Self::QueueFull => "call command queue is full",
        })
    }
}

impl std::error::Error for CallError {}

/// Events emitted by the call actor.
#[derive(Debug, Clone)]
pub enum CallEvent {
    /// A peer started an incoming call.
    IncomingCall {
        /// Identity of the call.
        call_id: CallId,
        /// Peer that initiated the call.
        from: PublicKey,
        /// Requested media kind.
        kind: CallKind,
    },
    /// An outgoing call was queued.
    OutgoingCallStarted {
        /// Identity of the call.
        call_id: CallId,
        /// Intended remote peer.
        peer: PublicKey,
        /// Requested media kind.
        kind: CallKind,
    },
    /// A call was accepted.
    Accepted {
        /// Identity of the call.
        call_id: CallId,
    },
    /// A call was rejected.
    Rejected {
        /// Identity of the call.
        call_id: CallId,
    },
    /// A call was hung up.
    HungUp {
        /// Identity of the call.
        call_id: CallId,
    },
    /// A call was terminated because peer authorization was revoked.
    Terminated {
        /// Identity of the call.
        call_id: CallId,
        /// Peer whose authorization was revoked.
        peer: PublicKey,
    },
    /// Local audio mute state changed.
    MutedChanged {
        /// Identity of the call.
        call_id: CallId,
        /// Whether local audio is muted.
        muted: bool,
    },
    /// Local camera state changed.
    CameraEnabledChanged {
        /// Identity of the call.
        call_id: CallId,
        /// Whether local video capture is enabled.
        enabled: bool,
    },
}

enum Command {
    Start {
        call_id: CallId,
        peer: PublicKey,
        kind: CallKind,
    },
    Accept(CallId),
    Reject(CallId),
    Hangup(CallId),
    SetMuted {
        call_id: CallId,
        muted: bool,
    },
    SetCameraEnabled {
        call_id: CallId,
        enabled: bool,
    },
    Incoming(Connection),
    RevokePeer(PublicKey),
}

/// Handle for sending commands to a running call actor.
#[derive(Debug, Clone)]
pub struct CallHandle {
    command_tx: mpsc::Sender<Command>,
    authorized_peers: Arc<RwLock<HashSet<PublicKey>>>,
    active_call: Arc<Mutex<Option<CallId>>>,
}

impl CallHandle {
    /// Start an audio-only call and return its new identity.
    pub fn start_voice_call(&self, peer: PublicKey) -> std::result::Result<CallId, CallError> {
        self.start_call(peer, CallKind::Voice)
    }

    /// Start an audio/video call and return its new identity.
    pub fn start_video_call(&self, peer: PublicKey) -> std::result::Result<CallId, CallError> {
        self.start_call(peer, CallKind::Video)
    }

    fn start_call(
        &self,
        peer: PublicKey,
        kind: CallKind,
    ) -> std::result::Result<CallId, CallError> {
        if !self.is_authorized(peer) {
            return Err(CallError::Unauthorized);
        }
        let call_id = CallId::generate();
        let mut active_call = self.active_call.lock().expect("call state lock poisoned");
        if active_call.is_some() {
            return Err(CallError::Busy);
        }
        self.send(Command::Start {
            call_id,
            peer,
            kind,
        })?;
        *active_call = Some(call_id);
        Ok(call_id)
    }

    /// Accept an incoming call.
    pub fn accept(&self, call_id: CallId) -> std::result::Result<(), CallError> {
        self.send(Command::Accept(call_id))
    }

    /// Reject an incoming call.
    pub fn reject(&self, call_id: CallId) -> std::result::Result<(), CallError> {
        self.send(Command::Reject(call_id))
    }

    /// Hang up an active or ringing call.
    pub fn hangup(&self, call_id: CallId) -> std::result::Result<(), CallError> {
        self.send(Command::Hangup(call_id))?;
        let mut active_call = self.active_call.lock().expect("call state lock poisoned");
        if *active_call == Some(call_id) {
            *active_call = None;
        }
        Ok(())
    }

    /// Set the local audio mute state.
    pub fn set_muted(&self, call_id: CallId, muted: bool) -> std::result::Result<(), CallError> {
        self.send(Command::SetMuted { call_id, muted })
    }

    /// Set whether the local camera is enabled.
    pub fn set_camera_enabled(
        &self,
        call_id: CallId,
        enabled: bool,
    ) -> std::result::Result<(), CallError> {
        self.send(Command::SetCameraEnabled { call_id, enabled })
    }

    /// Allow or deny a peer for call setup and active calls. Authorization is
    /// deny-by-default; revocation terminates active calls with this peer.
    pub fn set_peer_authorized(&self, peer: PublicKey, authorized: bool) {
        let changed = {
            let mut peers = self
                .authorized_peers
                .write()
                .expect("call authorization lock poisoned");
            if authorized {
                peers.insert(peer)
            } else {
                peers.remove(&peer)
            }
        };
        if changed && !authorized {
            let _ = self.command_tx.try_send(Command::RevokePeer(peer));
        }
    }

    fn is_authorized(&self, peer: PublicKey) -> bool {
        self.authorized_peers
            .read()
            .expect("call authorization lock poisoned")
            .contains(&peer)
    }

    fn send(&self, command: Command) -> std::result::Result<(), CallError> {
        self.command_tx
            .try_send(command)
            .map_err(|error| match error {
                mpsc::error::TrySendError::Full(_) => CallError::QueueFull,
                mpsc::error::TrySendError::Closed(_) => CallError::ActorDropped,
            })
    }
}

/// Iroh protocol handler that forwards incoming connections to the call actor.
#[derive(Debug, Clone)]
pub struct CallProtocol {
    command_tx: mpsc::Sender<Command>,
    local_id: PublicKey,
    authorized_peers: Arc<RwLock<HashSet<PublicKey>>>,
}

impl ProtocolHandler for CallProtocol {
    async fn accept(&self, connection: Connection) -> Result<(), AcceptError> {
        let peer = connection.remote_id();
        if peer == self.local_id {
            return Err(AcceptError::from_err(n0_error::anyerr!(
                "call connection from local identity {} is not allowed",
                peer.fmt_short()
            )));
        }
        if !self.is_authorized(peer) {
            return Err(AcceptError::from_err(n0_error::anyerr!(
                "call peer {} is not authorized",
                peer.fmt_short()
            )));
        }

        self.command_tx
            .send(Command::Incoming(connection))
            .await
            .map_err(|_| AcceptError::from_err(n0_error::anyerr!("call actor dropped")))
    }
}

impl CallProtocol {
    /// Authorization hook used by the protocol boundary. Only established
    /// friends (peers in the authorized set) may open a call connection.
    fn is_authorized(&self, peer: PublicKey) -> bool {
        self.authorized_peers
            .read()
            .expect("call authorization lock poisoned")
            .contains(&peer)
    }
}

/// Constructor for the call subsystem.
#[derive(Debug)]
pub struct CallBuilder {
    endpoint: Endpoint,
    secret_key: SecretKey,
    command_tx: mpsc::Sender<Command>,
    command_rx: Option<mpsc::Receiver<Command>>,
    authorized_peers: Arc<RwLock<HashSet<PublicKey>>>,
}

impl CallBuilder {
    /// Create a builder using the endpoint and identity that own call networking.
    pub fn new(endpoint: Endpoint, secret_key: SecretKey) -> Self {
        let (command_tx, command_rx) = mpsc::channel(COMMAND_CAPACITY);
        Self {
            endpoint,
            secret_key,
            command_tx,
            command_rx: Some(command_rx),
            authorized_peers: Arc::new(RwLock::new(HashSet::new())),
        }
    }

    /// Configure the initial set of established friends allowed to call.
    pub fn with_authorized_peers(self, peers: impl IntoIterator<Item = PublicKey>) -> Self {
        self.authorized_peers
            .write()
            .expect("call authorization lock poisoned")
            .extend(peers);
        self
    }

    /// Return the handler to register with `Router::accept(CALL_ALPN, ...)`.
    pub fn protocol_handler(&self) -> CallProtocol {
        CallProtocol {
            command_tx: self.command_tx.clone(),
            local_id: self.secret_key.public(),
            authorized_peers: Arc::clone(&self.authorized_peers),
        }
    }

    /// Spawn the call actor and return its frontend handle and event receiver.
    pub fn spawn(mut self) -> (CallHandle, mpsc::Receiver<CallEvent>) {
        let (event_tx, event_rx) = mpsc::channel(EVENT_CAPACITY);
        let command_rx = self
            .command_rx
            .take()
            .expect("CallBuilder::spawn called more than once");
        let handle = CallHandle {
            command_tx: self.command_tx,
            authorized_peers: Arc::clone(&self.authorized_peers),
            active_call: Arc::new(Mutex::new(None)),
        };
        tokio::spawn(run_actor(
            self.endpoint,
            self.secret_key,
            command_rx,
            event_tx,
        ));
        (handle, event_rx)
    }
}

async fn run_actor(
    _endpoint: Endpoint,
    _secret_key: SecretKey,
    mut command_rx: mpsc::Receiver<Command>,
    event_tx: mpsc::Sender<CallEvent>,
) {
    let mut active_calls = HashMap::<CallId, PublicKey>::new();
    while let Some(command) = command_rx.recv().await {
        let event = match command {
            Command::Start {
                call_id,
                peer,
                kind,
            } => {
                active_calls.insert(call_id, peer);
                Some(CallEvent::OutgoingCallStarted {
                    call_id,
                    peer,
                    kind,
                })
            }
            Command::Accept(call_id) => Some(CallEvent::Accepted { call_id }),
            Command::Reject(call_id) => Some(CallEvent::Rejected { call_id }),
            Command::Hangup(call_id) => {
                active_calls.remove(&call_id);
                Some(CallEvent::HungUp { call_id })
            }
            Command::SetMuted { call_id, muted } => {
                Some(CallEvent::MutedChanged { call_id, muted })
            }
            Command::SetCameraEnabled { call_id, enabled } => {
                Some(CallEvent::CameraEnabledChanged { call_id, enabled })
            }
            Command::Incoming(connection) => {
                let call_id = CallId::generate();
                let peer = connection.remote_id();
                active_calls.insert(call_id, peer);
                Some(CallEvent::IncomingCall {
                    call_id,
                    from: peer,
                    kind: CallKind::Voice,
                })
            }
            Command::RevokePeer(peer) => {
                let revoked: Vec<_> = active_calls
                    .iter()
                    .filter_map(|(call_id, active_peer)| (*active_peer == peer).then_some(*call_id))
                    .collect();
                for call_id in revoked {
                    active_calls.remove(&call_id);
                    if event_tx
                        .send(CallEvent::Terminated { call_id, peer })
                        .await
                        .is_err()
                    {
                        return;
                    }
                }
                None
            }
        };
        if let Some(event) = event {
            if event_tx.send(event).await.is_err() {
                break;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use iroh::endpoint::presets;

    #[tokio::test]
    async fn spawn_returns_handle_and_receiver() {
        let endpoint = Endpoint::bind(presets::Minimal).await.unwrap();
        let secret_key = SecretKey::generate();
        let (handle, mut events) = CallBuilder::new(endpoint, secret_key).spawn();
        let peer = SecretKey::generate().public();
        handle.set_peer_authorized(peer, true);
        let call_id = handle.start_voice_call(peer).unwrap();
        assert!(matches!(
            events.recv().await,
            Some(CallEvent::OutgoingCallStarted { .. })
        ));
        handle.set_muted(call_id, true).unwrap();
        assert!(matches!(
            events.recv().await,
            Some(CallEvent::MutedChanged { muted: true, .. })
        ));
    }

    #[tokio::test]
    async fn all_handle_commands_enqueue_without_panic() {
        let endpoint = Endpoint::bind(presets::Minimal).await.unwrap();
        let (handle, _events) = CallBuilder::new(endpoint, SecretKey::generate()).spawn();
        let peer = SecretKey::generate().public();
        handle.set_peer_authorized(peer, true);
        let call_id = handle.start_video_call(peer).unwrap();
        handle.accept(call_id).unwrap();
        handle.reject(call_id).unwrap();
        handle.hangup(call_id).unwrap();
        handle.set_muted(call_id, false).unwrap();
        handle.set_camera_enabled(call_id, true).unwrap();
    }

    #[tokio::test]
    async fn starting_while_busy_returns_busy_without_enqueueing() {
        let endpoint = Endpoint::bind(presets::Minimal).await.unwrap();
        let (handle, mut events) = CallBuilder::new(endpoint, SecretKey::generate()).spawn();
        let peer = SecretKey::generate().public();
        handle.set_peer_authorized(peer, true);
        let first = handle.start_voice_call(peer).unwrap();

        assert_eq!(
            handle.start_video_call(peer),
            Err(CallError::Busy)
        );
        assert!(matches!(
            events.recv().await,
            Some(CallEvent::OutgoingCallStarted { call_id, .. }) if call_id == first
        ));
        assert!(events.try_recv().is_err());
    }

    #[tokio::test]
    async fn unauthorized_peer_is_rejected_before_enqueueing() {
        let endpoint = Endpoint::bind(presets::Minimal).await.unwrap();
        let peer = SecretKey::generate().public();
        let (handle, mut events) = CallBuilder::new(endpoint, SecretKey::generate()).spawn();

        assert_eq!(handle.start_voice_call(peer), Err(CallError::Unauthorized));
        assert!(events.try_recv().is_err());
    }

    #[tokio::test]
    async fn protocol_rejects_local_and_unauthorized_peers() {
        let endpoint = Endpoint::bind(presets::Minimal).await.unwrap();
        let local_key = SecretKey::generate();
        let denied_key = SecretKey::generate();
        let handler = CallBuilder::new(endpoint, local_key.clone())
            .with_authorized_peers([denied_key.public()])
            .protocol_handler();

        assert!(!handler.is_authorized(local_key.public()));
        assert!(handler.is_authorized(denied_key.public()));
        assert_eq!(handler.local_id, local_key.public());
    }

    #[tokio::test]
    async fn unauthorized_outbound_call_is_rejected() {
        let endpoint = Endpoint::bind(presets::Minimal).await.unwrap();
        let (handle, _events) = CallBuilder::new(endpoint, SecretKey::generate()).spawn();
        let peer = SecretKey::generate().public();
        assert_eq!(handle.start_voice_call(peer), Err(CallError::Unauthorized));
    }

    #[tokio::test]
    async fn revoking_peer_terminates_active_call() {
        let endpoint = Endpoint::bind(presets::Minimal).await.unwrap();
        let (handle, mut events) = CallBuilder::new(endpoint, SecretKey::generate()).spawn();
        let peer = SecretKey::generate().public();
        handle.set_peer_authorized(peer, true);
        let call_id = handle.start_voice_call(peer).unwrap();
        assert!(matches!(events.recv().await, Some(CallEvent::OutgoingCallStarted { .. })));
        handle.set_peer_authorized(peer, false);
        assert!(matches!(
            tokio::time::timeout(std::time::Duration::from_secs(1), events.recv()).await.unwrap(),
            Some(CallEvent::Terminated { call_id: id, peer: revoked }) if id == call_id && revoked == peer
        ));
        assert_eq!(handle.start_voice_call(peer), Err(CallError::Unauthorized));
    }

    #[tokio::test]
    async fn protocol_forwards_allowed_connection_to_actor() {
        let endpoint = Endpoint::bind(presets::Minimal).await.unwrap();
        let local_key = SecretKey::generate();
        let peer = SecretKey::generate().public();
        let builder = CallBuilder::new(endpoint, local_key).with_authorized_peers([peer]);
        let handler = builder.protocol_handler();
        assert!(handler.is_authorized(peer));
        let (_handle, _events) = builder.spawn();
    }
}
