//! Audio-priority, bounded sender for live media datagrams.
//!
//! A single sender owns scheduling for both tracks. Audio is drained before
//! video, and video is retained as one whole encoded frame so congestion never
//! produces a partial frame on the wire.

use std::collections::VecDeque;
use std::time::{Duration, Instant};

use bytes::Bytes;

use super::audio::send::EncodedAudioFrame;
use super::media::{DatagramSizer, MediaDatagram, MediaDatagramError, MediaKind};
use super::video::codec::EncodedVideoFrame;
use super::video::packet::VideoPacketizer;
use super::CallId;

/// Default average media budget (audio + video), in bits per second.
pub const DEFAULT_TARGET_BITRATE_BPS: u64 = 1_000_000;
/// Maximum instantaneous budget used to bound bursts.
pub const DEFAULT_PEAK_BITRATE_BPS: u64 = 1_500_000;
/// Maximum token-bucket burst window.
pub const TOKEN_BURST: Duration = Duration::from_millis(100);
/// Maximum queued encoded audio frames.
pub const MAX_AUDIO_QUEUE: usize = 4;

/// Compatibility alias for callers that refer to the scheduler as a token
/// bucket rather than by its peak-rate policy.
pub type TokenBucket = PeakRateTokenBucket;

/// Non-blocking datagram transport owned by [`MediaSender`].
pub trait MediaDatagramTransport {
    /// Transport error type.
    type Error;
    /// Submit one datagram without waiting for capacity.
    fn try_send_datagram(&self, data: Bytes) -> Result<(), Self::Error>;
}

#[cfg(feature = "net")]
impl MediaDatagramTransport for iroh::endpoint::Connection {
    type Error = iroh::endpoint::SendDatagramError;

    fn try_send_datagram(&self, data: Bytes) -> Result<(), Self::Error> {
        self.send_datagram(data)
    }
}

/// A token bucket enforcing both the target average and peak burst rate.
#[derive(Debug, Clone)]
pub struct PeakRateTokenBucket {
    target_bytes_per_sec: u64,
    peak_bytes_per_sec: u64,
    tokens: u64,
    last: Instant,
}

impl PeakRateTokenBucket {
    /// Create a bucket. Rates are clamped so peak is never below target.
    pub fn new(target_bitrate_bps: u64, peak_bitrate_bps: u64, now: Instant) -> Self {
        let target = target_bitrate_bps.max(1).div_ceil(8);
        let peak = peak_bitrate_bps.max(target_bitrate_bps).div_ceil(8);
        Self {
            target_bytes_per_sec: target,
            peak_bytes_per_sec: peak,
            tokens: peak.saturating_mul(TOKEN_BURST.as_millis() as u64) / 1_000,
            last: now,
        }
    }

    /// Return the configured average and peak rates in bytes per second.
    pub const fn rates(&self) -> (u64, u64) {
        (self.target_bytes_per_sec, self.peak_bytes_per_sec)
    }

    fn capacity(&self) -> u64 {
        (self
            .peak_bytes_per_sec
            .saturating_mul(TOKEN_BURST.as_millis() as u64)
            / 1_000)
            .max(1)
    }

    /// Refill and atomically reserve `bytes`; returns whether they fit now.
    pub fn try_take(&mut self, bytes: usize, now: Instant) -> bool {
        let elapsed = now.saturating_duration_since(self.last);
        self.last = now;
        let refill = self
            .target_bytes_per_sec
            .saturating_mul(elapsed.as_millis() as u64)
            / 1_000;
        self.tokens = self.tokens.saturating_add(refill).min(self.capacity());
        let bytes = bytes as u64;
        if self.tokens < bytes {
            return false;
        }
        self.tokens -= bytes;
        true
    }

    /// Current token count, useful for diagnostics and deterministic tests.
    pub const fn available_tokens(&self) -> u64 {
        self.tokens
    }
}

/// Sender counters exported for call statistics and diagnostics.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct MediaSenderMetrics {
    /// Number of audio frames currently queued.
    pub audio_queued: usize,
    /// Number of video frames currently queued (zero or one).
    pub video_queued: usize,
    /// Age in milliseconds of the oldest queued audio frame.
    pub oldest_audio_age_ms: u64,
    /// Age in milliseconds of the queued video frame.
    pub video_age_ms: u64,
    /// Whole video frames displaced by a newer frame.
    pub video_frames_dropped: u64,
    /// Audio frames dropped due to queue, capacity, or token pressure.
    pub audio_frames_dropped: u64,
    /// Datagrams handed to the transport.
    pub datagrams_sent: u64,
    /// Bytes handed to the transport, including the 40-byte header.
    pub bytes_sent: u64,
}

#[derive(Debug)]
struct Queued<T> {
    item: T,
    queued_at: Instant,
}

/// One bounded scheduler for audio and video datagrams.
#[derive(Debug)]
pub struct MediaSender<T> {
    transport: T,
    call_id: CallId,
    audio_track_id: u32,
    video_track_id: u32,
    audio: VecDeque<Queued<MediaDatagram>>,
    video: Option<Queued<EncodedVideoFrame>>,
    packetizer: VideoPacketizer,
    sizer: DatagramSizer,
    bucket: PeakRateTokenBucket,
    metrics: MediaSenderMetrics,
}

impl<T> MediaSender<T> {
    /// Construct a sender with the standard one-megabit target and
    /// 1.5-megabit peak budget.
    pub fn with_defaults(
        transport: T,
        call_id: CallId,
        audio_track_id: u32,
        video_track_id: u32,
        now: Instant,
    ) -> Result<Self, &'static str> {
        Self::new(
            transport,
            call_id,
            audio_track_id,
            video_track_id,
            DEFAULT_TARGET_BITRATE_BPS,
            DEFAULT_PEAK_BITRATE_BPS,
            now,
        )
    }

    /// Construct a sender with explicit target and peak rates.
    pub fn new(
        transport: T,
        call_id: CallId,
        audio_track_id: u32,
        video_track_id: u32,
        target_bitrate_bps: u64,
        peak_bitrate_bps: u64,
        now: Instant,
    ) -> Result<Self, &'static str> {
        if audio_track_id == 0 || video_track_id == 0 {
            return Err("media track ids must be non-zero");
        }
        Ok(Self {
            transport,
            call_id,
            audio_track_id,
            video_track_id,
            audio: VecDeque::with_capacity(MAX_AUDIO_QUEUE),
            video: None,
            packetizer: VideoPacketizer::new(),
            sizer: DatagramSizer::per_frame(),
            bucket: PeakRateTokenBucket::new(target_bitrate_bps, peak_bitrate_bps, now),
            metrics: MediaSenderMetrics::default(),
        })
    }

    /// Queue an audio frame, retaining at most four frames.
    pub fn enqueue_audio(&mut self, frame: EncodedAudioFrame, now: Instant) -> bool {
        if self.audio.len() >= MAX_AUDIO_QUEUE {
            self.metrics.audio_frames_dropped += 1;
            return false;
        }
        self.audio.push_back(Queued {
            item: MediaDatagram {
                kind: MediaKind::Audio,
                flags: 0,
                call_id: self.call_id,
                track_id: self.audio_track_id,
                sequence: frame.sequence,
                timestamp: frame.timestamp,
                fragment_index: 0,
                fragment_count: 1,
                payload: frame.payload,
            },
            queued_at: now,
        });
        true
    }

    /// Replace the pending video frame. The displaced frame is dropped whole.
    pub fn enqueue_video(&mut self, frame: EncodedVideoFrame, now: Instant) -> bool {
        if let Some(old) = self.video.replace(Queued {
            item: frame,
            queued_at: now,
        }) {
            let _ = old;
            self.metrics.video_frames_dropped += 1;
        }
        true
    }

    /// Force a fresh path-MTU lookup before the next flush.
    pub fn refresh_capacity(&mut self) {
        self.sizer.refresh();
    }

    /// Snapshot queue, age, and drop metrics.
    pub fn metrics(&self, now: Instant) -> MediaSenderMetrics {
        let mut metrics = self.metrics;
        metrics.audio_queued = self.audio.len();
        metrics.video_queued = usize::from(self.video.is_some());
        metrics.oldest_audio_age_ms = self.audio.front().map_or(0, |q| {
            now.saturating_duration_since(q.queued_at).as_millis() as u64
        });
        metrics.video_age_ms = self.video.as_ref().map_or(0, |q| {
            now.saturating_duration_since(q.queued_at).as_millis() as u64
        });
        metrics
    }

    /// Send queued media once, always draining audio before video.
    ///
    /// A video frame is packetized only when selected and is removed only after
    /// all its fragments are sent. Any token, capacity, or transport failure
    /// drops the entire frame, never a subset of its fragments.
    pub fn flush_at(
        &mut self,
        maximum_datagram: Option<usize>,
        now: Instant,
    ) -> Result<usize, MediaDatagramError>
    where
        T: MediaDatagramTransport,
    {
        let capacity = self.sizer.payload_capacity_from(maximum_datagram)?;
        let mut sent = 0;
        while let Some(queued) = self.audio.front() {
            let encoded = queued.item.encode();
            if !self.bucket.try_take(encoded.len(), now) {
                break;
            }
            let queued = self.audio.pop_front().expect("front exists");
            if self
                .transport
                .try_send_datagram(Bytes::from(encoded))
                .is_err()
            {
                self.metrics.audio_frames_dropped += 1;
            } else {
                self.metrics.datagrams_sent += 1;
                self.metrics.bytes_sent += queued.item.encode().len() as u64;
                sent += 1;
            }
        }
        // Do not let video consume budget while audio is waiting. This keeps
        // the priority guarantee even when the target bucket is temporarily
        // empty.
        if !self.audio.is_empty() {
            return Ok(sent);
        }
        let Some(queued) = self.video.take() else {
            return Ok(sent);
        };
        let frame = match self.packetizer.fragment_frame(
            self.call_id,
            self.video_track_id,
            &queued.item,
            capacity + super::media::MEDIA_HEADER_SIZE,
        ) {
            Ok(frame) => frame,
            Err(error) => {
                self.metrics.video_frames_dropped += 1;
                return Err(error);
            }
        };
        let encoded: Vec<Vec<u8>> = frame.into_iter().map(|p| p.encode()).collect();
        let total: usize = encoded.iter().map(Vec::len).sum();
        if !self.bucket.try_take(total, now)
            || encoded.iter().any(|data| {
                self.transport
                    .try_send_datagram(Bytes::from(data.clone()))
                    .is_err()
            })
        {
            self.metrics.video_frames_dropped += 1;
            return Ok(sent);
        }
        self.metrics.datagrams_sent += encoded.len() as u64;
        self.metrics.bytes_sent += total as u64;
        Ok(sent + encoded.len())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;
    use std::rc::Rc;

    #[derive(Debug, Default)]
    struct Mock(Rc<RefCell<Vec<Bytes>>>);
    impl MediaDatagramTransport for Mock {
        type Error = ();
        fn try_send_datagram(&self, data: Bytes) -> Result<(), Self::Error> {
            self.0.borrow_mut().push(data);
            Ok(())
        }
    }

    fn audio(seq: u32) -> EncodedAudioFrame {
        EncodedAudioFrame {
            sequence: seq,
            timestamp: seq,
            payload: vec![1],
        }
    }
    fn video() -> EncodedVideoFrame {
        EncodedVideoFrame {
            codec: super::super::video::codec::VideoCodec::H264,
            width: 1,
            height: 1,
            timestamp_us: 1,
            keyframe: false,
            bytes: vec![2],
        }
    }

    #[test]
    fn audio_is_sent_before_video_and_new_video_replaces_old() {
        let now = Instant::now();
        let out = Rc::new(RefCell::new(Vec::new()));
        let mut sender = MediaSender::new(
            Mock(out.clone()),
            CallId::from_bytes([1; 16]),
            1,
            2,
            10_000_000,
            10_000_000,
            now,
        )
        .unwrap();
        sender.enqueue_video(video(), now);
        sender.enqueue_video(video(), now);
        sender.enqueue_audio(audio(1), now);
        sender.flush_at(Some(1200), now).unwrap();
        let packets: Vec<_> = out
            .borrow()
            .iter()
            .map(|b| MediaDatagram::parse(b).unwrap().kind)
            .collect();
        assert_eq!(packets[0], MediaKind::Audio);
        assert_eq!(sender.metrics(now).video_frames_dropped, 1);
    }

    #[test]
    fn bucket_refill_uses_target_but_never_exceeds_peak_capacity() {
        let start = Instant::now();
        let mut bucket = PeakRateTokenBucket::new(80_000, 160_000, start);
        assert!(bucket.try_take(2_000, start));
        assert!(bucket.available_tokens() <= 2_000);
        assert!(!bucket.try_take(20_000, start));
        assert!(bucket.try_take(2_000, start + Duration::from_millis(200)));
    }
}
