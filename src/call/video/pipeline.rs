//! Receive pipeline for latency-sensitive live-call video.
//!
//! The receive path owns the complete boundary from wire bytes to a decoded
//! frame.  In particular, it does not expose a frame queue: the UI consumes a
//! single latest-frame slot so a slow renderer cannot turn network jitter into
//! unbounded memory growth or increasing latency.

use anyhow::Result;

use super::capture::{CaptureConfig, CaptureSource, CapturedFrame};
use super::codec::{DecodedVideoFrame, OpenH264Decoder, RawVideoFrame, VideoDecoder, VideoEncoder};
use super::packet::{VideoPacket, VideoPacketizer};
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

/// The local camera pipeline. A captured frame is copied into a mirrored
/// preview slot, while the original (unmirrored) pixels are passed to the
/// encoder and packetizer. No network I/O is performed by this type.
#[allow(missing_debug_implementations)]
pub struct LocalVideoPipeline {
    config: CaptureConfig,
    encoder: Box<dyn VideoEncoder>,
    packetizer: VideoPacketizer,
    call_id: crate::call::CallId,
    track_id: u32,
    max_datagram_size: usize,
    latest_local_frame: Option<DecodedVideoFrame>,
    preview_frames: u64,
}

impl LocalVideoPipeline {
    /// Construct a local pipeline with an injected encoder.
    pub fn with_encoder<E>(
        config: CaptureConfig,
        call_id: crate::call::CallId,
        track_id: u32,
        max_datagram_size: usize,
        encoder: E,
    ) -> Self
    where
        E: VideoEncoder + 'static,
    {
        Self {
            config,
            encoder: Box::new(encoder),
            packetizer: VideoPacketizer::new(),
            call_id,
            track_id,
            max_datagram_size,
            latest_local_frame: None,
            preview_frames: 0,
        }
    }

    /// Process one captured frame and return encoded datagrams ready for the
    /// caller's network sender. This method itself never sends them.
    pub fn process_frame(
        &mut self,
        captured: CapturedFrame,
    ) -> anyhow::Result<Vec<crate::call::media::MediaDatagram>> {
        let raw = RawVideoFrame {
            width: self.config.width,
            height: self.config.height,
            timestamp_us: captured.timestamp_us,
            rgb: captured.data,
        };
        let preview = mirror_rgb(&raw.rgb, raw.width, raw.height)?;
        self.latest_local_frame = Some(DecodedVideoFrame {
            width: raw.width,
            height: raw.height,
            bytes: preview,
        });
        self.preview_frames = self.preview_frames.saturating_add(1);

        let encoded = self.encoder.encode(&raw)?;
        self.packetizer
            .fragment_frame(
                self.call_id,
                self.track_id,
                &encoded,
                self.max_datagram_size,
            )
            .map_err(Into::into)
    }

    /// Pull and process one frame from a capture source. The returned
    /// datagrams are still owned by the caller; submitting them is a separate
    /// network concern.
    pub fn process_next<S: CaptureSource>(
        &mut self,
        source: &mut S,
    ) -> anyhow::Result<Option<Vec<crate::call::media::MediaDatagram>>> {
        source
            .next_frame()
            .map(|frame| self.process_frame(frame))
            .transpose()
    }

    /// Borrow the newest mirrored local preview frame.
    pub fn latest_local_frame(&self) -> Option<&DecodedVideoFrame> {
        self.latest_local_frame.as_ref()
    }

    /// Number of captured frames copied into the preview slot.
    pub const fn preview_frames(&self) -> u64 {
        self.preview_frames
    }
}

fn mirror_rgb(rgb: &[u8], width: u32, height: u32) -> anyhow::Result<Vec<u8>> {
    if width == 0 || height == 0 {
        return Err(anyhow::anyhow!("RGB dimensions must be non-zero"));
    }
    let row_bytes = (width as usize)
        .checked_mul(3)
        .ok_or_else(|| anyhow::anyhow!("RGB row size overflow"))?;
    let expected = row_bytes
        .checked_mul(height as usize)
        .ok_or_else(|| anyhow::anyhow!("RGB frame size overflow"))?;
    if rgb.len() != expected {
        return Err(anyhow::anyhow!(
            "RGB frame has {} bytes, expected {expected}",
            rgb.len()
        ));
    }
    let mut mirrored = vec![0; rgb.len()];
    for (source_row, destination_row) in rgb
        .chunks_exact(row_bytes)
        .zip(mirrored.chunks_exact_mut(row_bytes))
    {
        for pixel in 0..width as usize {
            let source = (width as usize - 1 - pixel) * 3;
            let destination = pixel * 3;
            destination_row[destination..destination + 3]
                .copy_from_slice(&source_row[source..source + 3]);
        }
    }
    Ok(mirrored)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    #[derive(Clone, Default)]
    struct RecordingEncoder {
        seen: Arc<Mutex<Vec<u8>>>,
    }

    impl VideoEncoder for RecordingEncoder {
        fn encode(
            &mut self,
            frame: &RawVideoFrame,
        ) -> anyhow::Result<super::super::codec::EncodedVideoFrame> {
            *self.seen.lock().expect("recording encoder lock") = frame.rgb.clone();
            Ok(super::super::codec::EncodedVideoFrame {
                codec: super::super::codec::VideoCodec::H264,
                width: frame.width,
                height: frame.height,
                timestamp_us: frame.timestamp_us,
                keyframe: true,
                bytes: vec![1, 2, 3],
            })
        }

        fn request_keyframe(&mut self) {}
    }

    #[test]
    fn local_preview_is_mirrored_without_mutating_encoder_input() {
        let seen = Arc::new(Mutex::new(Vec::new()));
        let encoder = RecordingEncoder { seen: seen.clone() };
        let config = CaptureConfig {
            width: 2,
            height: 1,
            frame_interval: std::time::Duration::from_millis(33),
        };
        let original = vec![10, 11, 12, 20, 21, 22];
        let mut pipeline = LocalVideoPipeline::with_encoder(
            config,
            crate::call::CallId::generate(),
            1,
            100,
            encoder,
        );

        let datagrams = pipeline
            .process_frame(CapturedFrame {
                timestamp_us: 7,
                data: original.clone(),
            })
            .expect("local frame processed");

        assert!(!datagrams.is_empty());
        assert_eq!(
            pipeline.latest_local_frame().unwrap().bytes,
            vec![20, 21, 22, 10, 11, 12]
        );
        assert_eq!(*seen.lock().expect("recording encoder lock"), original);
        assert_eq!(pipeline.preview_frames(), 1);
    }

    #[test]
    fn local_preview_does_not_require_network_submission() {
        let config = CaptureConfig {
            width: 2,
            height: 1,
            frame_interval: std::time::Duration::from_millis(33),
        };
        let mut pipeline = LocalVideoPipeline::with_encoder(
            config,
            crate::call::CallId::generate(),
            1,
            100,
            RecordingEncoder::default(),
        );
        pipeline
            .process_frame(CapturedFrame {
                timestamp_us: 1,
                data: vec![0; 6],
            })
            .expect("local frame processed");
        assert!(pipeline.latest_local_frame().is_some());
    }

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
