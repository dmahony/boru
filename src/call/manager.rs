//! Builder and actor for the call control subsystem.
//!
//! The actor owns call state and the protocol handler is deliberately a thin
//! transport shim. Frontends only receive a [`CallHandle`]; they never own an
//! Iroh connection.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, RwLock};

use iroh::{
    endpoint::Connection,
    protocol::{AcceptError, ProtocolHandler},
    Endpoint, PublicKey, SecretKey,
};
use n0_error::Result;
use tokio::sync::mpsc;

use super::{wire::HangupReason, CallId, CallKind};

/// ALPN used by call-control connections.
pub const CALL_ALPN: &[u8] = b"/boru-call/1";

const COMMAND_CAPACITY: usize = 256;
const EVENT_CAPACITY: usize = 256;

/// Why a call reached its terminal state.
#[allow(missing_docs)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CallEndReason {
    LocalHangup,
    RemoteHangup,
    ConnectionLost,
    ProtocolError,
    AuthorizationRevoked,
    DeviceError,
    Shutdown,
    NegotiationTimeout,
}

impl From<HangupReason> for CallEndReason {
    fn from(reason: HangupReason) -> Self {
        match reason {
            HangupReason::LocalHangup => Self::LocalHangup,
            HangupReason::RemoteHangup => Self::RemoteHangup,
            HangupReason::ConnectionLost => Self::ConnectionLost,
            HangupReason::ProtocolError => Self::ProtocolError,
            HangupReason::AuthorizationRevoked => Self::AuthorizationRevoked,
            HangupReason::DeviceError => Self::DeviceError,
            HangupReason::Shutdown => Self::Shutdown,
            HangupReason::NegotiationTimeout => Self::NegotiationTimeout,
        }
    }
}

/// Error which prevents a call from becoming active.
#[allow(missing_docs)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CallError {
    Rejected,
    Connection,
    Protocol,
    Authorization,
    Device,
    NegotiationTimeout,
}

/// Placeholder for the statistics payload; fields are added with call stats.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CallStats;

/// Events emitted by the call actor.
#[allow(missing_docs)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CallEvent {
    Incoming {
        call_id: CallId,
        peer: PublicKey,
        kind: CallKind,
    },
    OutgoingRinging {
        call_id: CallId,
        peer: PublicKey,
    },
    Connecting {
        call_id: CallId,
    },
    Active {
        call_id: CallId,
        peer: PublicKey,
        kind: CallKind,
    },
    MediaStateChanged {
        call_id: CallId,
        audio_muted: bool,
        video_enabled: bool,
    },
    Stats(CallStats),
    Ended {
        call_id: CallId,
        reason: CallEndReason,
    },
    Failed {
        call_id: Option<CallId>,
        reason: CallError,
    },
}

impl CallEvent {
    /// Returns true for the only two terminal observations.
    pub const fn is_terminal(&self) -> bool {
        matches!(self, Self::Ended { .. } | Self::Failed { .. })
    }

    fn terminal_call_id(&self) -> Option<CallId> {
        match self {
            Self::Ended { call_id, .. } => Some(*call_id),
            Self::Failed { call_id, .. } => *call_id,
            _ => None,
        }
    }
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
    let mut terminal_calls = HashSet::new();
    let mut media_state = HashMap::new();
    while let Some(command) = command_rx.recv().await {
        let event = match command {
            Command::Start {
                call_id,
                peer,
                kind: _,
            } => Some(CallEvent::OutgoingRinging { call_id, peer }),
            Command::Accept(call_id) => Some(CallEvent::Connecting { call_id }),
            Command::Reject(call_id) => Some(CallEvent::Failed {
                call_id: Some(call_id),
                reason: CallError::Rejected,
            }),
            Command::Hangup(call_id) => Some(CallEvent::Ended {
                call_id,
                reason: CallEndReason::LocalHangup,
            }),
            Command::SetMuted { call_id, muted } => {
                let state = media_state.entry(call_id).or_insert((false, false));
                state.0 = muted;
                Some(CallEvent::MediaStateChanged {
                    call_id,
                    audio_muted: state.0,
                    video_enabled: state.1,
                })
            }
            Command::SetCameraEnabled { call_id, enabled } => {
                let state = media_state.entry(call_id).or_insert((false, false));
                state.1 = enabled;
                Some(CallEvent::MediaStateChanged {
                    call_id,
                    audio_muted: state.0,
                    video_enabled: state.1,
                })
            }
            Command::Incoming(connection) => Some(CallEvent::Incoming {
                call_id: CallId::generate(),
                peer: connection.remote_id(),
                kind: CallKind::Voice,
            }),
        };
        if let Some(event) = event {
            if let Some(call_id) = event.terminal_call_id() {
                // A call may have several teardown paths; expose only the
                // first terminal observation to the UI.
                if !terminal_calls.insert(call_id) {
                    continue;
                }
            }
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
            Some(CallEvent::OutgoingRinging { .. })
        ));
        handle.set_muted(call_id, true).await.unwrap();
        assert!(matches!(
            events.recv().await,
            Some(CallEvent::MediaStateChanged {
                audio_muted: true,
                ..
            })
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

    #[test]
    fn terminal_events_are_at_most_one_per_call() {
        let call_id = CallId::generate();
        let sequence = [
            CallEvent::Connecting { call_id },
            CallEvent::Ended {
                call_id,
                reason: CallEndReason::LocalHangup,
            },
            CallEvent::Failed {
                call_id: Some(call_id),
                reason: CallError::Connection,
            },
        ];
        assert!(
            sequence[0..2]
                .iter()
                .filter(|event| event.is_terminal())
                .count()
                <= 1
        );
        assert_eq!(
            sequence.iter().filter(|event| event.is_terminal()).count(),
            2
        );
        // A producer must suppress the second terminal observation for the
        // same call, regardless of whether it is Ended or Failed.
        let mut terminal_calls = HashSet::new();
        let accepted: Vec<_> = sequence
            .iter()
            .filter(|event| {
                event
                    .terminal_call_id()
                    .is_none_or(|id| terminal_calls.insert(id))
            })
            .collect();
        assert_eq!(
            accepted.iter().filter(|event| event.is_terminal()).count(),
            1
        );
    }
}
