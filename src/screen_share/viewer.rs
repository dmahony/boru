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
    /// Set when the pipeline determines a fresh keyframe is needed
    /// (corrupt decode, missing sequence gap, or a dependent frame that
    /// produced no picture). Cleared by [`Self::take_keyframe_request`] so
    /// the caller can emit a `KeyframeRequest` on the control channel.
    pending_keyframe_request: bool,
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
            pending_keyframe_request: false,
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
        // Missing-frame detection (PDF Task 8.1): a sequence jump means one or
        // more units were lost upstream (pacing drop, media channel full, or
        // stream reset). Dependent P-frames can never decode without their
        // references, so request a fresh keyframe instead of letting the
        // decoder silently stall. A keyframe arrival self-heals the gap.
        if let Some(last) = self.last_sequence {
            let gap = header.sequence.saturating_sub(last + 1);
            if gap > 0 {
                self.dropped_frames = self.dropped_frames.saturating_add(gap);
                for _ in 0..gap.min(64) { self.stats.observe_late_drop(); }
                if header.flags & MediaHeader::FLAG_KEYFRAME == 0 {
                    self.waiting_for_keyframe = true;
                    self.note_keyframe_request();
                }
            }
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
                encode_timestamp_us: header.encode_timestamp_us,
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
                    // The decoder needs more reference data. A dependent frame
                    // that produces no picture is missing/corrupt from the
                    // viewer's perspective: recover by requesting a keyframe
                    // instead of stalling on it.
                    if !frame.keyframe && !self.waiting_for_keyframe {
                        self.waiting_for_keyframe = true;
                        self.note_keyframe_request();
                    }
                    // A subsequent keyframe remains eligible; obsolete
                    // dependent frames do not hold up the pipeline.
                    self.last_sequence = Some(header.sequence);
                }
                Err(_) => {
                    self.stats.observe_decode(started.elapsed(), true);
                    self.stats.observe_media_reset();
                    self.decode_errors = self.decode_errors.saturating_add(1);
                    self.dropped_frames = self.dropped_frames.saturating_add(1);
                    self.waiting_for_keyframe = true;
                    self.note_keyframe_request();
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

    /// Returns `true` once when a fresh keyframe is needed, clearing the
    /// pending flag. The caller (decode worker) should emit a
    /// `KeyframeRequest` on the control channel whenever this returns true.
    pub fn take_keyframe_request(&mut self) -> bool {
        let pending = self.pending_keyframe_request;
        self.pending_keyframe_request = false;
        pending
    }

    /// Record one keyframe recovery request: bump the monotonic counter, set
    /// the pending flag for [`Self::take_keyframe_request`], and feed the
    /// stats snapshot (BORU-SS-28 metrics).
    fn note_keyframe_request(&mut self) {
        self.keyframe_requests = self.keyframe_requests.saturating_add(1);
        self.pending_keyframe_request = true;
        self.stats.observe_keyframe_request();
    }
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
                pixel_format: PixelFormat::Rgba8, stride: 8, pixels: vec![1; 16],
                gpu_handle: None, dirty_region: None,
            })))
        }
        fn metadata(&self) -> crate::screen_share::CodecMetadata { panic!("unused") }
        fn reset(&mut self) -> Result<(), ScreenShareError> { self.resets += 1; Ok(()) }
    }
    fn header(seq: u64, keyframe: bool) -> MediaHeader {
        MediaHeader { version: 1, session_id: [7; 16], sequence: seq, timestamp_us: seq, encode_timestamp_us: seq,
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
        assert!(p.take_keyframe_request(), "decode error must surface a pending keyframe request");
        assert_eq!(p.take_frame().unwrap().timestamp_us, 3);
    }
    #[test]
    fn missing_sequence_gap_requests_keyframe_and_drops_dependents() {
        // Frames 6..8 never arrive: a jump from seq 5 to seq 9 means three
        // units were lost upstream. The pipeline must count the gap as
        // dropped, request a fresh keyframe, and drop dependents until one
        // arrives (PDF Task 8.1: missing frames → keyframe request).
        let mut p = ViewerPipeline::new(FakeDecoder { outputs: VecDeque::new(), resets: 0 }, [7; 16], 4).unwrap();
        p.enqueue(header(5, true), vec![5]).unwrap();
        p.process();
        assert!(p.take_frame().is_some());
        p.enqueue(header(9, false), vec![9]).unwrap();
        assert_eq!(p.dropped_frames(), 3, "the 3 missing units are counted as dropped at enqueue");
        assert_eq!(p.keyframe_requests(), 1);
        assert!(p.take_keyframe_request());
        p.process();
        // Dependent frames keep arriving but produce nothing until a keyframe.
        p.enqueue(header(10, false), vec![10]).unwrap();
        p.process();
        assert!(p.take_frame().is_none());
        // The fresh keyframe resynchronises the pipeline.
        p.enqueue(header(11, true), vec![11]).unwrap();
        p.process();
        assert_eq!(p.take_frame().unwrap().timestamp_us, 11);
        assert!(!p.take_keyframe_request(), "keyframe arrival clears the pending request");
    }
    #[test]
    fn keyframe_arrival_self_heals_gap_without_request() {
        // A gap followed by a keyframe is self-healing: no request needed.
        let mut p = ViewerPipeline::new(FakeDecoder { outputs: VecDeque::new(), resets: 0 }, [7; 16], 4).unwrap();
        p.enqueue(header(5, true), vec![5]).unwrap();
        p.process();
        p.enqueue(header(8, true), vec![8]).unwrap();
        p.process();
        assert_eq!(p.keyframe_requests(), 0, "keyframe arrival after a gap must not request");
        assert_eq!(p.take_frame().unwrap().timestamp_us, 8);
    }
    #[test]
    fn dependent_frame_without_picture_requests_keyframe() {
        // A P-frame that decodes to no picture (missing reference) is treated
        // as missing/corrupt and triggers keyframe recovery (PDF Task 8.1).
        let mut outputs = VecDeque::new();
        outputs.push_back(Ok(Some(CapturedFrame {
            timestamp_us: 1, width: 2, height: 2, pixel_format: PixelFormat::Rgba8,
            stride: 8, pixels: vec![1; 16], gpu_handle: None, dirty_region: None,
        })));
        outputs.push_back(Ok(None));
        let mut p = ViewerPipeline::new(FakeDecoder { outputs, resets: 0 }, [7; 16], 4).unwrap();
        p.enqueue(header(1, true), vec![1]).unwrap();
        p.process();
        assert!(p.take_frame().is_some(), "keyframe decodes first");
        p.enqueue(header(2, false), vec![2]).unwrap();
        p.process();
        assert_eq!(p.keyframe_requests(), 1);
        assert!(p.take_keyframe_request());
    }
    #[test]
    fn take_keyframe_request_is_one_shot() {
        let mut outputs = VecDeque::new();
        outputs.push_back(Err(ScreenShareError::new("corrupt")));
        let mut p = ViewerPipeline::new(FakeDecoder { outputs, resets: 0 }, [7; 16], 2).unwrap();
        p.enqueue(header(1, true), vec![1]).unwrap();
        p.process();
        assert!(p.take_keyframe_request());
        assert!(!p.take_keyframe_request(), "second take must return false");
        assert_eq!(p.keyframe_requests(), 1, "counter is monotonic across takes");
    }
    #[test]
    fn revoke_stops_decode_immediately() {
        let mut p = ViewerPipeline::new(FakeDecoder { outputs: VecDeque::new(), resets: 0 }, [7; 16], 2).unwrap();
        p.revoke();
        assert!(!p.is_active());
        assert!(p.enqueue(header(1, true), vec![1]).is_err());
        assert_eq!(p.process(), 0);
    }
    #[test]
    fn session_ids_are_isolated_between_pipelines() {
        // Decoder state is isolated per session: one pipeline never accepts
        // media addressed to another viewer (chat/video-call playback state
        // is untouched by screen-share decode state).
        let mut a = ViewerPipeline::new(FakeDecoder { outputs: VecDeque::new(), resets: 0 }, [7; 16], 2).unwrap();
        let mut b = ViewerPipeline::new(FakeDecoder { outputs: VecDeque::new(), resets: 0 }, [8; 16], 2).unwrap();
        let mut h = header(1, true);
        h.session_id = [8; 16];
        assert!(a.enqueue(h, vec![1]).is_err(), "pipeline A must reject pipeline B media");
        assert_eq!(a.process(), 0);
    }
}
