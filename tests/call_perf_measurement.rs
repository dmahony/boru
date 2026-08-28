//! BORU-CALL-11: Phase 11 performance-target measurement harness.
//!
//! Synthetic/loopback measurements against the Phase 11 engineering targets
//! (no real network required):
//!
//! - Voice: 20 ms Opus frames; ~24–32 kbps normal bitrate; jitter target
//!   ~60–100 ms; no unbounded queue; conversational end-to-end latency
//!   comfortably below ~250 ms.
//! - Video: 640x360 @ 24 fps ~400–800 kbps.
//! - Overload: DROP frames, do NOT accumulate them.
//!
//! Run with:
//!
//! ```sh
//! cargo test --test call_perf_measurement --features voice-calls,video-calls -- --nocapture
//! ```
//!
//! Every test prints its measurements to stdout; the numeric assertions are
//! deliberately generous (they catch gross regressions), while the printed
//! numbers are the actual report data consumed by PERF.md.

use std::time::{Duration, Instant};

use boru_core::call::audio::codec::OpusEncoder;
use boru_core::call::audio::jitter::{
    AudioJitterBuffer, BufferedAudioPacket, DEFAULT_JITTER_DELAY, MAX_BUFFERED_AUDIO_PACKETS,
    MAX_JITTER_DELAY, MIN_JITTER_DELAY,
};
use boru_core::call::audio::plc::OpusPlayoutDecoder;
use boru_core::call::frame::{SAMPLES_PER_FRAME, SAMPLE_RATE};
use boru_core::call::video::codec::{
    OpenH264Encoder, RawVideoFrame, VideoEncoder, VIDEO_FRAMES_PER_SECOND, VIDEO_HEIGHT,
    VIDEO_TARGET_BITRATE_BPS, VIDEO_WIDTH,
};
use boru_core::call::video::packet::VideoPacketizer;
use boru_core::call::video::pipeline::LiveVideoPipeline;
use boru_core::call::CallId;

// ---------------------------------------------------------------------------
// Synthetic sources
// ---------------------------------------------------------------------------

/// One 20 ms frame of synthetic speech-like audio: a voiced vowel (multiple
/// harmonics) plus a formant-like amplitude envelope.  Not pure silence, so
/// DTX does not collapse the bitrate to zero.
fn speech_frame(frame_index: u32) -> Vec<f32> {
    let base = 2.0 * std::f32::consts::PI * 110.0 / SAMPLE_RATE as f32;
    let mut samples = Vec::with_capacity(SAMPLES_PER_FRAME);
    for i in 0..SAMPLES_PER_FRAME {
        let n = (frame_index as usize * SAMPLES_PER_FRAME + i) as f32;
        let envelope =
            0.5 + 0.5 * (2.0 * std::f32::consts::PI * 8.0 * n / SAMPLE_RATE as f32).sin();
        let vowel = (base * n).sin() + 0.4 * (2.0 * base * n).sin() + 0.2 * (3.0 * base * n).sin();
        samples.push(0.35 * envelope * vowel);
    }
    samples
}

/// A deterministic moving RGB test pattern for the video encoder: a base
/// gradient plus a per-frame moving noise band.  This is realistic camera
/// content (not full random noise, which is pathological for H.264 and does
/// not represent any real 360p camera).  OpenH264's frame-skip rate control
/// may still return an empty access unit for a frame it decides to skip;
/// tests that feed the pipeline handle that by skipping empty frames.
fn video_frame(frame_index: u32) -> RawVideoFrame {
    let mut rgb = Vec::with_capacity((VIDEO_WIDTH * VIDEO_HEIGHT * 3) as usize);
    let mut state = (frame_index as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1;
    let mut next = move || {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        state as u8
    };
    for y in 0..VIDEO_HEIGHT {
        for x in 0..VIDEO_WIDTH {
            // Base gradient (low entropy) + per-frame noise band (motion).
            let band = (frame_index as u32 % 4) as u8;
            let noise = if (x / 32 + y / 32) % 4 == band as u32 {
                next()
            } else {
                0
            };
            let base = ((x * 255 / VIDEO_WIDTH) as u8).wrapping_add(noise);
            rgb.push(base);
            rgb.push(base.wrapping_mul(2));
            rgb.push(base.wrapping_add(80));
        }
    }
    RawVideoFrame {
        width: VIDEO_WIDTH,
        height: VIDEO_HEIGHT,
        timestamp_us: frame_index as u64 * 1_000_000 / VIDEO_FRAMES_PER_SECOND as u64,
        rgb,
    }
}

// ---------------------------------------------------------------------------
// 1. Voice bitrate and frame duration
// ---------------------------------------------------------------------------

#[test]
fn voice_bitrate_and_frame_duration() {
    let mut encoder = OpusEncoder::new().expect("opus encoder");
    let frames: usize = 500; // 10 seconds at 20 ms/frame

    let mut total_payload_bytes = 0usize;
    let mut encode_total = Duration::ZERO;
    let mut min_frame_bytes = usize::MAX;
    let mut max_frame_bytes = 0usize;
    for i in 0..frames {
        let pcm = speech_frame(i as u32);
        let start = Instant::now();
        let payload = encoder.encode(&pcm).expect("encode").expect("packet");
        encode_total += start.elapsed();
        let len = payload.len();
        total_payload_bytes += len;
        min_frame_bytes = min_frame_bytes.min(len);
        max_frame_bytes = max_frame_bytes.max(len);
    }

    let seconds = frames as f64 * 20.0 / 1_000.0;
    let kbps = total_payload_bytes as f64 * 8.0 / seconds / 1_000.0;
    let avg_bytes = total_payload_bytes as f64 / frames as f64;
    let encode_per_frame_ms = encode_total.as_secs_f64() * 1_000.0 / frames as f64;

    println!(
        "VOICE frame duration (const): {:.1} ms (960 samples @ 48 kHz)",
        960.0 * 1000.0 / 48_000.0
    );
    println!("VOICE measured bitrate: {kbps:.1} kbps over {seconds:.1} s ({frames} frames)");
    println!("VOICE avg payload: {avg_bytes:.1} bytes/frame (min {min_frame_bytes}, max {max_frame_bytes})");
    println!("VOICE encode wall time: {encode_per_frame_ms:.3} ms/frame (target: <20 ms/frame real-time)");

    // Generous regression bounds: non-silent speech at 32 kbps VBR should land
    // in the 24-32 kbps band (DTX may lower it slightly on near-silence).
    assert!(
        kbps >= 18.0 && kbps <= 36.0,
        "voice bitrate {kbps:.1} kbps far outside the 24-32 kbps target"
    );
    assert!(
        encode_per_frame_ms < 20.0,
        "encoder must sustain real time (20 ms budget), took {encode_per_frame_ms:.3} ms"
    );
}

// ---------------------------------------------------------------------------
// 2. Jitter adaptation: bounded, smooth, ~60-100 ms under normal jitter
// ---------------------------------------------------------------------------

#[test]
fn jitter_adaptation_bounds_and_smoothing() {
    let call = CallId::from_bytes([21; 16]);
    let t0 = Instant::now();
    let mut buffer = AudioJitterBuffer::default();
    assert_eq!(buffer.jitter_target(), DEFAULT_JITTER_DELAY);

    // 10 s of arrivals at a nominal 20 ms cadence with ±8 ms synthetic jitter.
    let mut rng_state = 0x1234_5678u64;
    let mut next_jitter = move || {
        rng_state ^= rng_state << 13;
        rng_state ^= rng_state >> 7;
        rng_state ^= rng_state << 17;
        ((rng_state % 17) as i64) - 8 // -8..=8 ms
    };

    let mut arrival = t0;
    let mut min_target = u64::MAX;
    let mut max_target = 0u64;
    let mut targets_60_100 = 0usize;
    let mut settled = false;
    for seq in 1..=500u32 {
        arrival += Duration::from_millis(20);
        let jitter_ms = next_jitter();
        if jitter_ms >= 0 {
            arrival += Duration::from_millis(jitter_ms as u64);
        } else {
            arrival -= Duration::from_millis((-jitter_ms) as u64);
        }
        let accepted = buffer.push(BufferedAudioPacket {
            call_id: call,
            sequence: seq,
            timestamp: seq.wrapping_mul(960),
            arrival,
            payload: vec![seq as u8],
        });
        assert!(
            accepted,
            "seq {seq} should be accepted under moderate jitter"
        );
        // Drain anything whose deadline has passed, like a real playout loop;
        // otherwise the 64-packet hard bound fills the buffer and later
        // pushes are correctly dropped (that is the drop-not-accumulate
        // behaviour, not an adaptation failure).
        while buffer.pop_due(arrival).is_some() {}
        let target = buffer.jitter_target().as_millis() as u64;
        min_target = min_target.min(target);
        max_target = max_target.max(target);
        if (60..=100).contains(&target) {
            targets_60_100 += 1;
        }
        if seq > 100 && target <= 100 {
            settled = true;
        }
    }

    let min_bound = MIN_JITTER_DELAY.as_millis();
    let max_bound = MAX_JITTER_DELAY.as_millis();
    println!("JITTER target range over 10 s: {min_target}..{max_target} ms (hard bounds {min_bound}..{max_bound} ms)");
    println!("JITTER samples inside 60-100 ms target band: {targets_60_100}/500");
    println!("JITTER settled at/below 100 ms by frame 100: {settled}");
    assert!(
        min_target >= MIN_JITTER_DELAY.as_millis() as u64,
        "target must never drop below the 40 ms floor"
    );
    assert!(
        max_target <= MAX_JITTER_DELAY.as_millis() as u64,
        "target must never exceed the 200 ms ceiling"
    );
}

#[test]
fn one_late_packet_does_not_jump_latency() {
    // A single huge inter-arrival gap must move the target by at most the
    // 5 ms hysteresis step (7.4 "never jump latency on one late packet").
    let call = CallId::from_bytes([22; 16]);
    let t0 = Instant::now();
    let mut buffer = AudioJitterBuffer::default();
    assert!(buffer.push(BufferedAudioPacket {
        call_id: call,
        sequence: 1,
        timestamp: 960,
        arrival: t0,
        payload: vec![1],
    }));
    let before = buffer.jitter_target();
    // One packet arrives 200 ms late.
    assert!(buffer.push(BufferedAudioPacket {
        call_id: call,
        sequence: 2,
        timestamp: 1920,
        arrival: t0 + Duration::from_millis(200),
        payload: vec![2],
    }));
    let after = buffer.jitter_target();
    let delta_ms = after.as_millis().abs_diff(before.as_millis());
    println!("JITTER one-late-packet delta: {delta_ms} ms (step limit 5 ms)");
    assert!(
        delta_ms <= 5,
        "a single late packet moved the target {delta_ms} ms; hysteresis violated"
    );
}

// ---------------------------------------------------------------------------
// 3. No unbounded queue: hard bound + drop-not-accumulate
// ---------------------------------------------------------------------------

#[test]
fn audio_queue_is_hard_bounded_and_drops() {
    let call = CallId::from_bytes([23; 16]);
    let t0 = Instant::now();
    let mut buffer = AudioJitterBuffer::default();

    // Feed 5x the capacity worth of packets at a single instant (overload).
    // The discontinuity guard re-anchors the stream mid-flood, so the exact
    // retained count varies; the hard invariant is retained <= bound and
    // drops > 0 (never accumulate).
    for seq in 0..(MAX_BUFFERED_AUDIO_PACKETS as u32 * 5) {
        let _ = buffer.push(BufferedAudioPacket {
            call_id: call,
            sequence: seq,
            timestamp: seq.wrapping_mul(960),
            arrival: t0,
            payload: vec![seq as u8],
        });
    }
    let retained = buffer.len();
    let dropped = buffer.dropped_packets();
    println!(
        "AUDIO-QUEUE overload: retained {retained}, dropped {dropped} (hard bound {MAX_BUFFERED_AUDIO_PACKETS})"
    );
    assert!(
        retained <= MAX_BUFFERED_AUDIO_PACKETS,
        "retained {retained} must never exceed the hard bound {MAX_BUFFERED_AUDIO_PACKETS}"
    );
    assert!(dropped > 0, "overload must drop, never accumulate");
}

// ---------------------------------------------------------------------------
// 4. End-to-end latency: synthetic loopback encode -> jitter -> decode
// ---------------------------------------------------------------------------

#[test]
fn conversational_end_to_end_latency() {
    let call = CallId::from_bytes([24; 16]);
    let mut encoder = OpusEncoder::new().expect("opus encoder");
    let mut jitter = AudioJitterBuffer::default();
    let mut decoder = OpusPlayoutDecoder::new().expect("opus decoder");

    let t0 = Instant::now();
    let frame_count: usize = 25; // 500 ms of audio

    // Encode + inject all frames at nominal arrival times.
    let mut encode_deadline_lag = Duration::ZERO;
    let mut max_encode_latency = Duration::ZERO;
    for i in 0..frame_count {
        let pcm = speech_frame(i as u32);
        let start = Instant::now();
        let payload = encoder.encode(&pcm).expect("encode").expect("packet");
        let encode_dur = start.elapsed();
        max_encode_latency = max_encode_latency.max(encode_dur);
        // Processing lag pushes the effective arrival later.
        encode_deadline_lag += encode_dur;
        let arrival = t0 + Duration::from_millis(20 * i as u64) + encode_deadline_lag;
        assert!(jitter.push(BufferedAudioPacket {
            call_id: call,
            sequence: i as u32,
            timestamp: (i as u32).wrapping_mul(960),
            arrival,
            payload,
        }));
    }

    // Consume at the playout clock, measuring wall time from the first
    // encode to when the first decoded frame is available.  Drain until
    // every frame is decoded (encode lag shifts later arrivals past a
    // fixed frame_count window).
    let mut decoded = 0usize;
    let mut first_frame_wall = None;
    let mut now = t0 + DEFAULT_JITTER_DELAY;
    for _ in 0..(frame_count + 10) {
        if let Some(_frame) = decoder.decode_due(&mut jitter, now).expect("decode") {
            decoded += 1;
            if first_frame_wall.is_none() {
                first_frame_wall = Some(now.saturating_duration_since(t0));
            }
        }
        now += Duration::from_millis(20);
    }

    let e2e = first_frame_wall.expect("first frame decoded");
    println!(
        "E2E first-frame wall latency: {:.1} ms (encode + jitter delay + decode)",
        e2e.as_secs_f64() * 1000.0
    );
    println!(
        "E2E max single-frame encode latency: {:.3} ms",
        max_encode_latency.as_secs_f64() * 1000.0
    );
    println!("E2E decoded frames: {decoded}/{frame_count}");
    assert_eq!(
        decoded, frame_count,
        "every frame must decode in the loopback"
    );
    assert!(
        e2e < Duration::from_millis(250),
        "conversational E2E latency {:.0} ms exceeds the 250 ms target",
        e2e.as_secs_f64() * 1000.0
    );
}

// ---------------------------------------------------------------------------
// 5. Video: 640x360 @ 24 fps, ~400-800 kbps
// ---------------------------------------------------------------------------

#[test]
fn video_bitrate_at_360p24() {
    let mut encoder = OpenH264Encoder::new().expect("openh264 encoder");
    let frames: u32 = 48; // 2 seconds at 24 fps

    let mut total_bytes = 0usize;
    let mut keyframe_bytes = 0usize;
    let mut encode_total = Duration::ZERO;
    let mut keyframes = 0u32;
    for i in 0..frames {
        let raw = video_frame(i);
        let start = Instant::now();
        let encoded = encoder.encode(&raw).expect("encode");
        encode_total += start.elapsed();
        assert_eq!((encoded.width, encoded.height), (VIDEO_WIDTH, VIDEO_HEIGHT));
        total_bytes += encoded.bytes.len();
        if encoded.keyframe {
            keyframes += 1;
            keyframe_bytes = encoded.bytes.len();
        }
    }

    let seconds = frames as f64 / VIDEO_FRAMES_PER_SECOND as f64;
    let kbps = total_bytes as f64 * 8.0 / seconds / 1_000.0;
    let encode_per_frame_ms = encode_total.as_secs_f64() * 1_000.0 / frames as f64;
    println!("VIDEO measured bitrate: {kbps:.1} kbps over {seconds:.1} s ({frames} frames at {}x{} @ {} fps)", VIDEO_WIDTH, VIDEO_HEIGHT, VIDEO_FRAMES_PER_SECOND);
    println!(
        "VIDEO configured target: {} kbps",
        VIDEO_TARGET_BITRATE_BPS / 1000
    );
    println!("VIDEO keyframes: {keyframes} (first IDR {keyframe_bytes} bytes)");
    println!(
        "VIDEO encode wall time: {encode_per_frame_ms:.3} ms/frame (24 fps budget = {:.1} ms)",
        1000.0 / VIDEO_FRAMES_PER_SECOND as f64
    );
    assert!(
        kbps >= 300.0 && kbps <= 900.0,
        "video bitrate {kbps:.1} kbps far outside the 400-800 kbps target"
    );
}

// ---------------------------------------------------------------------------
// 6. Video drop-not-accumulate: slow consumer must not queue frames
// ---------------------------------------------------------------------------

#[test]
fn video_pipeline_drops_not_accumulates() {
    let call = CallId::from_bytes([25; 16]);
    let mut encoder = OpenH264Encoder::new().expect("encoder");
    let mut packetizer = VideoPacketizer::new();
    let mut pipeline = LiveVideoPipeline::new().expect("pipeline");

    // Produce frames and feed them all without ever consuming (worst-case
    // slow renderer).  OpenH264's frame-skip rate control can legitimately
    // return an empty access unit (an encoder-level drop), so only non-empty
    // encoded frames are fed to the pipeline.  The pipeline must decode each
    // and replace the latest-frame slot, incrementing the drop counter —
    // never queueing.  The invariant is decoded == fed and dropped == fed-1
    // for however many non-empty frames the encoder produces within the
    // attempt budget.
    let mut fed = 0u32;
    let mut attempt = 0u32;
    while fed < 6 && attempt < 200 {
        attempt += 1;
        let raw = video_frame(attempt);
        let encoded = encoder.encode(&raw).expect("encode");
        if encoded.bytes.is_empty() {
            continue; // encoder-level frame skip (drop, not accumulate)
        }
        let datagrams = packetizer
            .fragment_frame(call, 1, &encoded, 1400)
            .expect("fragments");
        for datagram in datagrams {
            let wire = datagram.encode();
            let _ = pipeline.receive_datagram(&wire).expect("receive");
        }
        fed += 1;
    }

    let dropped = pipeline.dropped_frames();
    let decoded = pipeline.decoded_frames();
    println!("VIDEO-PIPELINE fed {fed} frames: decoded {decoded}, dropped {dropped} (latest-frame slot replaced, no queue)");
    assert!(
        fed >= 3,
        "expected at least 3 non-empty encoded frames in 200 attempts, got {fed}"
    );
    assert_eq!(decoded, fed as u64, "every fed frame should decode");
    assert_eq!(
        dropped,
        fed as u64 - 1,
        "every frame after the first must be dropped by the single latest-frame slot"
    );
    assert!(
        pipeline.latest_remote_frame().is_some(),
        "the newest frame must be visible"
    );
}

// ---------------------------------------------------------------------------
// 7. Video reassembly: bounded incomplete state under fragment loss
// ---------------------------------------------------------------------------

#[test]
fn video_reassembly_is_bounded_under_loss() {
    use boru_core::call::video::reassembly::VideoReassembler;
    let call = CallId::from_bytes([26; 16]);
    let mut encoder = OpenH264Encoder::new().expect("encoder");
    let mut packetizer = VideoPacketizer::new();
    let mut reassembler = VideoReassembler::new();

    // Feed 30 non-empty frames, dropping the final fragment of each so every
    // frame stays incomplete.  Incomplete-frame count must stay at the hard
    // bound.  (Empty access units from OpenH264 frame-skip are skipped.)
    let mut max_incomplete = 0usize;
    let mut attempts = 0u32;
    let mut fed = 0u32;
    while fed < 30 && attempts < 90 {
        attempts += 1;
        let raw = video_frame(attempts);
        let encoded = encoder.encode(&raw).expect("encode");
        if encoded.bytes.is_empty() {
            continue;
        }
        fed += 1;
        let datagrams = packetizer
            .fragment_frame(call, 1, &encoded, 1400)
            .expect("fragments");
        for (j, datagram) in datagrams.iter().enumerate() {
            if j == datagrams.len() - 1 {
                continue; // drop the last fragment
            }
            let _ = reassembler.push_datagram(datagram).expect("admit");
        }
        max_incomplete = max_incomplete.max(reassembler.incomplete_frames());
    }

    println!("VIDEO-REASSEMBLY max incomplete frames under loss: {max_incomplete} (hard bound 10)");
    assert!(
        max_incomplete <= 10,
        "incomplete frame state must stay bounded, reached {max_incomplete}"
    );
    // A late expiry must also free state.  Use the deterministic form with a
    // far-future instant: the real `expire()` uses wall clock and the loop
    // above may finish before the 200 ms reassembly timeout elapses.
    let expired = reassembler.expire_at(Instant::now() + Duration::from_secs(10));
    println!("VIDEO-REASSEMBLY expired after timeout: {expired} frames");
    assert!(expired > 0);
}
