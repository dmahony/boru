//! Bounded receiver-side decode pipeline for screen-share media.
//!
//! QUIC media units are validated by the transport before they reach this
//! module. The pipeline still re-checks session and ordering invariants because
//! callers may feed it from tests or another transport. It deliberately drops
//! obsolete frames instead of waiting for a missing sequence number: interactive
//! video is better served by the newest decodable frame than by unbounded replay.

use std::collections::VecDeque;
use std::time::Instant;

use super::{
    codec::{EncodedFrame, VideoDecoder},
    transport::MediaHeader,
    CapturedFrame, ScreenShareError,
    stats::{ScreenShareStats, ScreenShareStatsSnapshot},
};

/// A decoded frame ready for presentation by the GUI.
pub type DecodedFrame = CapturedFrame;

/// Bounded, authorization-aware receiver decode pipeline.
///
/// `process` is synchronous by design and should be called from a worker task,
/// not from Iced's update function. At most `queue_capacity` encoded units and
/// one decoded frame are retained, so a slow decoder cannot grow memory without
/// bound.
#[allow(missing_debug_implementations)]
pub struct ViewerPipeline<D: VideoDecoder> {
    decoder: D,
    session_id: [u8; 16],
    queue: VecDeque<(MediaHeader, Vec<u8>)>,
    queue_capacity: usize,
    decoded: Option<DecodedFrame>,
    last_sequence: Option<u64>,
    waiting_for_keyframe: bool,
    authorized: bool,
    ended: bool,
    dropped_frames: u64,
    decode_errors: u64,
    keyframe_requests: u64,
    stats: ScreenShareStats,
}

impl<D: VideoDecoder> ViewerPipeline<D> {
    /// Create a pipeline for one authorized session.
    pub fn new(
        decoder: D,
        session_id: [u8; 16],
        queue_capacity: usize,
    ) -> Result<Self, ScreenShareError> {
        if session_id == [0; 16] {
            return Err(ScreenShareError::new("viewer session id is empty"));
        }
        if queue_capacity == 0 {
            return Err(ScreenShareError::new("viewer queue capacity must be non-zero"));
        }
        Ok(Self {
            decoder,
            session_id,
            queue: VecDeque::with_capacity(queue_capacity.min(8)),
            queue_capacity,
            decoded: None,
            last_sequence: None,
            waiting_for_keyframe: false,
            authorized: true,
            ended: false,
            dropped_frames: 0,
            decode_errors: 0,
            keyframe_requests: 0,
            stats: ScreenShareStats::new(),
        })
    }

    /// Revoke authorization and immediately discard queued/decoded media.
    pub fn revoke(&mut self) {
        self.authorized = false;
        self.queue.clear();
        self.decoded = None;
    }

    /// Mark the session ended and release all media state.
    pub fn end(&mut self) {
        self.ended = true;
        self.queue.clear();
        self.decoded = None;
    }

    /// Whether this pipeline can accept or decode media.
    pub fn is_active(&self) -> bool { self.authorized && !self.ended }

    /// Queue one already-framed media unit. Old frames are discarded promptly.
    pub fn enqueue(&mut self, header: MediaHeader, payload: Vec<u8>) -> Result<(), ScreenShareError> {
        if !self.is_active() {
            return Err(ScreenShareError::new("viewer session is not active"));
        }
        header.validate()?;
        if header.session_id != self.session_id {
            return Err(ScreenShareError::new("media session id does not match viewer"));
        }
        if self.last_sequence.is_some_and(|last| header.sequence <= last) {
            self.dropped_frames = self.dropped_frames.saturating_add(1);
            self.stats.observe_late_drop();
            return Ok(());
        }
        if payload.len() != header.payload_len as usize {
            return Err(ScreenShareError::new("media payload length mismatch"));
        }
        if self.queue.len() >= self.queue_capacity {
            self.queue.pop_front();
            self.dropped_frames = self.dropped_frames.saturating_add(1);
            self.stats.observe_late_drop();
        }
        // Advance the ordering watermark at enqueue time so out-of-order
        // arrivals are rejected promptly instead of queuing and possibly
        // winning the decode race over newer frames.
        self.last_sequence = Some(header.sequence);
        self.stats.observe_receive(header.timestamp_us, Instant::now());
        self.queue.push_back((header, payload));
        Ok(())
    }

    /// Decode all currently queued units without blocking for missing frames.
    /// Returns the number of units consumed.
    pub fn process(&mut self) -> usize {
        if !self.is_active() { return 0; }
        let mut consumed = 0;
        while let Some((header, payload)) = self.queue.pop_front() {
            consumed += 1;
            if self.waiting_for_keyframe && header.flags & MediaHeader::FLAG_KEYFRAME == 0 {
                self.dropped_frames = self.dropped_frames.saturating_add(1);
                self.stats.observe_late_drop();
                continue;
            }
            let frame = EncodedFrame {
                timestamp_us: header.timestamp_us,
                sequence: header.sequence,
                keyframe: header.flags & MediaHeader::FLAG_KEYFRAME != 0,
                config_generation: header.config_generation,
                width: header.width as u32,
                height: header.height as u32,
                bytes: payload,
            };
            let started = Instant::now();
            match self.decoder.decode(&frame) {
                Ok(Some(decoded)) => {
                    self.stats.observe_decode(started.elapsed(), false);
                    self.last_sequence = Some(header.sequence);
                    self.waiting_for_keyframe = false;
                    self.decoded = Some(decoded);
                }
                Ok(None) => {
                    self.stats.observe_decode(started.elapsed(), false);
                    // The decoder needs more reference data. A subsequent
                    // keyframe remains eligible; obsolete dependent frames do
                    // not hold up the pipeline.
                    self.last_sequence = Some(header.sequence);
                }
                Err(_) => {
                    self.stats.observe_decode(started.elapsed(), true);
                    self.stats.observe_media_reset();
                    self.decode_errors = self.decode_errors.saturating_add(1);
                    self.dropped_frames = self.dropped_frames.saturating_add(1);
                    self.keyframe_requests = self.keyframe_requests.saturating_add(1);
                    self.waiting_for_keyframe = true;
                    let _ = self.decoder.reset();
                }
            }
        }
        consumed
    }

    /// Take the newest decoded frame for conversion to an Iced image handle.
    pub fn take_frame(&mut self) -> Option<DecodedFrame> {
        let frame = self.decoded.take();
        if frame.is_some() { self.stats.observe_render(); }
        frame
    }

    /// Number of encoded units discarded by ordering or queue bounds.
    pub fn dropped_frames(&self) -> u64 { self.dropped_frames }
    /// Number of decoder failures that triggered recovery.
    pub fn decode_errors(&self) -> u64 { self.decode_errors }
    /// Number of keyframe recovery requests generated after corruption.
    pub fn keyframe_requests(&self) -> u64 { self.keyframe_requests }
    /// Return local pipeline diagnostics for a developer overlay or debug log.
    pub fn stats(&mut self) -> ScreenShareStatsSnapshot { self.stats.snapshot() }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::screen_share::capture::PixelFormat;
    use std::collections::VecDeque;

    struct FakeDecoder { outputs: VecDeque<Result<Option<CapturedFrame>, ScreenShareError>>, resets: usize }
    impl VideoDecoder for FakeDecoder {
        fn decode(&mut self, frame: &EncodedFrame) -> Result<Option<CapturedFrame>, ScreenShareError> {
            self.outputs.pop_front().unwrap_or_else(|| Ok(Some(CapturedFrame {
                timestamp_us: frame.timestamp_us, width: 2, height: 2,
                pixel_format: PixelFormat::Rgba8, pixels: vec![1; 16], gpu_handle: None,
            })))
        }
        fn metadata(&self) -> crate::screen_share::CodecMetadata { panic!("unused") }
        fn reset(&mut self) -> Result<(), ScreenShareError> { self.resets += 1; Ok(()) }
    }
    fn header(seq: u64, keyframe: bool) -> MediaHeader {
        MediaHeader { version: 1, session_id: [7; 16], sequence: seq, timestamp_us: seq,
            codec: 1, flags: if keyframe { MediaHeader::FLAG_KEYFRAME } else { 0 },
            width: 2, height: 2, config_generation: 0, payload_len: 1 }
    }
    #[test]
    fn newest_decodable_frame_wins_without_waiting_for_gaps() {
        let mut p = ViewerPipeline::new(FakeDecoder { outputs: VecDeque::new(), resets: 0 }, [7; 16], 2).unwrap();
        p.enqueue(header(3, true), vec![3]).unwrap();
        p.enqueue(header(1, true), vec![1]).unwrap();
        p.process();
        assert_eq!(p.take_frame().unwrap().timestamp_us, 3);
        assert_eq!(p.dropped_frames(), 1);
    }
    #[test]
    fn decode_error_discards_dependents_until_keyframe() {
        let mut outputs = VecDeque::new();
        outputs.push_back(Err(ScreenShareError::new("corrupt")));
        let mut p = ViewerPipeline::new(FakeDecoder { outputs, resets: 0 }, [7; 16], 4).unwrap();
        p.enqueue(header(1, true), vec![1]).unwrap();
        p.enqueue(header(2, false), vec![2]).unwrap();
        p.enqueue(header(3, true), vec![3]).unwrap();
        p.process();
        assert_eq!(p.keyframe_requests(), 1);
        assert_eq!(p.take_frame().unwrap().timestamp_us, 3);
    }
    #[test]
    fn revoke_stops_decode_immediately() {
        let mut p = ViewerPipeline::new(FakeDecoder { outputs: VecDeque::new(), resets: 0 }, [7; 16], 2).unwrap();
        p.revoke();
        assert!(!p.is_active());
        assert!(p.enqueue(header(1, true), vec![1]).is_err());
        assert_eq!(p.process(), 0);
    }
}
