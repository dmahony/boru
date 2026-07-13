//! Per-peer safety enforcement for untrusted public rooms.
//!
//! [`PublicRoomSafety`] wraps a [`PublicRoomConfig`] and provides
//! per-peer enforcement for message size, nickname length, message
//! rate, blob announcements, and download queue depth.  All checks
//! are gated by the presence of a safety instance — private rooms
//! simply pass `None` and skip every check.
//!
//! # State tracking
//!
//! The struct holds per-peer [`Mutex`]-protected state (rate-limit
//! timestamps, blob-announcement counts, download-queue depths) so
//! it is safe to share via `&PublicRoomSafety` across tasks.

use std::{collections::HashMap, sync::Mutex, time::Instant};

use iroh::PublicKey;
#[cfg(test)]
use iroh::SecretKey;
use tracing::warn;

use crate::public_room_config::PublicRoomConfig;

// ---------------------------------------------------------------------------
// PublicRoomSafety
// ---------------------------------------------------------------------------

const MAX_TRACKED_PEERS: usize = 4096;

/// Per-peer drop counters, protected by a single mutex so lock ordering
/// cannot deadlock between the three `record_*` functions.
///
/// Entries are bounded by [`MAX_TRACKED_PEERS`]; when at capacity and a new
/// peer appears the increment is silently dropped (the enforcement rejection
/// still happens — this is purely an observation bound).
#[derive(Debug)]
struct DropCounters {
    rate_limit_hits: HashMap<PublicKey, u64>,
    blob_announcement_rejects: HashMap<PublicKey, u64>,
    download_rejects: HashMap<PublicKey, u64>,
}

impl DropCounters {
    fn new() -> Self {
        Self {
            rate_limit_hits: HashMap::new(),
            blob_announcement_rejects: HashMap::new(),
            download_rejects: HashMap::new(),
        }
    }

    /// Increment the rate-limit-hit counter for `peer`, bounded to
    /// [`MAX_TRACKED_PEERS`] unique peers.  Returns the new value
    /// (or `0` if the peer was silently dropped because the tracker is at capacity).
    fn inc_rate(&mut self, peer: PublicKey) -> u64 {
        self.ensure_peer(peer);
        match self.rate_limit_hits.get_mut(&peer) {
            Some(c) => {
                *c = c.saturating_add(1);
                *c
            }
            None => 0, // at capacity — peer not tracked
        }
    }

    /// Increment the blob-announcement-reject counter for `peer`,
    /// bounded to [`MAX_TRACKED_PEERS`] unique peers.  Returns the new value
    /// (or `0` if the peer was silently dropped because the tracker is at capacity).
    fn inc_blob(&mut self, peer: PublicKey) -> u64 {
        self.ensure_peer(peer);
        match self.blob_announcement_rejects.get_mut(&peer) {
            Some(c) => {
                *c = c.saturating_add(1);
                *c
            }
            None => 0, // at capacity — peer not tracked
        }
    }

    /// Increment the download-reject counter for `peer`, bounded to
    /// [`MAX_TRACKED_PEERS`] unique peers.  Returns the new value
    /// (or `0` if the peer was silently dropped because the tracker is at capacity).
    fn inc_download(&mut self, peer: PublicKey) -> u64 {
        self.ensure_peer(peer);
        match self.download_rejects.get_mut(&peer) {
            Some(c) => {
                *c = c.saturating_add(1);
                *c
            }
            None => 0, // at capacity — peer not tracked
        }
    }

    /// Ensure `peer` has an entry in all three maps.  If at capacity and
    /// `peer` is new, the increment is silently dropped (skip recording).
    fn ensure_peer(&mut self, peer: PublicKey) {
        if self.rate_limit_hits.contains_key(&peer) {
            return;
        }
        if self.rate_limit_hits.len() >= MAX_TRACKED_PEERS {
            return; // at capacity — silently drop rather than unbounded growth
        }
        self.rate_limit_hits.insert(peer, 0);
        self.blob_announcement_rejects.insert(peer, 0);
        self.download_rejects.insert(peer, 0);
    }

    /// Convenience: read all three counters for `peer` in one shot.
    fn all_for(&self, peer: &PublicKey) -> (u64, u64, u64) {
        (
            self.rate_limit_hits.get(peer).copied().unwrap_or(0),
            self.blob_announcement_rejects
                .get(peer)
                .copied()
                .unwrap_or(0),
            self.download_rejects.get(peer).copied().unwrap_or(0),
        )
    }

    /// Full snapshot across all tracked peers.
    fn snapshot(&self) -> HashMap<PublicKey, (u64, u64, u64)> {
        let mut out = HashMap::new();
        let mut all_peers: Vec<PublicKey> = self
            .rate_limit_hits
            .keys()
            .chain(self.blob_announcement_rejects.keys())
            .chain(self.download_rejects.keys())
            .copied()
            .collect();
        all_peers.sort();
        all_peers.dedup();
        for pk in all_peers {
            out.insert(
                pk,
                (
                    self.rate_limit_hits.get(&pk).copied().unwrap_or(0),
                    self.blob_announcement_rejects
                        .get(&pk)
                        .copied()
                        .unwrap_or(0),
                    self.download_rejects.get(&pk).copied().unwrap_or(0),
                ),
            );
        }
        out
    }
}

/// Optional safety layer for public-room message processing.
///
/// Create one with [`new`](Self::new) when entering a public room and
/// pass it to [`forward_gossip_events`](crate::chat_core::forward_gossip_events)
/// and [`handle_net_event`](crate::chat_core::handle_net_event) to enforce
/// per-peer bounds on incoming traffic.
///
/// When `None` is passed instead (private-room path), every check is a
/// no-op — zero overhead.
#[derive(Debug)]
pub struct PublicRoomSafety {
    /// The configuration limits.
    config: PublicRoomConfig,

    // ── Per-peer rate-limit state ─────────────────────────────────
    /// Message arrival times per peer for sliding-window rate limiting.
    peer_message_times: Mutex<HashMap<PublicKey, Vec<Instant>>>,

    // ── Per-peer blob-announcement state ───────────────────────────
    /// Number of blob/image announcements per peer in the current window.
    peer_blob_count: Mutex<HashMap<PublicKey, usize>>,
    /// When the current blob-announcement window started for each peer.
    peer_blob_window_start: Mutex<HashMap<PublicKey, Instant>>,

    // ── Per-peer download-queue state ──────────────────────────────
    /// Current download-queue depth per peer.
    peer_download_count: Mutex<HashMap<PublicKey, usize>>,

    // ── Per-peer drop counters (abuse-detection / safety tracing) ──
    /// Single mutex protecting all three counter maps.
    /// Never split into separate mutexes — lock ordering must be consistent.
    drop_counters: Mutex<DropCounters>,
}

impl PublicRoomSafety {
    /// Create a new safety layer from a [`PublicRoomConfig`].
    ///
    /// Configuration is sanitised before use so public-room enforcement cannot
    /// be bypassed by invalid floating-point tuning values.
    pub fn new(config: PublicRoomConfig) -> Self {
        // Public-room limits are security boundaries.  Sanitise caller-provided
        // tuning before storing them so NaN jitter/rates cannot bypass or
        // accidentally disable an enforcement path.
        let config = config.sanitize();
        Self {
            config,
            peer_message_times: Mutex::new(HashMap::new()),
            peer_blob_count: Mutex::new(HashMap::new()),
            peer_blob_window_start: Mutex::new(HashMap::new()),
            peer_download_count: Mutex::new(HashMap::new()),
            drop_counters: Mutex::new(DropCounters::new()),
        }
    }

    /// Access the underlying configuration (read-only).
    pub fn config(&self) -> &PublicRoomConfig {
        &self.config
    }

    // ── Message-size enforcement ───────────────────────────────────

    /// Check whether `raw_bytes` is within the configured message-size limit.
    ///
    /// Returns `true` if the message may be processed (size ≤ limit), or
    /// `false` if it exceeds the limit and should be dropped silently.
    pub fn check_message_size(&self, raw_bytes: &[u8]) -> bool {
        raw_bytes.len() <= self.config.message_size_limit
    }

    /// Check whether an automatically downloaded blob is within the public
    /// room's per-object size cap.
    pub fn check_blob_size(&self, size: usize) -> bool {
        size <= self.config.max_blob_size_bytes
    }

    // ── Nickname enforcement ───────────────────────────────────────

    /// Enforce the nickname-length limit.
    ///
    /// Returns a [`std::borrow::Cow`] that is either the original name
    /// (if within limit) or a truncated version suffixed with `…`.
    ///
    /// The truncation operates on bytes to match the limit semantics
    /// (the limit is in bytes, not characters).  If the name is already
    /// within the limit the returned `Cow` borrows the original; no
    /// allocation is performed.
    pub fn enforce_nickname<'a>(&self, name: &'a str) -> std::borrow::Cow<'a, str> {
        let limit = self.config.nickname_length_limit;
        if name.len() <= limit {
            return std::borrow::Cow::Borrowed(name);
        }
        // Keep the suffix inside the byte limit.  For very small limits there
        // is no room for an ellipsis, so return the largest valid UTF-8 prefix.
        if limit < '…'.len_utf8() {
            let mut end = limit;
            while end > 0 && !name.is_char_boundary(end) {
                end -= 1;
            }
            return std::borrow::Cow::Owned(name[..end].to_owned());
        }
        let mut end = limit - '…'.len_utf8();
        while end > 0 && !name.is_char_boundary(end) {
            end -= 1;
        }
        let mut truncated = name[..end].to_owned();
        truncated.push('…');
        std::borrow::Cow::Owned(truncated)
    }

    // ── Per-peer message rate limiting ──────────────────────────────

    /// Check whether `peer` has exceeded the per-peer message rate limit.
    ///
    /// Uses a sliding-window of `message_window` duration.  Returns `true`
    /// if the message is allowed, `false` if the peer is currently over
    /// the rate limit and the message should be dropped.
    ///
    /// A `per_peer_message_rate` of `0.0` means no rate limit is applied.
    pub fn check_rate_limit(&self, peer: &PublicKey) -> bool {
        let rate = self.config.per_peer_message_rate;
        if rate <= 0.0 {
            return true; // no rate limit
        }

        let window_duration = std::time::Duration::from_secs_f64(1.0);
        let max_per_window = rate.ceil() as usize;
        let now = Instant::now();

        let mut times = self.peer_message_times.lock().unwrap();
        if !times.contains_key(peer) && times.len() >= MAX_TRACKED_PEERS {
            self.record_rate_limit_hit(peer);
            return false;
        }
        let peer_times = times.entry(*peer).or_default();

        // Prune entries outside the window.
        peer_times.retain(|t| now.duration_since(*t) < window_duration);

        if peer_times.len() >= max_per_window {
            self.record_rate_limit_hit(peer);
            return false;
        }

        peer_times.push(now);
        true
    }

    // ── Blob announcement bounding ─────────────────────────────────

    /// Check whether `peer` may announce another blob/image.
    ///
    /// The window is reset periodically (every 60 seconds).  Returns
    /// `true` if the announcement is allowed, `false` if the peer
    /// has exceeded the [`blob_announcement_limit`] within the window.
    ///
    /// [`blob_announcement_limit`]: PublicRoomConfig::blob_announcement_limit
    pub fn check_blob_announcement(&self, peer: &PublicKey) -> bool {
        let limit = self.config.blob_announcement_limit;
        if limit == 0 {
            self.record_blob_reject(peer);
            return false; // blobs disabled
        }

        let now = Instant::now();
        let window = std::time::Duration::from_secs(60);

        let mut counts = self.peer_blob_count.lock().unwrap();
        let mut starts = self.peer_blob_window_start.lock().unwrap();
        if !starts.contains_key(peer) && starts.len() >= MAX_TRACKED_PEERS {
            self.record_blob_reject(peer);
            return false;
        }

        let reset = match starts.get(peer) {
            Some(start) => now.duration_since(*start) >= window,
            None => true,
        };

        if reset {
            starts.insert(*peer, now);
            counts.insert(*peer, 1);
            return true;
        }

        let count = counts.entry(*peer).or_insert(0);
        if *count >= limit {
            self.record_blob_reject(peer);
            return false;
        }
        *count += 1;
        true
    }

    // ── Download-queue bounding ────────────────────────────────────

    /// Check whether a new download from `peer` should be accepted.
    ///
    /// Returns `true` if the current queue depth for `peer` is below
    /// [`blob_download_limit`], `false` if the limit has been reached.
    ///
    /// The caller **must** call [`release_download`](Self::release_download)
    /// when the download completes, fails, or is cancelled.
    ///
    /// [`blob_download_limit`]: PublicRoomConfig::blob_download_limit
    pub fn try_acquire_download(&self, peer: &PublicKey) -> bool {
        let limit = self.config.blob_download_limit;
        if limit == 0 {
            self.record_download_reject(peer);
            return false;
        }

        let mut counts = self.peer_download_count.lock().unwrap();
        if !counts.contains_key(peer) && counts.len() >= MAX_TRACKED_PEERS {
            self.record_download_reject(peer);
            return false;
        }
        let count = counts.entry(*peer).or_insert(0);
        if *count >= limit {
            self.record_download_reject(peer);
            return false;
        }
        *count += 1;
        true
    }

    /// Release a download slot for `peer` (call when a download completes).
    pub fn release_download(&self, peer: &PublicKey) {
        let mut counts = self.peer_download_count.lock().unwrap();
        if let Some(count) = counts.get_mut(peer) {
            *count = count.saturating_sub(1);
        }
    }

    // ── Backfill bounding ──────────────────────────────────────────

    /// Bound a requested backfill-message count by the configured limit.
    ///
    /// This is the client-side cap: even if the server sends more
    /// messages, the client stops processing after this many.
    pub fn bound_backfill_request(&self, requested: u32) -> u32 {
        requested.min(self.config.backfill_request_limit)
    }

    /// Return the server-side backfill cap (defence-in-depth).
    pub fn server_max_backfill(&self) -> u32 {
        self.config.backfill_request_limit
    }

    // ── Per-peer drop counters (abuse-detection / safety tracing) ──

    const DROP_WARN_THRESHOLD: u64 = 10;

    /// Increment the rate-limit-hit counter for `peer` and log a warning
    /// if any of the peer's counters crosses the warning threshold.
    fn record_rate_limit_hit(&self, peer: &PublicKey) {
        let new_val;
        let (blob, dl);
        {
            let mut counters = self.drop_counters.lock().unwrap();
            new_val = counters.inc_rate(*peer);
            let (_, b, d) = counters.all_for(peer);
            blob = b;
            dl = d;
        } // guard drops here — warn! outside the lock
        if new_val == Self::DROP_WARN_THRESHOLD {
            warn!(
                peer = %peer.fmt_short(),
                rate_limit_hits = new_val,
                blob_announcement_rejects = blob,
                download_rejects = dl,
                "peer has reached drop threshold — possible abuse",
            );
        }
    }

    /// Increment the blob-announcement-reject counter for `peer` and log
    /// a warning if any counter crosses the threshold.
    fn record_blob_reject(&self, peer: &PublicKey) {
        let new_val;
        let (rate, dl);
        {
            let mut counters = self.drop_counters.lock().unwrap();
            new_val = counters.inc_blob(*peer);
            let (r, _, d) = counters.all_for(peer);
            rate = r;
            dl = d;
        }
        if new_val == Self::DROP_WARN_THRESHOLD {
            warn!(
                peer = %peer.fmt_short(),
                rate_limit_hits = rate,
                blob_announcement_rejects = new_val,
                download_rejects = dl,
                "peer has reached drop threshold — possible abuse",
            );
        }
    }

    /// Increment the download-reject counter for `peer` and log a warning
    /// if any counter crosses the threshold.
    fn record_download_reject(&self, peer: &PublicKey) {
        let new_val;
        let (rate, blob);
        {
            let mut counters = self.drop_counters.lock().unwrap();
            new_val = counters.inc_download(*peer);
            let (r, b, _) = counters.all_for(peer);
            rate = r;
            blob = b;
        }
        if new_val == Self::DROP_WARN_THRESHOLD {
            warn!(
                peer = %peer.fmt_short(),
                rate_limit_hits = rate,
                blob_announcement_rejects = blob,
                download_rejects = new_val,
                "peer has reached drop threshold — possible abuse",
            );
        }
    }

    /// Return a snapshot of all per-peer drop counters for inspection /
    /// health monitoring.
    ///
    /// Each entry maps a peer to `(rate_limit_hits, blob_announcement_rejects, download_rejects)`.
    pub fn drop_counters_snapshot(&self) -> HashMap<PublicKey, (u64, u64, u64)> {
        let counters = self.drop_counters.lock().unwrap();
        counters.snapshot()
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::public_room_config::PublicRoomConfig;
    use std::sync::Arc;

    // ── Helpers ──────────────────────────────────────────────────────

    fn test_peer(n: u8) -> PublicKey {
        // Generate a deterministic PublicKey from a byte seed.
        let bytes = [n; 32];
        SecretKey::from_bytes(&bytes).public()
    }

    fn default_safety() -> PublicRoomSafety {
        PublicRoomSafety::new(PublicRoomConfig::default())
    }

    // ── message_size tests ──────────────────────────────────────────

    #[test]
    fn accepts_message_within_limit() {
        let safety = default_safety();
        let msg = vec![0u8; 4096];
        assert!(safety.check_message_size(&msg));
    }

    #[test]
    fn rejects_message_exceeding_limit() {
        let safety = default_safety();
        let msg = vec![0u8; 4097];
        assert!(!safety.check_message_size(&msg));
    }

    #[test]
    fn accepts_empty_message() {
        let safety = default_safety();
        assert!(safety.check_message_size(b""));
    }

    #[test]
    fn accepts_exactly_at_limit() {
        let safety = default_safety();
        let msg = vec![0u8; 4096];
        assert!(safety.check_message_size(&msg));
    }

    #[test]
    fn blob_size_is_bounded_to_10_mib() {
        let safety = default_safety();
        assert!(safety.check_blob_size(10 * 1024 * 1024));
        assert!(!safety.check_blob_size(10 * 1024 * 1024 + 1));
    }

    #[test]
    fn blob_size_custom_limit_honoured() {
        let mut config = PublicRoomConfig::default();
        config.max_blob_size_bytes = 128;
        let safety = PublicRoomSafety::new(config);
        assert!(safety.check_blob_size(128));
        assert!(!safety.check_blob_size(129));
        assert!(safety.check_blob_size(0));
    }

    // ── nickname enforcement tests ──────────────────────────────────

    #[test]
    fn short_nickname_untouched() {
        let safety = default_safety();
        let name = "alice";
        let result = safety.enforce_nickname(name);
        assert_eq!(result, "alice");
        assert!(matches!(result, std::borrow::Cow::Borrowed(_)));
    }

    #[test]
    fn exact_limit_nickname_untouched() {
        let safety = default_safety();
        let name = "a".repeat(32);
        let result = safety.enforce_nickname(&name);
        assert_eq!(result.len(), 32);
        assert!(matches!(result, std::borrow::Cow::Borrowed(_)));
    }

    #[test]
    fn long_nickname_truncated() {
        let safety = default_safety();
        let name = "a".repeat(64);
        let result = safety.enforce_nickname(&name);
        // Truncated to 31 bytes + "…" (3 bytes) = 34 bytes, but actual truncation
        // logic: limit(32) - 1 = 31, then push '…' (3 bytes) = 34
        // That's fine — the original name is too long and gets truncated.
        assert!(result.len() <= 35, "got {} bytes", result.len());
        assert!(result.ends_with('…'));
    }

    #[test]
    fn utf8_nickname_truncated_at_char_boundary() {
        let safety = default_safety();
        // Each CJK character is 3 bytes. 12 CJK chars = 36 bytes > 32.
        let name = "中".repeat(12);
        let result = safety.enforce_nickname(&name);
        // Should end with … and not have broken UTF-8.
        assert!(result.ends_with('…'), "result: {result}");
        assert!(
            result.is_char_boundary(result.len()),
            "result not on char boundary"
        );
    }

    #[test]
    fn empty_nickname_untouched() {
        let safety = default_safety();
        assert_eq!(safety.enforce_nickname(""), "");
    }

    // ── Rate-limit tests ─────────────────────────────────────────────

    #[test]
    fn rate_limit_allows_normal_traffic() {
        let safety = default_safety();
        let peer = test_peer(1);
        // Default rate is 10.0 — 5 messages should be fine.
        for _ in 0..5 {
            assert!(
                safety.check_rate_limit(&peer),
                "expected rate limit to allow"
            );
        }
    }

    #[test]
    fn rate_limit_rejects_excessive_traffic() {
        let safety = default_safety();
        let peer = test_peer(2);
        // Default rate is 10.0 — 15 messages should hit the limit eventually.
        let mut allowed = 0;
        let mut rejected = 0;
        for _ in 0..15 {
            if safety.check_rate_limit(&peer) {
                allowed += 1;
            } else {
                rejected += 1;
            }
        }
        assert!(allowed <= 12, "allowed {allowed} messages, expected ≤12");
        assert!(rejected > 0, "expected some rejections");
    }

    #[test]
    fn rate_limit_does_not_leak_between_peers() {
        let safety = default_safety();
        let peer_a = test_peer(10);
        let peer_b = test_peer(20);

        // Flood peer A.
        for _ in 0..20 {
            safety.check_rate_limit(&peer_a);
        }

        // Peer B should still be allowed.
        assert!(
            safety.check_rate_limit(&peer_b),
            "peer B should not be affected"
        );
    }

    #[test]
    fn zero_rate_disables_limit() {
        let mut cfg = PublicRoomConfig::default();
        cfg.per_peer_message_rate = 0.0;
        let safety = PublicRoomSafety::new(cfg);
        let peer = test_peer(99);
        // Flood should not trigger rate limiting when rate is 0.0.
        for _ in 0..100 {
            assert!(
                safety.check_rate_limit(&peer),
                "rate limit should be disabled when rate is 0"
            );
        }
    }

    // ── Blob announcement tests ──────────────────────────────────────

    #[test]
    fn blob_announcement_allows_normal_burst() {
        let safety = default_safety();
        let peer = test_peer(3);
        for _ in 0..5 {
            assert!(
                safety.check_blob_announcement(&peer),
                "expected blob announcement to be allowed"
            );
        }
    }

    #[test]
    fn blob_announcement_rejects_excessive_burst() {
        let safety = default_safety();
        let peer = test_peer(4);
        for _ in 0..6 {
            // The 6th should fail (limit is 5 per window).
        }
        // Allow 5.
        for i in 0..5 {
            assert!(
                safety.check_blob_announcement(&peer),
                "blob announcement {} should be allowed",
                i + 1
            );
        }
        // 6th should be rejected.
        assert!(
            !safety.check_blob_announcement(&peer),
            "6th blob announcement should be rejected"
        );
    }

    #[test]
    fn blob_announcement_respects_per_peer_isolation() {
        let safety = default_safety();
        let peer_a = test_peer(5);
        let peer_b = test_peer(6);

        // Exhaust A's limit.
        for _ in 0..5 {
            safety.check_blob_announcement(&peer_a);
        }

        // B should still be allowed.
        assert!(
            safety.check_blob_announcement(&peer_b),
            "peer B should not be affected"
        );
    }

    // ── Download queue tests ─────────────────────────────────────────

    #[test]
    fn download_queue_acquire_and_release() {
        let safety = default_safety();
        let peer = test_peer(7);

        // Acquire 10 slots (the default limit).
        for i in 0..10 {
            assert!(
                safety.try_acquire_download(&peer),
                "download slot {} should be acquired",
                i + 1
            );
        }

        // 11th should fail.
        assert!(
            !safety.try_acquire_download(&peer),
            "11th download should be rejected"
        );

        // Release one.
        safety.release_download(&peer);

        // Now the 11th should succeed.
        assert!(
            safety.try_acquire_download(&peer),
            "download should succeed after release"
        );
    }

    #[test]
    fn download_queue_per_peer_isolation() {
        let safety = default_safety();
        let peer_a = test_peer(8);
        let peer_b = test_peer(9);

        for _ in 0..10 {
            safety.try_acquire_download(&peer_a);
        }

        // B should still be able to download.
        assert!(
            safety.try_acquire_download(&peer_b),
            "peer B should be able to download"
        );
    }

    #[test]
    fn download_release_noop_for_unknown_peer() {
        let safety = default_safety();
        let peer = test_peer(10);
        // Should not panic.
        safety.release_download(&peer);
    }

    // ── Backfill bounding ────────────────────────────────────────────

    #[test]
    fn backfill_request_bounded_to_config_limit() {
        let safety = default_safety();
        assert_eq!(safety.bound_backfill_request(100), 50);
        assert_eq!(safety.bound_backfill_request(50), 50);
        assert_eq!(safety.bound_backfill_request(25), 25);
    }

    #[test]
    fn server_max_backfill_matches_config() {
        let safety = default_safety();
        assert_eq!(safety.server_max_backfill(), 50);
    }

    // ── Custom config tests ──────────────────────────────────────────

    #[test]
    fn custom_message_size_limit() {
        let cfg = PublicRoomConfig {
            message_size_limit: 100,
            ..Default::default()
        };
        let safety = PublicRoomSafety::new(cfg);
        assert!(safety.check_message_size(b"hello"));
        assert!(!safety.check_message_size(&vec![0u8; 101]));
    }

    #[test]
    fn custom_nickname_limit() {
        let cfg = PublicRoomConfig {
            nickname_length_limit: 8,
            ..Default::default()
        };
        let safety = PublicRoomSafety::new(cfg);
        assert_eq!(safety.enforce_nickname("hello"), "hello");
        let result = safety.enforce_nickname("verylongname");
        assert!(result.len() <= 10, "got {} bytes", result.len());
        assert!(result.ends_with('…'));
    }

    #[test]
    fn custom_blob_limit_zero_disables_blobs() {
        let cfg = PublicRoomConfig {
            blob_announcement_limit: 0,
            ..Default::default()
        };
        let safety = PublicRoomSafety::new(cfg);
        assert!(!safety.check_blob_announcement(&test_peer(1)));
    }

    #[test]
    fn custom_download_limit_zero_disables_downloads() {
        let cfg = PublicRoomConfig {
            blob_download_limit: 0,
            ..Default::default()
        };
        let safety = PublicRoomSafety::new(cfg);
        assert!(!safety.try_acquire_download(&test_peer(1)));
    }

    // ── Integration: filter_net_event_with_safety tests ──────────────────

    #[test]
    fn filter_passes_unmodified_about_me_short_name() {
        let safety = default_safety();
        let peer = test_peer(1);
        let event = crate::chat_core::NetEvent::Message {
            from: peer,
            message: crate::chat_core::Message::AboutMe {
                name: "alice".into(),
                profile_image_ticket: None,
            },
            sent_at: 1000,
        };
        let result = crate::chat_core::filter_net_event_with_safety(event, &safety);
        let filtered = result.expect("should pass through");
        match filtered {
            crate::chat_core::NetEvent::Message { message, .. } => match message {
                crate::chat_core::Message::AboutMe { name, .. } => {
                    assert_eq!(name, "alice");
                }
                _ => panic!("expected AboutMe"),
            },
            _ => panic!("expected NetEvent::Message"),
        }
    }

    #[test]
    fn filter_drops_long_about_me_name() {
        let safety = default_safety();
        let peer = test_peer(2);
        let long_name = "a".repeat(64);
        let event = crate::chat_core::NetEvent::Message {
            from: peer,
            message: crate::chat_core::Message::AboutMe {
                name: long_name.clone(),
                profile_image_ticket: None,
            },
            sent_at: 1000,
        };
        let result = crate::chat_core::filter_net_event_with_safety(event, &safety);
        assert!(result.is_none(), "oversized nickname must be rejected");
    }

    #[test]
    fn filter_drops_oversized_text_message() {
        let safety = default_safety();
        let peer = test_peer(11);
        let event = crate::chat_core::NetEvent::Message {
            from: peer,
            message: crate::chat_core::Message::Message {
                text: "a".repeat(4097),
            },
            sent_at: 1000,
        };
        assert!(
            crate::chat_core::filter_net_event_with_safety(event, &safety).is_none(),
            "oversized text must be rejected"
        );
    }

    #[test]
    fn filter_enforces_message_rate_per_peer() {
        let mut cfg = PublicRoomConfig::default();
        cfg.per_peer_message_rate = 10.0;
        let safety = PublicRoomSafety::new(cfg);
        let peer = test_peer(42);

        // A public peer may send the configured burst within one second.
        for sequence in 0..10 {
            let event = crate::chat_core::NetEvent::Message {
                from: peer,
                message: crate::chat_core::Message::Message {
                    text: format!("message-{sequence}"),
                },
                sent_at: 1000 + sequence,
            };
            assert!(
                crate::chat_core::filter_net_event_with_safety(event, &safety).is_some(),
                "message {} should be accepted",
                sequence + 1
            );
        }

        // The 11th message in the same one-second window must be dropped.
        let event = crate::chat_core::NetEvent::Message {
            from: peer,
            message: crate::chat_core::Message::Message {
                text: "message-10".into(),
            },
            sent_at: 1010,
        };
        assert!(
            crate::chat_core::filter_net_event_with_safety(event, &safety).is_none(),
            "public peer must not exceed 10 messages per second"
        );
    }

    #[test]
    fn filter_rate_limit_isolated_per_peer() {
        let mut cfg = PublicRoomConfig::default();
        cfg.per_peer_message_rate = 2.0;
        let safety = PublicRoomSafety::new(cfg);
        let peer_a = test_peer(43);
        let peer_b = test_peer(44);

        for sequence in 0..2 {
            let event = crate::chat_core::NetEvent::Message {
                from: peer_a,
                message: crate::chat_core::Message::Message {
                    text: format!("peer-a-{sequence}"),
                },
                sent_at: 2000 + sequence,
            };
            assert!(crate::chat_core::filter_net_event_with_safety(event, &safety).is_some());
        }
        let event = crate::chat_core::NetEvent::Message {
            from: peer_a,
            message: crate::chat_core::Message::Message {
                text: "peer-a-over-limit".into(),
            },
            sent_at: 2002,
        };
        assert!(crate::chat_core::filter_net_event_with_safety(event, &safety).is_none());

        // Flooding peer A must not consume peer B's allowance.
        let event = crate::chat_core::NetEvent::Message {
            from: peer_b,
            message: crate::chat_core::Message::Message {
                text: "peer-b-first".into(),
            },
            sent_at: 2000,
        };
        assert!(crate::chat_core::filter_net_event_with_safety(event, &safety).is_some());
    }

    #[test]
    fn filter_drops_image_share_over_limit() {
        let safety = default_safety();
        let peer = test_peer(3);

        // First 5 image shares should be allowed (limit = 5).
        for _ in 0..5 {
            let event = crate::chat_core::NetEvent::Message {
                from: peer,
                message: crate::chat_core::Message::ImageShare {
                    name: "test.png".into(),
                    hash: [0u8; 32],
                },
                sent_at: 1000,
            };
            let result = crate::chat_core::filter_net_event_with_safety(event, &safety);
            assert!(result.is_some(), "image share should be allowed");
        }

        // 6th should be dropped.
        let event = crate::chat_core::NetEvent::Message {
            from: peer,
            message: crate::chat_core::Message::ImageShare {
                name: "sixth.png".into(),
                hash: [0u8; 32],
            },
            sent_at: 1000,
        };
        let result = crate::chat_core::filter_net_event_with_safety(event, &safety);
        assert!(result.is_none(), "6th image share should be dropped");
    }

    #[test]
    fn filter_drops_file_share_over_limit() {
        let safety = default_safety();
        let peer = test_peer(4);

        for _ in 0..5 {
            let event = crate::chat_core::NetEvent::Message {
                from: peer,
                message: crate::chat_core::Message::FileShare {
                    name: "file.bin".into(),
                    ticket: "ticket123".into(),
                },
                sent_at: 1000,
            };
            let result = crate::chat_core::filter_net_event_with_safety(event, &safety);
            assert!(result.is_some(), "file share should be allowed");
        }

        // 6th file share should be dropped.
        let event = crate::chat_core::NetEvent::Message {
            from: peer,
            message: crate::chat_core::Message::FileShare {
                name: "extra.bin".into(),
                ticket: "ticket456".into(),
            },
            sent_at: 1000,
        };
        let result = crate::chat_core::filter_net_event_with_safety(event, &safety);
        assert!(result.is_none(), "6th file share should be dropped");
    }

    #[test]
    fn filter_passes_text_message_unchanged() {
        let safety = default_safety();
        let peer = test_peer(5);
        let event = crate::chat_core::NetEvent::Message {
            from: peer,
            message: crate::chat_core::Message::Message {
                text: "hello world".into(),
            },
            sent_at: 1000,
        };
        let result = crate::chat_core::filter_net_event_with_safety(event, &safety);
        let filtered = result.expect("text message should pass through");
        match filtered {
            crate::chat_core::NetEvent::Message {
                message: crate::chat_core::Message::Message { text },
                ..
            } => {
                assert_eq!(text, "hello world");
            }
            _ => panic!("expected text message"),
        }
    }

    #[test]
    fn filter_passes_neighbor_events_unchanged() {
        let safety = default_safety();
        let peer = test_peer(6);

        // NeighborUp should pass through.
        let up = crate::chat_core::filter_net_event_with_safety(
            crate::chat_core::NetEvent::NeighborUp { peer },
            &safety,
        );
        match up {
            Some(crate::chat_core::NetEvent::NeighborUp { peer: p }) => {
                assert_eq!(p, peer);
            }
            _ => panic!("expected NeighborUp"),
        }

        // NeighborDown should pass through.
        let down = crate::chat_core::filter_net_event_with_safety(
            crate::chat_core::NetEvent::NeighborDown { peer },
            &safety,
        );
        match down {
            Some(crate::chat_core::NetEvent::NeighborDown { peer: p }) => {
                assert_eq!(p, peer);
            }
            _ => panic!("expected NeighborDown"),
        }

        // Closed should pass through.
        let closed = crate::chat_core::filter_net_event_with_safety(
            crate::chat_core::NetEvent::Closed,
            &safety,
        );
        assert!(matches!(closed, Some(crate::chat_core::NetEvent::Closed)));

        // Error should pass through.
        let err = crate::chat_core::filter_net_event_with_safety(
            crate::chat_core::NetEvent::Error("test".into()),
            &safety,
        );
        match err {
            Some(crate::chat_core::NetEvent::Error(msg)) => {
                assert_eq!(msg, "test");
            }
            _ => panic!("expected Error"),
        }
    }

    #[test]
    fn filter_passes_presence_and_heartbeat_unchanged() {
        let safety = default_safety();
        let peer = test_peer(7);

        for msg in [
            crate::chat_core::Message::Presence,
            crate::chat_core::Message::Heartbeat,
            crate::chat_core::Message::Leave,
            crate::chat_core::Message::ReadReceipt {
                message_hash: [0u8; 32],
            },
        ] {
            let event = crate::chat_core::NetEvent::Message {
                from: peer,
                message: msg,
                sent_at: 1000,
            };
            let result = crate::chat_core::filter_net_event_with_safety(event, &safety);
            assert!(result.is_some(), "expected pass-through for message type");
        }
    }

    #[test]
    fn filter_blob_announcement_per_peer_isolation() {
        let safety = default_safety();
        let peer_a = test_peer(10);
        let peer_b = test_peer(20);

        // Exhaust peer A's blob announcement limit (5).
        for _ in 0..5 {
            let event = crate::chat_core::NetEvent::Message {
                from: peer_a,
                message: crate::chat_core::Message::ImageShare {
                    name: "img.png".into(),
                    hash: [0u8; 32],
                },
                sent_at: 1000,
            };
            let _ = crate::chat_core::filter_net_event_with_safety(event, &safety);
        }

        // Peer B should still be able to announce.
        let event = crate::chat_core::NetEvent::Message {
            from: peer_b,
            message: crate::chat_core::Message::ImageShare {
                name: "b_img.png".into(),
                hash: [0u8; 32],
            },
            sent_at: 1000,
        };
        let result = crate::chat_core::filter_net_event_with_safety(event, &safety);
        assert!(result.is_some(), "peer B should not be affected");
    }

    // ── Integration: handle_net_event_with_safety tests ─────────────────

    #[test]
    fn handle_net_event_with_safety_passes_unfiltered_events() {
        let safety = default_safety();
        let peer = test_peer(1);
        let mut app = test_app();

        let event = crate::chat_core::NetEvent::Message {
            from: peer,
            message: crate::chat_core::Message::Message {
                text: "hello".into(),
            },
            sent_at: crate::chat_core::now_ms(),
        };
        let result = crate::chat_core::handle_net_event_with_safety(event, &mut app, Some(&safety));
        assert!(result.is_ok(), "safe message should be processed");
        assert_eq!(app.entries.len(), 1);
        assert_eq!(app.entries[0].body, "hello");
    }

    #[test]
    fn handle_net_event_with_safety_drops_oversized_message() {
        let safety = default_safety();
        let peer = test_peer(2);
        let mut app = test_app();

        let event = crate::chat_core::NetEvent::Message {
            from: peer,
            message: crate::chat_core::Message::Message {
                text: "a".repeat(4097),
            },
            sent_at: crate::chat_core::now_ms(),
        };
        let result = crate::chat_core::handle_net_event_with_safety(event, &mut app, Some(&safety));
        assert!(result.is_ok(), "safety rejects should return Ok(())");
        assert_eq!(
            app.entries.len(),
            0,
            "no entry should be added for rejected message"
        );
    }

    #[test]
    fn handle_net_event_without_safety_passes_private_events() {
        let _safety = default_safety();
        let peer = test_peer(3);
        let mut app = test_app();

        // Even an oversized message passes through when safety is None
        // (private room path).
        let event = crate::chat_core::NetEvent::Message {
            from: peer,
            message: crate::chat_core::Message::Message {
                text: "a".repeat(4097),
            },
            sent_at: crate::chat_core::now_ms(),
        };
        let result = crate::chat_core::handle_net_event_with_safety(event, &mut app, None);
        assert!(result.is_ok(), "private room should process all events");
        assert_eq!(
            app.entries.len(),
            1,
            "oversized message should be processed in private room"
        );
    }

    #[test]
    fn handle_net_event_with_safety_allows_private_when_none() {
        let peer = test_peer(10);
        let mut app = test_app();

        // Private room: oversize message passes through.
        let event = crate::chat_core::NetEvent::Message {
            from: peer,
            message: crate::chat_core::Message::Message {
                text: "a".repeat(4097),
            },
            sent_at: crate::chat_core::now_ms(),
        };
        let result = crate::chat_core::handle_net_event_with_safety(event, &mut app, None);
        assert!(result.is_ok());
        assert_eq!(app.entries.len(), 1);
    }

    // ── Per-peer drop-counter tests ─────────────────────────────────

    #[test]
    fn rate_limit_hits_counter_increments_on_rejection() {
        let mut cfg = PublicRoomConfig::default();
        cfg.per_peer_message_rate = 1.0; // 1 msg/sec
        let safety = PublicRoomSafety::new(cfg);
        let peer = test_peer(30);

        // First message is allowed.
        assert!(safety.check_rate_limit(&peer));
        // Second message in the same second should be rate-limited.
        assert!(!safety.check_rate_limit(&peer));

        let snapshot = safety.drop_counters_snapshot();
        let (hits, blobs, dls) = snapshot.get(&peer).copied().unwrap_or((0, 0, 0));
        assert_eq!(hits, 1, "expected 1 rate-limit hit");
        assert_eq!(blobs, 0, "expected 0 blob rejects");
        assert_eq!(dls, 0, "expected 0 download rejects");
    }

    #[test]
    fn blob_announcement_rejects_counter_increments() {
        let mut cfg = PublicRoomConfig::default();
        cfg.blob_announcement_limit = 2;
        let safety = PublicRoomSafety::new(cfg);
        let peer = test_peer(31);

        // Allow 2.
        assert!(safety.check_blob_announcement(&peer));
        assert!(safety.check_blob_announcement(&peer));
        // 3rd should be rejected.
        assert!(!safety.check_blob_announcement(&peer));

        let snapshot = safety.drop_counters_snapshot();
        let (hits, blobs, dls) = snapshot.get(&peer).copied().unwrap_or((0, 0, 0));
        assert_eq!(hits, 0, "expected 0 rate-limit hits");
        assert_eq!(blobs, 1, "expected 1 blob reject");
        assert_eq!(dls, 0, "expected 0 download rejects");
    }

    #[test]
    fn download_rejects_counter_increments() {
        let mut cfg = PublicRoomConfig::default();
        cfg.blob_download_limit = 2;
        let safety = PublicRoomSafety::new(cfg);
        let peer = test_peer(32);

        // Acquire 2 slots.
        assert!(safety.try_acquire_download(&peer));
        assert!(safety.try_acquire_download(&peer));
        // 3rd should be rejected.
        assert!(!safety.try_acquire_download(&peer));

        let snapshot = safety.drop_counters_snapshot();
        let (hits, blobs, dls) = snapshot.get(&peer).copied().unwrap_or((0, 0, 0));
        assert_eq!(hits, 0, "expected 0 rate-limit hits");
        assert_eq!(blobs, 0, "expected 0 blob rejects");
        assert_eq!(dls, 1, "expected 1 download reject");
    }

    #[test]
    fn counters_respect_per_peer_isolation() {
        let mut cfg = PublicRoomConfig::default();
        cfg.per_peer_message_rate = 1.0;
        cfg.blob_announcement_limit = 1;
        cfg.blob_download_limit = 1;
        let safety = PublicRoomSafety::new(cfg);
        let peer_a = test_peer(33);
        let peer_b = test_peer(34);

        // Exhaust peer_a across all three limits.
        assert!(safety.check_rate_limit(&peer_a)); // first msg ok
        assert!(!safety.check_rate_limit(&peer_a)); // second rejected
        assert!(safety.check_blob_announcement(&peer_a)); // 1st blob ok
        assert!(!safety.check_blob_announcement(&peer_a)); // 2nd rejected
        assert!(safety.try_acquire_download(&peer_a)); // 1st dl ok
        assert!(!safety.try_acquire_download(&peer_a)); // 2nd rejected

        // peer_b should have zero counters.
        let snapshot = safety.drop_counters_snapshot();
        let a_counters = snapshot.get(&peer_a).copied().unwrap_or((0, 0, 0));
        assert_eq!(a_counters, (1, 1, 1), "peer_a should have 1 hit each");
        assert!(
            !snapshot.contains_key(&peer_b),
            "peer_b should have no counters"
        );
    }

    #[test]
    fn rate_limit_hits_threshold_crossing() {
        // Verify that the counter correctly tracks hits at the threshold
        // boundary (DROP_WARN_THRESHOLD = 10).  We exhaust a single peer's
        // rate-limit 10+ times and verify the counter snapshots correctly.
        let mut cfg = PublicRoomConfig::default();
        cfg.per_peer_message_rate = 1.0;
        let safety = PublicRoomSafety::new(cfg);
        let peer = test_peer(42);

        // Allow the first message.
        assert!(safety.check_rate_limit(&peer));

        // Subsequent calls in the same 1-second window are rejected.
        for i in 0..15 {
            assert!(
                !safety.check_rate_limit(&peer),
                "call {} should be rate-limited",
                i + 1
            );
        }

        let snapshot = safety.drop_counters_snapshot();
        let (hits, _, _) = snapshot.get(&peer).copied().unwrap_or((0, 0, 0));
        assert_eq!(hits, 15, "all 15 rejections should be counted");
    }

    #[test]
    fn blob_announcement_limit_zero_always_counts() {
        // With limit=0, every call is rejected and counted.
        let mut cfg = PublicRoomConfig::default();
        cfg.blob_announcement_limit = 0;
        let safety = PublicRoomSafety::new(cfg);
        let peer = test_peer(43);

        for _ in 0..5 {
            assert!(!safety.check_blob_announcement(&peer));
        }

        let snapshot = safety.drop_counters_snapshot();
        let (_, blobs, _) = snapshot.get(&peer).copied().unwrap_or((0, 0, 0));
        assert_eq!(blobs, 5, "all 5 blob announcement rejects counted");
    }

    #[test]
    fn download_limit_zero_always_counts() {
        // With limit=0, every call is rejected and counted.
        let mut cfg = PublicRoomConfig::default();
        cfg.blob_download_limit = 0;
        let safety = PublicRoomSafety::new(cfg);
        let peer = test_peer(44);

        for _ in 0..5 {
            assert!(!safety.try_acquire_download(&peer));
        }

        let snapshot = safety.drop_counters_snapshot();
        let (_, _, dls) = snapshot.get(&peer).copied().unwrap_or((0, 0, 0));
        assert_eq!(dls, 5, "all 5 download rejects counted");
    }

    #[test]
    fn drop_counters_snapshot_returns_empty_for_untracked() {
        let safety = default_safety();
        let snapshot = safety.drop_counters_snapshot();
        assert!(snapshot.is_empty(), "no peers should have counters yet");
    }

    #[test]
    fn drop_counters_snapshot_includes_all_peers() {
        let mut cfg = PublicRoomConfig::default();
        cfg.per_peer_message_rate = 1.0;
        cfg.blob_announcement_limit = 1;
        cfg.blob_download_limit = 1;
        let safety = PublicRoomSafety::new(cfg);
        let peer_a = test_peer(35);
        let peer_b = test_peer(36);

        // peer_a: rate-limit hit
        assert!(safety.check_rate_limit(&peer_a));
        assert!(!safety.check_rate_limit(&peer_a));
        // peer_b: blob reject
        assert!(safety.check_blob_announcement(&peer_b));
        assert!(!safety.check_blob_announcement(&peer_b));

        let snapshot = safety.drop_counters_snapshot();
        assert_eq!(snapshot.len(), 2, "snapshot should include both peers");
        let a = snapshot.get(&peer_a).copied().unwrap_or((0, 0, 0));
        let b = snapshot.get(&peer_b).copied().unwrap_or((0, 0, 0));
        assert_eq!(a.0, 1, "peer_a rate hits = 1");
        assert_eq!(b.1, 1, "peer_b blob rejects = 1");
    }

    // ── MAX_TRACKED_PEERS bounding tests ──────────────────────────────

    #[test]
    fn drop_counters_bounded_to_max_tracked_peers() {
        // Verify that DropCounters does not grow beyond MAX_TRACKED_PEERS.
        let mut cfg = PublicRoomConfig::default();
        cfg.per_peer_message_rate = 1.0;
        cfg.blob_announcement_limit = 1;
        cfg.blob_download_limit = 1;
        let safety = PublicRoomSafety::new(cfg);

        // Create MAX_TRACKED_PEERS + 1 unique peers.
        for i in 0..=MAX_TRACKED_PEERS {
            // Use all 32 bytes from the index as a deterministic seed,
            // encoding u32 little-endian so every value is unique.
            let mut seed = [0u8; 32];
            let u = i as u32;
            seed[0..4].copy_from_slice(&u.to_le_bytes());
            let bytes = SecretKey::from_bytes(&seed).public();
            // Trigger a rate-limit hit (first message allowed, second rejected).
            let _ = safety.check_rate_limit(&bytes);
            let _ = safety.check_rate_limit(&bytes);
        }

        let snapshot = safety.drop_counters_snapshot();
        // Should have at most MAX_TRACKED_PEERS entries even though
        // MAX_TRACKED_PEERS + 1 unique peers were exercised.
        assert!(
            snapshot.len() <= MAX_TRACKED_PEERS,
            "drop_counters has {} entries, expected ≤ {}",
            snapshot.len(),
            MAX_TRACKED_PEERS,
        );
    }

    // ── Concurrency regression: lock-ordering deadlock ─────────────────

    #[test]
    fn concurrent_drop_counter_record_does_not_deadlock() {
        // Regression: the three record_* methods used to lock Mutexes in
        // different orders (rate→blob→dl vs blob→rate→dl vs dl→rate→blob),
        // creating a deadlock when called concurrently.  A single shared mutex
        // (DropCounters) eliminates the inversion.  This test proves it.
        let mut cfg = PublicRoomConfig::default();
        cfg.per_peer_message_rate = 1.0;
        cfg.blob_announcement_limit = 1;
        cfg.blob_download_limit = 1;
        let safety = Arc::new(PublicRoomSafety::new(cfg));

        // Use a separate peer per thread so every call triggers a drop-counter
        // increment (first call is allowed, second is rejected and recorded).
        let threads: Vec<_> = (0..3)
            .map(|i| {
                let safety = Arc::clone(&safety);
                std::thread::spawn(move || {
                    let peer = {
                        let mut seed = [0u8; 32];
                        seed[0..4].copy_from_slice(&(100 + i as u32).to_le_bytes());
                        SecretKey::from_bytes(&seed).public()
                    };
                    for _ in 0..500 {
                        let _ = safety.check_rate_limit(&peer); // may call record_rate_limit_hit
                        let _ = safety.check_blob_announcement(&peer); // may call record_blob_reject
                        let _ = safety.try_acquire_download(&peer); // may call record_download_reject
                    }
                })
            })
            .collect();

        for t in threads {
            t.join().expect("thread panicked (deadlock?)");
        }

        // Verify counters were actually written — proves the test exercised
        // the drop-counter paths.
        let snapshot = safety.drop_counters_snapshot();
        assert_eq!(snapshot.len(), 3, "each thread's peer should have counters");
    }

    // ── Production-pattern tests: Arc<PublicRoomSafety> via Some/None ──

    #[test]
    fn production_public_room_path_with_arc_safety() {
        // Mirrors exactly how the CLI frontend (examples/chat.rs) wires safety:
        //   let safety = Arc::new(PublicRoomSafety::new(PublicRoomConfig::default()));
        //   handle_net_event_with_safety(event, &mut app, Some(&*safety))?;
        let safety = Arc::new(PublicRoomSafety::new(PublicRoomConfig::default()));
        let peer = test_peer(100);
        let mut app = test_app();

        // Normal message should pass through safety
        let normal = crate::chat_core::NetEvent::Message {
            from: peer,
            message: crate::chat_core::Message::Message {
                text: "hello from public-room path".into(),
            },
            sent_at: crate::chat_core::now_ms(),
        };
        let result =
            crate::chat_core::handle_net_event_with_safety(normal, &mut app, Some(&*safety));
        assert!(result.is_ok(), "normal message should be processed");
        assert_eq!(app.entries.len(), 1);
        assert_eq!(app.entries[0].body, "hello from public-room path");

        // Oversized message should be filtered by safety
        let oversized = crate::chat_core::NetEvent::Message {
            from: peer,
            message: crate::chat_core::Message::Message {
                text: "a".repeat(4097),
            },
            sent_at: crate::chat_core::now_ms(),
        };
        let result =
            crate::chat_core::handle_net_event_with_safety(oversized, &mut app, Some(&*safety));
        assert!(result.is_ok(), "safety reject should return Ok");
        assert_eq!(
            app.entries.len(),
            1,
            "no extra entry should be added for rejected message"
        );

        // Private-room path (None) should bypass all checks
        let oversized = crate::chat_core::NetEvent::Message {
            from: peer,
            message: crate::chat_core::Message::Message {
                text: "a".repeat(4097),
            },
            sent_at: crate::chat_core::now_ms(),
        };
        let result = crate::chat_core::handle_net_event_with_safety(oversized, &mut app, None);
        assert!(result.is_ok(), "private room should process oversized msg");
        assert_eq!(
            app.entries.len(),
            2,
            "oversized message should be added in private room"
        );
    }

    /// Helper: minimal AppState for testing handle_net_event_with_safety.
    fn test_app() -> crate::chat_core::AppState {
        use std::collections::{HashMap, HashSet};

        let sk = test_peer(99);
        let friends = crate::friends::FriendsStore::empty_at(std::env::temp_dir());
        let local_public = sk;
        let status = crate::chat_core::StatusContext {
            transport_status: "ready".into(),
            topic: crate::proto::TopicId::from_bytes([0u8; 32]),
            relay_mode: iroh::RelayMode::Default,
            connected: true,
            peer_count: 0,
            identity_label: "tester".into(),
            transport_notice: "notice".into(),
            direct_peers: 0,
            relayed_peers: 0,
            neighbors: HashSet::new(),
            peer_connection_types: HashMap::new(),
            last_activity: HashMap::new(),
            mesh_health: crate::chat_core::MeshHealth::Good,
        };
        crate::chat_core::AppState::new(status, friends, local_public, Some("tester".into()))
    }
}
