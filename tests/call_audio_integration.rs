//! Synthetic audio integration test (BORU-CALL-9.6).
//!
//! No real microphone or network is required: a known sine tone is encoded
//! with Opus, wrapped in media datagrams, pushed through the bounded jitter
//! buffer, and decoded back to PCM. The test verifies sample counts and
//! total duration are sane (one 960-sample frame per 20 ms tick, total
//! duration matching the input within tolerance).

use std::time::{Duration, Instant};

use boru_core::call::audio::codec::OpusEncoder;
use boru_core::call::audio::jitter::{
    AudioJitterBuffer, BufferedAudioPacket, DEFAULT_JITTER_DELAY,
};
use boru_core::call::audio::plc::OpusPlayoutDecoder;
use boru_core::call::frame::{SAMPLES_PER_FRAME, SAMPLE_RATE};
use boru_core::call::media::{MediaDatagram, MediaKind};
use boru_core::call::CallId;

/// A 440 Hz sine tone in normalized mono f32 PCM, one frame at a time.
fn sine_frame(frame_index: u32) -> Vec<f32> {
    let phase_step = 2.0 * std::f32::consts::PI * 440.0 / SAMPLE_RATE as f32;
    (0..SAMPLES_PER_FRAME)
        .map(|i| {
            let phase = phase_step * (frame_index as usize * SAMPLES_PER_FRAME + i) as f32;
            0.5 * phase.sin()
        })
        .collect()
}

/// Run the full synthetic pipeline: PCM -> Opus -> datagram -> jitter ->
/// Opus decode. Returns the decoded sample count and the per-frame totals.
fn run_synthetic_loop(frame_count: usize) -> (usize, usize) {
    let call_id = CallId::new();
    let mut encoder = OpusEncoder::new().expect("opus encoder");
    let mut jitter = AudioJitterBuffer::default();
    let mut decoder = OpusPlayoutDecoder::new().expect("opus decoder");
    let t0 = Instant::now();

    // Encode every frame and push it through the jitter buffer.
    let mut pushed = 0usize;
    for frame_index in 0..frame_count {
        let pcm = sine_frame(frame_index as u32);
        let payload = encoder
            .encode(&pcm)
            .expect("encode sine frame")
            .expect("non-empty packet");
        let datagram = MediaDatagram {
            kind: MediaKind::Audio,
            flags: 0,
            call_id,
            track_id: 1,
            sequence: frame_index as u32,
            timestamp: (frame_index as u32) * SAMPLES_PER_FRAME as u32,
            fragment_index: 0,
            fragment_count: 1,
            payload,
        }
        .encode();
        let packet = MediaDatagram::parse(&datagram).expect("round-trip parse");
        let arrival = t0 + Duration::from_millis(20 * frame_index as u64);
        assert!(
            jitter.push(BufferedAudioPacket {
                call_id: packet.call_id,
                sequence: packet.sequence,
                timestamp: packet.timestamp,
                arrival,
                payload: packet.payload,
            }),
            "frame {frame_index} should be accepted"
        );
        pushed += 1;
    }

    // Decode everything due after the initial jitter delay, frame by frame.
    let mut decoded_frames = 0usize;
    let mut decoded_samples = 0usize;
    let mut now = t0 + DEFAULT_JITTER_DELAY;
    for _ in 0..frame_count {
        if let Some(frame) = decoder
            .decode_due(&mut jitter, now)
            .expect("decode due frame")
        {
            assert_eq!(
                frame.samples.len(),
                SAMPLES_PER_FRAME,
                "every decoded frame is exactly one 20 ms frame"
            );
            decoded_samples += frame.samples.len();
            decoded_frames += 1;
        }
        now += Duration::from_millis(20);
    }

    (pushed, decoded_samples)
}

#[test]
fn synthetic_sine_round_trip_preserves_frame_and_duration_counts() {
    // 25 frames = 500 ms of audio at 20 ms/frame.
    let frame_count = 25;
    let (pushed, decoded_samples) = run_synthetic_loop(frame_count);

    assert_eq!(pushed, frame_count, "all frames accepted by jitter buffer");
    assert_eq!(
        decoded_samples,
        frame_count * SAMPLES_PER_FRAME,
        "decoded sample count matches the input (960 per 20 ms frame)"
    );

    let expected_duration_ms = frame_count * 20;
    let actual_duration_ms = decoded_samples as f64 / SAMPLE_RATE as f64 * 1_000.0;
    assert!(
        (actual_duration_ms - expected_duration_ms as f64).abs() < 1.0,
        "total duration {actual_duration_ms:.1} ms must match {expected_duration_ms} ms within 1 ms"
    );
}

#[test]
fn synthetic_sine_decodes_to_non_silent_pcm() {
    let (_, decoded_samples) = run_synthetic_loop(10);
    // We cannot observe the samples directly through the loop, so run a
    // minimal single-frame decode and assert the PCM is non-silent.
    let call_id = CallId::new();
    let mut encoder = OpusEncoder::new().expect("opus encoder");
    let mut jitter = AudioJitterBuffer::default();
    let mut decoder = OpusPlayoutDecoder::new().expect("opus decoder");
    let t0 = Instant::now();

    let pcm = sine_frame(0);
    let payload = encoder.encode(&pcm).expect("encode").expect("packet");
    let datagram = MediaDatagram {
        kind: MediaKind::Audio,
        flags: 0,
        call_id,
        track_id: 1,
        sequence: 0,
        timestamp: 0,
        fragment_index: 0,
        fragment_count: 1,
        payload,
    }
    .encode();
    let packet = MediaDatagram::parse(&datagram).expect("parse");
    assert!(jitter.push(BufferedAudioPacket {
        call_id: packet.call_id,
        sequence: packet.sequence,
        timestamp: packet.timestamp,
        arrival: t0,
        payload: packet.payload,
    }));

    let frame = decoder
        .decode_due(&mut jitter, t0 + DEFAULT_JITTER_DELAY)
        .expect("decode")
        .expect("frame due");
    assert_eq!(frame.samples.len(), SAMPLES_PER_FRAME);
    assert!(
        frame.samples.iter().any(|sample| sample.abs() > 0.001),
        "sine tone must decode to non-silent PCM"
    );
    assert!(!frame.concealed, "no packet loss in the synthetic loop");
    assert_eq!(decoded_samples, 10 * SAMPLES_PER_FRAME);
}
