//! Receive pipeline for latency-sensitive live-call video.
//!
//! The receive path owns the complete boundary from wire bytes to a decoded
//! frame.  In particular, it does not expose a frame queue: the UI consumes a
//! single latest-frame slot so a slow renderer cannot turn network jitter into
//! unbounded memory growth or increasing latency.

use anyhow::Result;

use super::codec::{DecodedVideoFrame, OpenH264Decoder, VideoDecoder};
use super::packet::VideoPacket;
use super::reassembly::{ReassemblyResult, VideoReassembler};
use crate::call::media::{MediaDatagram, MediaKind};

/// The independent live-call video receive pipeline.
#[allow(missing_debug_implementations)]
pub struct LiveVideoPipeline {
    reassembler: VideoReassembler,
    decoder: Box<dyn VideoDecoder>,
    latest_frame: Option<DecodedVideoFrame>,
    received_packets: u64,
    decoded_frames: u64,
    dropped_frames: u64,
}

impl LiveVideoPipeline {
    /// Create a pipeline using the negotiated OpenH264 decoder.
    pub fn new() -> Result<Self> {
        Ok(Self::with_decoder(OpenH264Decoder::new()?))
    }

    /// Create a pipeline with a decoder implementation (also useful for tests
    /// and for replacing OpenH264 with a hardware decoder).
    pub fn with_decoder<D>(decoder: D) -> Self
    where
        D: VideoDecoder + 'static,
    {
        Self {
            reassembler: VideoReassembler::new(),
            decoder: Box::new(decoder),
            latest_frame: None,
            received_packets: 0,
            decoded_frames: 0,
            dropped_frames: 0,
        }
    }

    /// Parse and receive one wire datagram.
    ///
    /// `Ok(None)` means that the frame is still being reassembled or that the
    /// decoder buffered the access unit.  A returned frame is also installed
    /// in the latest-frame slot.
    pub fn receive_datagram(&mut self, bytes: &[u8]) -> Result<Option<DecodedVideoFrame>> {
        let datagram = MediaDatagram::parse(bytes)?;
        self.receive_parsed(&datagram)
    }

    /// Receive an already parsed video datagram from the shared media reader.
    pub fn receive_parsed(
        &mut self,
        datagram: &MediaDatagram,
    ) -> Result<Option<DecodedVideoFrame>> {
        if datagram.kind != MediaKind::Video {
            return Ok(None);
        }
        self.received_packets = self.received_packets.saturating_add(1);
        let complete = self.reassembler.push_datagram(datagram)?;
        let ReassemblyResult::Complete(encoded) = complete else {
            return Ok(None);
        };
        let Some(decoded) = self.decoder.decode(&encoded)? else {
            return Ok(None);
        };
        self.decoded_frames = self.decoded_frames.saturating_add(1);
        if self.latest_frame.is_some() {
            self.dropped_frames = self.dropped_frames.saturating_add(1);
        }
        self.latest_frame = Some(decoded.clone());
        Ok(Some(decoded))
    }

    /// Compatibility entry point for callers that still hold a legacy packet.
    pub fn receive(&mut self, packet: VideoPacket) -> Result<Option<DecodedVideoFrame>> {
        let datagram = MediaDatagram {
            kind: MediaKind::Video,
            flags: if packet.keyframe {
                crate::call::media::FLAG_KEYFRAME
            } else {
                0
            },
            call_id: packet.call_id,
            track_id: 1,
            sequence: packet.sequence,
            timestamp: packet.timestamp,
            fragment_index: 0,
            fragment_count: 1,
            payload: packet.payload,
        };
        self.receive_parsed(&datagram)
    }

    /// Borrow the newest decoded frame without removing it.
    pub fn latest_frame(&self) -> Option<&DecodedVideoFrame> {
        self.latest_frame.as_ref()
    }

    /// Take the newest decoded frame, clearing the slot.
    pub fn take_latest_frame(&mut self) -> Option<DecodedVideoFrame> {
        self.latest_frame.take()
    }

    /// Number of video datagrams accepted by this pipeline.
    pub const fn received_packets(&self) -> u64 {
        self.received_packets
    }

    /// Number of complete access units decoded.
    pub const fn decoded_frames(&self) -> u64 {
        self.decoded_frames
    }

    /// Number of previously decoded frames replaced in the latest-frame slot.
    pub const fn dropped_frames(&self) -> u64 {
        self.dropped_frames
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::call::video::{
        OpenH264Encoder, RawVideoFrame, VideoCodec, VideoEncoder, VideoPacketizer,
    };
    use crate::call::CallId;

    fn raw(value: u8, timestamp_us: u64) -> RawVideoFrame {
        RawVideoFrame {
            width: 16,
            height: 16,
            timestamp_us,
            rgb: vec![value; 16 * 16 * 3],
        }
    }

    fn send_reversed(pipeline: &mut LiveVideoPipeline, datagrams: Vec<Vec<u8>>) {
        for datagram in datagrams.into_iter().rev() {
            pipeline
                .receive_datagram(&datagram)
                .expect("datagram accepted");
        }
    }

    #[test]
    fn encoded_fragments_reorder_decode_and_replace_latest_frame() {
        let mut encoder = OpenH264Encoder::new().expect("encoder");
        let first = encoder.encode(&raw(20, 0)).expect("first frame");
        let second = encoder.encode(&raw(220, 33_000)).expect("second frame");
        assert_eq!(first.codec, VideoCodec::H264);
        assert_eq!(second.codec, VideoCodec::H264);

        let call_id = CallId::generate();
        let mut packetizer = VideoPacketizer::new();
        let first_wire = packetizer
            .fragment_frame(call_id, 1, &first, 80)
            .expect("first fragments")
            .into_iter()
            .map(|fragment| fragment.encode())
            .collect();
        let second_wire = packetizer
            .fragment_frame(call_id, 1, &second, 80)
            .expect("second fragments")
            .into_iter()
            .map(|fragment| fragment.encode())
            .collect();

        let mut pipeline = LiveVideoPipeline::new().expect("decoder");
        send_reversed(&mut pipeline, first_wire);
        send_reversed(&mut pipeline, second_wire);

        assert_eq!(pipeline.received_packets(), 2);
        assert_eq!(pipeline.decoded_frames(), 2);
        assert_eq!(pipeline.dropped_frames(), 1);
        assert_eq!(
            pipeline
                .latest_frame()
                .map(|frame| (frame.width, frame.height)),
            Some((16, 16))
        );
        assert!(pipeline.take_latest_frame().is_some());
        assert!(pipeline.latest_frame().is_none());
    }

    #[test]
    fn non_video_datagrams_are_not_counted_or_decoded() {
        let mut pipeline = LiveVideoPipeline::new().expect("decoder");
        let audio = MediaDatagram {
            kind: MediaKind::Audio,
            flags: 0,
            call_id: CallId::generate(),
            track_id: 1,
            sequence: 0,
            timestamp: 0,
            fragment_index: 0,
            fragment_count: 1,
            payload: vec![1],
        };
        assert!(pipeline
            .receive_datagram(&audio.encode())
            .expect("audio parse")
            .is_none());
        assert_eq!(pipeline.received_packets(), 0);
    }
}
