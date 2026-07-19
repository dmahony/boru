//! Non-invasive performance instrumentation for the iced chat GUI.
//!
//! Provides tracing-based timing spans and an in-memory summary that is
//! printed at application exit when the `--perf` flag is active.
//!
//! # Usage
//! Call `PerfTracker::new()` at startup and attach it to the IcedChat struct.
//! Each timed operation calls `PerfTracker::record(...)` which writes a
//! `tracing::info!` span and stores a sample in-memory for later analysis.
//!
//! At shutdown (or /perf command), call `PerfTracker::report()` to print
//! the full baseline summary.

use parking_lot::Mutex;
use std::time::{Duration, Instant};

/// A single performance sample.
#[derive(Debug, Clone)]
pub struct PerfSample {
    pub label: &'static str,
    pub duration_ns: u64,
    pub context: String,
}

/// Thread-safe accumulator of performance samples.
///
/// Wrapped in `OnceLock` so there's exactly one global tracker per process.
pub static PERF: std::sync::LazyLock<PerfTracker> = std::sync::LazyLock::new(PerfTracker::new);

/// Global performance tracker.
pub struct PerfTracker {
    samples: Mutex<Vec<PerfSample>>,
    enabled: std::sync::atomic::AtomicBool,
}

impl PerfTracker {
    pub fn new() -> Self {
        Self {
            samples: Mutex::new(Vec::with_capacity(16384)),
            enabled: std::sync::atomic::AtomicBool::new(true),
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
        // Emit a tracing event so log viewers see the raw timing data.
        tracing::info!(
            target: "perf",
            label = %label,
            duration_ns = ns,
            duration_ms = duration.as_secs_f64() * 1000.0,
            %context,
            "PERF"
        );
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

    /// Time an async closure and record its duration.
    pub async fn time_async<F, T>(label: &'static str, context: impl Into<String>, f: F) -> T
    where
        F: std::future::Future<Output = T>,
    {
        let start = Instant::now();
        let result = f.await;
        let duration = start.elapsed();
        Self::record(label, duration, context.into());
        result
    }

    /// Return a timer that records elapsed time when dropped.
    pub fn timer(label: &'static str, context: impl Into<String>) -> PerfTimer {
        PerfTimer {
            label,
            context: Some(context.into()),
            start: Instant::now(),
        }
    }

    /// Print a full summary of all recorded samples to stderr.
    ///
    /// Call this at application shutdown or when the user runs /perf.
    pub fn print_report() {
        let samples = PERF.samples.lock();
        if samples.is_empty() {
            eprintln!("[PERF] No performance samples recorded.");
            return;
        }

        // Aggregate by label
        let mut by_label: std::collections::HashMap<&'static str, Vec<u64>> =
            std::collections::HashMap::new();
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
        all_samples.sort_by_key(|s| std::cmp::Reverse(s.duration_ns));
        eprintln!("  ── Top 10 Slowest Operations ──");
        for s in all_samples.iter().take(10) {
            let ms = s.duration_ns as f64 / 1_000_000.0;
            eprintln!("    {ms:>10.3} ms  {}  ({})", s.label, s.context);
        }
        eprintln!();
    }

    /// Print a JSON snapshot of the aggregated data (machine-readable).
    pub fn json_report() -> serde_json::Value {
        let samples = PERF.samples.lock();
        let mut by_label: std::collections::BTreeMap<&'static str, Vec<u64>> =
            std::collections::BTreeMap::new();
        for s in samples.iter() {
            by_label.entry(s.label).or_default().push(s.duration_ns);
        }

        let mut stats = serde_json::Map::new();
        let mut total_count = 0u64;
        let mut total_time_ns = 0u64;

        for (label, times) in &by_label {
            let sum: u64 = times.iter().sum();
            let count = times.len() as u64;
            total_count += count;
            total_time_ns += sum;
            let avg_ns = sum as f64 / count as f64;
            let min_ns = *times.iter().min().unwrap_or(&0);
            let max_ns = *times.iter().max().unwrap_or(&0);

            stats.insert(
                label.to_string(),
                serde_json::json!({
                    "count": count,
                    "total_ns": sum,
                    "total_ms": sum as f64 / 1_000_000.0,
                    "avg_ns": avg_ns.round() as u64,
                    "avg_ms": (avg_ns / 1_000_000.0 * 1000.0).round() / 1000.0,
                    "min_ns": min_ns,
                    "min_ms": (min_ns as f64 / 1_000_000.0 * 1000.0).round() / 1000.0,
                    "max_ns": max_ns,
                    "max_ms": (max_ns as f64 / 1_000_000.0 * 1000.0).round() / 1000.0,
                }),
            );
        }

        serde_json::json!({
            "total_samples": total_count,
            "total_time_ms": total_time_ns as f64 / 1_000_000.0,
            "metrics": stats,
        })
    }

    /// Reset all accumulated samples.
    pub fn reset() {
        let mut samples = PERF.samples.lock();
        samples.clear();
    }
}

/// A RAII timer that records a sample when dropped.
pub struct PerfTimer {
    label: &'static str,
    context: Option<String>,
    start: Instant,
}

impl PerfTimer {
    /// Abort without recording.
    pub fn cancel(&mut self) {
        self.context = None;
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
