//! Owned resources for one call's media incarnation.
//!
//! Media workers are deliberately owned by the call actor rather than by the
//! UI.  The runtime is created while negotiating, but media admission remains
//! closed until the call is active and consent has been granted.

use std::sync::{atomic::{AtomicBool, Ordering}, Arc};
use std::time::Duration;

use iroh::endpoint::Connection;
use tokio::sync::{mpsc, watch};
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

use super::stats::CallStatsRuntime;
use super::wire::NegotiatedMedia;
#[cfg(feature = "voice-calls")]
use super::audio::receive::AudioPlaybackControl;
#[cfg(feature = "video-calls")]
use super::video::capture::CapturedFrame;

pub(crate) const CALL_SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(2);
const WORKER_SLOTS: usize = 9;
const AUDIO_ROUTE_CAPACITY: usize = 64;
const VIDEO_ROUTE_CAPACITY: usize = 8;

/// Result of attempting to hand one inbound datagram to a media worker.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum MediaRouteResult {
    /// The datagram was accepted by its bounded worker queue.
    Accepted,
    /// The datagram was rejected because it does not belong to this call.
    Rejected,
    /// The datagram was valid but the worker queue was full.
    Dropped,
    /// Media admission is closed (before consent or after shutdown).
    Inactive,
}

/// All resources owned by one call/media incarnation.
#[derive(Debug)]
pub struct CallMediaRuntime {
    /// Call-scoped statistics and adaptation controller.
    pub(crate) stats: CallStatsRuntime,
    cancellation: CancellationToken,
    pub(crate) accepting_media: Arc<AtomicBool>,
    #[cfg(feature = "voice-calls")]
    pub(crate) playback_control: Arc<AudioPlaybackControl>,
    connection: Connection,
    negotiated: watch::Receiver<Option<NegotiatedMedia>>,
    negotiated_tx: watch::Sender<Option<NegotiatedMedia>>,
    #[cfg(feature = "video-calls")]
    local_frame_tx: watch::Sender<Option<Arc<CapturedFrame>>>,
    #[cfg(feature = "video-calls")]
    remote_frame_tx: watch::Sender<Option<Arc<CapturedFrame>>>,
    audio_route_tx: mpsc::Sender<super::media::MediaDatagram>,
    video_route_tx: mpsc::Sender<super::media::MediaDatagram>,
    expected_peer: Option<iroh::PublicKey>,
    expected_call_id: Option<super::CallId>,
    expected_generation: Option<u64>,
    pub(crate) control_reader_task: Option<JoinHandle<()>>,
    pub(crate) control_writer_task: Option<JoinHandle<()>>,
    pub(crate) media_reader_task: Option<JoinHandle<()>>,
    pub(crate) audio_capture_task: Option<JoinHandle<()>>,
    pub(crate) audio_send_task: Option<JoinHandle<()>>,
    pub(crate) audio_receive_task: Option<JoinHandle<()>>,
    pub(crate) video_capture_task: Option<JoinHandle<()>>,
    pub(crate) video_send_task: Option<JoinHandle<()>>,
    pub(crate) video_receive_task: Option<JoinHandle<()>>,
}

impl CallMediaRuntime {
    pub(crate) fn new(connection: Connection) -> Self {
        let (negotiated_tx, negotiated) = watch::channel(None);
        #[cfg(feature = "video-calls")]
        let (local_frame_tx, _) = watch::channel(None);
        #[cfg(feature = "video-calls")]
        let (remote_frame_tx, _) = watch::channel(None);
        let (audio_route_tx, mut audio_route_rx) = mpsc::channel(AUDIO_ROUTE_CAPACITY);
        let (video_route_tx, video_route_rx) = mpsc::channel(VIDEO_ROUTE_CAPACITY);
        let worker_cancel = CancellationToken::new();
        let audio_cancel = worker_cancel.clone();
        let audio_receive_task = tokio::spawn(async move {
            loop {
                tokio::select! {
                    _ = audio_cancel.cancelled() => break,
                    packet = audio_route_rx.recv() => if packet.is_none() { break },
                }
            }
        });
        #[cfg(feature = "video-calls")]
        let video_receive_task = {
            let video_cancel = worker_cancel.clone();
            let remote_frame_tx = remote_frame_tx.clone();
            tokio::spawn(async move {
                run_video_receive_worker(video_cancel, video_route_rx, remote_frame_tx).await;
            })
        };
        #[cfg(not(feature = "video-calls"))]
        let video_receive_task = tokio::spawn(async move {
            while video_route_rx.recv().await.is_some() {}
        });
        Self {
            stats: CallStatsRuntime::default(),
            cancellation: CancellationToken::new(),
            accepting_media: Arc::new(AtomicBool::new(false)),
            #[cfg(feature = "voice-calls")]
            playback_control: Arc::new(AudioPlaybackControl::default()),
            connection,
            negotiated,
            negotiated_tx,
            #[cfg(feature = "video-calls")]
            local_frame_tx,
            #[cfg(feature = "video-calls")]
            remote_frame_tx,
            audio_route_tx,
            video_route_tx,
            expected_peer: None,
            expected_call_id: None,
            expected_generation: None,
            control_reader_task: None, control_writer_task: None,
            media_reader_task: None, audio_capture_task: None,
            audio_send_task: None, audio_receive_task: Some(audio_receive_task),
            video_capture_task: None, video_send_task: None,
            video_receive_task: Some(video_receive_task),
        }
    }

    /// Cancellation shared by every worker in this runtime.
    pub(crate) fn cancellation(&self) -> CancellationToken { self.cancellation.clone() }

    /// Admission gate for media packets and capture workers.
    pub(crate) fn media_gate(&self) -> Arc<AtomicBool> { Arc::clone(&self.accepting_media) }

    /// Open media admission after Active state and explicit consent.
    pub(crate) fn activate_media(&self) { self.accepting_media.store(true, Ordering::Release); }

    /// Close media admission without ending the signalling call.
    pub(crate) fn deactivate_media(&self) { self.accepting_media.store(false, Ordering::Release); }

    /// Whether workers may currently send or accept media.
    pub(crate) fn media_allowed(&self) -> bool { self.accepting_media.load(Ordering::Acquire) }

    /// Bind the runtime to the authenticated call incarnation before routing media.
    pub(crate) fn bind_identity(&mut self, peer: iroh::PublicKey, call_id: super::CallId, generation: u64) {
        self.expected_peer = Some(peer);
        self.expected_call_id = Some(call_id);
        self.expected_generation = Some(generation);
    }

    /// Route a parsed datagram without blocking the call actor or UI.
    ///
    /// Audio has a larger queue because it is continuous and latency-sensitive;
    /// video uses a small queue so stale frames are discarded under load.
    pub(crate) fn try_route(
        &self,
        peer: iroh::PublicKey,
        call_id: super::CallId,
        generation: u64,
        datagram: super::media::MediaDatagram,
    ) -> MediaRouteResult {
        if !self.media_allowed() {
            return MediaRouteResult::Inactive;
        }
        if self.expected_peer != Some(peer)
            || self.expected_call_id != Some(call_id)
            || self.expected_generation != Some(generation)
            || datagram.call_id != call_id
            || datagram.track_id != 1
        {
            return MediaRouteResult::Rejected;
        }
        let result = match datagram.kind {
            super::media::MediaKind::Audio => self.audio_route_tx.try_send(datagram),
            super::media::MediaKind::Video => self.video_route_tx.try_send(datagram),
        };
        match result {
            Ok(()) => MediaRouteResult::Accepted,
            Err(mpsc::error::TrySendError::Full(_)) => MediaRouteResult::Dropped,
            Err(mpsc::error::TrySendError::Closed(_)) => MediaRouteResult::Inactive,
        }
    }

    /// Publish the negotiated media state to all workers and observers.
    pub(crate) fn set_negotiated(&self, state: NegotiatedMedia) { let _ = self.negotiated_tx.send(Some(state)); }

    pub(crate) fn negotiated(&self) -> watch::Receiver<Option<NegotiatedMedia>> { self.negotiated.clone() }

    #[cfg(feature = "video-calls")]
    pub(crate) fn local_frames(&self) -> watch::Receiver<Option<Arc<CapturedFrame>>> { self.local_frame_tx.subscribe() }

    #[cfg(feature = "video-calls")]
    pub(crate) fn remote_frames(&self) -> watch::Receiver<Option<Arc<CapturedFrame>>> { self.remote_frame_tx.subscribe() }

    /// Install a worker in a bounded slot; replacing a slot aborts its old worker.

    /// Deterministically cancel and join all workers. Safe to call repeatedly.
    pub(crate) async fn shutdown(&mut self) {
        self.deactivate_media();
        self.cancellation.cancel();
        self.connection.close(0u32.into(), b"call terminated");
        let deadline = tokio::time::Instant::now() + CALL_SHUTDOWN_TIMEOUT;
        let mut workers = [
            &mut self.control_reader_task, &mut self.control_writer_task,
            &mut self.media_reader_task, &mut self.audio_capture_task,
            &mut self.audio_send_task, &mut self.audio_receive_task,
            &mut self.video_capture_task, &mut self.video_send_task,
            &mut self.video_receive_task,
        ];
        debug_assert_eq!(workers.len(), WORKER_SLOTS);
        for worker in &mut workers {
            let Some(mut task) = worker.take() else { continue; };
            let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
            if remaining.is_zero() || tokio::time::timeout(remaining, &mut task).await.is_err() {
                task.abort();
            }
        }
        let _ = self.negotiated_tx.send(None);
        #[cfg(feature = "video-calls")]
        {
            let _ = self.local_frame_tx.send(None);
            let _ = self.remote_frame_tx.send(None);
        }
    }
}

#[cfg(feature = "video-calls")]
async fn run_video_receive_worker(
    cancellation: CancellationToken,
    mut packets: mpsc::Receiver<super::media::MediaDatagram>,
    frames: watch::Sender<Option<Arc<CapturedFrame>>>,
) {
    let Ok(mut pipeline) = super::video::pipeline::LiveVideoPipeline::new() else {
        return;
    };
    loop {
        let packet = tokio::select! {
            _ = cancellation.cancelled() => None,
            packet = packets.recv() => packet,
        };
        let Some(packet) = packet else { break };
        if let Ok(Some(decoded)) = pipeline.receive_parsed(&packet) {
            let _ = frames.send(Some(Arc::new(CapturedFrame {
                timestamp_us: packet.timestamp as u64,
                data: decoded.bytes,
            })));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn worker_capacity_is_fixed() { assert_eq!(WORKER_SLOTS, 9); }
}
