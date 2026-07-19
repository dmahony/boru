//! Performance instrumentation for boru-chat.
//!
//! Provides lightweight, feature-gated timing primitives that record duration
//! samples, emit tracing events, and optionally warn when operations exceed a
//! configurable slow threshold.
//!
//! # Enable
//!
//! Set `BORU_PERF=1` at runtime to activate recording.  A summary is printed
//! at process exit via `BORU_PERF_PRINT=1` (default: on).
//!
//! ```sh
//! BORU_PERF=1 cargo run --example iced_chat
//! ```
//!
//! # Slow-operation threshold
//!
//! Set `BORU_PERF_SLOW_MS` (default 100) — any single operation exceeding
//! that threshold logs a `warn!`-level tracing event tagged `target: "perf_slow"`.

use parking_lot::Mutex;
use std::collections::BTreeMap;
use std::time::{Duration, Instant};

// ---------------------------------------------------------------------------
// Global state
// ---------------------------------------------------------------------------

/// Global singleton performance tracker.  Initialised on first access.
pub static PERF: std::sync::LazyLock<PerfTracker> = std::sync::LazyLock::new(PerfTracker::new);

/// A single performance sample recorded during execution.
#[derive(Debug, Clone)]
pub struct PerfSample {
    /// Operation label (static string).
    pub label: &'static str,
    /// Duration in nanoseconds.
    pub duration_ns: u64,
    /// Contextual information (e.g. topic, peer, message count).
    pub context: String,
}

/// Thread-safe performance sample accumulator.
#[derive(Debug)]
pub struct PerfTracker {
    samples: Mutex<Vec<PerfSample>>,
    enabled: std::sync::atomic::AtomicBool,
}

impl PerfTracker {
    fn new() -> Self {
        Self {
            samples: Mutex::new(Vec::with_capacity(16384)),
            enabled: std::sync::atomic::AtomicBool::new(false),
        }
    }

    /// Enable or disable recording globally.
    pub fn set_enabled(enabled: bool) {
        PERF.enabled
            .store(enabled, std::sync::atomic::Ordering::Relaxed);
    }

    /// Returns true if recording is active.
    pub fn is_enabled() -> bool {
        PERF.enabled.load(std::sync::atomic::Ordering::Relaxed)
    }

    /// Record a single duration sample.
    pub fn record(label: &'static str, duration: Duration, context: String) {
        if !Self::is_enabled() {
            return;
        }
        let ns = duration.as_nanos() as u64;
        let ms = duration.as_secs_f64() * 1000.0;
        tracing::info!(
            target: "perf",
            label = %label,
            duration_ns = ns,
            duration_ms = ms,
            %context,
            "PERF"
        );
        // Check slow-operation threshold.
        if let Ok(threshold) = std::env::var("BORU_PERF_SLOW_MS") {
            if let Ok(threshold_ms) = threshold.parse::<f64>() {
                if ms > threshold_ms {
                    tracing::warn!(
                        target: "perf_slow",
                        label = %label,
                        duration_ns = ns,
                        duration_ms = ms,
                        threshold_ms = threshold_ms,
                        %context,
                        "SLOW OPERATION"
                    );
                }
            }
        }
        let mut samples = PERF.samples.lock();
        samples.push(PerfSample {
            label,
            duration_ns: ns,
            context,
        });
    }

    /// Time a closure and record its duration.
    pub fn time<F, T>(label: &'static str, context: impl Into<String>, f: F) -> T
    where
        F: FnOnce() -> T,
    {
        let start = Instant::now();
        let result = f();
        let duration = start.elapsed();
        Self::record(label, duration, context.into());
        result
    }

    /// Return a RAII timer that records elapsed time when dropped.
    pub fn timer(label: &'static str, context: impl Into<String>) -> PerfTimer {
        PerfTimer {
            label,
            context: Some(context.into()),
            start: Instant::now(),
        }
    }

    /// Print a full summary of all recorded samples to stderr.
    pub fn print_report() {
        let samples = PERF.samples.lock();
        if samples.is_empty() {
            eprintln!("[PERF] No performance samples recorded.");
            return;
        }

        // Aggregate by label
        let mut by_label: BTreeMap<&'static str, Vec<u64>> = BTreeMap::new();
        for s in samples.iter() {
            by_label.entry(s.label).or_default().push(s.duration_ns);
        }

        // Sort labels by total time descending
        let mut sorted: Vec<_> = by_label.iter().collect();
        sorted.sort_by(|a, b| {
            let sum_a: u64 = a.1.iter().sum();
            let sum_b: u64 = b.1.iter().sum();
            sum_b.cmp(&sum_a)
        });

        eprintln!();
        eprintln!("╔══════════════════════════════════════════════════════════════╗");
        eprintln!("║              PERFORMANCE BASELINE REPORT                   ║");
        eprintln!("╠══════════════════════════════════════════════════════════════╣");
        eprintln!("║ Samples recorded:  {:>38} ║", samples.len());
        eprintln!("╚══════════════════════════════════════════════════════════════╝");
        eprintln!();

        for (label, times) in &sorted {
            let sum: u64 = times.iter().sum();
            let count = times.len();
            let avg_ns = sum as f64 / count as f64;
            let min_ns = *times.iter().min().unwrap_or(&0);
            let max_ns = *times.iter().max().unwrap_or(&0);

            let avg_ms = avg_ns / 1_000_000.0;
            let min_ms = min_ns as f64 / 1_000_000.0;
            let max_ms = max_ns as f64 / 1_000_000.0;
            let total_ms = sum as f64 / 1_000_000.0;

            eprintln!("  {label}");
            eprintln!("    count:  {count:>8}    total: {total_ms:>10.1} ms");
            eprintln!("    avg:    {avg_ms:>10.3} ms    min: {min_ms:>10.3} ms");
            eprintln!("    max:    {max_ms:>10.3} ms");
            eprintln!();
        }

        // Top-N slowest individual operations
        let mut all_samples: Vec<_> = samples.clone();
        all_samples.sort_by_key(|b| std::cmp::Reverse(b.duration_ns));
        eprintln!("  ── Top 10 Slowest Operations ──");
        for s in all_samples.iter().take(10) {
            let ms = s.duration_ns as f64 / 1_000_000.0;
            eprintln!("    {ms:>10.3} ms  {}  ({})", s.label, s.context);
        }
        eprintln!();

        // Summary statistics
        let total_time_ns: u64 = all_samples.iter().map(|s| s.duration_ns).sum();
        let total_time_ms = total_time_ns as f64 / 1_000_000.0;
        eprintln!("  Total sampled time: {total_time_ms:.1} ms");
        eprintln!("  Total operations:   {}", all_samples.len());
        eprintln!();
    }

    /// Reset all accumulated samples.
    pub fn reset() {
        let mut samples = PERF.samples.lock();
        samples.clear();
    }
}

/// A RAII timer that records a performance sample when dropped.
#[derive(Debug)]
pub struct PerfTimer {
    /// Operation label (static string).
    label: &'static str,
    /// Context string. `None` means the timer was canceled.
    context: Option<String>,
    /// Instant when the timer was created (or rearmed).
    start: Instant,
}

impl PerfTimer {
    /// Cancel the timer — no sample is recorded on drop.
    pub fn cancel(&mut self) {
        self.context = None;
    }

    /// Re-arm after a previous `cancel()`.
    pub fn rearm(&mut self, context: impl Into<String>) {
        self.context = Some(context.into());
        self.start = Instant::now();
    }
}

impl Drop for PerfTimer {
    fn drop(&mut self) {
        if let Some(ctx) = self.context.take() {
            let duration = self.start.elapsed();
            PerfTracker::record(self.label, duration, ctx);
        }
    }
}

impl Clone for PerfTimer {
    fn clone(&self) -> Self {
        Self {
            label: self.label,
            context: self.context.clone(),
            start: Instant::now(),
        }
    }
}

// ===========================================================================
// Initialisation
// ===========================================================================

/// Initialise the perf system.  Reads environment variables and enables
/// recording if `BORU_PERF=1`.  Safe to call multiple times.
pub fn init() {
    let enabled = std::env::var("BORU_PERF").as_deref() == Ok("1");
    PerfTracker::set_enabled(enabled);
    if !enabled {
        return;
    }
    // Set default slow threshold if not already set.
    let _ = std::env::var("BORU_PERF_SLOW_MS").unwrap_or_else(|_| {
        // Rust 2024 marks environment mutation as unsafe because it can race
        // with concurrent process environment access.
        unsafe { std::env::set_var("BORU_PERF_SLOW_MS", "100") };
        "100".to_string()
    });
    tracing::info!(target: "perf", "perf instrumentation enabled (BORU_PERF=1)");
    eprintln!("[PERF] Instrumentation enabled.  Set BORU_PERF=0 to disable at runtime.");
}

// ===========================================================================
// Macro: emit a slow-operation warning at a label + threshold
// ===========================================================================

/// Check if a duration exceeds the slow threshold and emit a warning if so.
/// Threshold is in milliseconds.
pub fn check_slow(label: &'static str, duration: Duration, threshold_ms: f64, context: &str) {
    let ms = duration.as_secs_f64() * 1000.0;
    if ms > threshold_ms {
        tracing::warn!(
            target: "perf_slow",
            label = %label,
            duration_ns = duration.as_nanos(),
            duration_ms = ms,
            threshold_ms = threshold_ms,
            %context,
            "SLOW OPERATION"
        );
    }
}

// ===========================================================================
// Tests
// ===========================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    /// Helper: return samples filtered by label.
    fn samples_by_label(label: &str) -> Vec<PerfSample> {
        let guard = PERF.samples.lock();
        guard.iter().filter(|s| s.label == label).cloned().collect()
    }

    #[test]
    fn test_timer_records_duration() {
        PerfTracker::set_enabled(true);
        PerfTracker::reset();

        {
            let _timer = PerfTracker::timer("test_timer", "sleep");
            std::thread::sleep(Duration::from_millis(5));
        }

        let our_samples = samples_by_label("test_timer");
        assert_eq!(
            our_samples.len(),
            1,
            "should have recorded 1 'test_timer' sample"
        );
        assert_eq!(our_samples[0].label, "test_timer");
        assert!(our_samples[0].duration_ns >= 4_000_000, "should be >= 4ms");
        assert_eq!(our_samples[0].context, "sleep");
    }

    #[test]
    fn test_time_closure_records() {
        PerfTracker::set_enabled(true);
        PerfTracker::reset();

        let result = PerfTracker::time("test_closure", "add", || {
            std::thread::sleep(Duration::from_millis(2));
            42
        });

        assert_eq!(result, 42);
        let our_samples = samples_by_label("test_closure");
        assert_eq!(our_samples.len(), 1);
    }

    #[test]
    fn test_disabled_by_default() {
        PerfTracker::set_enabled(false);
        PerfTracker::reset();
        PerfTracker::record("test_disabled", Duration::from_millis(10), "nope".into());
        let our_samples = samples_by_label("test_disabled");
        assert!(our_samples.is_empty(), "should not record when disabled");
    }

    #[test]
    fn test_canceled_timer_does_not_record() {
        PerfTracker::set_enabled(true);
        PerfTracker::reset();

        {
            let mut timer = PerfTracker::timer("test_cancel", "will cancel");
            std::thread::sleep(Duration::from_millis(2));
            timer.cancel();
        }

        let our_samples = samples_by_label("test_cancel");
        assert!(our_samples.is_empty(), "canceled timer should not record");
    }
}
