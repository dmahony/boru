//! Builder and actor for the call control subsystem.
//!
//! The actor owns call state and the protocol handler is deliberately a thin
//! transport shim. Frontends only receive a [`CallHandle`]; they never own an
//! Iroh connection.

use std::collections::HashSet;
use std::sync::{Arc, RwLock};

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
}

/// Handle for sending commands to a running call actor.
#[derive(Debug, Clone)]
pub struct CallHandle {
    command_tx: mpsc::Sender<Command>,
}

impl CallHandle {
    /// Start an audio-only call and return its new identity.
    pub async fn start_voice_call(&self, peer: PublicKey) -> Result<CallId> {
        self.start_call(peer, CallKind::Voice).await
    }

    /// Start an audio/video call and return its new identity.
    pub async fn start_video_call(&self, peer: PublicKey) -> Result<CallId> {
        self.start_call(peer, CallKind::Video).await
    }

    async fn start_call(&self, peer: PublicKey, kind: CallKind) -> Result<CallId> {
        let call_id = CallId::generate();
        self.send(Command::Start {
            call_id,
            peer,
            kind,
        })
        .await?;
        Ok(call_id)
    }

    /// Accept an incoming call.
    pub async fn accept(&self, call_id: CallId) -> Result<()> {
        self.send(Command::Accept(call_id)).await
    }

    /// Reject an incoming call.
    pub async fn reject(&self, call_id: CallId) -> Result<()> {
        self.send(Command::Reject(call_id)).await
    }

    /// Hang up an active or ringing call.
    pub async fn hangup(&self, call_id: CallId) -> Result<()> {
        self.send(Command::Hangup(call_id)).await
    }

    /// Set the local audio mute state.
    pub async fn set_muted(&self, call_id: CallId, muted: bool) -> Result<()> {
        self.send(Command::SetMuted { call_id, muted }).await
    }

    /// Set whether the local camera is enabled.
    pub async fn set_camera_enabled(&self, call_id: CallId, enabled: bool) -> Result<()> {
        self.send(Command::SetCameraEnabled { call_id, enabled })
            .await
    }

    async fn send(&self, command: Command) -> Result<()> {
        self.command_tx
            .send(command)
            .await
            .map_err(|_| n0_error::anyerr!("call actor dropped"))
    }
}

/// Iroh protocol handler that forwards incoming connections to the call actor.
#[derive(Debug, Clone)]
pub struct CallProtocol {
    command_tx: mpsc::Sender<Command>,
    denied_peers: Arc<RwLock<HashSet<PublicKey>>>,
}

impl ProtocolHandler for CallProtocol {
    async fn accept(&self, connection: Connection) -> Result<(), AcceptError> {
        let peer = connection.remote_id();
        if self
            .denied_peers
            .read()
            .expect("call authorization lock poisoned")
            .contains(&peer)
        {
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

/// Constructor for the call subsystem.
#[derive(Debug)]
pub struct CallBuilder {
    endpoint: Endpoint,
    secret_key: SecretKey,
    command_tx: mpsc::Sender<Command>,
    command_rx: Option<mpsc::Receiver<Command>>,
    denied_peers: Arc<RwLock<HashSet<PublicKey>>>,
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
            denied_peers: Arc::new(RwLock::new(HashSet::new())),
        }
    }

    /// Configure the initial set of peers denied from starting calls.
    pub fn with_denied_peers(self, peers: impl IntoIterator<Item = PublicKey>) -> Self {
        self.denied_peers
            .write()
            .expect("call authorization lock poisoned")
            .extend(peers);
        self
    }

    /// Return the handler to register with `Router::accept(CALL_ALPN, ...)`.
    pub fn protocol_handler(&self) -> CallProtocol {
        CallProtocol {
            command_tx: self.command_tx.clone(),
            denied_peers: Arc::clone(&self.denied_peers),
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
    while let Some(command) = command_rx.recv().await {
        let event = match command {
            Command::Start {
                call_id,
                peer,
                kind,
            } => Some(CallEvent::OutgoingCallStarted {
                call_id,
                peer,
                kind,
            }),
            Command::Accept(call_id) => Some(CallEvent::Accepted { call_id }),
            Command::Reject(call_id) => Some(CallEvent::Rejected { call_id }),
            Command::Hangup(call_id) => Some(CallEvent::HungUp { call_id }),
            Command::SetMuted { call_id, muted } => {
                Some(CallEvent::MutedChanged { call_id, muted })
            }
            Command::SetCameraEnabled { call_id, enabled } => {
                Some(CallEvent::CameraEnabledChanged { call_id, enabled })
            }
            Command::Incoming(connection) => Some(CallEvent::IncomingCall {
                call_id: CallId::generate(),
                from: connection.remote_id(),
                kind: CallKind::Voice,
            }),
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
        let call_id = handle.start_voice_call(peer).await.unwrap();
        assert!(matches!(
            events.recv().await,
            Some(CallEvent::OutgoingCallStarted { .. })
        ));
        handle.set_muted(call_id, true).await.unwrap();
        assert!(matches!(
            events.recv().await,
            Some(CallEvent::MutedChanged { muted: true, .. })
        ));
    }

    #[tokio::test]
    async fn all_handle_commands_enqueue_without_panic() {
        let endpoint = Endpoint::bind(presets::Minimal).await.unwrap();
        let (handle, _events) = CallBuilder::new(endpoint, SecretKey::generate()).spawn();
        let call_id = handle
            .start_video_call(SecretKey::generate().public())
            .await
            .unwrap();
        handle.accept(call_id).await.unwrap();
        handle.reject(call_id).await.unwrap();
        handle.hangup(call_id).await.unwrap();
        handle.set_muted(call_id, false).await.unwrap();
        handle.set_camera_enabled(call_id, true).await.unwrap();
    }
}
