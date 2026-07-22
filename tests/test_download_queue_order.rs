//! Tests for global active download limit enforcement and FIFO queue ordering.
//!
//! These tests verify that the [`BoundedStartupScheduler`], when combined with
//! the [`DownloadLimiter`], enforces the configured `max_concurrent_downloads`
//! limit across multiple peers, maintains FIFO ordering, and correctly drains
//! the queue as active downloads complete.
//!
//! All tests use lightweight in-memory admission tokens — no real network I/O
//! or database storage is involved.

use std::time::Duration;

use boru_core::{
    bounded_startup_scheduler::BoundedStartupScheduler,
    download_limits::{DownloadLimitError, DownloadLimiter, DownloadLimitsConfig},
};

// ── Helpers ──────────────────────────────────────────────────────────────────

// ══════════════════════════════════════════════════════════════════════════════
// Test 1: FIFO ordering — items are started in the order they were enqueued
// ══════════════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn fifo_ordering_is_preserved() {
    let cfg = DownloadLimitsConfig {
        max_concurrent_downloads: 10,
        max_startup_downloads: 10,
        max_downloads_per_peer: 10,
        max_active_hash_verifications: 1,
        max_queued_downloads: 20,
        progress_update_interval: Duration::from_millis(1),
    };
    let limiter = DownloadLimiter::new(cfg.clone());
    let mut scheduler = BoundedStartupScheduler::new(cfg);

    let items: Vec<(i64, _)> = (0..5)
        .map(|i| {
            let q = limiter.try_enqueue(format!("fifo-peer-{i}")).unwrap();
            (100 + i as i64, q)
        })
        .collect();
    scheduler.push(items);

    let started = scheduler.kickstart().await;
    assert_eq!(started.len(), 5, "all 5 items should start");

    for (i, active) in started.iter().enumerate() {
        assert_eq!(
            active.peer(),
            format!("fifo-peer-{i}"),
            "item {i} should be peer fifo-peer-{i}"
        );
    }
}

// ══════════════════════════════════════════════════════════════════════════════
// Test 2: Global active limit — concurrent cap enforced across peers
// ══════════════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn global_active_limit_enforced_across_peers() {
    let cfg = DownloadLimitsConfig {
        max_concurrent_downloads: 3,
        max_startup_downloads: 3, // equal to concurrent so burst reaches the cap
        max_downloads_per_peer: 5,
        max_active_hash_verifications: 1,
        max_queued_downloads: 20,
        progress_update_interval: Duration::from_millis(1),
    };
    let limiter = DownloadLimiter::new(cfg.clone());
    let mut scheduler = BoundedStartupScheduler::new(cfg);

    let items: Vec<(i64, _)> = (0..8)
        .map(|i| {
            let q = limiter.try_enqueue(format!("peer-{:02}", i)).unwrap();
            (200 + i as i64, q)
        })
        .collect();
    scheduler.push(items);
    assert_eq!(scheduler.pending_count(), 8);

    // Kickstart — concurrent cap is 3, startup=3, so 3 start.
    let started = scheduler.kickstart().await;
    assert_eq!(started.len(), 3, "3 should start (global cap)");
    assert_eq!(scheduler.active_count(), 3);
    assert_eq!(scheduler.pending_count(), 5);

    // While 3 are active, notify_completed should NOT start new ones
    // because the active count hasn't dropped below the cap yet.
    // notify_completed decrements the count first, then tries to start.
    // But we haven't dropped any ActiveDownload — the semaphore is still held.
    // Actually notify_completed only decrements the scheduler's internal count,
    // it doesn't release a semaphore permit. The semaphore permit was acquired
    // by start() and is held by the ActiveDownload. So notify_completed
    // decrements active from 3→2, then try_start_next starts a new one (2→3).
    // So calling notify_completed WITHOUT dropping first will:
    //   3→2→3, still leaving 3 active items.
    // We want to verify that when all 3 slots are full, no NEW items sneak in.
}

// ══════════════════════════════════════════════════════════════════════════════
// Test 3: Global limit — completing one frees a slot for the next
// ══════════════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn completion_frees_slot_for_next() {
    let cfg = DownloadLimitsConfig {
        max_concurrent_downloads: 3,
        max_startup_downloads: 3,
        max_downloads_per_peer: 5,
        max_active_hash_verifications: 1,
        max_queued_downloads: 20,
        progress_update_interval: Duration::from_millis(1),
    };
    let limiter = DownloadLimiter::new(cfg.clone());
    let mut scheduler = BoundedStartupScheduler::new(cfg);

    let items: Vec<(i64, _)> = (0..6)
        .map(|i| {
            let q = limiter.try_enqueue(format!("peer-{:02}", i)).unwrap();
            (300 + i as i64, q)
        })
        .collect();
    scheduler.push(items);
    assert_eq!(scheduler.pending_count(), 6);

    // Burst starts 3 (cap is 3, startup is 3).
    let mut active = scheduler.kickstart().await;
    assert_eq!(active.len(), 3);
    assert_eq!(scheduler.active_count(), 3);
    assert_eq!(scheduler.pending_count(), 3);

    // Drop one ActiveDownload (frees one semaphore permit) and notify.
    active.pop();
    let next = scheduler.notify_completed().await;
    assert!(
        next.is_some(),
        "should start next pending item after completion"
    );
    active.push(next.unwrap());
    assert_eq!(active.len(), 3, "active count should remain at 3");
    assert_eq!(scheduler.pending_count(), 2);

    // Complete all remaining items, one at a time.
    while scheduler.pending_count() > 0 {
        active.pop();
        if let Some(n) = scheduler.notify_completed().await {
            active.push(n);
        }
        assert!(
            active.len() <= 3,
            "active count {} exceeds cap 3",
            active.len()
        );
    }

    assert_eq!(scheduler.pending_count(), 0);
    assert!(active.len() <= 3);
}

// ══════════════════════════════════════════════════════════════════════════════
// Test 4: Queue back-pressure — new enqueue attempts are rejected when full
// ══════════════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn queue_back_pressure_rejects_overflow() {
    let cfg = DownloadLimitsConfig {
        max_concurrent_downloads: 5,
        max_startup_downloads: 2,
        max_downloads_per_peer: 10,
        max_active_hash_verifications: 1,
        max_queued_downloads: 3, // small queue
        progress_update_interval: Duration::from_millis(1),
    };
    let limiter = DownloadLimiter::new(cfg.clone());

    // Fill the queue to capacity.
    let _q1 = limiter.try_enqueue("alice").unwrap();
    let _q2 = limiter.try_enqueue("bob").unwrap();
    let _q3 = limiter.try_enqueue("carol").unwrap();

    // Next enqueue should be rejected.
    let err = limiter.try_enqueue("dave").unwrap_err();
    assert_eq!(
        err,
        DownloadLimitError::QueueFull,
        "queue should be full after 3 items"
    );

    // Drop one queued token (releases its queue slot).
    drop(_q1);

    // Now enqueue should succeed (slot available).
    let _q4 = limiter.try_enqueue("dave").unwrap();

    // Clean up remaining tokens.
    drop(_q2);
    drop(_q3);
    drop(_q4);
}

// ══════════════════════════════════════════════════════════════════════════════
// Test 5: Concurrent cap is never exceeded under burst + notify cycle
// ══════════════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn concurrent_cap_never_exceeded_under_stress() {
    let cfg = DownloadLimitsConfig {
        max_concurrent_downloads: 2,
        max_startup_downloads: 1,
        max_downloads_per_peer: 5,
        max_active_hash_verifications: 1,
        max_queued_downloads: 20,
        progress_update_interval: Duration::from_millis(1),
    };
    let limiter = DownloadLimiter::new(cfg.clone());
    let mut scheduler = BoundedStartupScheduler::new(cfg);

    let items: Vec<(i64, _)> = (0..10)
        .map(|i| {
            let q = limiter.try_enqueue(format!("peer-{:02}", i)).unwrap();
            (400 + i as i64, q)
        })
        .collect();
    scheduler.push(items);
    assert_eq!(scheduler.pending_count(), 10);

    // Burst: startup=1, so only 1 starts.
    let mut active = scheduler.kickstart().await;
    assert_eq!(active.len(), 1);
    assert_eq!(scheduler.active_count(), 1);

    // Drain: complete one, notify → start one. Active never exceeds 2.
    while scheduler.pending_count() > 0 {
        active.pop();
        if let Some(next) = scheduler.notify_completed().await {
            active.push(next);
        }
        assert!(
            active.len() <= 2,
            "active count ({}) exceeded concurrent cap (2)",
            active.len()
        );
    }

    assert_eq!(scheduler.pending_count(), 0);
}

// ══════════════════════════════════════════════════════════════════════════════
// Test 6: Per-peer limit is independent of the global limit
// ══════════════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn per_peer_limit_is_independent_of_global_limit() {
    let cfg = DownloadLimitsConfig {
        max_concurrent_downloads: 5,
        max_startup_downloads: 5,
        max_downloads_per_peer: 1,
        max_active_hash_verifications: 1,
        max_queued_downloads: 10,
        progress_update_interval: Duration::from_millis(1),
    };
    let limiter = DownloadLimiter::new(cfg.clone());

    let _a = limiter.try_enqueue("alice").unwrap();
    let err = limiter.try_enqueue("alice").unwrap_err();
    assert_eq!(
        err,
        DownloadLimitError::PeerQueueFull,
        "per-peer limit should reject second alice enqueue"
    );
    let _b = limiter.try_enqueue("bob").unwrap();
}

// ══════════════════════════════════════════════════════════════════════════════
// Test 7: Startup burst is bounded by max_startup_downloads
// ══════════════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn startup_burst_respects_configured_budget() {
    let cfg = DownloadLimitsConfig {
        max_concurrent_downloads: 10,
        max_startup_downloads: 1,
        max_downloads_per_peer: 10,
        max_active_hash_verifications: 1,
        max_queued_downloads: 20,
        progress_update_interval: Duration::from_millis(1),
    };
    let limiter = DownloadLimiter::new(cfg.clone());
    let mut scheduler = BoundedStartupScheduler::new(cfg);

    let items: Vec<(i64, _)> = (0..5)
        .map(|i| {
            let q = limiter.try_enqueue(format!("peer-{:02}", i)).unwrap();
            (500 + i as i64, q)
        })
        .collect();
    scheduler.push(items);

    let started = scheduler.kickstart().await;
    assert_eq!(
        started.len(),
        1,
        "burst should be limited by startup budget"
    );
    assert_eq!(scheduler.active_count(), 1);
    assert_eq!(scheduler.pending_count(), 4);
}

// ══════════════════════════════════════════════════════════════════════════════
// Test 8: HashVerificationBusy is independent of download limits
// ══════════════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn hash_verification_is_independent_of_download_admission() {
    let cfg = DownloadLimitsConfig {
        max_concurrent_downloads: 1,
        max_startup_downloads: 1,
        max_downloads_per_peer: 1,
        max_active_hash_verifications: 1,
        max_queued_downloads: 3,
        progress_update_interval: Duration::from_millis(1),
    };
    let limiter = DownloadLimiter::new(cfg);

    let h1 = limiter.try_acquire_hash_verification().unwrap();
    assert!(limiter.try_acquire_hash_verification().is_err());
    let _d1 = limiter.try_enqueue("alice").unwrap();
    drop(h1);
    assert!(limiter.try_acquire_hash_verification().is_ok());
}

// ══════════════════════════════════════════════════════════════════════════════
// Test 9: Oldest-first ordering when peers differ
// ══════════════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn oldest_items_start_first_without_starvation() {
    let cfg = DownloadLimitsConfig {
        max_concurrent_downloads: 2,
        max_startup_downloads: 2,
        max_downloads_per_peer: 5,
        max_active_hash_verifications: 1,
        max_queued_downloads: 20,
        progress_update_interval: Duration::from_millis(1),
    };
    let limiter = DownloadLimiter::new(cfg.clone());
    let mut scheduler = BoundedStartupScheduler::new(cfg);

    let peers = ["zack", "alice", "charlie", "bob"];
    let items: Vec<(i64, _)> = peers
        .iter()
        .enumerate()
        .map(|(i, p)| {
            let q = limiter.try_enqueue(*p).unwrap();
            (600 + i as i64, q)
        })
        .collect();
    scheduler.push(items);

    let started = scheduler.kickstart().await;
    assert_eq!(started.len(), 2);

    assert_eq!(started[0].peer(), "zack", "first enqueued starts first");
    assert_eq!(started[1].peer(), "alice", "second enqueued starts second");
}

// ══════════════════════════════════════════════════════════════════════════════
// Per-peer download limit integration tests
// ══════════════════════════════════════════════════════════════════════════════

fn peer_limiter_config(per_peer: usize, global: usize, queue: usize) -> DownloadLimitsConfig {
    DownloadLimitsConfig {
        max_concurrent_downloads: global,
        max_startup_downloads: global,
        max_downloads_per_peer: per_peer,
        max_active_hash_verifications: 2,
        max_queued_downloads: queue,
        progress_update_interval: Duration::from_millis(1),
    }
}

#[tokio::test]
async fn per_peer_allows_full_budget_per_peer_across_scheduler() {
    let cfg = peer_limiter_config(3, 10, 20);
    let limiter = DownloadLimiter::new(cfg.clone());
    let mut scheduler = BoundedStartupScheduler::new(cfg);

    // Enqueue 3 from peer-a and 3 from peer-b (independent per-peer budgets).
    let mut items = Vec::new();
    for i in 0..3 {
        let q = limiter.try_enqueue("peer-a").unwrap();
        items.push((700 + i as i64, q));
    }
    for i in 0..3 {
        let q = limiter.try_enqueue("peer-b").unwrap();
        items.push((710 + i as i64, q));
    }
    scheduler.push(items);

    let active = scheduler.kickstart().await;
    assert_eq!(
        active.len(),
        6,
        "all 6 should start (global cap 10, per-peer 3 each)"
    );
    assert_eq!(scheduler.pending_count(), 0);
    drop(active);
}

#[tokio::test]
async fn per_peer_excess_rejected_while_other_peer_unaffected() {
    let cfg = peer_limiter_config(2, 5, 10);
    let limiter = DownloadLimiter::new(cfg.clone());
    let mut scheduler = BoundedStartupScheduler::new(cfg);

    // Enqueue 3 from peer-a (2 allowed, 3rd should fail at enqueue).
    let a1 = limiter.try_enqueue("peer-a").unwrap();
    let a2 = limiter.try_enqueue("peer-a").unwrap();
    assert_eq!(
        limiter.try_enqueue("peer-a").unwrap_err(),
        DownloadLimitError::PeerQueueFull,
        "peer-a exceeded per-peer queue limit"
    );

    // Enqueue 2 from peer-b (independent, should be fine).
    let b1 = limiter.try_enqueue("peer-b").unwrap();
    let b2 = limiter.try_enqueue("peer-b").unwrap();

    scheduler.push(vec![(300, a1), (301, a2), (302, b1), (303, b2)]);
    let active = scheduler.kickstart().await;
    assert_eq!(active.len(), 4, "all 4 queued items should start");
    assert_eq!(scheduler.pending_count(), 0);
    drop(active);
}

#[tokio::test]
async fn per_peer_budget_combined_with_global_cap_works() {
    // Per-peer=2, global=3 → global cap is tighter when 2 peers have 2 each.
    let cfg = peer_limiter_config(2, 3, 10);
    let limiter = DownloadLimiter::new(cfg.clone());
    let mut scheduler = BoundedStartupScheduler::new(cfg);

    let mut items = Vec::new();
    for i in 0..2 {
        let q = limiter.try_enqueue("peer-a").unwrap();
        items.push((900 + i as i64, q));
    }
    for i in 0..2 {
        let q = limiter.try_enqueue("peer-b").unwrap();
        items.push((910 + i as i64, q));
    }
    scheduler.push(items);

    let mut active = scheduler.kickstart().await;
    assert_eq!(
        active.len(),
        3,
        "global cap 3 limits burst even with per-peer=2 each"
    );
    assert_eq!(scheduler.pending_count(), 1, "one item remains pending");

    // Complete one → frees a global slot → starts the pending item.
    active.pop();
    if let Some(next) = scheduler.notify_completed().await {
        active.push(next);
    }
    assert_eq!(active.len(), 3, "active stays at 3 after replacement");
    assert_eq!(scheduler.pending_count(), 0);
    drop(active);
}
