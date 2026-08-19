//! Builder and actor for the call-control subsystem.
//!
//! The actor owns call state.  A protocol handler only hands accepted Iroh
//! connections to it; all call signalling is carried over one bounded,
//! length-prefixed bidirectional stream per call.

use std::collections::{HashMap, HashSet};
use std::sync::{
    atomic::{AtomicBool, AtomicU64, Ordering},
    Arc, RwLock,
};
use std::time::{Duration, Instant};

use iroh::{
    endpoint::Connection,
    protocol::{AcceptError, ProtocolHandler},
    Endpoint, EndpointAddr, PublicKey, SecretKey,
};
use n0_error::Result;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;
use tracing::warn;

use super::adaptation::{AdaptationController, AdaptationDecision};
#[cfg(feature = "voice-calls")]
use super::audio::receive::AudioPlaybackControl;
use super::media::{media_reader, MediaDatagram, MediaReaderEvent};
use super::session::{CallSession, SessionSignal, SessionState};
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

/// Queue a `RevokePeer` command without silently dropping it.
///
/// `set_peer_authorized` is a synchronous API (called from UI code), so this
/// cannot await the bounded command channel.  We fast-path `try_send`; if the
/// queue is full or closed, we count the failure and spawn a best-effort task
/// to deliver the revocation when a runtime is available.  Revocation is
/// security-critical: silently dropping it would leave a revoked peer's call
/// channel open.
fn enqueue_revoke_peer(command_tx: &mpsc::Sender<Command>, peer: PublicKey) {
    if command_tx.try_send(Command::RevokePeer(peer)).is_ok() {
        return;
    }
    CALL_REVOKE_SEND_FAILURES.fetch_add(1, Ordering::Relaxed);
    warn!(
        peer = %peer.fmt_short(),
        "call revoke command queue full; retrying asynchronously"
    );
    if let Ok(handle) = tokio::runtime::Handle::try_current() {
        let command_tx = command_tx.clone();
        handle.spawn(async move {
            if let Err(e) = command_tx.send(Command::RevokePeer(peer)).await {
                warn!(
                    peer = %peer.fmt_short(),
                    error = %e,
                    "call revoke command lost: actor dropped"
                );
            }
        });
    }
}

/// ALPN used by call-control connections.
pub const CALL_ALPN: &[u8] = b"/boru-call/1";
const COMMAND_CAPACITY: usize = 256;
const EVENT_CAPACITY: usize = 256;

/// Counter of `RevokePeer` commands that could not be queued on the first
/// attempt and had to be retried asynchronously (BORU-AUDIT-08).
static CALL_REVOKE_SEND_FAILURES: AtomicU64 = AtomicU64::new(0);
const NEGOTIATION_TIMEOUT: Duration = Duration::from_secs(5);
const KEEPALIVE_INTERVAL: Duration = Duration::from_secs(15);
const STATS_INTERVAL: Duration = Duration::from_secs(1);
const CALL_SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(2);
/// Maximum incoming offers retained while the user decides what to do.
///
/// This is checked before inserting a peer-controlled call id into `calls`.
const MAX_PENDING_CALL_OFFERS: usize = 32;

/// Monotonically increasing identity for a call incarnation.
pub type CallGeneration = u64;

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
    Busy,
    Connection,
    Protocol,
    /// The peer is not currently authorized for calls.
    Unauthorized,
    Authorization,
    Device,
    NegotiationTimeout,
}

/// A low-frequency snapshot of local call media health.
///
/// Counters are cumulative for the lifetime of the call actor.  The actor
/// emits snapshots once per second; media paths must update the accumulator,
/// rather than emitting an event for every packet.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CallStats {
    /// Round-trip time measured by the control/transport path.
    pub rtt: Duration,
    /// Audio packets handed to the network.
    pub audio_packets_sent: u64,
    /// Audio packets accepted from the network.
    pub audio_packets_received: u64,
    /// Missing audio sequence numbers inferred from received packets.
    pub audio_packets_lost: u64,
    /// Audio packets arriving after the current receive sequence.
    pub audio_packets_late: u64,
    /// Estimated mean audio inter-arrival jitter in milliseconds.
    pub audio_jitter_ms: u64,
    /// Audio playback ticks that had no packet available.
    pub audio_playback_underruns: u64,
    /// Video packets handed to the network.
    pub video_packets_sent: u64,
    /// Video packets accepted from the network.
    pub video_packets_received: u64,
    /// Video packets dropped before decoding.
    pub video_packets_dropped: u64,
    /// Video frames handed to the encoder.
    pub video_frames_encoded: u64,
    /// Video frames successfully decoded.
    pub video_frames_decoded: u64,
    /// Decoded frames replaced before presentation.
    pub video_frames_dropped: u64,
    /// Requests sent to restart decoding from a keyframe.
    pub keyframe_requests: u64,
    /// Estimated send bitrate in bits per second.
    pub estimated_send_bitrate: u64,
    /// Estimated receive bitrate in bits per second.
    pub estimated_receive_bitrate: u64,
}

impl Default for CallStats {
    fn default() -> Self {
        Self {
            rtt: Duration::ZERO,
            audio_packets_sent: 0,
            audio_packets_received: 0,
            audio_packets_lost: 0,
            audio_packets_late: 0,
            audio_jitter_ms: 0,
            audio_playback_underruns: 0,
            video_packets_sent: 0,
            video_packets_received: 0,
            video_packets_dropped: 0,
            video_frames_encoded: 0,
            video_frames_decoded: 0,
            video_frames_dropped: 0,
            keyframe_requests: 0,
            estimated_send_bitrate: 0,
            estimated_receive_bitrate: 0,
        }
    }
}

#[derive(Debug, Default)]
struct CallStatsAccumulator {
    snapshot: CallStats,
    last_audio_sequence: Option<u32>,
    last_video_sequence: Option<u32>,
    audio_jitter: AudioJitterStats,
}

#[derive(Debug, Default)]
struct AudioJitterStats {
    estimate_ms: u64,
    target_ms: u64,
    last_arrival: Option<Instant>,
    last_sequence: Option<u32>,
}

impl AudioJitterStats {
    fn observe(&mut self, sequence: u32, arrival: Instant) {
        if self.target_ms == 0 {
            self.target_ms = 75;
        }
        let in_order = self
            .last_sequence
            .is_none_or(|last| sequence.wrapping_sub(last) < 0x8000_0000);
        if !in_order {
            return;
        }
        if let Some(previous) = self.last_arrival {
            let deviation = arrival
                .saturating_duration_since(previous)
                .as_millis()
                .abs_diff(20) as u64;
            self.estimate_ms = (self.estimate_ms * 7 + deviation) / 8;
            let desired = (40 + self.estimate_ms.saturating_mul(2)).clamp(40, 200);
            if desired.abs_diff(self.target_ms) > 4 {
                self.target_ms = if desired > self.target_ms {
                    (self.target_ms + 5).min(desired)
                } else {
                    self.target_ms.saturating_sub(5).max(desired)
                };
            }
        }
        self.last_arrival = Some(arrival);
        self.last_sequence = Some(sequence);
    }
}

impl CallStatsAccumulator {
    fn observe_received(&mut self, packet: &MediaDatagram, arrival: Instant) {
        let (last, previous) = match packet.kind {
            super::media::MediaKind::Audio => {
                self.snapshot.audio_packets_received =
                    self.snapshot.audio_packets_received.saturating_add(1);
                let previous = self.last_audio_sequence;
                self.audio_jitter.observe(packet.sequence, arrival);
                self.snapshot.audio_jitter_ms = self.audio_jitter.target_ms;
                (&mut self.last_audio_sequence, previous)
            }
            super::media::MediaKind::Video => {
                self.snapshot.video_packets_received =
                    self.snapshot.video_packets_received.saturating_add(1);
                let previous = self.last_video_sequence;
                (&mut self.last_video_sequence, previous)
            }
        };
        if let Some(previous) = previous {
            let delta = packet.sequence.wrapping_sub(previous);
            if delta == 0 || delta > 0x8000_0000 {
                match packet.kind {
                    super::media::MediaKind::Audio => {
                        self.snapshot.audio_packets_late =
                            self.snapshot.audio_packets_late.saturating_add(1)
                    }
                    super::media::MediaKind::Video => {
                        self.snapshot.video_packets_dropped =
                            self.snapshot.video_packets_dropped.saturating_add(1)
                    }
                }
            } else if packet.kind == super::media::MediaKind::Audio && delta > 1 {
                self.snapshot.audio_packets_lost = self
                    .snapshot
                    .audio_packets_lost
                    .saturating_add((delta - 1) as u64);
            }
        }
        if previous.is_none_or(|previous| packet.sequence.wrapping_sub(previous) < 0x8000_0000) {
            *last = Some(packet.sequence);
        }
    }

    fn observe_malformed(&mut self) {
        self.snapshot.video_packets_dropped = self.snapshot.video_packets_dropped.saturating_add(1);
    }

    fn snapshot(&self) -> CallStats {
        self.snapshot
    }
}

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
    MediaReceived {
        peer: PublicKey,
        datagram: MediaDatagram,
    },
    MediaMalformed {
        peer: PublicKey,
    },
    Stats(CallStats),
    /// A changed congestion decision, with audio taking priority over video.
    AdaptationChanged(AdaptationDecision),
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

    SetMuted {
        call_id: CallId,
        muted: bool,
    },
    SetDeafened {
        call_id: CallId,
        deafened: bool,
    },
    SetCameraEnabled {
        call_id: CallId,
        enabled: bool,
    },
    Incoming(Connection),
    RevokePeer(PublicKey),
    Control {
        peer: PublicKey,
        control: CallControl,
        tx: WireTx,
        connection: Connection,
    },
    ConnectionClosed {
        peer: PublicKey,
    },
    Media {
        peer: PublicKey,
        event: MediaReaderEvent,
    },
    NegotiationTimeout(CallId),
    Terminate {
        call_id: CallId,
        generation: CallGeneration,
        reason: HangupReason,
    },
    Reconnect(CallId),
    Shutdown,
}

/// Handle for sending commands to a running call actor.
#[derive(Debug, Clone)]
pub struct CallHandle {
    command_tx: mpsc::Sender<Command>,
    authorized_peers: Arc<RwLock<HashSet<PublicKey>>>,
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
        if !self.is_peer_authorized(peer) {
            return Err(n0_error::anyerr!(
                "call peer {} is not authorized ({:?})",
                peer.fmt_short(),
                CallError::Unauthorized
            ));
        }
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
        self.terminate_call(call_id, 0, HangupReason::LocalHangup)
            .await
    }

    /// Request a new media generation while retaining the call identity.
    pub async fn reconnect(&self, call_id: CallId) -> Result<()> {
        self.send(Command::Reconnect(call_id)).await
    }

    /// Route any call-ending condition through the actor's single termination path.
    ///
    /// A generation of zero means "the current generation" and is intended for
    /// frontend calls. Background tasks must pass the generation they captured.
    pub async fn terminate_call(
        &self,
        call_id: CallId,
        generation: CallGeneration,
        reason: HangupReason,
    ) -> Result<()> {
        self.send(Command::Terminate {
            call_id,
            generation,
            reason,
        })
        .await
    }

    /// Terminate every call during application shutdown.
    pub async fn shutdown(&self) -> Result<()> {
        self.send(Command::Shutdown).await
    }
    /// Set the local audio mute state.
    pub async fn set_muted(&self, call_id: CallId, muted: bool) -> Result<()> {
        self.send(Command::SetMuted { call_id, muted }).await
    }
    /// Set local playback suppression without notifying the peer.
    pub async fn set_deafened(&self, call_id: CallId, deafened: bool) -> Result<()> {
        self.send(Command::SetDeafened { call_id, deafened }).await
    }
    /// Set whether the local camera is enabled.
    pub async fn set_camera_enabled(&self, call_id: CallId, enabled: bool) -> Result<()> {
        self.send(Command::SetCameraEnabled { call_id, enabled })
            .await
    }

    /// Authorize or revoke a peer for call setup and active calls.
    ///
    /// Revocation is routed through the actor's generation-aware termination
    /// path, so stale cleanup cannot affect a later call incarnation.
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
            enqueue_revoke_peer(&self.command_tx, peer);
        }
    }

    fn is_peer_authorized(&self, peer: PublicKey) -> bool {
        self.authorized_peers
            .read()
            .expect("call authorization lock poisoned")
            .contains(&peer)
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
        if !self
            .authorized_peers
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

    /// Configure the initial set of peers denied from starting calls.
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
            command_tx: self.command_tx.clone(),
            authorized_peers: Arc::clone(&self.authorized_peers),
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
    local_audio_muted: bool,
    remote_audio_muted: bool,
    local_video_enabled: bool,
    remote_video_enabled: bool,
    generation: CallGeneration,
    session: CallSession,
    ending: bool,
    runtime: CallRuntime,
}

impl CallState {
    fn is_active(&self) -> bool {
        self.session.state() == SessionState::Active
    }
}

/// Owns all resources belonging to one call incarnation.
///
/// Keeping the cancellation token, transport, and task handles together makes
/// it impossible for a stale media/control task to outlive the call state
/// without also being cancelled by `terminate_call`.
#[derive(Debug)]
pub struct CallRuntime {
    cancellation: CancellationToken,
    accepting_media: Arc<AtomicBool>,
    #[cfg(feature = "voice-calls")]
    playback_control: Arc<AudioPlaybackControl>,
    connection: Connection,
    control_reader_task: Option<JoinHandle<()>>,
    control_writer_task: Option<JoinHandle<()>>,
    media_reader_task: Option<JoinHandle<()>>,
    audio_capture_task: Option<JoinHandle<()>>,
    audio_send_task: Option<JoinHandle<()>>,
    audio_receive_task: Option<JoinHandle<()>>,
    video_capture_task: Option<JoinHandle<()>>,
    video_send_task: Option<JoinHandle<()>>,
    video_receive_task: Option<JoinHandle<()>>,
}

impl CallRuntime {
    fn new(connection: Connection) -> Self {
        Self {
            cancellation: CancellationToken::new(),
            accepting_media: Arc::new(AtomicBool::new(true)),
            #[cfg(feature = "voice-calls")]
            playback_control: Arc::new(AudioPlaybackControl::default()),
            connection,
            control_reader_task: None,
            control_writer_task: None,
            media_reader_task: None,
            audio_capture_task: None,
            audio_send_task: None,
            audio_receive_task: None,
            video_capture_task: None,
            video_send_task: None,
            video_receive_task: None,
        }
    }

    /// Stop every resource owned by this call in the terminal-transition order.
    async fn shutdown(mut self) {
        // Cancellation is the common stop signal for capture, codecs, playback,
        // and the control/media readers.  The explicit task groups below are
        // intentionally kept separate: adding a task to the wrong group would
        // otherwise make shutdown order invisible and regressible.
        self.cancellation.cancel();
        // No new datagrams may enter the media pipeline after cancellation.
        self.accepting_media.store(false, Ordering::Release);

        // Closing the connection also closes the control and media streams.
        self.connection.close(0u32.into(), b"call terminated");

        let deadline = tokio::time::Instant::now() + CALL_SHUTDOWN_TIMEOUT;
        let mut tasks = Vec::new();
        tasks.extend(self.video_capture_task.take());
        tasks.extend(self.audio_capture_task.take());
        tasks.extend(self.video_send_task.take());
        tasks.extend(self.audio_send_task.take());
        tasks.extend(self.video_receive_task.take());
        tasks.extend(self.audio_receive_task.take());
        tasks.extend(self.media_reader_task.take());
        tasks.extend(self.control_reader_task.take());
        tasks.extend(self.control_writer_task.take());

        // A wedged device/codec must not hold the actor forever.  Abort only
        // after the bounded grace period so normal cancellation can clean up.
        // Each iteration waits at most the time remaining until `deadline`
        // (zero remaining -> abort immediately), so the loop as a whole is
        // already bounded by CALL_SHUTDOWN_TIMEOUT.  Do NOT wrap it in another
        // timeout: dropping the join loop early would detach the remaining
        // JoinHandles instead of aborting them, leaking their tasks.
        for mut task in tasks {
            let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
            if remaining.is_zero() {
                task.abort();
                continue;
            }
            if tokio::time::timeout(remaining, &mut task).await.is_err() {
                task.abort();
            }
        }
    }
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
    let mut stats = CallStatsAccumulator::default();
    let mut adaptation = AdaptationController::default();
    let mut stats_tick =
        tokio::time::interval_at(tokio::time::Instant::now() + STATS_INTERVAL, STATS_INTERVAL);
    stats_tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let mut next_generation: CallGeneration = 1;
    loop {
        let command = tokio::select! {
            command = command_rx.recv() => command,
            _ = stats_tick.tick() => {
                let snapshot = stats.snapshot();
                emit(&event_tx, CallEvent::Stats(snapshot)).await;
                let previous_decision = adaptation.decision();
                let decision = adaptation.update(snapshot);
                if decision != previous_decision {
                    emit(&event_tx, CallEvent::AdaptationChanged(decision)).await;
                }
                continue;
            }
        };
        let Some(command) = command else { break };
        match command {
            Command::Start {
                call_id,
                peer,
                kind,
            } => {
                if calls.values().any(|call| call.is_active() || call.peer == peer) {
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
                    Ok(connection) => {
                        let media_connection = connection.clone();
                        match connection.open_bi().await {
                            Ok((send, recv)) => {
                                spawn_media_reader(media_connection, peer, command_tx.clone());
                                let (tx, rx) = mpsc::channel(32);
                                let reply_tx = spawn_wire_session(
                                    peer,
                                    connection.clone(),
                                    send,
                                    recv,
                                    rx,
                                    command_tx.clone(),
                                );
                                let state = CallState {
                                    peer,
                                    kind,
                                    tx: reply_tx,
                                    incoming: false,

                                    local_audio_muted: false,
                                    remote_audio_muted: false,
                                    local_video_enabled: false,
                                    remote_video_enabled: false,
                                    generation: next_generation,
                                    session: {
                                        let mut session = CallSession::new(
                                            call_id,
                                            kind,
                                            secret_key.public(),
                                            peer,
                                        );
                                        let _ = session.start_offer(NEGOTIATION_TIMEOUT);
                                        session
                                    },
                                    ending: false,
                                    runtime: CallRuntime::new(connection.clone()),
                                };
                                next_generation = next_generation.wrapping_add(1).max(1);
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
                                    let _ =
                                        timeout_tx.send(Command::NegotiationTimeout(call_id)).await;
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
                        }
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
                }
            }
            Command::Incoming(connection) => {
                let peer = connection.remote_id();
                let session_tx = command_tx.clone();
                tokio::spawn(async move {
                    let media_connection = connection.clone();
                    let (send, recv) = match connection.accept_bi().await {
                        Ok(streams) => streams,
                        Err(_) => return,
                    };
                    spawn_media_reader(media_connection, peer, session_tx.clone());
                    let (_tx, rx) = mpsc::channel(32);
                    let _ =
                        spawn_wire_session(peer, connection.clone(), send, recv, rx, session_tx);
                });
                // Accepting a bidirectional stream is isolated from the actor
                // so a peer that connects without opening a stream cannot
                // block later incoming calls.
            }
            Command::RevokePeer(peer) => {
                let revoked: Vec<_> = calls
                    .iter()
                    .filter_map(|(call_id, state)| {
                        (state.peer == peer).then_some((*call_id, state.generation))
                    })
                    .collect();
                for (call_id, generation) in revoked {
                    terminate_call(
                        &mut calls,
                        &mut terminal_calls,
                        &event_tx,
                        call_id,
                        generation,
                        HangupReason::AuthorizationRevoked,
                        true,
                        false,
                    )
                    .await;
                }
            }
            Command::Control {
                peer,
                control,
                tx,
                connection,
            } => {
                handle_control(
                    &mut calls,
                    &mut terminal_calls,
                    &event_tx,
                    peer,
                    control,
                    tx,
                    connection,
                    &mut next_generation,
                    secret_key.public(),
                )
                .await;
            }
            Command::ConnectionClosed { peer } => {
                let ended: Vec<_> = calls
                    .iter()
                    .filter_map(|(id, state)| (state.peer == peer).then_some(*id))
                    .collect();
                for call_id in ended {
                    let generation = calls.get(&call_id).map(|call| call.generation).unwrap_or(0);
                    terminate_call(
                        &mut calls,
                        &mut terminal_calls,
                        &event_tx,
                        call_id,
                        generation,
                        HangupReason::ConnectionLost,
                        false,
                        false,
                    )
                    .await;
                }
            }
            Command::Media { peer, event } => match event {
                MediaReaderEvent::Packet { datagram, arrival } => {
                    stats.observe_received(&datagram, arrival);
                    emit(&event_tx, CallEvent::MediaReceived { peer, datagram }).await;
                }
                MediaReaderEvent::Malformed(_) => {
                    stats.observe_malformed();
                    emit(&event_tx, CallEvent::MediaMalformed { peer }).await;
                }
            },
            Command::Accept(call_id) => {
                if let Some(state) = calls.get_mut(&call_id) {
                    if state
                        .session
                        .apply(state.peer, SessionSignal::Accept { call_id })
                        .is_err()
                    {
                        continue;
                    }
                    let selected = negotiate_for_kind(state.kind);
                    let _ = state
                        .tx
                        .send(CallControl::Accept { call_id, selected })
                        .await;

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
                if let Some(state) = calls.get_mut(&call_id) {
                    let _ = state
                        .session
                        .apply(state.peer, SessionSignal::Decline { call_id });
                }
                if let Some(state) = calls.get(&call_id) {
                    let _ = state
                        .tx
                        .send(CallControl::Reject {
                            call_id,
                            reason: RejectReason::Declined,
                        })
                        .await;
                }
                let generation = calls.get(&call_id).map(|call| call.generation).unwrap_or(0);
                terminate_call(
                    &mut calls,
                    &mut terminal_calls,
                    &event_tx,
                    call_id,
                    generation,
                    HangupReason::LocalHangup,
                    false,
                    false,
                )
                .await;
            }

            Command::NegotiationTimeout(call_id) => {
                if calls
                    .get_mut(&call_id)
                    .and_then(|state| state.session.expire(std::time::Instant::now()))
                    .is_some()
                {
                    let generation = calls.get(&call_id).map(|call| call.generation).unwrap_or(0);
                    terminate_call(
                        &mut calls,
                        &mut terminal_calls,
                        &event_tx,
                        call_id,
                        generation,
                        HangupReason::NegotiationTimeout,
                        true,
                        false,
                    )
                    .await;
                }
            }
            Command::Terminate {
                call_id,
                generation,
                reason,
            } => {
                if let Some(state) = calls.get_mut(&call_id) {
                    let _ = state
                        .session
                        .apply(state.peer, SessionSignal::Leave { call_id });
                }
                terminate_call(
                    &mut calls,
                    &mut terminal_calls,
                    &event_tx,
                    call_id,
                    generation,
                    reason,
                    true,
                    false,
                )
                .await;
            }
            Command::Reconnect(call_id) => {
                if let Some(state) = calls.get_mut(&call_id) {
                    let generation = state.session.generation().saturating_add(1);
                    if state
                        .session
                        .apply(
                            state.peer,
                            SessionSignal::Reconnect {
                                call_id,
                                generation,
                            },
                        )
                        .is_ok()
                    {
                        state.generation = generation;

                        let _ = state
                            .tx
                            .send(CallControl::Reconnect {
                                call_id,
                                generation,
                            })
                            .await;
                    }
                }
            }
            Command::Shutdown => {
                let active: Vec<_> = calls
                    .iter()
                    .map(|(id, state)| (*id, state.generation))
                    .collect();
                for (call_id, generation) in active {
                    terminate_call(
                        &mut calls,
                        &mut terminal_calls,
                        &event_tx,
                        call_id,
                        generation,
                        HangupReason::Shutdown,
                        false,
                        false,
                    )
                    .await;
                }
                break;
            }
            Command::SetMuted { call_id, muted } => {
                let state = media_state.entry(call_id).or_insert((false, false));
                if state.0 == muted {
                    continue;
                }
                state.0 = muted;
                if let Some(call) = calls.get_mut(&call_id) {
                    call.local_audio_muted = muted;
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
                if let Some(call) = calls.get_mut(&call_id) {
                    call.local_video_enabled = enabled;
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
            Command::SetDeafened { call_id, deafened } => {
                #[cfg(feature = "voice-calls")]
                if let Some(call) = calls.get(&call_id) {
                    call.runtime.playback_control.set_deafened(deafened);
                }
                #[cfg(not(feature = "voice-calls"))]
                let _ = (call_id, deafened);
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
    connection: Connection,
    next_generation: &mut CallGeneration,
    local_peer: PublicKey,
) {
    match control {
        CallControl::Hello { .. } => {}
        CallControl::Offer {
            call_id,
            kind,
            capabilities,
        } => {
            let pending_offers = calls
                .values()
                .filter(|call| call.incoming && !call.is_active())
                .count();
            if pending_offers >= MAX_PENDING_CALL_OFFERS {
                let _ = tx.send(CallControl::Busy { call_id }).await;
                return;
            }
            if let Some(existing) = calls.values().find(|call| call.is_active() || call.peer == peer) {
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
            let mut session = CallSession::new(call_id, kind, local_peer, peer);
            if session
                .apply(peer, SessionSignal::Offer { call_id, kind })
                .is_err()
            {
                return;
            }
            calls.insert(
                call_id,
                CallState {
                    peer,
                    kind,
                    tx: tx.clone(),
                    incoming: true,

                    local_audio_muted: false,
                    remote_audio_muted: false,
                    local_video_enabled: false,
                    remote_video_enabled: false,
                    generation: *next_generation,
                    session,
                    ending: false,
                    runtime: CallRuntime::new(connection),
                },
            );
            *next_generation = next_generation.wrapping_add(1).max(1);
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
                if call
                    .session
                    .apply(call.peer, SessionSignal::Accept { call_id })
                    .is_err()
                {
                    return;
                }

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
        CallControl::Reject { call_id, .. } => {
            if let Some(call) = calls.get_mut(&call_id) {
                let _ = call
                    .session
                    .apply(call.peer, SessionSignal::Decline { call_id });
            }
            let generation = calls.get(&call_id).map(|call| call.generation).unwrap_or(0);
            terminate_call(
                calls,
                terminal_calls,
                events,
                call_id,
                generation,
                HangupReason::RemoteHangup,
                false,
                true,
            )
            .await;
        }
        CallControl::Busy { call_id } => {
            let generation = calls.get(&call_id).map(|call| call.generation).unwrap_or(0);
            terminate_call_with_error(
                calls,
                terminal_calls,
                events,
                call_id,
                generation,
                CallError::Busy,
            )
            .await;
        }
        CallControl::MediaState {
            call_id,
            audio_muted,
            video_enabled,
        } => {
            if let Some(call) = calls.get_mut(&call_id) {
                call.remote_audio_muted = audio_muted;
                call.remote_video_enabled = video_enabled;
            }
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
        CallControl::Reconnect {
            call_id,
            generation,
        } => {
            if let Some(call) = calls.get_mut(&call_id) {
                if call
                    .session
                    .apply(
                        call.peer,
                        SessionSignal::Reconnect {
                            call_id,
                            generation,
                        },
                    )
                    .is_ok()
                {
                    call.generation = generation;

                }
            }
        }
        CallControl::RequestKeyframe { .. } | CallControl::KeepAlive { .. } => {}
        CallControl::Hangup { call_id, reason } => {
            if let Some(call) = calls.get_mut(&call_id) {
                let _ = call
                    .session
                    .apply(call.peer, SessionSignal::Leave { call_id });
            }
            let generation = calls.get(&call_id).map(|call| call.generation).unwrap_or(0);
            terminate_call(
                calls,
                terminal_calls,
                events,
                call_id,
                generation,
                reason,
                false,
                false,
            )
            .await;
        }
    }
}

/// The only function allowed to remove a live call.
///
/// `notify_peer` controls whether a `CallControl::Hangup` is sent back to the
/// peer (false for wire-initiated terminations like remote Hangup, Reject,
/// Busy, and connection loss, where echoing a Hangup would be wrong or
/// impossible). `failed` selects the terminal event shape: a rejected/busy
/// call surfaces `CallEvent::Failed { Rejected }`, every other condition
/// surfaces `CallEvent::Ended { reason }`.
async fn terminate_call(
    calls: &mut HashMap<CallId, CallState>,
    terminal_calls: &mut HashSet<CallId>,
    events: &mpsc::Sender<CallEvent>,
    call_id: CallId,
    generation: CallGeneration,
    reason: HangupReason,
    notify_peer: bool,
    failed: bool,
) {
    terminate_call_inner(
        calls,
        terminal_calls,
        events,
        call_id,
        generation,
        reason,
        notify_peer,
        failed.then_some(CallError::Rejected),
    )
    .await;
}

async fn terminate_call_with_error(
    calls: &mut HashMap<CallId, CallState>,
    terminal_calls: &mut HashSet<CallId>,
    events: &mpsc::Sender<CallEvent>,
    call_id: CallId,
    generation: CallGeneration,
    error: CallError,
) {
    terminate_call_inner(
        calls,
        terminal_calls,
        events,
        call_id,
        generation,
        HangupReason::RemoteHangup,
        false,
        Some(error),
    )
    .await;
}

async fn terminate_call_inner(
    calls: &mut HashMap<CallId, CallState>,
    terminal_calls: &mut HashSet<CallId>,
    events: &mpsc::Sender<CallEvent>,
    call_id: CallId,
    generation: CallGeneration,
    reason: HangupReason,
    notify_peer: bool,
    failed_reason: Option<CallError>,
) {
    let Some(current_generation) = calls.get(&call_id).map(|call| call.generation) else {
        return;
    };
    // Zero is the frontend shorthand for the current incarnation. Every
    // background task supplies its captured non-zero generation.
    if !generation_matches(current_generation, generation) {
        return;
    }
    // Mark Ending before taking ownership of the resources.  This is the
    // actor's atomic state transition: every command is serialized here, and
    // a late task with another generation fails the check above.
    if calls.get(&call_id).is_some_and(|state| state.ending) {
        return;
    }
    if let Some(state) = calls.get_mut(&call_id) {
        state.ending = true;
    }
    // Reserve the terminal notification before awaiting any shutdown work.
    // This makes the single-event invariant explicit even if another caller
    // queues a duplicate termination command.
    if !terminal_calls.insert(call_id) {
        return;
    }
    let Some(state) = calls.remove(&call_id) else {
        return;
    };

    // 2. Best-effort Hangup while the control transport is still usable.
    if notify_peer {
        let _ = tokio::time::timeout(
            Duration::from_millis(100),
            state.tx.send(CallControl::Hangup { call_id, reason }),
        )
        .await;
    }

    // 3–12. Runtime owns the cancellation token, media admission gate,
    // capture/codec/playback tasks, streams, connection, and bounded joins.
    state.runtime.shutdown().await;

    // 13. The state was removed only for this matching generation above; a
    // stale task can therefore never transition a later incarnation to Idle.
    // 14. Emit exactly one terminal event after all resources are quiescent.
    if let Some(failed_reason) = failed_reason {
        emit(
            events,
            CallEvent::Failed {
                call_id: Some(call_id),
                reason: failed_reason,
            },
        )
        .await;
    } else {
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

fn generation_matches(current: CallGeneration, requested: CallGeneration) -> bool {
    requested == 0 || requested == current
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

fn spawn_media_reader(connection: Connection, peer: PublicKey, command_tx: mpsc::Sender<Command>) {
    let (media_tx, mut media_rx) = mpsc::channel(64);
    tokio::spawn(async move {
        media_reader(connection, media_tx).await;
    });
    tokio::spawn(async move {
        while let Some(event) = media_rx.recv().await {
            if command_tx
                .send(Command::Media { peer, event })
                .await
                .is_err()
            {
                break;
            }
        }
    });
}

fn spawn_wire_session<R, W>(
    peer: PublicKey,
    connection: Connection,
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
        let mut call_ids = HashSet::<CallId>::new();
        let mut outbound = Some(outbound);
        loop {
            tokio::select! {
                result = read_call_control(&mut recv) => match result {
                    Ok(Some(control)) => {
                        call_ids.insert(control_call_id(&control));
                        if command_tx.send(Command::Control { peer, control, tx: command_reply_tx.clone(), connection: connection.clone() }).await.is_err() { break; }
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
                        call_ids.insert(control_call_id(&control));
                        if write_call_control(&mut send, &control).await.is_err() { break; }
                    }
                    None => outbound = None,
                },
                maybe = reply_rx.recv() => match maybe {
                    Some(control) => {
                        call_ids.insert(control_call_id(&control));
                        if write_call_control(&mut send, &control).await.is_err() { break; }
                    }
                    None => break,
                },
                _ = keepalive.tick() => {
                    for call_id in call_ids.iter().copied() {
                        if write_call_control(&mut send, &CallControl::KeepAlive { call_id }).await.is_err() { break; }
                    }
                }
            }
        }
        let _ = command_tx.send(Command::ConnectionClosed { peer }).await;
    });
    reply_tx
}

fn control_call_id(control: &CallControl) -> CallId {
    match control {
        CallControl::Hello { call_id, .. }
        | CallControl::Offer { call_id, .. }
        | CallControl::Ringing { call_id }
        | CallControl::Accept { call_id, .. }
        | CallControl::Reject { call_id, .. }
        | CallControl::Busy { call_id }
        | CallControl::MediaState { call_id, .. }
        | CallControl::RequestKeyframe { call_id, .. }
        | CallControl::KeepAlive { call_id }
        | CallControl::Reconnect { call_id, .. }
        | CallControl::Hangup { call_id, .. } => *call_id,
    }
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
    use crate::call::media::MediaKind;
    use iroh::endpoint::presets;
    use iroh::protocol::Router;

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

    #[test]
    fn stale_generation_cannot_terminate_new_incarnation() {
        assert!(generation_matches(13, 0));
        assert!(generation_matches(13, 13));
        assert!(!generation_matches(13, 12));
        assert!(!generation_matches(13, 14));
    }

    fn media_packet(kind: MediaKind, sequence: u32) -> MediaDatagram {
        MediaDatagram {
            kind,
            flags: 0,
            call_id: CallId::from_bytes([7; 16]),
            track_id: 1,
            sequence,
            timestamp: sequence,
            fragment_index: 0,
            fragment_count: 1,
            payload: vec![1],
        }
    }

    #[test]
    fn stats_accumulate_received_packets_and_audio_loss() {
        let mut stats = CallStatsAccumulator::default();
        let now = Instant::now();
        stats.observe_received(&media_packet(MediaKind::Audio, 10), now);
        stats.observe_received(
            &media_packet(MediaKind::Audio, 12),
            now + Duration::from_millis(20),
        );
        stats.observe_received(
            &media_packet(MediaKind::Audio, 11),
            now + Duration::from_millis(40),
        );

        let snapshot = stats.snapshot();
        assert_eq!(snapshot.audio_packets_received, 3);
        assert_eq!(snapshot.audio_packets_lost, 1);
        assert_eq!(snapshot.audio_packets_late, 1);
    }

    #[test]
    fn stats_event_has_complete_default_shape() {
        let event = CallEvent::Stats(CallStats::default());
        assert_eq!(event, CallEvent::Stats(CallStats::default()));
        assert!(!event.is_terminal());
    }

    #[tokio::test]
    async fn spawn_returns_handle_and_receiver() {
        let endpoint = Endpoint::bind(presets::Minimal).await.unwrap();
        let peer = SecretKey::generate().public();
        let (handle, mut events) = CallBuilder::new(endpoint, SecretKey::generate()).spawn();
        handle.set_peer_authorized(peer, true);
        let call_id = handle.start_voice_call(peer).await.unwrap();
        assert!(
            matches!(events.recv().await, Some(CallEvent::Failed { call_id: Some(id), .. }) if id == call_id)
        );
    }

    #[tokio::test]
    async fn stats_emit_approximately_once_per_second() {
        let endpoint = Endpoint::bind(presets::Minimal).await.unwrap();
        let (handle, mut events) = CallBuilder::new(endpoint, SecretKey::generate()).spawn();

        let first = tokio::time::timeout(Duration::from_millis(1_500), async {
            loop {
                if let Some(event @ CallEvent::Stats(_)) = events.recv().await {
                    break event;
                }
            }
        })
        .await
        .expect("first stats event should arrive after one second");
        assert!(matches!(first, CallEvent::Stats(stats) if stats == CallStats::default()));
        assert!(
            tokio::time::timeout(Duration::from_millis(200), events.recv())
                .await
                .is_err()
        );
        handle.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn unauthorized_outbound_call_is_rejected_before_connect() {
        let endpoint = Endpoint::bind(presets::Minimal).await.unwrap();
        let (handle, mut events) = CallBuilder::new(endpoint, SecretKey::generate()).spawn();
        let peer = SecretKey::generate().public();

        let error = handle.start_voice_call(peer).await.unwrap_err();
        assert!(error.to_string().contains("not authorized"));
        assert!(events.try_recv().is_err());
    }

    #[tokio::test]
    async fn protocol_policy_is_allow_list_and_tracks_handle_updates() {
        let endpoint = Endpoint::bind(presets::Minimal).await.unwrap();
        let local = SecretKey::generate();
        let peer = SecretKey::generate().public();
        let builder = CallBuilder::new(endpoint, local).with_authorized_peers([peer]);
        let handler = builder.protocol_handler();
        assert!(handler.authorized_peers.read().unwrap().contains(&peer));
        assert!(!handler
            .authorized_peers
            .read()
            .unwrap()
            .contains(&SecretKey::generate().public()));
    }

    #[tokio::test]
    async fn authorization_revocation_uses_terminal_ended_event() {
        let (connection, router, _client) = live_connection().await;
        let (event_tx, mut event_rx) = mpsc::channel(EVENT_CAPACITY);
        let (control_tx, _control_rx) = mpsc::channel(32);
        let mut calls = HashMap::new();
        let mut terminal_calls = HashSet::new();
        let call_id = CallId::generate();
        calls.insert(
            call_id,
            CallState {
                peer: SecretKey::generate().public(),
                kind: CallKind::Voice,
                tx: control_tx,
                incoming: false,

                local_audio_muted: false,
                remote_audio_muted: false,
                local_video_enabled: false,
                remote_video_enabled: false,
                generation: 9,
                session: CallSession::new(
                    call_id,
                    CallKind::Voice,
                    SecretKey::generate().public(),
                    SecretKey::generate().public(),
                ),
                ending: false,
                runtime: CallRuntime::new(connection),
            },
        );
        terminate_call(
            &mut calls,
            &mut terminal_calls,
            &event_tx,
            call_id,
            9,
            HangupReason::AuthorizationRevoked,
            true,
            false,
        )
        .await;
        assert!(matches!(event_rx.recv().await,
            Some(CallEvent::Ended { call_id: id, reason: CallEndReason::AuthorizationRevoked }) if id == call_id));
        let _ = router.shutdown().await;
    }

    #[test]
    fn frame_limit_is_checked_before_allocation() {
        let mut frame = (MAX_CALL_CONTROL_FRAME_SIZE as u32 + 1)
            .to_be_bytes()
            .to_vec();
        frame.extend_from_slice(&[0; 8]);
        assert!(read_declared_size(&frame).is_err());
    }

    /// Bind two minimal endpoints and return a live call connection plus the
    /// router that keeps the server side alive (mirrors tests/call_e2e.rs).
    async fn live_connection() -> (Connection, Router, Endpoint) {
        let server = Endpoint::bind(presets::Minimal).await.unwrap();
        let server_builder = CallBuilder::new(server.clone(), server.secret_key().clone());
        let server_handler = server_builder.protocol_handler();
        let (_server_handle, _server_events) = server_builder.spawn();
        let server_router = Router::builder(server.clone())
            .accept(CALL_ALPN, server_handler)
            .spawn();
        let client = Endpoint::bind(presets::Minimal).await.unwrap();
        let connection = client
            .connect(server.addr(), CALL_ALPN)
            .await
            .expect("probe connection should establish");
        (connection, server_router, client)
    }

    #[tokio::test]
    async fn runtime_shutdown_closes_media_gate_and_bounded_abort_wedged_task() {
        let (connection, router, _client) = live_connection().await;
        let mut runtime = CallRuntime::new(connection);
        // A wedged device/codec task that never finishes on its own.
        let (abort_tx, abort_rx) = tokio::sync::oneshot::channel::<()>();
        runtime.audio_capture_task = Some(tokio::spawn(async move {
            let _ = abort_rx.await;
        }));

        let started = tokio::time::Instant::now();
        runtime.shutdown().await;
        let elapsed = started.elapsed();

        // Bounded shutdown: never wait longer than the timeout for a wedged task.
        assert!(
            elapsed < CALL_SHUTDOWN_TIMEOUT * 2,
            "shutdown took {elapsed:?}, bounded await violated"
        );
        // The wedged task must have been aborted by the bounded join.
        abort_tx.send(()).ok();
        let _ = router.shutdown().await;
    }

    #[tokio::test]
    async fn shutdown_releases_every_resource_slot_and_closes_connection() {
        let (connection, router, _client) = live_connection().await;
        let observer = connection.clone();
        let mut runtime = CallRuntime::new(connection);

        // Populate every resource slot with a task that blocks forever on a
        // oneshot.  After shutdown each must be stopped (bounded-join aborts
        // it) — proven by the sender observing a dropped receiver.
        let mut signals = Vec::new();
        let mut add_task = |slot: &mut Option<JoinHandle<()>>| {
            let (tx, rx) = tokio::sync::oneshot::channel::<()>();
            signals.push(tx);
            *slot = Some(tokio::spawn(async move {
                let _ = rx.await;
            }));
        };
        add_task(&mut runtime.control_reader_task);
        add_task(&mut runtime.control_writer_task);
        add_task(&mut runtime.media_reader_task);
        add_task(&mut runtime.audio_capture_task);
        add_task(&mut runtime.audio_send_task);
        add_task(&mut runtime.audio_receive_task);
        add_task(&mut runtime.video_capture_task);
        add_task(&mut runtime.video_send_task);
        add_task(&mut runtime.video_receive_task);
        assert_eq!(signals.len(), 9, "expected one task per resource slot");

        let started = tokio::time::Instant::now();
        runtime.shutdown().await;
        let elapsed = started.elapsed();
        assert!(
            elapsed < CALL_SHUTDOWN_TIMEOUT * 2,
            "shutdown took {elapsed:?}, bounded await violated"
        );

        // Aborts land asynchronously (the runtime drops the cancelled future
        // on a later poll), so wait a bounded window for every task to
        // terminate before asserting.  A leaked task would never close.
        let settle = tokio::time::Instant::now() + Duration::from_secs(2);
        while !signals.iter().all(|tx| tx.is_closed()) {
            assert!(
                tokio::time::Instant::now() < settle,
                "resource tasks did not terminate within the settle window"
            );
            tokio::task::yield_now().await;
        }

        // Every task was stopped: each sender sees its receiver dropped.
        for (i, tx) in signals.into_iter().enumerate() {
            assert!(
                tx.send(()).is_err(),
                "resource slot {i} task still alive after shutdown"
            );
        }
        // The call connection was closed by shutdown.
        assert!(
            observer.close_reason().is_some(),
            "connection must be closed after runtime shutdown"
        );
        let _ = router.shutdown().await;
    }

    #[tokio::test]
    async fn terminate_call_emits_exactly_one_ended_event() {
        let (connection, router, _client) = live_connection().await;
        let (event_tx, mut event_rx) = mpsc::channel(EVENT_CAPACITY);
        let (control_tx, _control_rx) = mpsc::channel(32);
        let mut calls = HashMap::new();
        let mut terminal_calls = HashSet::new();
        let call_id = CallId::generate();

        calls.insert(
            call_id,
            CallState {
                peer: SecretKey::generate().public(),
                kind: CallKind::Voice,
                tx: control_tx,
                incoming: false,

                local_audio_muted: false,
                remote_audio_muted: false,
                local_video_enabled: false,
                remote_video_enabled: false,
                generation: 7,
                session: CallSession::new(
                    call_id,
                    CallKind::Voice,
                    SecretKey::generate().public(),
                    SecretKey::generate().public(),
                ),
                ending: false,
                runtime: CallRuntime::new(connection),
            },
        );

        // First termination wins and emits exactly one Ended.
        terminate_call(
            &mut calls,
            &mut terminal_calls,
            &event_tx,
            call_id,
            0,
            HangupReason::LocalHangup,
            false,
            false,
        )
        .await;
        // A duplicate/stale termination (same call, now terminal) is a no-op.
        terminate_call(
            &mut calls,
            &mut terminal_calls,
            &event_tx,
            call_id,
            7,
            HangupReason::ConnectionLost,
            false,
            false,
        )
        .await;
        // A stale generation from a previous incarnation is ignored entirely.
        terminate_call(
            &mut calls,
            &mut terminal_calls,
            &event_tx,
            call_id,
            6,
            HangupReason::LocalHangup,
            false,
            false,
        )
        .await;

        let mut ended = 0;
        while let Ok(event) = event_rx.try_recv() {
            if matches!(event, CallEvent::Ended { call_id: id, .. } if id == call_id) {
                ended += 1;
            }
        }
        assert_eq!(ended, 1, "expected exactly one Ended event, got {ended}");
        assert!(
            calls.is_empty(),
            "call state must be removed after termination"
        );
        let _ = router.shutdown().await;
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
