//! Owned resources for one call's media incarnation.
//!
//! Media workers are deliberately owned by the call actor rather than by the
//! UI.  The runtime is created while negotiating, but media admission remains
//! closed until the call is active and consent has been granted.

use std::sync::{atomic::{AtomicBool, Ordering}, Arc};
use std::time::Duration;

use iroh::endpoint::Connection;
use tokio::sync::watch;
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

use super::stats::CallStatsRuntime;
use super::wire::NegotiatedMedia;
#[cfg(feature = "voice-calls")]
use super::audio::receive::AudioPlaybackControl;
#[cfg(feature = "video-calls")]
use super::video::VideoFrame;

pub(crate) const CALL_SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(2);
const WORKER_SLOTS: usize = 9;

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
    local_frame_tx: watch::Sender<Option<Arc<VideoFrame>>>,
    #[cfg(feature = "video-calls")]
    remote_frame_tx: watch::Sender<Option<Arc<VideoFrame>>>,
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
            control_reader_task: None, control_writer_task: None,
            media_reader_task: None, audio_capture_task: None,
            audio_send_task: None, audio_receive_task: None,
            video_capture_task: None, video_send_task: None,
            video_receive_task: None,
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

    /// Publish the negotiated media state to all workers and observers.
    pub(crate) fn set_negotiated(&self, state: NegotiatedMedia) { let _ = self.negotiated_tx.send(Some(state)); }

    pub(crate) fn negotiated(&self) -> watch::Receiver<Option<NegotiatedMedia>> { self.negotiated.clone() }

    #[cfg(feature = "video-calls")]
    pub(crate) fn local_frames(&self) -> watch::Receiver<Option<Arc<VideoFrame>>> { self.local_frame_tx.subscribe() }

    #[cfg(feature = "video-calls")]
    pub(crate) fn remote_frames(&self) -> watch::Receiver<Option<Arc<VideoFrame>>> { self.remote_frame_tx.subscribe() }

    /// Publish the newest local preview without retaining a frame history.
    #[cfg(feature = "video-calls")]
    pub(crate) fn publish_local_frame(&self, frame: VideoFrame) {
        let _ = self.local_frame_tx.send(Some(Arc::new(frame)));
    }

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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn worker_capacity_is_fixed() { assert_eq!(WORKER_SLOTS, 9); }
}
