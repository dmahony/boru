//! Fast deterministic media-link tests (VC-003).

mod support;

use support::impaired_link::{ImpairedLink, ImpairedLinkConfig, MediaPriority, Packet};

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
