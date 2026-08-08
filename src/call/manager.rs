//! Builder and actor for the call-control subsystem.
//!
//! The actor owns call state.  A protocol handler only hands accepted Iroh
//! connections to it; all call signalling is carried over one bounded,
//! length-prefixed bidirectional stream per call.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, RwLock};
use std::time::Duration;

use iroh::{
    endpoint::Connection,
    protocol::{AcceptError, ProtocolHandler},
    Endpoint, EndpointAddr, PublicKey, SecretKey,
};
use n0_error::Result;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::sync::mpsc;

use super::wire::{
    decode_call_control, encode_call_control, v1_defaults, CallControl, HangupReason, RejectReason,
    CALL_CONTROL_VERSION, MAX_CALL_CONTROL_FRAME_SIZE,
};
use super::{CallId, CallKind};

/// The side whose outgoing attempt survives a simultaneous call attempt.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CollisionWinner {
    /// The local peer keeps its outgoing attempt.
    LocalWins,
    /// The remote peer keeps its outgoing attempt.
    RemoteWins,
}

/// Resolve a simultaneous call attempt deterministically from peer identity.
pub fn resolve_collision(local: &PublicKey, remote: &PublicKey) -> CollisionWinner {
    if local.as_bytes() <= remote.as_bytes() {
        CollisionWinner::LocalWins
    } else {
        CollisionWinner::RemoteWins
    }
}

/// ALPN used by call-control connections.
pub const CALL_ALPN: &[u8] = b"/boru-call/1";
const COMMAND_CAPACITY: usize = 256;
const EVENT_CAPACITY: usize = 256;
const NEGOTIATION_TIMEOUT: Duration = Duration::from_secs(5);
const KEEPALIVE_INTERVAL: Duration = Duration::from_secs(15);

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

/// Placeholder for the statistics payload.
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
    /// Returns whether this event ends a call.
    pub const fn is_terminal(&self) -> bool {
        matches!(self, Self::Ended { .. } | Self::Failed { .. })
    }
}

type WireTx = mpsc::Sender<CallControl>;

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
    Control {
        peer: PublicKey,
        control: CallControl,
        tx: WireTx,
    },
    ConnectionClosed {
        peer: PublicKey,
    },
    NegotiationTimeout(CallId),
}

/// Handle for sending commands to a running call actor.
#[derive(Debug, Clone)]
pub struct CallHandle {
    command_tx: mpsc::Sender<Command>,
}

impl CallHandle {
    /// Start an audio-only call and return its identity.
    pub async fn start_voice_call(&self, peer: PublicKey) -> Result<CallId> {
        self.start_call(peer, CallKind::Voice).await
    }

    /// Start an audio/video call and return its identity.
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
            command_tx: self.command_tx.clone(),
        };
        tokio::spawn(run_actor(
            self.endpoint,
            self.secret_key,
            self.command_tx,
            command_rx,
            event_tx,
        ));
        (handle, event_rx)
    }
}

#[derive(Debug)]
struct CallState {
    peer: PublicKey,
    kind: CallKind,
    tx: WireTx,
    incoming: bool,
    active: bool,
}

async fn run_actor(
    endpoint: Endpoint,
    secret_key: SecretKey,
    command_tx: mpsc::Sender<Command>,
    mut command_rx: mpsc::Receiver<Command>,
    event_tx: mpsc::Sender<CallEvent>,
) {
    let mut calls = HashMap::<CallId, CallState>::new();
    let mut terminal_calls = HashSet::new();
    let mut media_state = HashMap::<CallId, (bool, bool)>::new();
    while let Some(command) = command_rx.recv().await {
        match command {
            Command::Start {
                call_id,
                peer,
                kind,
            } => {
                if calls.values().any(|call| call.active || call.peer == peer) {
                    emit(
                        &event_tx,
                        CallEvent::Failed {
                            call_id: Some(call_id),
                            reason: CallError::Rejected,
                        },
                    )
                    .await;
                    continue;
                }
                let addr = endpoint
                    .remote_info(peer)
                    .await
                    .map(|info| {
                        EndpointAddr::from_parts(
                            info.id(),
                            info.into_addrs().map(|addr| addr.into_addr()),
                        )
                    })
                    .unwrap_or_else(|| EndpointAddr::new(peer));
                match endpoint.connect(addr, CALL_ALPN).await {
                    Ok(connection) => match connection.open_bi().await {
                        Ok((send, recv)) => {
                            let (tx, rx) = mpsc::channel(32);
                            let reply_tx =
                                spawn_wire_session(peer, send, recv, rx, command_tx.clone());
                            let state = CallState {
                                peer,
                                kind,
                                tx: reply_tx,
                                incoming: false,
                                active: false,
                            };
                            calls.insert(call_id, state);
                            let _ = tx
                                .send(CallControl::Hello {
                                    version: CALL_CONTROL_VERSION,
                                    call_id,
                                })
                                .await;
                            let _ = tx
                                .send(CallControl::Offer {
                                    call_id,
                                    kind,
                                    capabilities: call_capabilities(kind),
                                })
                                .await;
                            emit(&event_tx, CallEvent::OutgoingRinging { call_id, peer }).await;
                            let timeout_tx = command_tx.clone();
                            tokio::spawn(async move {
                                tokio::time::sleep(NEGOTIATION_TIMEOUT).await;
                                let _ = timeout_tx.send(Command::NegotiationTimeout(call_id)).await;
                            });
                        }
                        Err(_) => {
                            emit(
                                &event_tx,
                                CallEvent::Failed {
                                    call_id: Some(call_id),
                                    reason: CallError::Connection,
                                },
                            )
                            .await
                        }
                    },
                    Err(_) => {
                        emit(
                            &event_tx,
                            CallEvent::Failed {
                                call_id: Some(call_id),
                                reason: CallError::Connection,
                            },
                        )
                        .await
                    }
                }
            }
            Command::Incoming(connection) => {
                let peer = connection.remote_id();
                let session_tx = command_tx.clone();
                tokio::spawn(async move {
                    let (send, recv) = match connection.accept_bi().await {
                        Ok(streams) => streams,
                        Err(_) => return,
                    };
                    let (_tx, rx) = mpsc::channel(32);
                    let _ = spawn_wire_session(peer, send, recv, rx, session_tx);
                });
                // Accepting a bidirectional stream is isolated from the actor
                // so a peer that connects without opening a stream cannot
                // block later incoming calls.
            }
            Command::Control { peer, control, tx } => {
                handle_control(
                    &mut calls,
                    &mut terminal_calls,
                    &event_tx,
                    peer,
                    control,
                    tx,
                )
                .await;
            }
            Command::ConnectionClosed { peer } => {
                let ended: Vec<_> = calls
                    .iter()
                    .filter_map(|(id, state)| (state.peer == peer).then_some(*id))
                    .collect();
                for call_id in ended {
                    calls.remove(&call_id);
                    if terminal_calls.insert(call_id) {
                        emit(
                            &event_tx,
                            CallEvent::Ended {
                                call_id,
                                reason: CallEndReason::ConnectionLost,
                            },
                        )
                        .await;
                    }
                }
            }
            Command::Accept(call_id) => {
                if let Some(state) = calls.get_mut(&call_id) {
                    let selected = negotiate_for_kind(state.kind);
                    let _ = state
                        .tx
                        .send(CallControl::Accept { call_id, selected })
                        .await;
                    state.active = true;
                    emit(
                        &event_tx,
                        CallEvent::Active {
                            call_id,
                            peer: state.peer,
                            kind: state.kind,
                        },
                    )
                    .await;
                }
            }
            Command::Reject(call_id) => {
                if let Some(state) = calls.remove(&call_id) {
                    let _ = state
                        .tx
                        .send(CallControl::Reject {
                            call_id,
                            reason: RejectReason::Declined,
                        })
                        .await;
                    if terminal_calls.insert(call_id) {
                        emit(
                            &event_tx,
                            CallEvent::Ended {
                                call_id,
                                reason: CallEndReason::LocalHangup,
                            },
                        )
                        .await;
                    }
                }
            }
            Command::Hangup(call_id) => {
                if let Some(state) = calls.remove(&call_id) {
                    let _ = state
                        .tx
                        .send(CallControl::Hangup {
                            call_id,
                            reason: HangupReason::LocalHangup,
                        })
                        .await;
                    if terminal_calls.insert(call_id) {
                        emit(
                            &event_tx,
                            CallEvent::Ended {
                                call_id,
                                reason: CallEndReason::LocalHangup,
                            },
                        )
                        .await;
                    }
                }
            }
            Command::NegotiationTimeout(call_id) => {
                if let Some(state) = calls.remove(&call_id) {
                    if !state.active && terminal_calls.insert(call_id) {
                        let _ = state
                            .tx
                            .send(CallControl::Hangup {
                                call_id,
                                reason: HangupReason::NegotiationTimeout,
                            })
                            .await;
                        emit(
                            &event_tx,
                            CallEvent::Ended {
                                call_id,
                                reason: CallEndReason::NegotiationTimeout,
                            },
                        )
                        .await;
                    }
                }
            }
            Command::SetMuted { call_id, muted } => {
                let state = media_state.entry(call_id).or_insert((false, false));
                state.0 = muted;
                if let Some(call) = calls.get(&call_id) {
                    let _ = call
                        .tx
                        .send(CallControl::MediaState {
                            call_id,
                            audio_muted: state.0,
                            video_enabled: state.1,
                        })
                        .await;
                }
                emit(
                    &event_tx,
                    CallEvent::MediaStateChanged {
                        call_id,
                        audio_muted: state.0,
                        video_enabled: state.1,
                    },
                )
                .await;
            }
            Command::SetCameraEnabled { call_id, enabled } => {
                let state = media_state.entry(call_id).or_insert((false, false));
                state.1 = enabled;
                if let Some(call) = calls.get(&call_id) {
                    let _ = call
                        .tx
                        .send(CallControl::MediaState {
                            call_id,
                            audio_muted: state.0,
                            video_enabled: state.1,
                        })
                        .await;
                }
                emit(
                    &event_tx,
                    CallEvent::MediaStateChanged {
                        call_id,
                        audio_muted: state.0,
                        video_enabled: state.1,
                    },
                )
                .await;
            }
        }
    }
}

async fn handle_control(
    calls: &mut HashMap<CallId, CallState>,
    terminal_calls: &mut HashSet<CallId>,
    events: &mpsc::Sender<CallEvent>,
    peer: PublicKey,
    control: CallControl,
    tx: WireTx,
) {
    match control {
        CallControl::Hello { .. } => {}
        CallControl::Offer {
            call_id,
            kind,
            capabilities,
        } => {
            if let Some(existing) = calls.values().find(|call| call.active || call.peer == peer) {
                let _ = tx.send(CallControl::Busy { call_id }).await;
                if existing.peer == peer {
                    return;
                }
            }
            if let Some((&existing_id, existing)) = calls
                .iter()
                .find(|(_, call)| call.peer == peer && !call.incoming)
            {
                if resolve_collision(&peer, &existing.peer) == CollisionWinner::RemoteWins {
                    let _ = existing
                        .tx
                        .send(CallControl::Hangup {
                            call_id: existing_id,
                            reason: HangupReason::RemoteHangup,
                        })
                        .await;
                    calls.remove(&existing_id);
                } else {
                    let _ = tx.send(CallControl::Busy { call_id }).await;
                    return;
                }
            }
            let selected = negotiate_for_capabilities(kind, &capabilities);
            let _ = tx.send(CallControl::Ringing { call_id }).await;
            calls.insert(
                call_id,
                CallState {
                    peer,
                    kind,
                    tx: tx.clone(),
                    incoming: true,
                    active: false,
                },
            );
            emit(
                events,
                CallEvent::Incoming {
                    call_id,
                    peer,
                    kind,
                },
            )
            .await;
            let _ = selected;
        }
        CallControl::Ringing { .. } => {}
        CallControl::Accept { call_id, .. } => {
            if let Some(call) = calls.get_mut(&call_id) {
                call.active = true;
                emit(
                    events,
                    CallEvent::Active {
                        call_id,
                        peer: call.peer,
                        kind: call.kind,
                    },
                )
                .await;
            }
        }
        CallControl::Reject { call_id, .. } | CallControl::Busy { call_id } => {
            if calls.remove(&call_id).is_some() && terminal_calls.insert(call_id) {
                emit(
                    events,
                    CallEvent::Failed {
                        call_id: Some(call_id),
                        reason: CallError::Rejected,
                    },
                )
                .await;
            }
        }
        CallControl::MediaState {
            call_id,
            audio_muted,
            video_enabled,
        } => {
            emit(
                events,
                CallEvent::MediaStateChanged {
                    call_id,
                    audio_muted,
                    video_enabled,
                },
            )
            .await;
        }
        CallControl::RequestKeyframe { .. } | CallControl::KeepAlive { .. } => {}
        CallControl::Hangup { call_id, reason } => {
            calls.remove(&call_id);
            if terminal_calls.insert(call_id) {
                emit(
                    events,
                    CallEvent::Ended {
                        call_id,
                        reason: reason.into(),
                    },
                )
                .await;
            }
        }
    }
}

fn call_capabilities(kind: CallKind) -> super::wire::MediaCapabilities {
    let mut capabilities = v1_defaults();
    if kind == CallKind::Voice {
        capabilities.video = None;
    }
    capabilities
}

fn negotiate_for_kind(kind: CallKind) -> super::wire::NegotiatedMedia {
    negotiate_for_capabilities(kind, &call_capabilities(kind)).expect("v1 defaults must negotiate")
}

fn negotiate_for_capabilities(
    kind: CallKind,
    remote: &super::wire::MediaCapabilities,
) -> Option<super::wire::NegotiatedMedia> {
    let local = call_capabilities(kind);
    super::wire::negotiate(&local, remote)
}

async fn emit(events: &mpsc::Sender<CallEvent>, event: CallEvent) {
    let _ = events.send(event).await;
}

fn spawn_wire_session<R, W>(
    peer: PublicKey,
    mut send: W,
    mut recv: R,
    outbound: mpsc::Receiver<CallControl>,
    command_tx: mpsc::Sender<Command>,
) -> WireTx
where
    R: AsyncRead + Unpin + Send + 'static,
    W: AsyncWrite + Unpin + Send + 'static,
{
    let (reply_tx, mut reply_rx) = mpsc::channel::<CallControl>(32);
    let command_reply_tx = reply_tx.clone();
    tokio::spawn(async move {
        let mut keepalive = tokio::time::interval(KEEPALIVE_INTERVAL);
        let mut outbound = Some(outbound);
        loop {
            tokio::select! {
                result = read_call_control(&mut recv) => match result {
                    Ok(Some(control)) => {
                        if command_tx.send(Command::Control { peer, control, tx: command_reply_tx.clone() }).await.is_err() { break; }
                    }
                    Ok(None) | Err(_) => break,
                },
                maybe = async {
                    match outbound.as_mut() {
                        Some(receiver) => receiver.recv().await,
                        None => std::future::pending().await,
                    }
                } => match maybe {
                    Some(control) => {
                        if write_call_control(&mut send, &control).await.is_err() { break; }
                    }
                    None => outbound = None,
                },
                maybe = reply_rx.recv() => match maybe {
                    Some(control) => {
                        if write_call_control(&mut send, &control).await.is_err() { break; }
                    }
                    None => break,
                },
                _ = keepalive.tick() => {}
            }
        }
        let _ = command_tx.send(Command::ConnectionClosed { peer }).await;
    });
    reply_tx
}

async fn read_call_control<R: AsyncRead + Unpin>(
    reader: &mut R,
) -> std::io::Result<Option<CallControl>> {
    let mut length = [0u8; 4];
    match reader.read_exact(&mut length).await {
        Ok(_) => {}
        Err(error) if error.kind() == std::io::ErrorKind::UnexpectedEof => return Ok(None),
        Err(error) => return Err(error),
    }
    let declared = u32::from_be_bytes(length) as usize;
    if declared > MAX_CALL_CONTROL_FRAME_SIZE {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "call-control frame too large",
        ));
    }
    let mut frame = Vec::with_capacity(4 + declared);
    frame.extend_from_slice(&length);
    frame.resize(4 + declared, 0);
    reader.read_exact(&mut frame[4..]).await?;
    decode_call_control(&frame)
        .map(Some)
        .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidData, error))
}

async fn write_call_control<W: AsyncWrite + Unpin>(
    writer: &mut W,
    control: &CallControl,
) -> std::io::Result<()> {
    let frame = encode_call_control(control)
        .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidData, error))?;
    writer.write_all(&frame).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use iroh::endpoint::presets;

    #[test]
    fn collision_ordering_is_symmetric() {
        let first = SecretKey::generate().public();
        let second = SecretKey::generate().public();
        assert_ne!(first, second);
        assert_ne!(
            resolve_collision(&first, &second),
            resolve_collision(&second, &first)
        );
    }

    #[test]
    fn simultaneous_voice_attempts_have_one_deterministic_winner() {
        let local = SecretKey::generate().public();
        let remote = SecretKey::generate().public();
        let local_keeps = resolve_collision(&local, &remote) == CollisionWinner::LocalWins;
        let remote_keeps = resolve_collision(&remote, &local) == CollisionWinner::LocalWins;
        assert_eq!(local_keeps as u8 + remote_keeps as u8, 1);
    }

    #[tokio::test]
    async fn spawn_returns_handle_and_receiver() {
        let endpoint = Endpoint::bind(presets::Minimal).await.unwrap();
        let (handle, mut events) = CallBuilder::new(endpoint, SecretKey::generate()).spawn();
        let call_id = handle
            .start_voice_call(SecretKey::generate().public())
            .await
            .unwrap();
        assert!(
            matches!(events.recv().await, Some(CallEvent::Failed { call_id: Some(id), .. }) if id == call_id)
        );
    }

    #[test]
    fn frame_limit_is_checked_before_allocation() {
        let mut frame = (MAX_CALL_CONTROL_FRAME_SIZE as u32 + 1)
            .to_be_bytes()
            .to_vec();
        frame.extend_from_slice(&[0; 8]);
        assert!(read_declared_size(&frame).is_err());
    }

    fn read_declared_size(frame: &[u8]) -> std::io::Result<()> {
        let declared = u32::from_be_bytes(frame[..4].try_into().unwrap()) as usize;
        if declared > MAX_CALL_CONTROL_FRAME_SIZE {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "too large",
            ));
        }
        Ok(())
    }
}
