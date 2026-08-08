//! Non-blocking Opus audio datagram sending.
//!
//! Audio is real-time media: an encoded frame that cannot be handed to QUIC
//! immediately is stale, so it is dropped rather than waiting behind older
//! frames. The queue is deliberately capped at four frames (80 ms at the
//! current 20 ms frame clock); callers should normally enqueue and flush once
//! per capture tick.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, Ordering};

use bytes::Bytes;

use crate::call::media::{MediaDatagram, MediaKind};
use crate::call::CallId;

/// Maximum number of encoded audio frames retained by the outbound path.
///
/// This is a hard bound, not a tuning hint: it prevents congestion from
/// turning into seconds of latency or unbounded memory use.
pub const MAX_OUTBOUND_AUDIO_FRAMES: usize = 4;

/// One already-encoded Opus frame and its media-clock metadata.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EncodedAudioFrame {
    /// Media sequence number assigned by the audio frame clock.
    pub sequence: u32,
    /// Media timestamp assigned by the audio frame clock.
    pub timestamp: u32,
    /// One Opus packet, without the media datagram header.
    pub payload: Vec<u8>,
}

/// Minimal non-blocking datagram interface used by [`AudioSender`].
///
/// `try_send_datagram` must not wait for buffer capacity. The Iroh
/// implementation below uses `Connection::send_datagram`, deliberately not
/// `send_datagram_wait`.
pub trait AudioDatagramTransport {
    /// Error returned when the datagram cannot be accepted immediately.
    type Error;

    /// Try once; this method must never wait for buffer capacity.
    fn try_send_datagram(&self, data: Bytes) -> Result<(), Self::Error>;
}

#[cfg(feature = "net")]
impl AudioDatagramTransport for iroh::endpoint::Connection {
    type Error = iroh::endpoint::SendDatagramError;

    fn try_send_datagram(&self, data: Bytes) -> Result<(), Self::Error> {
        self.send_datagram(data)
    }
}

/// Bounded, drop-on-congestion sender for encoded audio.
#[derive(Debug)]
pub struct AudioSender<T> {
    transport: T,
    call_id: CallId,
    track_id: u32,
    queue: VecDeque<MediaDatagram>,
    dropped_outbound: u64,
    muted: AtomicBool,
}

impl<T> AudioSender<T> {
    /// Create a sender for one audio track. Track id zero is not valid on the
    /// media wire format and is rejected immediately.
    pub fn new(transport: T, call_id: CallId, track_id: u32) -> Result<Self, &'static str> {
        if track_id == 0 {
            return Err("audio track id must be non-zero");
        }
        Ok(Self {
            transport,
            call_id,
            track_id,
            queue: VecDeque::with_capacity(MAX_OUTBOUND_AUDIO_FRAMES),
            dropped_outbound: 0,
            muted: AtomicBool::new(false),
        })
    }

    /// Number of frames discarded due to a full queue or send congestion.
    pub const fn dropped_outbound(&self) -> u64 {
        self.dropped_outbound
    }

    /// Number of frames currently waiting for a non-blocking flush.
    pub fn queued_frames(&self) -> usize {
        self.queue.len()
    }

    /// Change the local mute gate. Muting suppresses new network media while
    /// leaving the reliable control stream and keepalive unaffected.
    pub fn set_muted(&self, muted: bool) {
        self.muted.store(muted, Ordering::Release);
    }

    /// Return whether this sender currently suppresses audio frames.
    pub fn is_muted(&self) -> bool {
        self.muted.load(Ordering::Acquire)
    }

    /// Queue one Opus frame without waiting for network capacity.
    ///
    /// If the four-frame bound is reached, the new frame is dropped. Keeping
    /// older frames in this tiny queue preserves ordering while ensuring that
    /// the queue can never accumulate seconds of stale speech.
    pub fn enqueue(&mut self, frame: EncodedAudioFrame) -> bool {
        if self.is_muted() {
            return false;
        }
        if self.queue.len() == MAX_OUTBOUND_AUDIO_FRAMES {
            self.dropped_outbound += 1;
            return false;
        }

        self.queue.push_back(MediaDatagram {
            kind: MediaKind::Audio,
            flags: 0,
            call_id: self.call_id,
            track_id: self.track_id,
            sequence: frame.sequence,
            timestamp: frame.timestamp,
            fragment_index: 0,
            fragment_count: 1,
            payload: frame.payload,
        });
        true
    }

    /// Enqueue and immediately attempt to send one frame without waiting.
    pub fn try_send(&mut self, frame: EncodedAudioFrame) -> bool
    where
        T: AudioDatagramTransport,
    {
        if !self.enqueue(frame) {
            return false;
        }
        self.flush() == 1
    }

    /// Attempt every queued frame once, dropping failures and continuing.
    ///
    /// A failed `send_datagram` is congestion or an unavailable datagram path;
    /// neither condition justifies awaiting capacity for live audio. Returns
    /// the number of frames handed to the transport successfully.
    pub fn flush(&mut self) -> usize
    where
        T: AudioDatagramTransport,
    {
        let mut sent = 0;
        while let Some(packet) = self.queue.pop_front() {
            if self
                .transport
                .try_send_datagram(Bytes::from(packet.encode()))
                .is_ok()
            {
                sent += 1;
            } else {
                self.dropped_outbound += 1;
            }
        }
        sent
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::call::media::MediaDatagram;
    use std::rc::Rc;

    #[derive(Debug, Default)]
    struct MockTransport {
        sent: Rc<std::cell::RefCell<Vec<Bytes>>>,
        congested: bool,
    }

    impl AudioDatagramTransport for MockTransport {
        type Error = ();

        fn try_send_datagram(&self, data: Bytes) -> Result<(), Self::Error> {
            if self.congested {
                return Err(());
            }
            self.sent.borrow_mut().push(data);
            Ok(())
        }
    }

    fn frame(sequence: u32) -> EncodedAudioFrame {
        EncodedAudioFrame {
            sequence,
            timestamp: sequence * 960,
            payload: vec![0xAA, sequence as u8],
        }
    }

    fn sender(transport: MockTransport) -> AudioSender<MockTransport> {
        AudioSender::new(transport, CallId::from_bytes([1; 16]), 1).unwrap()
    }

    #[test]
    fn normal_frame_is_encoded_and_sent_as_audio_datagram() {
        let transport = MockTransport::default();
        let sent = Rc::clone(&transport.sent);
        let mut sender = sender(transport);

        assert!(sender.try_send(frame(7)));
        assert_eq!(sender.dropped_outbound(), 0);
        let packet = MediaDatagram::parse(&sent.borrow()[0]).unwrap();
        assert_eq!(packet.kind, MediaKind::Audio);
        assert_eq!(packet.sequence, 7);
        assert_eq!(packet.timestamp, 7 * 960);
        assert_eq!(packet.payload, vec![0xAA, 7]);
    }

    #[test]
    fn congestion_drops_frames_and_counts_each_drop() {
        let mut sender = sender(MockTransport {
            congested: true,
            ..MockTransport::default()
        });
        assert!(!sender.try_send(frame(1)));
        assert_eq!(sender.queued_frames(), 0);
        assert_eq!(sender.dropped_outbound(), 1);
    }

    #[test]
    fn outbound_queue_is_hard_bounded_to_four_frames() {
        let mut sender = sender(MockTransport {
            congested: true,
            ..MockTransport::default()
        });
        for sequence in 0..MAX_OUTBOUND_AUDIO_FRAMES {
            assert!(sender.enqueue(frame(sequence as u32)));
        }
        assert!(!sender.enqueue(frame(99)));
        assert_eq!(sender.queued_frames(), MAX_OUTBOUND_AUDIO_FRAMES);
        assert_eq!(sender.dropped_outbound(), 1);
        assert_eq!(sender.flush(), 0);
        assert_eq!(sender.dropped_outbound(), 5);
    }

    #[test]
    fn mute_suppresses_frames_and_unmute_resumes() {
        let transport = MockTransport::default();
        let sent = Rc::clone(&transport.sent);
        let mut sender = sender(transport);
        sender.set_muted(true);
        assert!(!sender.try_send(frame(1)));
        assert!(sent.borrow().is_empty());
        sender.set_muted(false);
        assert!(sender.try_send(frame(2)));
        assert_eq!(sent.borrow().len(), 1);
    }
}
