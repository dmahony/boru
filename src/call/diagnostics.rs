//! Privacy-safe, local-only diagnostics for live calls.
//!
//! This module deliberately stores only bounded aggregates and stable labels.
//! It never accepts media payloads, pixels, audio, endpoint identities, IP
//! addresses, device identifiers, or message content.

use std::collections::BTreeMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

/// Current schema for [`CallDiagnosticsSnapshot`].
pub const DIAGNOSTICS_SCHEMA_VERSION: u16 = 1;
/// Maximum number of reason labels retained in one snapshot.
pub const MAX_REASONS: usize = 16;
/// Maximum number of counters retained in one snapshot.
pub const MAX_COUNTERS: usize = 32;
/// Maximum diagnostic log lines permitted in one window.
pub const MAX_LOGS_PER_WINDOW: u32 = 4;
const LOG_WINDOW: Duration = Duration::from_secs(10);
const MAX_LABEL: usize = 64;

/// Codec and implementation labels negotiated locally.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CodecDiagnostics {
    /// Selected audio codec, such as `opus`.
    pub audio: Option<String>,
    /// Selected video codec, such as `h264`.
    pub video: Option<String>,
    /// Local implementation label, not a device identifier.
    pub implementation: Option<String>,
    /// Quality profile label, such as `q1`.
    pub profile: Option<String>,
}

/// Negotiated media rates and observed bitrates.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct RateDiagnostics {
    /// Audio sample rate in Hz.
    pub audio_sample_rate_hz: Option<u32>,
    /// Audio channels.
    pub audio_channels: Option<u8>,
    /// Video frame rate.
    pub video_fps: Option<u32>,
    /// Target audio bitrate in bits per second.
    pub audio_bitrate_bps: Option<u64>,
    /// Target video bitrate in bits per second.
    pub video_bitrate_bps: Option<u64>,
    /// Aggregate observed send bitrate in bits per second.
    pub send_bitrate_bps: Option<u64>,
    /// Aggregate observed receive bitrate in bits per second.
    pub receive_bitrate_bps: Option<u64>,
}

/// Queue and timing health, with no queue contents.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct QueueDiagnostics {
    /// Current media queue depth.
    pub depth: u32,
    /// Configured queue capacity.
    pub capacity: u32,
    /// Number of queue overflows.
    pub overflows: u64,
    /// Current jitter-buffer target in milliseconds.
    pub jitter_buffer_ms: u32,
    /// Capture-to-send timing in milliseconds.
    pub capture_to_send_ms: u32,
    /// Decode-to-present timing in milliseconds.
    pub decode_to_present_ms: u32,
}

/// Cumulative drop counters.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct DropDiagnostics {
    /// Packets dropped before decoding.
    pub packets: u64,
    /// Frames dropped before presentation.
    pub frames: u64,
    /// Audio packets lost or late.
    pub audio: u64,
    /// Keyframes requested.
    pub keyframes: u64,
}

/// Event counters useful for diagnosing adaptation and fallback.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CallDiagnosticsCounters {
    /// Adaptation decisions applied.
    pub adaptation: u64,
    /// Codec or implementation fallbacks.
    pub fallback: u64,
    /// Keyframe requests sent.
    pub keyframe: u64,
    /// Track starts/stops/reconnects.
    pub track: u64,
    /// Probe requests/responses/timeouts.
    pub probe: u64,
}

/// Privacy-safe reason category for a recent call event.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DiagnosticReason {
    /// Congestion adaptation was applied.
    Congestion,
    /// Codec fallback was selected.
    CodecFallback,
    /// Keyframe was requested.
    Keyframe,
    /// Track lifecycle changed.
    Track,
    /// Probe timed out or failed.
    Probe,
    /// Queue pressure was observed.
    QueuePressure,
    /// Media was dropped.
    MediaDrop,
}

/// Bounded local-only snapshot suitable for display or support export.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CallDiagnosticsSnapshot {
    /// Snapshot schema version.
    pub schema_version: u16,
    /// Selected codec and implementation labels.
    pub codec: CodecDiagnostics,
    /// Negotiated and observed rates.
    pub rates: RateDiagnostics,
    /// Queue and pipeline timing aggregates.
    pub queue: QueueDiagnostics,
    /// Round-trip time in milliseconds.
    pub rtt_ms: Option<u32>,
    /// Cumulative drops.
    pub drops: DropDiagnostics,
    /// Adaptation/fallback/lifecycle counters.
    pub counters: CallDiagnosticsCounters,
    /// Recent bounded reason categories; never free-form error text.
    pub reasons: Vec<DiagnosticReason>,
}

impl Default for CallDiagnosticsSnapshot {
    fn default() -> Self {
        Self {
            schema_version: DIAGNOSTICS_SCHEMA_VERSION,
            codec: CodecDiagnostics::default(),
            rates: RateDiagnostics::default(),
            queue: QueueDiagnostics::default(),
            rtt_ms: None,
            drops: DropDiagnostics::default(),
            counters: CallDiagnosticsCounters::default(),
            reasons: Vec::new(),
        }
    }
}

/// Event categories accepted by [`CallDiagnostics::record`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiagnosticEvent {
    /// An adaptation decision changed.
    Adaptation,
    /// A fallback was applied.
    Fallback,
    /// A keyframe request was sent.
    Keyframe,
    /// A track lifecycle operation occurred.
    Track,
    /// A probe operation occurred.
    Probe,
    /// Queue pressure was observed.
    QueuePressure,
    /// A packet or frame was dropped.
    MediaDrop,
}

#[derive(Debug)]
struct State {
    snapshot: CallDiagnosticsSnapshot,
    window_started: Instant,
    logs_in_window: u32,
}

/// In-memory diagnostics accumulator. It is never serialized or transmitted.
#[derive(Debug)]
pub struct CallDiagnostics {
    state: Mutex<State>,
}

impl Default for CallDiagnostics {
    fn default() -> Self {
        Self::new()
    }
}

impl CallDiagnostics {
    /// Create an empty accumulator.
    pub fn new() -> Self {
        Self {
            state: Mutex::new(State {
                snapshot: CallDiagnosticsSnapshot::default(),
                window_started: Instant::now(),
                logs_in_window: 0,
            }),
        }
    }

    /// Return a bounded copy of the current local snapshot.
    pub fn snapshot(&self) -> CallDiagnosticsSnapshot {
        self.state
            .lock()
            .expect("call diagnostics lock poisoned")
            .snapshot
            .clone()
    }

    /// Replace negotiated codec labels with bounded, caller-supplied values.
    pub fn set_codec(&self, codec: CodecDiagnostics) {
        let mut state = self.state.lock().expect("call diagnostics lock poisoned");
        state.snapshot.codec = CodecDiagnostics {
            audio: codec.audio.map(|v| redact_support_text(&v)),
            video: codec.video.map(|v| redact_support_text(&v)),
            implementation: codec.implementation.map(|v| redact_support_text(&v)),
            profile: codec.profile.map(|v| redact_support_text(&v)),
        };
    }

    /// Replace aggregate rate values. No media samples are retained.
    pub fn set_rates(&self, rates: RateDiagnostics) {
        self.state
            .lock()
            .expect("call diagnostics lock poisoned")
            .snapshot
            .rates = rates;
    }

    /// Replace aggregate queue/timing values. Queue contents are never accepted.
    pub fn set_queue(&self, queue: QueueDiagnostics) {
        self.state
            .lock()
            .expect("call diagnostics lock poisoned")
            .snapshot
            .queue = queue;
    }

    /// Record the latest bounded round-trip-time measurement.
    pub fn set_rtt(&self, rtt: Duration) {
        self.state
            .lock()
            .expect("call diagnostics lock poisoned")
            .snapshot
            .rtt_ms = Some(rtt.as_millis().min(u32::MAX as u128) as u32);
    }

    /// Record a category-only event; payloads and free-form details are not accepted.
    pub fn record(&self, event: DiagnosticEvent) {
        let mut state = self.state.lock().expect("call diagnostics lock poisoned");
        match event {
            DiagnosticEvent::Adaptation => {
                state.snapshot.counters.adaptation =
                    state.snapshot.counters.adaptation.saturating_add(1);
                push_reason(&mut state.snapshot.reasons, DiagnosticReason::Congestion);
            }
            DiagnosticEvent::Fallback => {
                state.snapshot.counters.fallback =
                    state.snapshot.counters.fallback.saturating_add(1);
                push_reason(&mut state.snapshot.reasons, DiagnosticReason::CodecFallback);
            }
            DiagnosticEvent::Keyframe => {
                state.snapshot.counters.keyframe =
                    state.snapshot.counters.keyframe.saturating_add(1);
                state.snapshot.drops.keyframes = state.snapshot.drops.keyframes.saturating_add(1);
                push_reason(&mut state.snapshot.reasons, DiagnosticReason::Keyframe);
            }
            DiagnosticEvent::Track => {
                state.snapshot.counters.track = state.snapshot.counters.track.saturating_add(1);
                push_reason(&mut state.snapshot.reasons, DiagnosticReason::Track);
            }
            DiagnosticEvent::Probe => {
                state.snapshot.counters.probe = state.snapshot.counters.probe.saturating_add(1);
                push_reason(&mut state.snapshot.reasons, DiagnosticReason::Probe);
            }
            DiagnosticEvent::QueuePressure => {
                state.snapshot.queue.overflows = state.snapshot.queue.overflows.saturating_add(1);
                push_reason(&mut state.snapshot.reasons, DiagnosticReason::QueuePressure);
            }
            DiagnosticEvent::MediaDrop => {
                state.snapshot.drops.packets = state.snapshot.drops.packets.saturating_add(1);
                push_reason(&mut state.snapshot.reasons, DiagnosticReason::MediaDrop);
            }
        }
    }

    /// Permit a low-frequency diagnostic log line. The caller should log only
    /// a fixed category label, never an error or media payload.
    pub fn allow_log(&self, now: Instant) -> bool {
        let mut state = self.state.lock().expect("call diagnostics lock poisoned");
        if now.saturating_duration_since(state.window_started) >= LOG_WINDOW {
            state.window_started = now;
            state.logs_in_window = 0;
        }
        if state.logs_in_window >= MAX_LOGS_PER_WINDOW {
            return false;
        }
        state.logs_in_window += 1;
        true
    }
}

fn push_reason(reasons: &mut Vec<DiagnosticReason>, reason: DiagnosticReason) {
    if reasons.last().copied() != Some(reason) {
        reasons.push(reason);
        if reasons.len() > MAX_REASONS {
            reasons.remove(0);
        }
    }
}

/// Redact untrusted support text while retaining a safe category hint.
///
/// This is intentionally conservative: URLs, paths, IP-like values, tokens,
/// and anything after common secret/content keys are replaced.
pub fn redact_support_text(input: &str) -> String {
    let lower = input.to_ascii_lowercase();
    let sensitive = [
        "token",
        "secret",
        "password",
        "authorization",
        "payload",
        "message",
        "content",
        "audio",
        "pixel",
        "device_id",
        "identity",
        "ip",
        "path",
        "file",
    ];
    if sensitive.iter().any(|key| lower.contains(key))
        || looks_like_ip(&lower)
        || lower
            .split_whitespace()
            .any(|word| word.len() >= 64 && word.chars().all(|c| c.is_ascii_hexdigit()))
    {
        return "details redacted".to_string();
    }
    input
        .chars()
        .take(MAX_LABEL)
        .map(|c| if c.is_control() { '_' } else { c })
        .collect()
}

fn looks_like_ip(value: &str) -> bool {
    value
        .split(|c: char| !c.is_ascii_digit() && c != '.')
        .any(|word| {
            let parts: Vec<_> = word.split('.').collect();
            parts.len() == 4
                && parts
                    .iter()
                    .all(|part| !part.is_empty() && part.len() <= 3 && part.parse::<u8>().is_ok())
        })
}

/// Produce a deterministic, bounded map for support bundles.
pub fn support_fields(snapshot: &CallDiagnosticsSnapshot) -> BTreeMap<String, String> {
    let mut fields = BTreeMap::new();
    fields.insert("schema_version".into(), snapshot.schema_version.to_string());
    fields.insert("audio_codec".into(), label(snapshot.codec.audio.as_deref()));
    fields.insert("video_codec".into(), label(snapshot.codec.video.as_deref()));
    fields.insert(
        "implementation".into(),
        label(snapshot.codec.implementation.as_deref()),
    );
    fields.insert("profile".into(), label(snapshot.codec.profile.as_deref()));
    fields.insert(
        "rtt_ms".into(),
        snapshot
            .rtt_ms
            .map_or_else(|| "none".into(), |v| v.to_string()),
    );
    fields.insert(
        "adaptation_count".into(),
        snapshot.counters.adaptation.to_string(),
    );
    fields.insert(
        "fallback_count".into(),
        snapshot.counters.fallback.to_string(),
    );
    fields.insert(
        "keyframe_count".into(),
        snapshot.counters.keyframe.to_string(),
    );
    fields.insert("track_count".into(), snapshot.counters.track.to_string());
    fields.insert("probe_count".into(), snapshot.counters.probe.to_string());
    fields
}

fn label(value: Option<&str>) -> String {
    value.map_or_else(|| "none".into(), redact_support_text)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn snapshot_is_bounded_and_counts_categories() {
        let diagnostics = CallDiagnostics::new();
        for _ in 0..100 {
            diagnostics.record(DiagnosticEvent::Fallback);
            diagnostics.record(DiagnosticEvent::MediaDrop);
        }
        let snapshot = diagnostics.snapshot();
        assert_eq!(snapshot.counters.fallback, 100);
        assert_eq!(snapshot.drops.packets, 100);
        assert!(snapshot.reasons.len() <= MAX_REASONS);
    }

    #[test]
    fn logs_are_throttled_then_reset() {
        let diagnostics = CallDiagnostics::new();
        let now = Instant::now();
        assert_eq!(
            (0..MAX_LOGS_PER_WINDOW + 1)
                .filter(|_| diagnostics.allow_log(now))
                .count(),
            MAX_LOGS_PER_WINDOW as usize
        );
        assert!(diagnostics.allow_log(now + LOG_WINDOW));
    }

    #[test]
    fn support_redaction_drops_content_and_sensitive_identifiers() {
        for value in [
            "message=hello",
            "payload pixels",
            "AUTHORIZATION=secret",
            "/home/user/file.wav",
            "device_id=abc",
            "192.168.1.2",
        ] {
            assert_eq!(redact_support_text(value), "details redacted");
        }
        assert_eq!(redact_support_text("codec=h264"), "codec=h264");
    }

    #[test]
    fn support_fields_contain_aggregates_only() {
        let diagnostics = CallDiagnostics::new();
        diagnostics.record(DiagnosticEvent::Adaptation);
        let fields = support_fields(&diagnostics.snapshot());
        assert_eq!(fields.get("adaptation_count"), Some(&"1".to_string()));
        assert!(!fields.contains_key("peer_id"));
        assert!(!fields.contains_key("payload"));
    }
}
