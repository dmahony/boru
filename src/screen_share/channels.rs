//! Logical channel separation for the screen-share transport (PDF Task 3.2).
//!
//! Two independent, bounded channels run on top of the shared QUIC screen-share
//! connection:
//!
//! - [`ControlChannel`]: reliable control traffic — negotiation, lifecycle,
//!   errors, keyframe requests, input events, and quality changes. Control
//!   messages are low-rate, so a bounded queue with *awaiting* backpressure is
//!   the right policy: a full control queue means the connection is wedged and
//!   senders should wait, not grow memory.
//! - [`MediaChannel`]: video packets only. A small bounded queue with a
//!   drop-oldest policy (latest-frame-wins) means a slow network can never
//!   accumulate stale frames — the oldest queued frame is discarded to make
//!   room and the drop is counted.
//!
//! Chat traffic is on a separate QUIC connection (the gossip protocol), so it
//! cannot block screen-share frames at the transport level; these channels make
//! the control/media separation explicit *inside* the screen-share connection
//! and bound every queue on the media path.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;

use tokio::sync::{mpsc, Notify};

use super::{
    codec::EncodedFrame,
    protocol::{ControlMessage, ScreenShareMessage},
    transport::QuicScreenTransport,
    ScreenShareError,
};

/// Default number of encoded frames a [`MediaChannel`] may hold while the
/// network drains. Deliberately small: interactive video wants the newest
/// frame, not a backlog of stale ones.
pub const DEFAULT_MEDIA_QUEUE_CAPACITY: usize = 2;
/// Default number of control messages that may wait for the reliable control
/// stream. Control traffic is low-rate; a full queue means the connection is
/// wedged, so senders block (backpressure) instead of growing memory.
pub const DEFAULT_CONTROL_QUEUE_CAPACITY: usize = 64;

/// Bounded FIFO queue of encoded frames with a drop-oldest overflow policy.
///
/// Memory is bounded by construction: at most `capacity` frames are retained,
/// and a push onto a full queue discards the oldest frame. This is the
/// "latest-frame strategy" the screen-share plan requires (stale video frames
/// never grow memory without limit; obsolete frames are dropped rather than
/// building latency).
#[derive(Debug)]
pub struct BoundedFrameQueue {
    frames: VecDeque<EncodedFrame>,
    capacity: usize,
    /// Number of frames discarded because the queue was full.
    drops: u64,
}

impl BoundedFrameQueue {
    /// Create a queue that retains at most `capacity` frames.
    pub fn new(capacity: usize) -> Result<Self, ScreenShareError> {
        if capacity == 0 {
            return Err(ScreenShareError::new("media queue capacity must be non-zero"));
        }
        Ok(Self {
            frames: VecDeque::with_capacity(capacity.min(16)),
            capacity,
            drops: 0,
        })
    }

    /// Maximum number of frames this queue can hold.
    pub fn capacity(&self) -> usize {
        self.capacity
    }

    /// Current number of queued frames.
    pub fn len(&self) -> usize {
        self.frames.len()
    }

    /// Whether the queue holds no frames.
    pub fn is_empty(&self) -> bool {
        self.frames.is_empty()
    }

    /// Whether the queue is at capacity.
    pub fn is_full(&self) -> bool {
        self.frames.len() >= self.capacity
    }

    /// Frames discarded because the queue was full.
    pub fn drops(&self) -> u64 {
        self.drops
    }

    /// Push one frame. When the queue is full the OLDEST frame is dropped to
    /// make room (latest-frame-wins) and counted; returns `true` when that
    /// happened.
    pub fn push(&mut self, frame: EncodedFrame) -> bool {
        let dropped_stale = if self.frames.len() >= self.capacity {
            self.frames.pop_front();
            self.drops += 1;
            true
        } else {
            false
        };
        self.frames.push_back(frame);
        dropped_stale
    }

    /// Pop the oldest frame (FIFO order).
    pub fn pop_front(&mut self) -> Option<EncodedFrame> {
        self.frames.pop_front()
    }

    /// Discard every queued frame (used when the transport fails).
    pub fn clear(&mut self) {
        self.frames.clear();
    }

    /// Sequence numbers of queued frames in FIFO order (tests/diagnostics).
    pub fn sequences(&self) -> Vec<u64> {
        self.frames.iter().map(|frame| frame.sequence).collect()
    }
}

/// Outbound control traffic. Both the legacy control encoding and the
/// versioned protocol messages travel on the same reliable control channel so
/// negotiation, lifecycle, keyframe requests, and quality changes stay ordered
/// and never share a queue with video data.
#[derive(Debug, Clone)]
pub enum ControlOut {
    /// Legacy control-plane wire encoding (Hello/Accept/Reject/EndSession/
    /// RequestControl/GrantControl/RevokeControl/Input).
    Legacy(ControlMessage),
    /// Versioned protocol message (negotiation/lifecycle/keyframe/quality).
    Versioned(ScreenShareMessage),
}

/// Reliable, bounded control channel.
///
/// [`send`](Self::send) applies backpressure by awaiting buffer space when the
/// queue is full — the correct policy for low-rate control traffic. A worker
/// task drains the queue onto fresh QUIC streams, so a large video frame on
/// the media channel never blocks control delivery (streams are independent).
#[derive(Debug, Clone)]
pub struct ControlChannel {
    tx: mpsc::Sender<ControlOut>,
    failed: Arc<AtomicBool>,
}

impl ControlChannel {
    /// Create a channel and spawn its reliable transport worker.
    pub fn new(transport: QuicScreenTransport, capacity: usize) -> Result<Self, ScreenShareError> {
        if capacity == 0 {
            return Err(ScreenShareError::new("control channel capacity must be non-zero"));
        }
        let (tx, rx) = mpsc::channel(capacity);
        let channel = Self {
            tx,
            failed: Arc::new(AtomicBool::new(false)),
        };
        channel.spawn_worker(transport, rx);
        Ok(channel)
    }

    fn spawn_worker(&self, transport: QuicScreenTransport, mut rx: mpsc::Receiver<ControlOut>) {
        let failed = Arc::clone(&self.failed);
        tokio::spawn(async move {
            while let Some(message) = rx.recv().await {
                let result = match &message {
                    ControlOut::Legacy(message) => transport.send_control(message).await,
                    ControlOut::Versioned(message) => transport.send_screen_share(message).await,
                };
                if let Err(error) = result {
                    tracing::warn!(error = %error, "screen-share: control channel send failed");
                    failed.store(true, Ordering::Relaxed);
                    break;
                }
            }
        });
    }

    /// Send one control message, awaiting buffer space when the queue is full.
    pub async fn send(&self, message: ControlOut) -> Result<(), ScreenShareError> {
        self.tx
            .send(message)
            .await
            .map_err(|_| ScreenShareError::new("control channel closed"))
    }

    /// Send one control message without blocking. Fails with an error when the
    /// queue is full or the worker has exited.
    pub fn try_send(&self, message: ControlOut) -> Result<(), ScreenShareError> {
        self.tx.try_send(message).map_err(|error| match error {
            mpsc::error::TrySendError::Full(_) => {
                ScreenShareError::new("control channel full")
            }
            mpsc::error::TrySendError::Closed(_) => {
                ScreenShareError::new("control channel closed")
            }
        })
    }

    /// Whether the transport worker has exited after a send failure.
    pub fn failed(&self) -> bool {
        self.failed.load(Ordering::Relaxed)
    }

    /// Whether the channel is closed (worker exited or all senders dropped).
    pub fn is_closed(&self) -> bool {
        self.tx.is_closed()
    }
}

/// Dedicated media channel with a bounded latest-frame-drop queue.
///
/// The capture/encode loop pushes frames with [`send_frame`](Self::send_frame),
/// which never blocks and never grows memory without bound: when the queue is
/// full the oldest frame is dropped to make room. A worker task drains the
/// queue onto QUIC media streams, applying network backpressure there (it
/// awaits the actual write), so a slow link slows the *worker*, not the
/// capture loop, and only ever discards stale frames.
#[derive(Debug, Clone)]
pub struct MediaChannel {
    queue: Arc<tokio::sync::Mutex<BoundedFrameQueue>>,
    notify: Arc<Notify>,
    /// Frames dropped because the queue was full.
    dropped: Arc<AtomicU64>,
    /// Frames successfully handed to the transport worker.
    sent: Arc<AtomicU64>,
    /// Set when the transport worker hit a fatal send error.
    failed: Arc<AtomicBool>,
}

impl MediaChannel {
    /// Create a channel and spawn its media transport worker.
    pub fn new(transport: QuicScreenTransport, capacity: usize) -> Result<Self, ScreenShareError> {
        let channel = Self::new_shared(capacity)?;
        channel.spawn_worker(transport);
        Ok(channel)
    }

    /// Create a channel without a transport worker (tests drive the queue
    /// directly; production goes through [`new`](Self::new)).
    pub(crate) fn new_shared(capacity: usize) -> Result<Self, ScreenShareError> {
        Ok(Self {
            queue: Arc::new(tokio::sync::Mutex::new(BoundedFrameQueue::new(capacity)?)),
            notify: Arc::new(Notify::new()),
            dropped: Arc::new(AtomicU64::new(0)),
            sent: Arc::new(AtomicU64::new(0)),
            failed: Arc::new(AtomicBool::new(false)),
        })
    }

    /// Push one encoded frame without blocking. When the queue is full the
    /// oldest queued frame is dropped (latest-frame-wins) and the drop is
    /// counted; returns `true` when a stale frame was dropped to make room.
    pub async fn send_frame(&self, frame: EncodedFrame) -> bool {
        let dropped_stale = {
            let mut queue = self.queue.lock().await;
            queue.push(frame)
        };
        self.notify.notify_one();
        if dropped_stale {
            self.dropped.fetch_add(1, Ordering::Relaxed);
        }
        dropped_stale
    }

    /// Current number of queued frames.
    pub async fn len(&self) -> usize {
        self.queue.lock().await.len()
    }

    /// Pop the oldest queued frame (used by tests to drain a worker-less
    /// channel).
    pub async fn pop_frame(&self) -> Option<EncodedFrame> {
        self.queue.lock().await.pop_front()
    }

    /// Frames discarded because the queue was full.
    pub fn dropped(&self) -> u64 {
        self.dropped.load(Ordering::Relaxed)
    }

    /// Frames handed to the transport worker (successful sends).
    pub fn sent(&self) -> u64 {
        self.sent.load(Ordering::Relaxed)
    }

    /// Whether the transport worker has exited after a fatal send error.
    pub fn failed(&self) -> bool {
        self.failed.load(Ordering::Relaxed)
    }

    /// Discard every queued frame (used when the session ends).
    pub async fn clear(&self) {
        self.queue.lock().await.clear();
    }

    fn spawn_worker(&self, transport: QuicScreenTransport) {
        let queue = Arc::clone(&self.queue);
        let notify = Arc::clone(&self.notify);
        let sent = Arc::clone(&self.sent);
        let failed = Arc::clone(&self.failed);
        tokio::spawn(async move {
            loop {
                // Register the waiter BEFORE draining so a push between the
                // empty check and the await cannot be missed: a notify_one
                // that arrives before we await completes the future
                // immediately (tokio stores the permit).
                let notified = notify.notified();
                loop {
                    let frame = {
                        let mut queue = queue.lock().await;
                        queue.pop_front()
                    };
                    let Some(frame) = frame else { break };
                    match transport.send_frame(&frame).await {
                        Ok(()) => {
                            sent.fetch_add(1, Ordering::Relaxed);
                        }
                        Err(error) => {
                            tracing::warn!(
                                error = %error,
                                "screen-share: media channel send failed; discarding queue"
                            );
                            failed.store(true, Ordering::Relaxed);
                            let mut queue = queue.lock().await;
                            queue.clear();
                            return;
                        }
                    }
                }
                notified.await;
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::screen_share::protocol::SCREEN_SHARE_PROTOCOL_VERSION;
    use crate::screen_share::session::ScreenShareSessionId;

    fn frame(sequence: u64, keyframe: bool) -> EncodedFrame {
        EncodedFrame {
            timestamp_us: sequence,
            sequence,
            keyframe,
            config_generation: 0,
            width: 640,
            height: 360,
            bytes: vec![0xAB; 64],
        }
    }

    #[test]
    fn bounded_queue_drops_oldest_when_full() {
        let mut queue = BoundedFrameQueue::new(2).unwrap();
        assert!(!queue.push(frame(1, true)));
        assert!(!queue.push(frame(2, false)));
        // Third push overflows: the oldest (seq 1) is dropped, not the newest.
        assert!(queue.push(frame(3, false)), "push onto a full queue must drop the oldest");
        assert_eq!(queue.len(), 2);
        assert_eq!(queue.sequences(), vec![2, 3]);
        assert_eq!(queue.drops(), 1);
    }

    #[test]
    fn bounded_queue_never_grows_beyond_capacity() {
        let mut queue = BoundedFrameQueue::new(4).unwrap();
        let mut dropped = 0;
        for sequence in 1..=100 {
            dropped += usize::from(queue.push(frame(sequence, sequence % 25 == 0)));
            assert!(queue.len() <= 4, "queue must never exceed capacity");
        }
        assert_eq!(queue.drops() as usize, dropped);
        assert_eq!(queue.drops(), 96);
        // Only the newest 4 frames survive.
        assert_eq!(queue.sequences(), vec![97, 98, 99, 100]);
    }

    #[test]
    fn bounded_queue_rejects_zero_capacity() {
        assert!(BoundedFrameQueue::new(0).is_err());
    }

    #[tokio::test]
    async fn media_channel_drops_stale_frames_without_blocking() {
        let channel = MediaChannel::new_shared(2).unwrap();
        for sequence in 1..=50 {
            // send_frame never blocks and never grows the queue.
            channel.send_frame(frame(sequence, sequence % 25 == 0)).await;
            assert!(channel.len().await <= 2);
        }
        // 48 of the 50 frames were dropped as stale; the newest two survive.
        assert_eq!(channel.dropped(), 48);
        assert_eq!(channel.len().await, 2);
        let first = channel.pop_frame().await.unwrap();
        let second = channel.pop_frame().await.unwrap();
        assert_eq!((first.sequence, second.sequence), (49, 50));
    }

    #[tokio::test]
    async fn media_channel_latest_frame_wins_under_sustained_load() {
        let channel = MediaChannel::new_shared(1).unwrap();
        for sequence in 1..=10 {
            channel.send_frame(frame(sequence, false)).await;
        }
        // Capacity 1 + drop-oldest ⇒ exactly one frame remains: the newest.
        assert_eq!(channel.len().await, 1);
        assert_eq!(channel.pop_frame().await.unwrap().sequence, 10);
        assert_eq!(channel.dropped(), 9);
    }

    #[tokio::test]
    async fn control_channel_backpressure_blocks_when_full() {
        let (tx, mut rx) = mpsc::channel::<ControlOut>(1);
        let channel = ControlChannel {
            tx,
            failed: Arc::new(AtomicBool::new(false)),
        };
        let end = |id: u8| {
            ControlOut::Legacy(ControlMessage::EndSession {
                version: SCREEN_SHARE_PROTOCOL_VERSION,
                session_id: ScreenShareSessionId::from_bytes([id; 16]),
            })
        };
        // First message fills the single-slot queue.
        channel.send(end(1)).await.unwrap();
        // Second send must NOT complete while the queue is full.
        let blocked = tokio::time::timeout(
            std::time::Duration::from_millis(50),
            channel.send(end(2)),
        )
        .await;
        assert!(blocked.is_err(), "control send must apply backpressure when the queue is full");
        // Draining one slot lets the blocked send complete.
        let _ = rx.recv().await.unwrap();
        let completed = tokio::time::timeout(
            std::time::Duration::from_secs(1),
            channel.send(end(2)),
        )
        .await;
        assert!(completed.is_ok(), "control send must complete once buffer space is available");
        let _ = rx.recv().await.unwrap();
    }

    #[tokio::test]
    async fn control_channel_try_send_refuses_when_full() {
        let (tx, mut rx) = mpsc::channel::<ControlOut>(1);
        let channel = ControlChannel {
            tx,
            failed: Arc::new(AtomicBool::new(false)),
        };
        let end = |id: u8| {
            ControlOut::Legacy(ControlMessage::EndSession {
                version: SCREEN_SHARE_PROTOCOL_VERSION,
                session_id: ScreenShareSessionId::from_bytes([id; 16]),
            })
        };
        assert!(channel.try_send(end(3)).is_ok());
        assert!(
            channel.try_send(end(3)).is_err(),
            "try_send must refuse when the control queue is full"
        );
        let _ = rx.recv().await.unwrap();
        assert!(channel.try_send(end(3)).is_ok());
    }

    #[tokio::test]
    async fn control_traffic_is_not_blocked_by_a_full_media_queue() {
        // A media channel stuck at capacity (no worker draining it) must not
        // affect the independent control channel: this is the control-vs-media
        // isolation property.
        let media = MediaChannel::new_shared(1).unwrap();
        media.send_frame(frame(1, true)).await;
        assert_eq!(media.len().await, 1, "media queue is full and stalled");

        let (tx, mut rx) = mpsc::channel::<ControlOut>(1);
        let control = ControlChannel {
            tx,
            failed: Arc::new(AtomicBool::new(false)),
        };
        let end = ControlOut::Legacy(ControlMessage::EndSession {
            version: SCREEN_SHARE_PROTOCOL_VERSION,
            session_id: ScreenShareSessionId::from_bytes([4; 16]),
        });
        // Control can still enqueue and drain while the media queue is full.
        control.send(end).await.unwrap();
        let _ = rx.recv().await.unwrap();
        // Media keeps dropping stale frames without blocking control.
        media.send_frame(frame(2, false)).await;
        assert_eq!(media.dropped(), 1);
        assert_eq!(media.len().await, 1);
        assert_eq!(media.pop_frame().await.unwrap().sequence, 2);
    }
}
