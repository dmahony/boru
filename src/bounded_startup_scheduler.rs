//! Bounded startup burst scheduler for queued download admissions.
//!
//! The scheduler bridges the gap between [`DownloadManager`] (which creates
//! [`QueuedDownload`] admissions) and the actual start of download work
//! (which requires [`QueuedDownload::start()`] to acquire the global active
//! semaphore).
//!
//! # Behaviour
//!
//! 1. **Startup burst** — [`kickstart`](BoundedStartupScheduler::kickstart)
//!    starts up to `max_startup_downloads` admissions immediately, respecting
//!    the `max_concurrent_downloads` cap.  Each admission transitions from
//!    queued to active by calling [`QueuedDownload::start()`], which acquires
//!    a global semaphore with exactly `max_concurrent_downloads` permits.
//! 2. **Ongoing cap** — after the burst, each call to
//!    [`notify_completed`](BoundedStartupScheduler::notify_completed)
//!    triggers the next pending item to start, keeping the active count
//!    at or below `max_concurrent_downloads`.
//! 3. **FIFO ordering** — items are processed in the order they were pushed.
//!
//! The scheduler is not thread-safe by itself; it is designed to be owned by
//! the download-lifecycle controller and driven from a single async task.

#![allow(missing_docs)]

use std::collections::{HashSet, VecDeque};

use tracing::info;

use crate::download_limits::{ActiveDownload, DownloadLimitsConfig, QueuedDownload};

/// Bounded startup burst scheduler for queued download admissions.
///
/// See the [module docs](self) for a full description.
#[derive(Debug)]
pub struct BoundedStartupScheduler {
    config: DownloadLimitsConfig,
    /// FIFO queue of pending admissions waiting to be started, keyed by
    /// download id so items can be removed before they start.
    pending: VecDeque<(i64, QueuedDownload)>,
    /// All download IDs that have been pushed into this scheduler, including
    /// items that were already started (via kickstart/notify_completed).
    /// Used by the download manager to avoid claiming scheduled items in
    /// the tick loop.
    ids: HashSet<i64>,
    /// Number of currently active downloads (start() has been called and
    /// the ActiveDownload is still alive).
    active: usize,
}

impl BoundedStartupScheduler {
    /// Create a new scheduler with the given configuration.
    ///
    /// Call [`push`] to add admissions, then [`kickstart`] to begin the
    /// startup burst.
    ///
    /// [`push`]: Self::push
    /// [`kickstart`]: Self::kickstart
    pub fn new(config: DownloadLimitsConfig) -> Self {
        Self {
            config,
            pending: VecDeque::new(),
            ids: HashSet::new(),
            active: 0,
        }
    }

    /// Add admissions to the pending queue (back-push, FIFO order).
    ///
    /// Each item is a `(download_id, QueuedDownload)` tuple so the scheduler
    /// can identify individual downloads for removal.
    pub fn push(&mut self, items: impl IntoIterator<Item = (i64, QueuedDownload)>) {
        for (id, queued) in items {
            self.ids.insert(id);
            self.pending.push_back((id, queued));
        }
    }

    /// Number of pending admissions not yet started.
    pub fn pending_count(&self) -> usize {
        self.pending.len()
    }

    /// Number of currently active downloads (started but not yet completed).
    pub fn active_count(&self) -> usize {
        self.active
    }

    /// The `max_concurrent_downloads` cap from configuration.
    pub fn max_concurrent(&self) -> usize {
        self.config.max_concurrent_downloads
    }

    /// The `max_startup_downloads` burst limit from configuration.
    pub fn max_startup(&self) -> usize {
        self.config.max_startup_downloads
    }

    /// Remove a specific download from the pending queue.
    ///
    /// Used when a startup-pending download completes locally (file already
    /// present) so the scheduler does not attempt to start a network transfer
    /// for it later.  Returns `true` if the item was found and removed.
    pub fn remove_from_pending(&mut self, id: i64) -> bool {
        if let Some(pos) = self.pending.iter().position(|(n, _)| *n == id) {
            self.pending.remove(pos);
            self.ids.remove(&id);
            true
        } else {
            false
        }
    }

    /// Check whether a download ID is managed by this scheduler (pending or
    /// already started).  Used by the download manager to avoid double-claiming
    /// items in the tick loop.
    pub fn contains(&self, id: i64) -> bool {
        self.ids.contains(&id)
    }

    /// Remove a download ID from the tracker without affecting the pending
    /// queue.  Used when a scheduler-started download completes or when the
    /// download manager resolves a scheduler-managed download locally.
    pub fn remove_id(&mut self, id: i64) {
        self.ids.remove(&id);
    }

    /// Start the startup burst.
    ///
    /// Starts up to `max_startup_downloads` pending admissions immediately,
    /// also bounded by the `max_concurrent_downloads` semaphore capacity.
    /// Items that cannot acquire a permit stay in the pending queue.
    ///
    /// Returns the [`ActiveDownload`] handles for the started downloads.
    pub async fn kickstart(&mut self) -> Vec<ActiveDownload> {
        // Burst is effectively bounded by both the startup budget and the
        // concurrent semaphore: we can't start more items at once than
        // there are permits.  Start them sequentially so each call to
        // start() blocks on the semaphore until a permit is available.
        let budget = self
            .config
            .max_startup_downloads
            .min(self.config.max_concurrent_downloads);
        let mut started = Vec::with_capacity(budget);

        for _ in 0..budget {
            let Some((_id, queued)) = self.pending.pop_front() else {
                break;
            };
            match queued.start().await {
                Ok(active) => {
                    self.active += 1;
                    info!("bounded-startup-scheduler: started download (burst)");
                    started.push(active);
                }
                Err(e) => {
                    info!(
                        "bounded-startup-scheduler: burst start failed: {e:?}, \
                         ending burst early"
                    );
                    break;
                }
            }
        }

        started
    }

    /// Pop items from the pending queue up to the burst budget.
    ///
    /// The caller is expected to start the returned items outside of any
    /// lock, then call [`record_started`](Self::record_started) to update
    /// the active count.
    pub fn pop_burst(&mut self) -> Vec<(i64, QueuedDownload)> {
        let budget = self
            .config
            .max_startup_downloads
            .min(self.config.max_concurrent_downloads);
        let mut items = Vec::with_capacity(budget);
        for _ in 0..budget {
            let Some(item) = self.pending.pop_front() else {
                break;
            };
            items.push(item);
        }
        items
    }

    /// Decrement the active count (called when a download completes).
    pub fn notify_completed_sync(&mut self) {
        self.active = self.active.saturating_sub(1);
    }

    /// Pop one pending item if the concurrent cap allows.
    ///
    /// Returns `None` when at cap or the queue is empty.  The caller starts
    /// the item outside any lock, then calls [`record_started`](Self::record_started).
    pub fn pop_next_to_start(&mut self) -> Option<(i64, QueuedDownload)> {
        if self.active >= self.config.max_concurrent_downloads {
            return None;
        }
        self.pending.pop_front()
    }

    /// Record that items were started (increment the active count).
    ///
    /// Must be balanced with [`notify_completed_sync`](Self::notify_completed_sync)
    /// when those downloads complete.
    pub fn record_started(&mut self, count: usize) {
        self.active += count;
    }

    /// Notify the scheduler that a download completed.
    ///
    /// Decrements the active count and, if the queue is non-empty and the
    /// active count is below `max_concurrent_downloads`, starts the next
    /// pending item.
    ///
    /// Returns `Some(ActiveDownload)` if a new download was started, or
    /// `None` if no more items are pending or at cap.
    pub async fn notify_completed(&mut self) -> Option<ActiveDownload> {
        self.notify_completed_sync();
        self.try_start_next().await
    }

    /// Start one pending item if under the concurrent cap.
    async fn try_start_next(&mut self) -> Option<ActiveDownload> {
        if self.active >= self.config.max_concurrent_downloads {
            return None;
        }
        while let Some((_id, queued)) = self.pending.pop_front() {
            match queued.start().await {
                Ok(active) => {
                    self.active += 1;
                    info!("bounded-startup-scheduler: started next queued download");
                    return Some(active);
                }
                Err(e) => {
                    info!(
                        "bounded-startup-scheduler: start failed: {e:?}, \
                         skipping item"
                    );
                    continue;
                }
            }
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::download_limits::{DownloadLimiter, DownloadLimitsConfig};
    use std::time::Duration;

    fn config() -> DownloadLimitsConfig {
        DownloadLimitsConfig {
            max_concurrent_downloads: 3,
            max_startup_downloads: 2,
            max_downloads_per_peer: 3,
            max_active_hash_verifications: 1,
            max_queued_downloads: 10,
            progress_update_interval: Duration::from_millis(1),
        }
    }

    fn limiter() -> DownloadLimiter {
        DownloadLimiter::new(config())
    }

    #[test]
    fn empty_scheduler_starts_nothing() {
        let s = BoundedStartupScheduler::new(config());
        assert_eq!(s.pending_count(), 0);
        assert_eq!(s.active_count(), 0);
    }

    #[test]
    fn push_and_pending_count() {
        let l = limiter();
        let mut s = BoundedStartupScheduler::new(config());
        let a = l.try_enqueue("a").unwrap();
        let b = l.try_enqueue("b").unwrap();
        s.push([(1, a), (2, b)]);
        assert_eq!(s.pending_count(), 2);
        assert_eq!(s.active_count(), 0);
    }

    #[tokio::test]
    async fn kickstart_starts_up_to_max_startup() {
        let l = limiter();
        let mut s = BoundedStartupScheduler::new(config());
        for i in 0..3 {
            s.push([(100 + i, l.try_enqueue("a").unwrap())]);
        }
        // max_startup_downloads = 2, so only 2 should start
        let started = s.kickstart().await;
        assert_eq!(started.len(), 2);
        assert_eq!(s.active_count(), 2);
        assert_eq!(s.pending_count(), 1);
    }

    #[tokio::test]
    async fn kickstart_handles_fewer_items_than_budget() {
        let l = limiter();
        let mut s = BoundedStartupScheduler::new(config());
        s.push([(1, l.try_enqueue("a").unwrap())]);
        // max_startup_downloads = 2, but only 1 item
        let started = s.kickstart().await;
        assert_eq!(started.len(), 1);
        assert_eq!(s.active_count(), 1);
        assert_eq!(s.pending_count(), 0);
    }

    #[tokio::test]
    async fn kickstart_respects_concurrent_cap() {
        // concurrent=1, startup=3 -- burst should only start 1
        let cfg = DownloadLimitsConfig {
            max_concurrent_downloads: 1,
            max_startup_downloads: 3,
            max_downloads_per_peer: 3,
            max_active_hash_verifications: 1,
            max_queued_downloads: 10,
            progress_update_interval: Duration::from_millis(1),
        };
        let l = DownloadLimiter::new(cfg.clone());
        let mut s = BoundedStartupScheduler::new(cfg);
        for i in 0..3 {
            s.push([(100 + i, l.try_enqueue("a").unwrap())]);
        }
        let started = s.kickstart().await;
        // Only 1 permit available, only 1 starts
        assert_eq!(started.len(), 1);
        assert_eq!(s.active_count(), 1);
        assert_eq!(s.pending_count(), 2);
    }

    #[tokio::test]
    async fn notify_completed_starts_next() {
        let l = limiter();
        let mut s = BoundedStartupScheduler::new(config());
        for i in 0..3 {
            s.push([(100 + i, l.try_enqueue("a").unwrap())]);
        }
        let started = s.kickstart().await;
        assert_eq!(started.len(), 2);
        assert_eq!(s.pending_count(), 1);

        // Drop one ActiveDownload and notify
        drop(started.into_iter().next().unwrap());
        let next = s.notify_completed().await;
        assert!(next.is_some(), "should start next after completion");
        assert_eq!(s.active_count(), 2);
        assert_eq!(s.pending_count(), 0);
    }

    #[tokio::test]
    async fn notify_completed_returns_none_when_no_pending() {
        let l = limiter();
        let mut s = BoundedStartupScheduler::new(config());
        s.push([(1, l.try_enqueue("a").unwrap())]);
        let started = s.kickstart().await;
        assert_eq!(started.len(), 1);

        drop(started.into_iter().next().unwrap());
        let next = s.notify_completed().await;
        assert!(next.is_none(), "no more items");
        assert_eq!(s.active_count(), 0);
    }

    #[tokio::test]
    async fn notify_completed_starts_when_cap_freed() {
        // concurrent=1, so after starting 1 item, the cap is hit
        let cfg = DownloadLimitsConfig {
            max_concurrent_downloads: 1,
            max_startup_downloads: 1,
            max_downloads_per_peer: 2,
            max_active_hash_verifications: 1,
            max_queued_downloads: 10,
            progress_update_interval: Duration::from_millis(1),
        };
        let l = DownloadLimiter::new(cfg.clone());
        let mut s = BoundedStartupScheduler::new(cfg);
        s.push([(1, l.try_enqueue("a").unwrap())]);
        s.push([(2, l.try_enqueue("a").unwrap())]);
        let started = s.kickstart().await;
        assert_eq!(started.len(), 1);
        assert_eq!(s.active_count(), 1);
        assert_eq!(s.pending_count(), 1);

        // Free the cap, then notify
        drop(started.into_iter().next().unwrap());
        let next = s.notify_completed().await;
        assert!(next.is_some(), "should start after cap freed");
        assert_eq!(s.active_count(), 1);
        assert_eq!(s.pending_count(), 0);
    }

    #[test]
    fn config_accessors() {
        let s = BoundedStartupScheduler::new(config());
        assert_eq!(s.max_concurrent(), 3);
        assert_eq!(s.max_startup(), 2);
    }

    #[test]
    fn remove_from_pending_removes_by_id() {
        let l = limiter();
        let mut s = BoundedStartupScheduler::new(config());
        s.push([(10, l.try_enqueue("a").unwrap())]);
        s.push([(20, l.try_enqueue("a").unwrap())]);
        assert_eq!(s.pending_count(), 2);

        assert!(s.remove_from_pending(10));
        assert_eq!(s.pending_count(), 1);

        assert!(!s.remove_from_pending(10), "already removed");
        assert_eq!(s.pending_count(), 1);

        assert!(s.remove_from_pending(20));
        assert_eq!(s.pending_count(), 0);
    }

    #[test]
    fn remove_from_pending_returns_false_for_unknown_id() {
        let l = limiter();
        let mut s = BoundedStartupScheduler::new(config());
        s.push([(1, l.try_enqueue("a").unwrap())]);
        assert!(!s.remove_from_pending(999));
        assert_eq!(s.pending_count(), 1);
    }
}
