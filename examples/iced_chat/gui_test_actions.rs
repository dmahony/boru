//! GUI test action types and channel infrastructure.
//!
//! Provides the shared types that bridge MCP-driven test commands into
//! the Iced GUI event loop.  The channel is a bounded tokio mpsc that
//! feeds into an Iced subscription, producing
//! [`AppMessage::GuiTestActionReceived`] for the normal `update()` path.
//!
//! # Security
//!
//! - No secrets (keys, tickets, tokens) are exposed.
//! - Input strings are bounded (max 4096 chars, no control characters).
//! - Rate limiting: max 10 actions/sec, max 100/min.
//! - Queue bounded at 256 pending actions.
//! - History bounded at 1000 recent actions.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;

pub use boru_chat::diagnostics::GuiTestCommand;
use serde::{Deserialize, Serialize};

/// Maximum length of any user-facing string parameter.
pub const MAX_STRING_LEN: usize = 4096;

/// Maximum pending actions in the channel.
pub const MAX_PENDING: usize = 256;

/// Maximum recent actions retained in the action history.
pub const MAX_HISTORY: usize = 1000;

/// Minimum interval between actions (nanos) — corresponds to 10/sec.
pub const MIN_ACTION_INTERVAL_NS: u64 = 100_000_000; // 100ms

/// Maximum actions per rolling 60-second window.
pub const MAX_ACTIONS_PER_MINUTE: usize = 100;

// =============================================================================
// GUI action errors
// =============================================================================

/// Errors that can occur when validating a GUI test command against the
/// current application state.
///
/// These are **semantic** errors — the command is syntactically valid (hex
/// parses, strings are bounded) but fails a state-dependent precondition.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GuiActionError {
    /// The referenced room topic is not known to the application.
    UnknownRoom(String),
    /// The referenced room was deleted from history.
    DeletedRoom(String),
    /// A blocking dialog is currently open (e.g. create-room dialog).
    BlockingDialog,
    /// The referenced conversation (peer) is unknown.
    UnknownConversation(String),
    /// No room is currently active/selected.
    NoActiveRoom,
    /// Composer text exceeds the maximum allowed length.
    TextTooLong { actual: usize, max: usize },
    /// Composer text is empty — nothing to submit.
    EmptyComposer,
    /// The send action is disabled (no subscription or inactive room).
    SendDisabled,
    /// The active conversation is not yet ready (no subscription attached).
    InactiveRoom,
    /// The referenced peer is unknown to the application.
    UnknownPeer(String),
    /// The command is syntactically invalid.
    InvalidCommand(String),
    /// No dialog is currently open — nothing for CloseDialog to close.
    NoDialog,
}

impl std::fmt::Display for GuiActionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            GuiActionError::UnknownRoom(room) => write!(f, "Unknown room: {}", room),
            GuiActionError::DeletedRoom(room) => write!(f, "Room was deleted: {}", room),
            GuiActionError::BlockingDialog => write!(f, "A blocking dialog is currently open"),
            GuiActionError::UnknownConversation(peer) => {
                write!(f, "Unknown conversation peer: {}", peer)
            }
            GuiActionError::NoActiveRoom => write!(f, "No active room selected"),
            GuiActionError::TextTooLong { actual, max } => {
                write!(f, "Text too long ({} chars, max {})", actual, max)
            }
            GuiActionError::EmptyComposer => write!(f, "Composer is empty"),
            GuiActionError::SendDisabled => write!(f, "Sending is disabled"),
            GuiActionError::InactiveRoom => write!(f, "Room is not yet active"),
            GuiActionError::UnknownPeer(peer) => write!(f, "Unknown peer: {}", peer),
            GuiActionError::InvalidCommand(msg) => write!(f, "Invalid command: {}", msg),
            GuiActionError::NoDialog => write!(f, "No dialog is currently open"),
        }
    }
}

/// A structured error returned when a GUI test action cannot be sent
/// through the channel (queue full or closed).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GuiTestSendError {
    /// The action queue is full (at capacity).
    Full { capacity: usize },
    /// The action queue has been closed (application shutting down).
    Closed,
}

impl std::fmt::Display for GuiTestSendError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            GuiTestSendError::Full { capacity } => {
                write!(f, "GUI action queue is full (max {})", capacity)
            }
            GuiTestSendError::Closed => write!(f, "GUI action queue is closed"),
        }
    }
}

// =============================================================================
// Request / Status types
// =============================================================================

/// A complete GUI test action request, with an idempotency key for status
/// tracking.
#[derive(Debug, Clone)]
pub struct GuiActionRequest {
    /// Unique idempotency key for this action (generated by the MCP tool
    /// or provided by the caller).
    pub idempotency_key: String,
    /// The command to execute.
    pub command: GuiTestCommand,
    /// When this request was created (monotonic clock).
    pub created_at: Instant,
}

/// Status of a processed GUI test action.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum ActionStatus {
    /// The action has been queued but not yet processed.
    Queued,
    /// The action was processed successfully.
    Processed,
    /// The action failed with an error.
    Failed { error: String },
    /// The action timed out (wait conditions only).
    TimedOut { elapsed_ms: u64 },
}

/// A recorded action in the action history.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ActionRecord {
    /// Idempotency key for this action.
    pub idempotency_key: String,
    /// Serialized command description.
    pub command: String,
    /// Final status.
    pub status: ActionStatus,
    /// Wall-clock timestamp (ms since epoch).
    pub timestamp_ms: i64,
    /// Processing duration in milliseconds.
    pub duration_ms: u64,
}

// =============================================================================
// Action history
// =============================================================================

/// Bounded ring buffer of recent GUI test actions.
#[derive(Debug, Clone)]
pub struct GuiActionHistory {
    inner: Arc<Mutex<GuiActionHistoryInner>>,
}

#[derive(Debug)]
struct GuiActionHistoryInner {
    records: VecDeque<ActionRecord>,
    /// Counter for unique sequence numbers.
    sequence: u64,
}

impl GuiActionHistory {
    /// Create a new empty action history (bounded at `MAX_HISTORY`).
    pub fn new() -> Self {
        Self {
            inner: Arc::new(Mutex::new(GuiActionHistoryInner {
                records: VecDeque::with_capacity(MAX_HISTORY),
                sequence: 0,
            })),
        }
    }

    /// Record a completed action.
    pub fn record(&self, record: ActionRecord) {
        let mut inner = self.inner.lock().unwrap();
        inner.sequence += 1;
        if inner.records.len() >= MAX_HISTORY {
            inner.records.pop_front();
        }
        inner.records.push_back(record);
    }

    /// Find an action by its idempotency key.
    pub fn find(&self, idempotency_key: &str) -> Option<ActionRecord> {
        let inner = self.inner.lock().unwrap();
        inner
            .records
            .iter()
            .rev()
            .find(|r| r.idempotency_key == idempotency_key)
            .cloned()
    }

    /// Get all recent actions.
    pub fn all(&self) -> Vec<ActionRecord> {
        self.inner
            .lock()
            .unwrap()
            .records
            .iter()
            .rev()
            .cloned()
            .collect()
    }

    /// Get the latest sequence number.
    pub fn latest_sequence(&self) -> u64 {
        self.inner.lock().unwrap().sequence
    }
}

impl Default for GuiActionHistory {
    fn default() -> Self {
        Self::new()
    }
}

// =============================================================================
// Rate limit error
// =============================================================================

/// Structured error returned when a GUI test action is rate-limited.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum RateLimitError {
    /// Exceeded the per-second burst limit (10/sec).
    BurstLimit {
        /// Maximum allowed burst rate (actions per second).
        max_per_sec: usize,
        /// How long to wait before the next action, in milliseconds.
        retry_after_ms: u64,
    },
    /// Exceeded the per-minute sustained limit (100/min).
    MinuteLimit {
        /// Maximum allowed actions per minute.
        max_per_minute: usize,
        /// How long to wait before the rate window resets, in milliseconds.
        retry_after_ms: u64,
    },
}

impl std::fmt::Display for RateLimitError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RateLimitError::BurstLimit {
                max_per_sec,
                retry_after_ms,
            } => write!(
                f,
                "Rate limit exceeded: max {} actions/sec. Retry after ~{}ms",
                max_per_sec, retry_after_ms
            ),
            RateLimitError::MinuteLimit {
                max_per_minute,
                retry_after_ms,
            } => write!(
                f,
                "Rate limit exceeded: max {} actions/min. Retry after ~{}ms",
                max_per_minute, retry_after_ms
            ),
        }
    }
}

// =============================================================================
// Rate limiter
// =============================================================================

/// Simple rate limiter for GUI test actions.
///
/// Enforces two limits:
/// - **Burst**: max [`MIN_ACTION_INTERVAL_NS`] between actions (10/sec).
/// - **Sustained**: max [`MAX_ACTIONS_PER_MINUTE`] actions in a rolling 60s window.
///
/// The limiter is designed to be shared across MCP connections via
/// `Arc<Mutex<GuiActionRateLimiter>>`.
#[derive(Debug)]
pub struct GuiActionRateLimiter {
    /// Timestamps of recent actions (in nanosecond precision).
    recent_actions: VecDeque<Instant>,
}

impl GuiActionRateLimiter {
    /// Create a new rate limiter.
    pub fn new() -> Self {
        Self {
            recent_actions: VecDeque::new(),
        }
    }

    /// Check if an action is allowed. If so, record it and return `Ok(())`.
    /// Otherwise return a structured [`RateLimitError`] with retry timing.
    pub fn check_and_record(&mut self) -> Result<(), RateLimitError> {
        let now = Instant::now();

        // Prune actions older than 60 seconds
        while let Some(&t) = self.recent_actions.front() {
            if now.duration_since(t).as_secs() >= 60 {
                self.recent_actions.pop_front();
            } else {
                break;
            }
        }

        // Check per-minute limit
        if self.recent_actions.len() >= MAX_ACTIONS_PER_MINUTE {
            let oldest = self.recent_actions.front().copied().unwrap_or(now);
            let wait_ms = 60_000 - now.duration_since(oldest).as_millis() as u64;
            return Err(RateLimitError::MinuteLimit {
                max_per_minute: MAX_ACTIONS_PER_MINUTE,
                retry_after_ms: wait_ms,
            });
        }

        // Check per-interval limit (10/sec)
        if let Some(&last) = self.recent_actions.back() {
            let elapsed = now.duration_since(last).as_nanos() as u64;
            if elapsed < MIN_ACTION_INTERVAL_NS {
                let wait_ns = MIN_ACTION_INTERVAL_NS - elapsed;
                return Err(RateLimitError::BurstLimit {
                    max_per_sec: 10,
                    retry_after_ms: wait_ns / 1_000_000,
                });
            }
        }

        self.recent_actions.push_back(now);
        Ok(())
    }
}

impl Default for GuiActionRateLimiter {
    fn default() -> Self {
        Self::new()
    }
}

// =============================================================================
// Channel type aliases
// =============================================================================

/// Sender half — held by the MCP server to inject test actions.
pub type GuiActionSender = tokio::sync::mpsc::Sender<GuiActionRequest>;

/// Receiver half — held by the Iced subscription to read test actions.
pub type GuiActionReceiver = tokio::sync::mpsc::Receiver<GuiActionRequest>;

/// Create a new bounded channel for GUI test actions.
pub fn gui_action_channel() -> (GuiActionSender, GuiActionReceiver) {
    tokio::sync::mpsc::channel(MAX_PENDING)
}

/// Generate a unique idempotency key.
pub fn generate_action_key() -> String {
    static COUNTER: AtomicU64 = AtomicU64::new(1);
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_micros();
    let seq = COUNTER.fetch_add(1, Ordering::Relaxed);
    format!("gui_action_{:x}_{}", now, seq)
}

/// Snapshot of current GUI state, exposed via the MCP `boru_get_gui_snapshot` tool.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GuiSnapshot {
    /// Current screen name.
    pub screen: String,
    /// Whether dark mode is active.
    pub dark_mode: bool,
    /// Whether help is visible.
    pub help_visible: bool,
    /// Current composer text (first 200 chars).
    pub composer_text: String,
    /// Number of chat entries in the active room.
    pub entries_count: usize,
    /// Active room topic (hex), if any.
    pub active_room_topic: Option<String>,
    /// Whether settings screen is visible.
    pub settings_visible: bool,
    /// Whether the app has a valid network connection.
    pub notice: String,
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    // ── RateLimitError tests ──────────────────────────────────────────

    #[test]
    fn test_rate_limit_error_display_burst() {
        let err = RateLimitError::BurstLimit {
            max_per_sec: 10,
            retry_after_ms: 50,
        };
        let msg = err.to_string();
        assert!(msg.contains("Rate limit exceeded"), "msg: {msg}");
        assert!(msg.contains("10"), "msg: {msg}");
        assert!(msg.contains("50ms"), "msg: {msg}");
    }

    #[test]
    fn test_rate_limit_error_display_minute() {
        let err = RateLimitError::MinuteLimit {
            max_per_minute: 100,
            retry_after_ms: 30_000,
        };
        let msg = err.to_string();
        assert!(msg.contains("Rate limit exceeded"), "msg: {msg}");
        assert!(msg.contains("100"), "msg: {msg}");
        assert!(msg.contains("30000ms"), "msg: {msg}");
    }

    #[test]
    fn test_rate_limit_error_serde_json() {
        let err = RateLimitError::BurstLimit {
            max_per_sec: 10,
            retry_after_ms: 75,
        };
        let json = serde_json::to_string(&err).expect("serialize");
        let deser: RateLimitError = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(err, deser);

        // Verify JSON structure
        let val: serde_json::Value = serde_json::from_str(&json).expect("parse");
        assert_eq!(val["type"], "burst_limit");
        assert_eq!(val["max_per_sec"], 10);
        assert_eq!(val["retry_after_ms"], 75);
    }

    #[test]
    fn test_rate_limit_error_serde_json_minute() {
        let err = RateLimitError::MinuteLimit {
            max_per_minute: 100,
            retry_after_ms: 30_000,
        };
        let json = serde_json::to_string(&err).expect("serialize");
        let deser: RateLimitError = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(err, deser);

        let val: serde_json::Value = serde_json::from_str(&json).expect("parse");
        assert_eq!(val["type"], "minute_limit");
        assert_eq!(val["max_per_minute"], 100);
        assert_eq!(val["retry_after_ms"], 30_000);
    }

    // ── Rate limiter constructor tests ─────────────────────────────────

    #[test]
    fn test_rate_limiter_new_is_empty() {
        let limiter = GuiActionRateLimiter::new();
        assert!(limiter.recent_actions.is_empty());
    }

    #[test]
    fn test_rate_limiter_default_is_new() {
        let limiter = GuiActionRateLimiter::default();
        assert!(limiter.recent_actions.is_empty());
    }

    // ── Burst limit tests ──────────────────────────────────────────────

    #[test]
    fn test_rate_limiter_first_action_ok() {
        let mut limiter = GuiActionRateLimiter::new();
        assert!(
            limiter.check_and_record().is_ok(),
            "First action should always pass"
        );
        assert_eq!(limiter.recent_actions.len(), 1);
    }

    #[test]
    fn test_rate_limiter_burst_rejects_fast_action() {
        let mut limiter = GuiActionRateLimiter::new();
        assert!(limiter.check_and_record().is_ok());

        // Immediate second call — should fail burst limit
        let result = limiter.check_and_record();
        assert!(result.is_err(), "Fast second action should be rejected");
        match result.unwrap_err() {
            RateLimitError::BurstLimit {
                max_per_sec,
                retry_after_ms: _,
            } => {
                assert_eq!(max_per_sec, 10);
            }
            other => panic!("Expected BurstLimit, got: {other:?}"),
        }
        // Should still only have 1 recorded action
        assert_eq!(limiter.recent_actions.len(), 1);
    }

    #[test]
    fn test_rate_limiter_burst_allows_after_interval() {
        let mut limiter = GuiActionRateLimiter::new();
        assert!(limiter.check_and_record().is_ok());

        // Wait for burst interval to pass
        std::thread::sleep(Duration::from_millis(100));

        // Now should succeed
        let result = limiter.check_and_record();
        assert!(result.is_ok(), "Action after 100ms should pass: {result:?}");
        assert_eq!(limiter.recent_actions.len(), 2);
    }

    #[test]
    fn test_rate_limiter_burst_rejects_any_fast_action() {
        // Verify that ANY action within 100ms of previous is rejected
        let mut limiter = GuiActionRateLimiter::new();
        assert!(limiter.check_and_record().is_ok());

        // Try 10 more fast calls — all should fail
        for _ in 0..10 {
            let result = limiter.check_and_record();
            assert!(result.is_err(), "Fast action should be rejected");
        }
        // Only the first action should be recorded
        assert_eq!(limiter.recent_actions.len(), 1);
    }

    // ── Minute limit tests (use backdated entries to avoid wall-clock waits) ──

    #[test]
    fn test_rate_limiter_minute_limit_hit() {
        let mut limiter = GuiActionRateLimiter::new();
        let now = Instant::now();

        // Backdate MAX_ACTIONS_PER_MINUTE entries, all within the last 60s
        for i in 0..MAX_ACTIONS_PER_MINUTE {
            let age_ms = (MAX_ACTIONS_PER_MINUTE as u64 - i as u64) * 100;
            let ts = now - Duration::from_millis(age_ms);
            limiter.recent_actions.push_back(ts);
        }

        // Now check_and_record — should hit minute limit
        let result = limiter.check_and_record();
        assert!(result.is_err(), "Should hit minute limit with 100 entries");
        match result.unwrap_err() {
            RateLimitError::MinuteLimit {
                max_per_minute,
                retry_after_ms,
            } => {
                assert_eq!(max_per_minute, MAX_ACTIONS_PER_MINUTE);
                assert!(retry_after_ms > 0, "retry_after_ms should be positive");
            }
            other => panic!("Expected MinuteLimit, got: {other:?}"),
        }
        // Should NOT have added a new entry
        assert_eq!(limiter.recent_actions.len(), MAX_ACTIONS_PER_MINUTE);
    }

    #[test]
    fn test_rate_limiter_minute_limit_exact_boundary() {
        // With MAX_ACTIONS_PER_MINUTE - 1 entries, the next one should pass
        let mut limiter = GuiActionRateLimiter::new();
        let now = Instant::now();

        for i in 0..MAX_ACTIONS_PER_MINUTE - 1 {
            let age_ms = (MAX_ACTIONS_PER_MINUTE as u64 - i as u64) * 100;
            let ts = now - Duration::from_millis(age_ms);
            limiter.recent_actions.push_back(ts);
        }

        let result = limiter.check_and_record();
        assert!(result.is_ok(), "Action at boundary should pass: {result:?}");
        assert_eq!(limiter.recent_actions.len(), MAX_ACTIONS_PER_MINUTE);
    }

    #[test]
    fn test_rate_limiter_minute_limit_prunes_old_entries() {
        let mut limiter = GuiActionRateLimiter::new();
        let now = Instant::now();

        // Add entries older than 60s — they should be pruned
        for i in 0..MAX_ACTIONS_PER_MINUTE {
            let ts = now - Duration::from_secs(60 + i as u64 + 1);
            limiter.recent_actions.push_back(ts);
        }

        // Even with 100 entries, they're all > 60s old
        let result = limiter.check_and_record();
        assert!(
            result.is_ok(),
            "Old entries should be pruned, allowed: {result:?}"
        );
        // After pruning, the new entry should be the only one
        assert_eq!(
            limiter.recent_actions.len(),
            1,
            "Old entries pruned, new one recorded"
        );
    }

    #[test]
    fn test_rate_limiter_mixed_burst_and_minute() {
        // With entries near the minute limit but none recent, the burst check
        // can still reject a fast action before the minute check
        let mut limiter = GuiActionRateLimiter::new();
        let now = Instant::now();

        // Fill with entries spread 600ms apart, all starting 60s ago
        // This way none are pruned and none are within 100ms of "now"
        for i in 0..MAX_ACTIONS_PER_MINUTE - 1 {
            let age_ms = 60_000 - i as u64 * 600;
            let ts = now - Duration::from_millis(age_ms);
            limiter.recent_actions.push_back(ts);
        }
        // Add one very recent entry
        let recent = now - Duration::from_millis(10);
        limiter.recent_actions.push_back(recent);

        // Now we have 100 entries, the last one is 10ms ago
        // Should fail burst check (10ms < 100ms)
        let result = limiter.check_and_record();
        assert!(result.is_err());
        match result.unwrap_err() {
            RateLimitError::BurstLimit { .. } | RateLimitError::MinuteLimit { .. } => {}
        }
    }

    // ── Concurrent access tests ────────────────────────────────────────

    #[test]
    fn test_rate_limiter_shared_via_arc_mutex() {
        let limiter = Arc::new(Mutex::new(GuiActionRateLimiter::new()));

        {
            let mut guard = limiter.lock().unwrap();
            assert!(guard.check_and_record().is_ok());
        }

        {
            let mut guard = limiter.lock().unwrap();
            // Fast action should fail
            assert!(guard.check_and_record().is_err());
        }
    }

    #[test]
    fn test_rate_limiter_arc_mutex_multiple_access() {
        // Verify lock/unlock works correctly by accessing from the same thread
        let limiter = Arc::new(Mutex::new(GuiActionRateLimiter::new()));

        // First action
        limiter.lock().unwrap().check_and_record().unwrap();

        // Wait for burst
        std::thread::sleep(Duration::from_millis(100));

        // Second action
        limiter.lock().unwrap().check_and_record().unwrap();
        assert_eq!(limiter.lock().unwrap().recent_actions.len(), 2);
    }

    // ── Bounded-load measurements ──────────────────────────────────────

    #[test]
    fn test_rate_limiter_load_is_bounded_and_reproducible() {
        // This deliberately does not sleep: the burst policy is a structural
        // bound, so a fast load must admit one action and reject the rest.
        let mut limiter = GuiActionRateLimiter::new();
        let started = Instant::now();
        let mut accepted = 0;
        let mut rejected = 0;
        for _ in 0..10_000 {
            match limiter.check_and_record() {
                Ok(()) => accepted += 1,
                Err(_) => rejected += 1,
            }
        }

        assert_eq!(accepted, 1, "a burst admits exactly the first action");
        assert_eq!(rejected, 9_999);
        assert!(limiter.recent_actions.len() <= MAX_ACTIONS_PER_MINUTE);
        eprintln!(
            "GUI rate-limit load: 10,000 checks in {:?} (accepted={accepted}, rejected={rejected})",
            started.elapsed()
        );
    }

    #[test]
    fn test_gui_action_history_trimming_under_load() {
        let history = GuiActionHistory::new();
        let started = Instant::now();
        for i in 0..10_000 {
            history.record(ActionRecord {
                idempotency_key: format!("load-{i}"),
                command: "go_to_chat_list".to_string(),
                status: ActionStatus::Processed,
                timestamp_ms: i,
                duration_ms: 0,
            });
        }

        let records = history.all();
        assert_eq!(records.len(), MAX_HISTORY);
        assert_eq!(records.first().unwrap().idempotency_key, "load-9999");
        assert_eq!(records.last().unwrap().idempotency_key, "load-9000");
        assert!(history.find("load-8999").is_none());
        assert_eq!(history.latest_sequence(), 10_000);
        eprintln!(
            "GUI action-history load: 10,000 records in {:?} (retained={})",
            started.elapsed(),
            records.len()
        );
    }

    #[test]
    fn test_gui_action_queue_capacity_and_drain_throughput() {
        let (tx, mut rx) = gui_action_channel();
        let request = || GuiActionRequest {
            idempotency_key: generate_action_key(),
            command: GuiTestCommand::GoToChatList,
            created_at: Instant::now(),
        };
        let started = Instant::now();
        for _ in 0..MAX_PENDING {
            tx.try_send(request())
                .expect("queue accepts its full capacity");
        }
        assert!(
            tx.try_send(request()).is_err(),
            "queue must reject item N+1"
        );

        let mut drained = 0;
        while rx.try_recv().is_ok() {
            drained += 1;
        }
        assert_eq!(drained, MAX_PENDING);
        assert!(rx.try_recv().is_err(), "drain must leave the queue empty");
        eprintln!(
            "GUI action queue load: filled and drained {MAX_PENDING} items in {:?}",
            started.elapsed()
        );
    }
}
