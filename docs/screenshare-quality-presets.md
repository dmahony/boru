# Screen-share quality presets: LAN vs relay (BORU-SS-39)

Phase 14 (PDF T14) capability: choose the initial stream quality from the
connection path (direct/LAN vs relayed) and surface the active
preset/adaptation state in the screen-share panel.

## Why presets

A direct/LAN path (peer reachable over IP without a relay) has high
bandwidth headroom and low latency, so the stream can afford a higher
bitrate and the crisper `HighQuality` encode. A relayed path (traffic
bounced through the configured iroh relay server) has lower effective
bandwidth and higher latency, so a conservative bitrate/fps ceiling and the
faster `LowLatency` encode keep the stream usable without saturating the
relay. Choosing the ceiling from the path avoids both wasting LAN headroom
and overshooting a relay.

## Presets

| Preset | Path kind | Bitrate | FPS | QualityProfile |
|---|---|---|---|---|
| `LanHigh` | `PathKind::Direct` | 2× base (cap 12 Mbps) | capture fps | `HighQuality` |
| `Balanced` | `PathKind::Unknown` (default) | base | capture fps | `Balanced` |
| `RelayConservative` | `PathKind::Relay` | 50% base (floor 500 kbps) | min(capture, 20) | `LowLatency` |

The bitrate/fps multipliers are relative to the capture session's base
rates (the default 4 Mbps / 30 fps or whatever a future negotiation picks),
so the presets scale with the negotiated geometry.

## Wiring

- `QualityPreset` (`src/screen_share/presets.rs`): the named preset enum,
  `for_path(PathKind)` mapping, `apply_to_config(&CodecConfig)` rate/profile
  application, and the stable `as_u8`/`from_u8` round-trip.
- `selected_path_kind(&iroh::endpoint::Connection)` (`transport.rs`):
  classifies the selected QUIC path (`Direct`/`Relay`/`Unknown`) using
  `Connection::paths()` — the documented iroh API for path inspection.
- Host (`host.rs`):
  1. **Initial config**: after the viewer connection is established, the
     host reads `path_kind()`, selects the initial preset, and applies it to
     the encoder config before streaming starts. The pre-preset config is
     kept as the `preset_reference` so later changes recompute the ceiling
     relative to the capture rates, not the currently adapted level.
  2. **User override**: `HostCommand::SetQualityPreset(Option<QualityPreset>)`
     (the UI preset buttons) applies the chosen ceiling immediately via
     `AdaptiveQuality::override_ceiling`. `None` restores the path-derived
     auto preset. A manual override wins over the path-derived preset until
     the session ends.
  3. **Mid-session path change**: the streaming loop polls
     `selected_path_kind(&connection)` on each adaptive tick (~1 Hz). When
     the selected path switches Direct↔Relay and no manual override is set,
     the new path's preset ceiling is fed to `AdaptiveQuality::set_ceiling`.
- `AdaptiveQuality` (`adaptation.rs`):
  - `set_ceiling` — the path-change signal. **Conservative, never a sudden
    jump**: a lowered ceiling clamps the current config immediately (never
    overshoot a relay); a raised ceiling is headroom only — the current
    config is preserved and recovery climbs toward the new base gradually
    (one half-gap step per 8 clean ticks, the same hysteresis the
    congestion recovery uses).
  - `override_ceiling` — the user override. Applies immediately in both
    directions (the sharer explicitly asked for it).
- Metrics/UI (`stats.rs`, `app/chat.rs`): `ScreenShareSessionMetrics` now
  carries `path_kind`, `preset`, and `adaptive_level`, published ~1 Hz via
  `SessionEvent::Metrics`. The sharer panel shows a visible
  `Quality: <preset> · Path: <path> · Adaptive L<level>` line plus preset
  buttons (LAN High / Balanced / Relay / Auto); the dev overlay's
  `screen_share_metrics_lines` gained a preset/path/level line too.

## Public API / spec used

- iroh `Connection::paths()` / `PathList` for Direct-vs-Relay:
  https://docs.iroh.computer
- Existing `QualityProfile` + `AdaptiveQuality` (`codec.rs`, `adaptation.rs`)
- OpenH264 `SEncParamExt` (bitrate/fps): https://github.com/cisco/openh264
- iced panel UI (existing screen-share panel in `examples/iced_chat`)

Licensing: all permissive (iroh MIT/Apache-2.0, OpenH264 BSD 2-clause). No
AGPL material is used; RustDesk remains reference-only.

## Tests

- `presets.rs`: path→preset mapping, bitrate/fps effects, identity of
  `Balanced`, wire round-trip, relative-application, geometry preservation.
- `adaptation.rs`: lowered ceiling clamps immediately; raised ceiling never
  jumps and recovers gradually; `override_ceiling` applies immediately in
  both directions; ceiling changes preserve capture geometry.
- `stats.rs`: `ScreenShareSessionMetrics` carries the new fields.

## Verification

```
rb check --all-targets --features screen-sharing
```
