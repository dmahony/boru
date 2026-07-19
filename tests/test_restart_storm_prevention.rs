//! Integration tests for restart storm prevention.
//!
//! These tests verify that the [`BoundedStartupScheduler`], when fed many
//! queued admissions (as would happen after restoring from durable storage
//! on application restart), limits the simultaneous startup burst to
//! `max_startup_downloads` and then enforces the ongoing
//! `max_concurrent_downloads` cap.
//!
//! The `QueuedDownload` / `ActiveDownload` types serve as natural test
//! doubles — dropping an `ActiveDownload` releases the semaphore permits
//! held by that download, simulating completion on demand.

use std::time::Duration;

use boru_chat::{
    bounded_startup_scheduler::BoundedStartupScheduler,
    download_limits::{DownloadLimiter, DownloadLimitsConfig},
};

fn storm_config() -> DownloadLimitsConfig {
    DownloadLimitsConfig {
        max_concurrent_downloads: 4,
        max_startup_downloads: 3,
        max_downloads_per_peer: 50,
        max_active_hash_verifications: 1,
        max_queued_downloads: 50,
        progress_update_interval: Duration::from_millis(1),
    }
}

fn config_with(startup: usize, concurrent: usize) -> DownloadLimitsConfig {
    DownloadLimitsConfig {
        max_concurrent_downloads: concurrent,
        max_startup_downloads: startup,
        max_downloads_per_peer: 10,
        max_active_hash_verifications: 1,
        max_queued_downloads: 50,
        progress_update_interval: Duration::from_millis(1),
    }
}

/// Enqueue N distinct admissions through the limiter.
fn enqueue_n(
    limiter: &DownloadLimiter,
    n: usize,
) -> Vec<boru_chat::download_limits::QueuedDownload> {
    (0..n)
        .map(|i| {
            let peer = format!("peer-{:02}", i % 10);
            limiter.try_enqueue(peer).unwrap()
        })
        .collect()
}

// ═════════════════════════════════════════════════════════════════════════════
// Test 1: Burst is limited to max_startup_downloads
// ═════════════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn restart_storm_burst_limited_to_max_startup() {
    let config = storm_config();
    let limiter = DownloadLimiter::new(config.clone());
    let mut scheduler = BoundedStartupScheduler::new(config.clone());

    // Enqueue 50 items, simulating what a restart would restore.
    let items = enqueue_n(&limiter, 50);
    // Wrap each admission with a dummy download id for the scheduler.
    let items: Vec<_> = items
        .into_iter()
        .enumerate()
        .map(|(i, q)| (i as i64, q))
        .collect();
    scheduler.push(items);

    assert_eq!(scheduler.pending_count(), 50, "all 50 should be pending");
    assert_eq!(scheduler.active_count(), 0);

    // Kickstart — only max_startup_downloads (3) should start.
    let started = scheduler.kickstart().await;
    assert_eq!(
        started.len(),
        config.max_startup_downloads,
        "kickstart should start exactly {} items",
        config.max_startup_downloads
    );
    assert_eq!(
        scheduler.active_count(),
        config.max_startup_downloads,
        "active count should match started count"
    );
    assert_eq!(
        scheduler.pending_count(),
        50 - config.max_startup_downloads,
        "remaining should still be pending"
    );
}

// ═════════════════════════════════════════════════════════════════════════════
// Test 2: When the concurrent cap is lower than the startup burst, the
//         concurrent cap wins.
// ═════════════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn restart_storm_burst_respects_concurrent_cap() {
    // startup = 5, concurrent = 2 — burst is clamped to 2.
    let config = config_with(5, 2);
    let limiter = DownloadLimiter::new(config.clone());
    let mut scheduler = BoundedStartupScheduler::new(config);

    let items = enqueue_n(&limiter, 10);
    let items: Vec<_> = items
        .into_iter()
        .enumerate()
        .map(|(i, q)| (i as i64, q))
        .collect();
    scheduler.push(items);

    let started = scheduler.kickstart().await;
    assert_eq!(
        started.len(),
        2,
        "concurrent cap (2) should clamp the startup burst (5)"
    );
    assert_eq!(scheduler.active_count(), 2);
    assert_eq!(scheduler.pending_count(), 8);
}

// ═════════════════════════════════════════════════════════════════════════════
// Test 3: After the burst, notify_completed starts one more per completion
//         and the concurrent cap is never exceeded.
// ═════════════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn restart_storm_ongoing_cap_enforced_via_notify() {
    let config = storm_config(); // startup=3, concurrent=4
    let limiter = DownloadLimiter::new(config.clone());
    let mut scheduler = BoundedStartupScheduler::new(config.clone());

    let items = enqueue_n(&limiter, 50);
    // Wrap each admission with a dummy download id for the scheduler.
    let items: Vec<_> = items
        .into_iter()
        .enumerate()
        .map(|(i, q)| (i as i64, q))
        .collect();
    scheduler.push(items);

    // Burst: 3 start.
    let mut active: Vec<_> = scheduler.kickstart().await;
    assert_eq!(active.len(), config.max_startup_downloads); // 3
    let mut total_started = active.len();

    // Drain the pending queue by completing one, then notifying.
    // Each cycle: drop 1 active → notify → start 1 more (if pending).
    // Active count should never exceed max_concurrent_downloads.
    while scheduler.pending_count() > 0 {
        // Drop one active handle (simulates download completion).
        active.pop();

        // Notify scheduler — should try to start next pending item.
        match scheduler.notify_completed().await {
            Some(next) => {
                active.push(next);
                total_started += 1;
            }
            None => {
                // No pending items to start, which is fine.
            }
        }

        // NEVER exceed the concurrent cap.
        assert!(
            active.len() <= config.max_concurrent_downloads,
            "active count {} exceeds cap {}",
            active.len(),
            config.max_concurrent_downloads
        );
    }

    // All 50 items were eventually started.
    assert_eq!(total_started, 50, "all 50 items should have been started");
    assert_eq!(
        scheduler.pending_count(),
        0,
        "pending queue should be drained"
    );

    // Finish remaining active items.
    while !active.is_empty() {
        active.pop();
        scheduler.notify_completed().await;
    }
    assert_eq!(scheduler.active_count(), 0, "all downloads completed");
}

// ═════════════════════════════════════════════════════════════════════════════
// Test 4: Full restart simulation — enqueue, restart (fresh scheduler),
//         kickstart, drain. All items eventually process.
// ═════════════════════════════════════════════════════════════════════════════

#[tokio::test]
async fn restart_storm_all_items_eventually_started() {
    let config = storm_config(); // startup=3, concurrent=4
    let limiter = DownloadLimiter::new(config.clone());

    // Phase 1 — "Pre-restart": enqueue items into the limiter.
    let admissions = enqueue_n(&limiter, 50);

    // Phase 2 — "Restart": create a fresh scheduler with the same config.
    let mut scheduler = BoundedStartupScheduler::new(config.clone());
    let admissions: Vec<_> = admissions
        .into_iter()
        .enumerate()
        .map(|(i, q)| (i as i64, q))
        .collect();
    scheduler.push(admissions);
    assert_eq!(scheduler.pending_count(), 50);
    assert_eq!(scheduler.active_count(), 0);

    // Phase 3 — Kickstart: burst-limited start.
    let mut active = scheduler.kickstart().await;
    let mut total_started = active.len();

    // Phase 4 — Drain via notify_completed.
    while scheduler.pending_count() > 0 {
        let _ = active.pop();
        if let Some(next) = scheduler.notify_completed().await {
            active.push(next);
            total_started += 1;
        }
    }

    assert_eq!(total_started, 50, "all 50 items must eventually start");
    assert_eq!(scheduler.pending_count(), 0);

    // Drain remaining active handles.
    while !active.is_empty() {
        active.pop();
        scheduler.notify_completed().await;
    }
    assert_eq!(scheduler.active_count(), 0);
}
