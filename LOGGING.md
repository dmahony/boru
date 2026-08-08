# Call logging policy

This policy applies to the live voice/video call subsystem under `src/call`.
It is deliberately narrow: logs help diagnose call setup and aggregate quality
without becoming a second media archive.

## Allowed fields

Call lifecycle logs may contain:

- lifecycle state or transition (`incoming`, `ringing`, `connecting`, `active`,
  `ended`, or `failed`);
- a short, redacted peer identifier (the existing short public-key formatter);
- the short call identifier (`CallId`'s display form, not its raw bytes);
- negotiated codec name;
- negotiated video resolution;
- configured bitrate;
- aggregate packet-loss count or rate;
- aggregate jitter;
- one terminal reason from `CallEndReason`.

Quality values must be aggregate values over a bounded reporting interval or the
whole call. Do not emit one log event per packet, frame, sample, or fragment.
Counters and rates must not include payload contents.

## Forbidden fields and operations

Call logs must never contain:

- raw microphone samples or camera pixels;
- encoded Opus, H.264, or other media packet bytes;
- complete media datagrams, frame buffers, or codec access units;
- cryptographic private keys, secrets, ticket material, or complete identities;
- message text, file contents, filesystem paths, or unrelated user data.

The real-time media paths (`audio`, `video`, and `media`) must not log at all.
They process sensitive media at high frequency and have no useful diagnostic
reason to format or emit per-sample/per-packet records. Lifecycle and aggregate
quality logging belongs at the call-control/actor boundary, where values can be
explicitly selected and redacted before emission.

## Implementation rules

1. Prefer structured `tracing` fields with the allow-listed names above.
2. Use short identifiers only; never add `Debug` output of a peer, call, packet,
   frame, or codec buffer to a log event.
3. Log terminal state once, with the terminal reason, rather than repeatedly
   logging cleanup attempts.
4. Keep logging disabled or low-volume by default through the application's
   normal `tracing` subscriber configuration. This policy does not authorize
   adding a new telemetry destination or network sink.
5. When a new call log is added, update the allow-list review and the source
   guard in `tests/call_logging_policy.rs` if the log is outside the media hot
   paths.

`tests/call_logging_policy.rs` statically guards the high-risk boundary. The
source-level guard is intentional: media features are optional and may not run
on a CI host without audio/video devices.
