//! Synthetic video integration test (BORU-CALL-9.7).
//!
//! No real camera is required: a synthetic moving RGB test pattern is encoded
//! with OpenH264, fragmented into media datagrams, randomly reordered,
//! reassembled, and decoded back to RGB.  Verifies dimensions survive the full
//! round trip, a decoded frame exists, and reassembly state does not grow
//! across frames (bounded allocations).

use boru_core::call::media::MediaDatagram;
use boru_core::call::video::codec::{
    OpenH264Decoder, OpenH264Encoder, RawVideoFrame, VideoDecoder, VideoEncoder,
};
use boru_core::call::video::packet::VideoPacketizer;
use boru_core::call::video::reassembly::{ReassemblyResult, VideoReassembler};
use boru_core::call::CallId;

const WIDTH: u32 = 64;
const HEIGHT: u32 = 64;
const FRAMES: u32 = 4;

/// A deterministic moving test pattern (pseudo-random noise that shifts per
/// frame — high entropy so the encoded frame spans multiple datagrams).
fn synthetic_frame(frame_index: u32) -> RawVideoFrame {
    let mut rgb = Vec::with_capacity((WIDTH * HEIGHT * 3) as usize);
    let mut state = (frame_index as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1;
    let mut next = move || {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        state as u8
    };
    for _ in 0..(WIDTH * HEIGHT) {
        rgb.push(next());
        rgb.push(next());
        rgb.push(next());
    }
    RawVideoFrame {
        width: WIDTH,
        height: HEIGHT,
        timestamp_us: frame_index as u64 * 33_000,
        rgb,
    }
}

/// Deterministic pseudo-random permutation (xorshift) for reproducible order.
fn shuffled_order(len: usize, seed: u64) -> Vec<usize> {
    let mut state = seed | 1;
    let mut indices: Vec<usize> = (0..len).collect();
    for i in (1..len).rev() {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        let j = (state as usize) % (i + 1);
        indices.swap(i, j);
    }
    indices
}

#[test]
fn synthetic_video_encode_fragment_reorder_reassemble_decode() {
    let call_id = CallId::generate();
    let mut encoder = OpenH264Encoder::new().expect("openh264 encoder");
    let mut packetizer = VideoPacketizer::new();
    let mut reassembler = VideoReassembler::new();
    let mut decoder = OpenH264Decoder::new().expect("openh264 decoder");

    let mut decoded_frames = 0u32;
    for frame_index in 0..FRAMES {
        let raw = synthetic_frame(frame_index);

        // 1. Encode (H.264).
        let encoded = encoder.encode(&raw).expect("encode synthetic frame");
        assert_eq!((encoded.width, encoded.height), (WIDTH, HEIGHT));
        assert!(!encoded.bytes.is_empty());

        // 2. Fragment into datagrams sized for a small datagram transport.
        let mut datagrams = packetizer
            .fragment_frame(call_id, 1, &encoded, 256)
            .expect("fragment encoded frame");
        assert!(datagrams.len() >= 2, "frame must span multiple datagrams");

        // 3. Random reorder (deterministic seed so failures reproduce).
        let order = shuffled_order(datagrams.len(), frame_index as u64 + 1);
        let reordered: Vec<MediaDatagram> =
            order.iter().map(|&i| datagrams[i].clone()).collect();

        // 4. Reassemble from the shuffled datagrams.
        let mut result = ReassemblyResult::Pending;
        for datagram in &reordered {
            result = reassembler
                .push_datagram(datagram)
                .expect("reassembly admits fragment");
        }
        let reassembled = match result {
            ReassemblyResult::Complete(bytes) => bytes,
            ReassemblyResult::Pending => panic!("frame {frame_index} did not reassemble"),
        };
        assert_eq!(reassembled, encoded.bytes);

        // Reassembly must not retain the completed frame: incomplete state
        // returns to zero after every complete frame (bounded allocations).
        assert_eq!(
            reassembler.incomplete_frames(),
            0,
            "no incomplete frames may accumulate across iterations"
        );

        // 5. Decode (H.264) and verify dimensions.
        if let Some(decoded) = decoder.decode(&reassembled).expect("decode reassembled frame") {
            decoded_frames += 1;
            assert_eq!((decoded.width, decoded.height), (WIDTH, HEIGHT));
            assert_eq!(decoded.bytes.len(), (WIDTH * HEIGHT * 3) as usize);
        }
    }

    // The first frame is a keyframe and must decode immediately; later
    // frames may be buffered by the decoder, so at least the keyframe must
    // produce a visible picture.
    assert!(
        decoded_frames >= 1,
        "expected at least the keyframe to decode, got {decoded_frames}"
    );
}

#[test]
fn synthetic_video_frame_is_parseable_round_trip() {
    // A packetizer-produced datagram must survive encode -> parse unchanged,
    // proving the integration path uses the same wire representation the
    // network media reader would deliver.
    let call_id = CallId::generate();
    let mut encoder = OpenH264Encoder::new().expect("openh264 encoder");
    let mut packetizer = VideoPacketizer::new();
    let raw = synthetic_frame(0);
    let encoded = encoder.encode(&raw).expect("encode");
    let datagrams = packetizer
        .fragment_frame(call_id, 1, &encoded, 256)
        .expect("fragment");
    for datagram in &datagrams {
        let wire = datagram.encode();
        let parsed = MediaDatagram::parse(&wire).expect("wire datagram parses");
        assert_eq!(parsed, *datagram, "datagram must survive the wire round trip");
    }
}
