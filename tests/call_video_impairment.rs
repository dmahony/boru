//! Fast deterministic media-link tests (VC-003).

mod support;

use support::impaired_link::{ImpairedLink, ImpairedLinkConfig, MediaPriority, Packet};

use boru_core::call::adaptation::{AdaptationController, QualityLevel};
use boru_core::call::media::MediaKind;
use boru_core::call::stats::CallStats;
use boru_core::call::video::config::VideoProfile;

fn trace(seed: u64) -> Vec<(u64, u64)> {
    let config = ImpairedLinkConfig {
        capacity_bps: 80_000,
        base_delay_us: 500,
        jitter_us: 250,
        loss_percent: 17,
        duplicate_percent: 23,
        reorder_window: 3,
        queue_limit_bytes: 16 * 1024,
    };
    let mut link = ImpairedLink::new(config, seed);
    for id in 0..30 {
        link.send(Packet::new(
            id,
            80 + (id as usize % 5) * 10,
            if id % 4 == 0 {
                MediaPriority::Audio
            } else {
                MediaPriority::Video
            },
        ));
    }
    link.advance_to(2_000_000);
    link.drain_delivered()
        .into_iter()
        .map(|packet| (packet.packet.id, packet.delivered_at_us))
        .collect()
}

#[test]
fn identical_seed_produces_identical_delivery_trace() {
    assert_eq!(trace(0xfeed_beef), trace(0xfeed_beef));
    assert_ne!(trace(0xfeed_beef), trace(0xfeed_babe));
}

#[test]
fn delay_and_token_bucket_are_advanced_without_sleeping() {
    let config = ImpairedLinkConfig {
        capacity_bps: 1_000_000,
        base_delay_us: 1_000,
        queue_limit_bytes: 1024,
        ..Default::default()
    };
    let mut link = ImpairedLink::new(config, 1);
    assert!(link.send(Packet::new(7, 100, MediaPriority::Audio)));
    link.advance_to(1_799);
    assert!(link.drain_delivered().is_empty());
    link.advance_to(1_800);
    assert_eq!(link.drain_delivered()[0].packet.id, 7);
    assert_eq!(link.counters().bytes_delivered, 100);
}

#[test]
fn loss_and_duplication_are_counted() {
    let config = ImpairedLinkConfig {
        capacity_bps: 1_000_000,
        loss_percent: 100,
        duplicate_percent: 100,
        queue_limit_bytes: 1024,
        ..Default::default()
    };
    let mut link = ImpairedLink::new(config, 9);
    assert!(link.send(Packet::new(1, 10, MediaPriority::Video)));
    link.advance_to(1_000);
    assert!(link.drain_delivered().is_empty());
    assert_eq!(link.counters().dropped_loss, 1);
    assert_eq!(
        link.counters().duplicated,
        0,
        "loss occurs before duplication"
    );
}

#[test]
fn audio_evicts_video_when_a_constrained_queue_is_full() {
    let config = ImpairedLinkConfig {
        capacity_bps: 1,
        queue_limit_bytes: 100,
        ..Default::default()
    };
    let mut link = ImpairedLink::new(config, 2);
    assert!(link.send(Packet::new(1, 60, MediaPriority::Video)));
    assert!(link.send(Packet::new(2, 40, MediaPriority::Video)));
    assert!(link.send(Packet::new(3, 40, MediaPriority::Audio)));
    assert_eq!(link.queued_bytes(), 80);
    link.set_capacity_bps(1_000_000);
    link.advance_to(10_000);
    let delivered = link.drain_delivered();
    assert!(delivered.iter().any(|packet| packet.packet.id == 3));
    assert_eq!(link.counters().dropped_queue, 1);
}

fn media_packet(kind: MediaKind, sequence: u32, size: usize) -> Packet {
    Packet::new(
        sequence as u64,
        size,
        match kind {
            MediaKind::Audio => MediaPriority::Audio,
            MediaKind::Video => MediaPriority::Video,
        },
    )
}

#[test]
fn capacity_profiles_preserve_audio_from_2mbps_to_96kbps() {
    // These are intentionally small packets: the test validates scheduling
    // and queue bounds, rather than pretending to encode a real frame.
    for capacity_bps in [2_000_000, 700_000, 350_000, 200_000, 128_000, 96_000] {
        let mut link = ImpairedLink::new(
            ImpairedLinkConfig {
                capacity_bps,
                base_delay_us: 8_000,
                queue_limit_bytes: 8 * 1024,
                ..Default::default()
            },
            capacity_bps,
        );
        for sequence in 0..8 {
            assert!(link.send(media_packet(MediaKind::Audio, sequence, 160)));
            let _ = link.send(media_packet(MediaKind::Video, sequence, 700));
        }
        assert!(link.queued_bytes() <= 8 * 1024);
        link.advance_to(2_000_000);
        let delivered = link.drain_delivered();
        assert!(
            delivered.iter().any(|packet| packet.packet.priority == MediaPriority::Audio),
            "audio was starved at {capacity_bps} bps"
        );
        assert!(delivered.iter().all(|packet| packet.delivered_at_us <= 2_000_000));
    }
}

#[test]
fn stepped_capacity_recovers_without_unbounded_queue_or_frame_age() {
    let mut link = ImpairedLink::new(
        ImpairedLinkConfig {
            capacity_bps: 96_000,
            base_delay_us: 10_000,
            queue_limit_bytes: 4 * 1024,
            ..Default::default()
        },
        0xdecafbad,
    );
    for sequence in 0..12 {
        let _ = link.send(media_packet(MediaKind::Audio, sequence, 180));
        let _ = link.send(media_packet(MediaKind::Video, sequence, 900));
    }
    assert!(link.queued_bytes() <= 4 * 1024);
    link.advance_to(100_000);
    link.set_capacity_bps(2_000_000);
    for sequence in 12..24 {
        let _ = link.send(media_packet(MediaKind::Audio, sequence, 180));
        let _ = link.send(media_packet(MediaKind::Video, sequence, 900));
    }
    link.advance_to(1_000_000);
    let delivered = link.drain_delivered();
    assert!(!delivered.is_empty(), "recovery delivered no media");
    assert!(delivered.iter().all(|packet| packet.delivered_at_us <= 1_000_000));
    assert!(link.queued_bytes() <= 4 * 1024);
}

#[test]
fn loss_and_reorder_keep_audio_and_bound_video_recovery() {
    let mut link = ImpairedLink::new(
        ImpairedLinkConfig {
            capacity_bps: 200_000,
            base_delay_us: 5_000,
            jitter_us: 2_000,
            loss_percent: 12,
            reorder_window: 4,
            queue_limit_bytes: 12 * 1024,
            ..Default::default()
        },
        0x1234_5678,
    );
    for sequence in 0..80 {
        let kind = if sequence % 5 == 0 {
            MediaKind::Audio
        } else {
            MediaKind::Video
        };
        let _ = link.send(media_packet(
            kind,
            sequence,
            if kind == MediaKind::Audio { 160 } else { 500 },
        ));
    }
    link.advance_to(5_000_000);
    let delivered = link.drain_delivered();
    assert!(
        delivered
            .iter()
            .any(|packet| packet.packet.priority == MediaPriority::Audio)
    );
    assert!(link.counters().dropped_loss > 0);
    assert!(link.queued_bytes() <= 12 * 1024);
}

#[test]
fn profile_transitions_degrade_then_recover_without_keyframe_storm() {
    let mut controller = AdaptationController::default();
    let mut stats = CallStats::default();
    controller.update(stats);
    for expected_level in 1..=3 {
        stats.audio_packets_lost += 1;
        stats.video_frames_dropped += 1;
        controller.update(stats);
        stats.audio_packets_lost += 1;
        stats.video_frames_dropped += 1;
        let decision = controller.update(stats);
        assert_eq!(controller.level(), QualityLevel::from_u8(expected_level));
        let profile = [VideoProfile::Q0, VideoProfile::Q1, VideoProfile::Q2, VideoProfile::Q3]
            [expected_level as usize];
        assert_eq!(decision.video.fps, profile.config().fps);
        assert!(decision.audio.bitrate_kbps >= 16);
        assert_eq!(
            stats.keyframe_requests, 0,
            "adaptation must not request a keyframe storm"
        );
    }
    for _ in 0..9 {
        controller.update(stats);
    }
    assert_eq!(controller.level(), QualityLevel::Q0, "healthy samples should recover to Q0");
    assert_eq!(VideoProfile::Q0.config().keyframe_interval, 48);
}
