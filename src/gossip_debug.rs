//! Opt-in gossip debug tracing — append-only event log for diagnosing
//! mesh-forwarding bugs.
//!
//! Enable by setting the `IROH_GOSSIP_DEBUG` environment variable to `1`.
//! The log is written to:
//!
//!   `~/.local/share/iroh-gossip/gossip-debug.log`
//!
//! …or the path in `IROH_GOSSIP_DEBUG_PATH` if that variable is set.
//!
//! ## Format
//!
//! Each line is a single event:
//!
//! ```text
//! 2026-07-09T21:00:00.123456Z [abc12] NeighborUp   [topic:deadb] [peer:xyz89]
//! 2026-07-09T21:00:01.456789Z [abc12] Received     [topic:deadb] [peer:xyz89] [size:1024]
//! 2026-07-09T21:00:02.789012Z [abc12] NeighborDown [topic:deadb] [peer:xyz89]
//! 2026-07-09T21:00:03.000000Z [abc12] Lagged       [topic:deadb]
//! ```
//!
//! ## Security
//!
//! - The log file is created with permissions `0600` (owner read/write only).
//! - The parent directory is created with permissions `0700`.
//! - **No message bodies** are ever written to the log — only sizes for
//!   `Received` events and short peer/topic identifiers.
//! - Append-only: the file is never truncated or rewritten by this module.

use std::{
    fs::{self, File, OpenOptions},
    io::Write,
    path::PathBuf,
    sync::{Mutex, OnceLock},
    time::SystemTime,
};

/// Singleton debug log state.  `None` means tracing is disabled.
static DEBUG_STATE: OnceLock<Mutex<DebugLog>> = OnceLock::new();

/// The backing state behind the debug log.
struct DebugLog {
    file: File,
    local_id: String,
    /// Cached path so we can re-print it in error messages.
    path: PathBuf,
}

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Initialise the gossip debug log.
///
/// Called automatically by the gossip actor at startup when
/// `IROH_GOSSIP_DEBUG=1`.  Calling this more than once is a no-op (the first
/// call wins).
///
/// `local_id` should be the short-form peer ID of the local node
/// (e.g. `endpoint.id().fmt_short()`).
pub fn init(local_id: &str) {
    if !env_is_enabled() {
        return;
    }

    let local_id = local_id.to_owned();
    #[allow(unused)]
    let _ = &local_id;
    let path = log_path();

    // Create parent directory with 0700 permissions.
    if let Some(parent) = path.parent() {
        if let Err(e) = fs::create_dir_all(parent) {
            eprintln!("[gossip_debug] failed to create directory {parent:?}: {e}");
            return;
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = fs::set_permissions(parent, fs::Permissions::from_mode(0o700));
        }
    }

    // Open (or create) the log file.
    let file = match OpenOptions::new().create(true).append(true).open(&path) {
        Ok(f) => f,
        Err(e) => {
            eprintln!("[gossip_debug] failed to open {path:?}: {e}");
            return;
        }
    };

    // Set 0600 permissions on the file.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = fs::set_permissions(&path, fs::Permissions::from_mode(0o600));
    }

    let log = DebugLog {
        file,
        local_id,
        path,
    };

    // `OnceLock` guarantees we only do this once.
    let _ = DEBUG_STATE.set(Mutex::new(log));
}

/// Returns `true` when gossip debug tracing is active.
pub fn is_enabled() -> bool {
    DEBUG_STATE.get().is_some()
}

/// Log a gossip-level event.
///
/// Parameters:
/// - `kind` — event type string, e.g. `"NeighborUp"`, `"NeighborDown"`,
///   `"Received"`, `"Lagged"`.
/// - `topic` — short topic ID, or `None` for topic-agnostic events.
/// - `peer` — short remote peer ID, or `None` if this event has no peer
///   (e.g. `Lagged`).
/// - `size` — message size in bytes (for `Received`); ignored when `None`.
pub fn log_event(kind: &str, topic: Option<&str>, peer: Option<&str>, size: Option<usize>) {
    let guard = match DEBUG_STATE.get() {
        Some(g) => g,
        None => return,
    };

    let mut log = match guard.lock() {
        Ok(l) => l,
        Err(_) => return,
    };

    let local_id = log.local_id.clone();
    write_event(&mut log.file, &local_id, kind, topic, peer, size);
}

// ---------------------------------------------------------------------------
// Internals
// ---------------------------------------------------------------------------

/// Read `IROH_GOSSIP_DEBUG` once per process.
fn env_is_enabled() -> bool {
    std::env::var("IROH_GOSSIP_DEBUG").as_deref() == Ok("1")
}

/// Determine the log file path.
fn log_path() -> PathBuf {
    if let Ok(p) = std::env::var("IROH_GOSSIP_DEBUG_PATH") {
        return PathBuf::from(p);
    }

    // Default: ~/.local/share/iroh-gossip/gossip-debug.log
    let home = std::env::var("HOME")
        .or_else(|_| std::env::var("USERPROFILE"))
        .unwrap_or_else(|_| ".".into());

    PathBuf::from(home)
        .join(".local")
        .join("share")
        .join("iroh-gossip")
        .join("gossip-debug.log")
}

/// Write one line to the log file.
///
/// Format:
///   `<iso-ts> [<local_id>] <kind>\t[topic:<topic>]\t[peer:<peer>]\t[size:<N>]\n`
///
/// Missing fields are omitted (the tab separators are still present for
/// machine-parseability).
fn write_event(
    file: &mut File,
    local_id: &str,
    kind: &str,
    topic: Option<&str>,
    peer: Option<&str>,
    size: Option<usize>,
) {
    let ts = timestamp();

    // Build the line as a single write to minimise syscalls.
    let mut line = String::with_capacity(128);
    line.push_str(&ts);
    line.push_str(" [");
    line.push_str(local_id);
    line.push_str("] ");
    line.push_str(kind);

    if let Some(t) = topic {
        line.push_str("\t[topic:");
        line.push_str(t);
        line.push(']');
    }

    if let Some(p) = peer {
        line.push_str("\t[peer:");
        line.push_str(p);
        line.push(']');
    }

    if let Some(s) = size {
        line.push_str("\t[size:");
        // Use itoa-like inline formatting to avoid a dep.
        push_usize(&mut line, s);
        line.push(']');
    }

    line.push('\n');

    if let Err(e) = file.write_all(line.as_bytes()) {
        eprintln!("[gossip_debug] write error: {e}");
    }

    // Flush immediately so the log is durable on crash.
    let _ = file.flush();
}

/// ISO-8601 timestamp with microseconds.
fn timestamp() -> String {
    let now = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default();

    let secs = now.as_secs();
    let micros = now.subsec_nanos() / 1_000;

    // Format the seconds portion manually to avoid pulling in chrono.
    // We use the same approach as `chrono`: split into date & time.
    let (year, month, day, hour, min, sec) = seconds_to_datetime(secs);

    // 32 bytes is enough for "YYYY-MM-DDTHH:MM:SS.ffffffZ"
    let mut buf = String::with_capacity(32);
    push_u4(&mut buf, year / 1000 % 10);
    push_u4(&mut buf, year / 100 % 10);
    push_u4(&mut buf, year / 10 % 10);
    push_u4(&mut buf, year % 10);
    buf.push('-');
    push_u2(&mut buf, month);
    buf.push('-');
    push_u2(&mut buf, day);
    buf.push('T');
    push_u2(&mut buf, hour);
    buf.push(':');
    push_u2(&mut buf, min);
    buf.push(':');
    push_u2(&mut buf, sec);
    buf.push('.');
    push_u6(&mut buf, micros.into());
    buf.push('Z');
    buf
}

/// Convert seconds since epoch to (year, month, day, hour, minute, second).
/// Uses a simple algorithm valid for timestamps between 1970 and 2099.
fn seconds_to_datetime(secs: u64) -> (u64, u64, u64, u64, u64, u64) {
    // Days since Unix epoch.
    let days = secs / 86400;
    let day_secs = secs % 86400;
    let hour = day_secs / 3600;
    let minute = (day_secs % 3600) / 60;
    let sec = day_secs % 60;

    // Year/month/day from days since epoch.
    let (y, m, d) = days_to_date(days);
    (y, m as u64, d as u64, hour, minute, sec)
}

/// Convert days since Unix epoch (1970-01-01) to (year, month, day).
///
/// Uses the C F置换 algorithm (valid 1970–2099).
fn days_to_date(mut days: u64) -> (u64, u32, u32) {
    // 1970-01-01 is day 0.
    // Calculate year.
    let mut year = 1970u64;
    loop {
        let days_in_year = if is_leap(year) { 366 } else { 365 };
        if days < days_in_year {
            break;
        }
        days -= days_in_year;
        year += 1;
    }

    // Month and day from day-of-year (0-indexed).
    let mdays: [u64; 12] = if is_leap(year) {
        [31, 29, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31]
    } else {
        [31, 28, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31]
    };

    let mut month = 0u32;
    for (i, &md) in mdays.iter().enumerate() {
        if days < md {
            month = (i + 1) as u32;
            break;
        }
        days -= md;
    }
    let day = (days + 1) as u32; // 1-indexed

    (year, month, day)
}

fn is_leap(year: u64) -> bool {
    (year % 4 == 0 && year % 100 != 0) || year % 400 == 0
}

// ---------------------------------------------------------------------------
// Inline digit formatting helpers (no formatting deps)
// ---------------------------------------------------------------------------

fn push_u4(s: &mut String, v: u64) {
    s.push(char::from(b'0' + (v as u8)));
}
fn push_u2(s: &mut String, v: u64) {
    s.push(char::from(b'0' + ((v / 10) as u8)));
    s.push(char::from(b'0' + ((v % 10) as u8)));
}
fn push_u6(s: &mut String, v: u64) {
    push_u2(s, v / 10000);
    push_u2(s, (v / 100) % 100);
    push_u2(s, v % 100);
}
fn push_usize(s: &mut String, v: usize) {
    // Write digits in reverse order, then flip.
    if v == 0 {
        s.push('0');
        return;
    }
    let mut buf = [0u8; 20];
    let mut i = buf.len();
    let mut n = v;
    while n > 0 {
        i -= 1;
        buf[i] = b'0' + (n % 10) as u8;
        n /= 10;
    }
    // SAFETY: we only wrote ASCII digits.
    let digits = unsafe { std::str::from_utf8_unchecked(&buf[i..]) };
    s.push_str(digits);
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_env_not_set_by_default() {
        // IROH_GOSSIP_DEBUG is not set in tests, so init should be a no-op.
        init("test");
        assert!(!is_enabled());
    }

    #[test]
    fn test_timestamp_format() {
        let ts = timestamp();
        // Expect something like "2026-07-09T21:00:00.123456Z"
        // Check that it ends with Z and contains the expected separators.
        assert!(ts.ends_with('Z'), "timestamp should end with Z: {ts}");
        assert_eq!(&ts[4..5], "-", "expected - after year: {ts}");
        assert_eq!(&ts[7..8], "-", "expected - after month: {ts}");
        assert_eq!(&ts[10..11], "T", "expected T after date: {ts}");
        assert_eq!(&ts[13..14], ":", "expected : after hour: {ts}");
        assert_eq!(&ts[16..17], ":", "expected : after minute: {ts}");
        assert_eq!(&ts[19..20], ".", "expected . after seconds: {ts}");
        // The year should be reasonable (current era).
        let year: u64 = ts[..4].parse().expect("year should be numeric");
        assert!(
            (2023..=2030).contains(&year),
            "year {year} out of range in: {ts}"
        );
    }

    #[test]
    fn test_push_usize() {
        let mut s = String::new();
        push_usize(&mut s, 0);
        assert_eq!(s, "0");

        let mut s = String::new();
        push_usize(&mut s, 42);
        assert_eq!(s, "42");

        let mut s = String::new();
        push_usize(&mut s, 123456789);
        assert_eq!(s, "123456789");
    }
}
