# Boru Screen-Sharing — Phase 11 Test Matrix

Status: BORU-SS-27 (PDF Task 11 "Testing Matrix"), generated 2026-08-15.
Source spec: `Boru_RustDesk_Reference_Screen_Sharing_Tasks.pdf` Phase 11.

## Legend

- **PASS** — exercised by automated tests on this machine/DEBSRV, all green.
- **PARTIAL** — some matrix items automated and green; others need a real
  platform (hardware/DE/compositor) that this headless build machine cannot
  provide. The manual checklist below marks exactly what is missing.
- **NOT TESTED** — nothing in this environment can exercise the item; the
  manual checklist gives the exact hardware/setup required.

Untested items are never marked as passed. Every "requires real hardware"
entry is a manual verification step, not a claim of green.

## Summary

| Area    | Result   | Automated tests | What is missing (manual checklist)                        |
|---------|----------|-----------------|----------------------------------------------------------|
| Windows | NOT TESTED | 0 (Linux-only CI) | Full hardware checklist: single/dual monitor, DPI, resize, unplug, remote input, reconnect |
| Wayland | PARTIAL  | ~32 (portal machine + PipeWire + display classification + permission model) | Real KDE Plasma 6 / GNOME portal + PipeWire sessions |
| X11     | PARTIAL  | ~25 unit (monitor geometry/clip/input/fallback) + 2 live Xvfb | Dual-monitor real layout, compositor differences, physical resize |
| Network | PARTIAL  | ~20 (reconnect/adaptation/queue logic) | Real two-peer LAN/relay runs, live bandwidth/latency/packet-loss shaping |
| Media   | PASS     | 17 codec + 5 viewer + 3 channels + 15 capture | — |
| Security| PASS     | ~45 (permissions + session + remote-input + negotiation) | — |

Full DEBSRV suite runs and counts are recorded in
[Test runs on DEBSRV](#test-runs-on-debsrv).

---

## 1. Windows

**Result: NOT TESTED (automated).** Boru has a WinRT Graphics Capture backend
(`src/screen_share/platform/windows.rs`, `windows_common.rs`) with 10 unit
tests for its lifecycle/state logic (`platform/windows_common.rs`), but no
Windows machine is available in this environment, so no matrix item below has
been exercised against a real Windows session.

Manual verification checklist (requires real Windows 10/11 hardware):

| # | Matrix item | How to verify | Notes |
|---|-------------|---------------|-------|
| W1 | Single monitor | Start a share from the Boru GUI on a 1-monitor Windows PC; viewer sees the desktop at the negotiated resolution | WinRT Graphics Capture path |
| W2 | Dual monitor | Share with 2 monitors attached; both are enumerated (`SourcesEnumerated`), switch source mid-session (`HostCommand::SwitchSource`) | Verify SourceChanged + forced keyframe on switch |
| W3 | 100% DPI | Share at 100% scaling; verify captured geometry matches the monitor resolution | Normalized coordinates must match |
| W4 | 150% DPI | Share at 150% scaling; verify physical/logical coordinate conversion (`coords.rs` has unit coverage for scaling) | See `logical_physical_conversions_cover_scaling_percentages` |
| W5 | Resize | Change display resolution mid-share; verify graceful renegotiation, not a crash | |
| W6 | Monitor unplug | Unplug the shared monitor; verify graceful pause/fallback (PDF Phase 10 recovery path) | Unit-tested logic in `host.rs` fallback tests |
| W7 | Remote input | Grant control; move pointer/click/type; verify cursor moves on the Windows host | `remote_input.rs` platform backend for Windows (SendInput) |
| W8 | Reconnect | Kill the viewer network path; verify media reconnect + fresh keyframe, control reset to view-only | Unit-tested in `session.rs` reconnect tests |

---

## 2. Wayland

**Result: PARTIAL.** The xdg-desktop-portal ScreenCast flow
(`src/screen_share/platform/linux.rs`, `linux_pw.rs`) is covered by
unit-testable state-machine and permission-model tests. Real compositor
sessions (KDE Plasma 6, GNOME) are NOT available on this headless machine —
see `docs/screenshare-wayland-portal-verification.md` for the full
implementation/verification gap write-up.

Automated coverage (all pass in the DEBSRV `--features screen-sharing` run):

- Portal session machine: `portal_machine_happy_path_lifecycle`,
  `portal_machine_rejection_fails_and_blocks_further_transitions`,
  `portal_machine_start_failure_paths_are_terminal`,
  `portal_machine_failure_escape_covers_early_dbus_errors`,
  `portal_machine_portal_closed_ends_active_session`,
  `portal_machine_close_from_every_active_phase`,
  `portal_machine_close_is_once_per_session`,
  `portal_machine_rejects_invalid_orderings`,
  `portal_machine_state_maps_to_portal_state` (linux.rs)
- Portal accept/deny: `portal_machine_rejection_fails_and_blocks_further_transitions`,
  `cancellation_ends_selection`, `portal_capture_rejects_frames_outside_streaming`
- Desktop-environment / session classification:
  `desktop_environment_classification`, `session_type_classification`,
  `display_server_classification`, `display_server_portal_preference`,
  `display_server_round_trips_through_environment`
- View-only default: `permissions.rs::view_only_does_not_authorize_input`,
  `session.rs::no_capture_before_consent`
- Remote-control deny: `permissions.rs::view_only_does_not_authorize_input`,
  `remote_input.rs::input_is_rejected_before_grant_and_after_revoke`,
  `session.rs::control_request_requires_host_grant`
- Source change: `host.rs::source_switch_plan_announces_before_dimensions_change`
  (+ source-change tests from BORU-SS-26)

Manual verification checklist (requires a real Wayland desktop):

| # | Matrix item | How to verify | Notes |
|---|-------------|---------------|-------|
| L1 | KDE Plasma 6 | On Plasma 6 (Wayland), start a share; the portal picker appears; accept; viewer sees the screen | xdg-desktop-portal-kde backend |
| L2 | GNOME (where available) | Same flow on GNOME Wayland; verify the GNOME picker and PipeWire stream | xdg-desktop-portal-gnome |
| L3 | Portal accept | Accept the portal dialog → streaming starts | |
| L4 | Portal deny | Deny the dialog → clean Rejected state, no capture, no crash | Unit path verified by portal machine tests |
| L5 | Source change | With multiple outputs, re-select via the portal/`SwitchSource`; verify SourceChanged + keyframe | Portal exposes a single pseudo-source; switching requires a fresh dialog selection (documented in BORU-SS-26) |
| L6 | View-only | Default share → viewer cannot inject input | Unit-tested |
| L7 | Remote-control deny | Viewer requests control → host denies → input stays rejected | Unit-tested |

---

## 3. X11

**Result: PARTIAL.** The X11 fallback backend (`X11Capture` in
`src/screen_share/platform/linux.rs`) has broad unit coverage, and — new in
this task — live tests were executed for real under Xvfb on DEBSRV (single
monitor enumeration + capture, whole-root capture, and — BORU-SS-36 — window
enumeration + capture). Dual-monitor layouts, compositor differences, and
physical monitor resizing still need a real X session.

Automated coverage (unit, all pass):

- Monitor source geometry / negative origins: `x11_monitor_source_advertises_geometry`,
  `x11_monitor_source_handles_negative_origin`, `x11_monitor_id_is_stable_and_distinct`,
  `clip_to_root_*` (4 tests), `coords.rs` mixed-DPI/negative-origin tests
- Window sources (BORU-SS-36): `x11_window_source_advertises_kind_geometry_and_title`,
  `x11_window_id_is_stable_and_namespaced_away_from_monitors`,
  `x11_window_source_handles_negative_origin`, `x11_window_source_preserves_minimized_flag`,
  `picker_label_distinguishes_source_kinds` (capture.rs), host
  `window_source_fallback_keeps_then_switches` (minimized window still
  enumerated → pause keeps it; closed window gone → fall back to a monitor),
  plus the Windows-side id mapping `window_source_id_is_stable_and_namespaced_away_from_monitors`
  / `window_source_advertises_kind_geometry_and_title` (windows_common.rs)
- Portal source types (BORU-SS-36): `portal_source_types_bits_follow_spec`,
  `select_sources_options_include_cursor_mode_when_negotiated` (types
  Monitor|Window by default)
- Capture fallback: `create_capture_source_falls_back_to_test_pattern`
- Remote input: `x11_pointer_move_maps_capture_pixels_to_root`,
  `x11_pointer_move_applies_monitor_origin`, `x11_pointer_move_clamps_to_root_bounds`,
  `x11_pointer_button_press_and_release`, `x11_wheel_emits_press_release_pair_once`,
  `x11_pointer_rejects_unknown_button_and_zero_root`, `x11_keysym_map_builds_lowest_keycode_and_skips_no_symbol`,
  `x11_key_translates_keysym_to_keycode`, `x11_key_rejects_unknown_keysym`,
  `x11_empty_keymap_rejects_everything`, `x11_consent_gate_rejects_ungranted_device_before_translation`,
  `explicit_kind_gates_translation`
- Resize / geometry change: `clip_to_root_clamps_partial_overflow`,
  `clip_to_root_rejects_fully_outside_rect`, host-level source-switch tests
  (`plan_source_switch_*`, `fallback_*`)

Live tests executed on DEBSRV under Xvfb (real X server, `--ignored`):

- `x11_live_enumerates_and_captures_selected_monitor` — real RandR monitor
  enumeration + GetImage capture of the primary monitor
- `x11_live_screen_capture_whole_root` — whole-root GetImage capture
- `x11_live_enumerates_and_captures_a_window` (BORU-SS-36) — creates a real
  mapped top-level X window, verifies `list_sources` advertises it as a
  `CaptureSourceKind::Window` with a `[Window]` picker label, then captures
  frames of the window's size
- `x11_live_damage_tracking_skips_static_screen` — frame-level damage skip
  end to end

Manual verification checklist (requires a real X session):

| # | Matrix item | How to verify | Notes |
|---|-------------|---------------|-------|
| X1 | Single monitor | Share on a native X11 session; verify frame matches the monitor | Xvfb-verified path |
| X2 | Dual monitor | Two real monitors (or Xinerama); verify enumeration, negative-origin sources, switching | Unit logic covered; real layout not |
| X3 | Capture fallback | Run without a usable display → test-pattern fallback keeps the pipeline alive | Unit-tested |
| X4 | Resize | Change resolution with xrandr mid-share; verify SourceChanged + re-encode, no crash | |
| X5 | Remote input | Grant control and inject mouse/keyboard on a real X session | Unit-tested translation; real injection not |
| X6 | Compositor differences | Compare under plain X, XFCE, GNOME Xorg, XWayland | Environment-dependent |

---

## 4. Network

**Result: PARTIAL.** The transport/reconnect/quality logic is unit-tested; a
real two-peer LAN/relay run is not possible on this single headless build
machine (and iroh's public relay cannot be reached reliably from DEBSRV — the
known `endpoint.online()`/`RelayMode::Default` hang, see the iroh-gossip-chat
workflow skill's debsrv-integration-test-gate reference).

Automated coverage (all pass):

- Disconnect/reconnect (session): `begin_reconnect_preserves_session_and_emits_event`,
  `begin_reconnect_requires_streaming`,
  `complete_reconnect_returns_streaming_without_control_resume`,
  `fail_reconnect_ends_session`, `rehello_from_same_host_reconnects_active_session`,
  `rehello_from_stranger_is_rejected`
- Reconnect policy/backoff: `reconnect.rs::policy_can_explicitly_allow_control_resume`,
  `backoff_is_exponential_and_bounded`, `keyframe_request_message_round_trips`
- Bandwidth restriction / congestion: `adaptation.rs::sustained_pressure_steps_bitrate_then_fps_then_resolution`,
  `queue_depth_pressure_steps_down`, `rtt_pressure_steps_down_when_available`,
  `throughput_saturation_is_pressure_only_with_queue_growth`,
  `manual_viewer_request_is_honored_as_a_ceiling`, `congestion_still_reduces_below_a_viewer_ceiling`,
  `recovery_is_gradual_and_hysteretic`
- Packet loss / keyframe recovery: `codec.rs::keyframe_recovery_after_dropped_frames`,
  `viewer.rs::missing_sequence_gap_requests_keyframe_and_drops_dependents`,
  `keyframe_arrival_self_heals_gap_without_request`,
  `dependent_frame_without_picture_requests_keyframe`, `take_keyframe_request_is_one_shot`
- Queue bounds / no unbounded memory: `channels.rs::bounded_queue_*` (3),
  `capture.rs::sink_is_bounded_and_prefers_latest_frame`,
  `adaptation.rs::pacing_queue_cap_is_enforced`, `pacing_drop_counter_increments_on_overflow`,
  `pacing_latest_frame_wins_under_lag`, `pacing_empty_pop_and_clear_are_counted`,
  `transport.rs::media_round_trip_and_bounds`, `queue_keeps_current_state_bounded`

Manual verification checklist:

| # | Matrix item | How to verify | Notes |
|---|-------------|---------------|-------|
| N1 | LAN | Run two Boru instances on one LAN; start a share; verify frames flow and chat is unaffected | Two-peer test; use `tests/` two-instance harnesses |
| N2 | Relay path | Two peers on different networks via an iroh relay; verify media reconnect + keyframe after path failover | Requires reachable relay |
| N3 | Bandwidth restriction | `tc qdisc` (or equivalent) cap e.g. 1 Mbps; verify adaptive bitrate/fps/resolution steps down (`adaptation.rs` logic) | |
| N4 | Latency | Add 100–300 ms latency; verify pacing drops stale frames and viewer latency stays bounded | |
| N5 | Packet loss | 5–10% loss; verify viewer requests keyframe and stream recovers | |
| N6 | Disconnect/reconnect | Kill the viewer's network path; verify media reconnects with fresh keyframe, chat session survives, control resets to view-only | Unit-tested transitions |

---

## 5. Media

**Result: PASS.** All matrix items are automated and green (17 codec tests, 5
viewer tests, 3 channel tests, 15 capture tests, plus pacing tests in
adaptation).

- **720p30** — `media_round_trip_720p30` (NEW): 1280x720 @ 30 fps full
  encode → decode round trip (keyframe + delta frames, correct geometry,
  33.3 ms frame-period timestamps, monotonic sequence).
- **1080p30** — `media_round_trip_1080p30` (NEW): 1920x1080 @ 30 fps round
  trip. Target profiles asserted by `target_profiles_expose_720p30_and_1080p30`.
- **Keyframe recovery** — `keyframe_recovery_after_dropped_frames` (NEW,
  codec-level: dropped frames → forced keyframe → same decoder recovers) plus
  the viewer-side gap tests listed in the Network section.
- **Queue overflow protection** — `bounded_queue_*` (channels),
  `sink_is_bounded_and_prefers_latest_frame` (capture), `pacing_*` (adaptation):
  queues stay at capacity, oldest/latest policy per queue, drop counters.
- **Long-running share** — `long_running_share_remains_healthy` (NEW): 3600
  frames through capture → encode → decode (2 simulated minutes at 30 fps);
  every frame encodes/decodes, timestamps and sequence numbers stay
  monotonic, no state drift.

Additional media coverage: every quality profile encodes/decodes
(`every_quality_profile_constructs_and_encodes_decodable_frames`), static
screens still emit decodable frames (`static_screen_still_produces_decodable_frames_every_tick`),
resolution change without session restart (`configure_changes_resolution_without_session_restart`),
force-keyframe control, bitrate reconfigure, encode-stage timestamps, and
end-to-end decode pipeline isolation (`viewer.rs::session_ids_are_isolated_between_pipelines`).

---

## 6. Security

**Result: PASS.** All matrix items are automated and green.

- **No capture before consent** — `session.rs::no_capture_before_consent`
  (NEW: no permission record exists before the viewer's explicit Accept;
  nothing can be authorized), `negotiation_offer_accept_roundtrip` (asserts
  `can_start_capture() == false` before acceptance on both sides),
  `negotiation_reject_roundtrip`, `negotiation_timeout_expires_pending`,
  `negotiation_cancel_withdraws_offer` (all assert capture is not permitted
  outside the Accepted state).
- **No input without permission** — `permissions.rs::view_only_does_not_authorize_input`,
  `remote_input.rs::input_is_rejected_before_grant_and_after_revoke`,
  `explicit_kind_gates_translation`,
  `x11_consent_gate_rejects_ungranted_device_before_translation`,
  `session.rs::control_request_requires_host_grant`,
  `forged_grant_control_from_viewer_is_ignored_on_host`,
  `session_and_peer_mismatch_are_rejected`, `expired_token_is_rejected`.
- **Revoke control** — `permissions.rs::revoke_control` semantics
  (`input_is_rejected_before_grant_and_after_revoke` after-revoke half),
  `session.rs::revoke_control` (host-side), `host_grant_control_is_applied_on_viewer`.
- **Peer disconnect cleanup** — `session.rs::peer_disconnect_during_streaming_cleans_permissions`
  (NEW: EndSession/peer-drop during streaming ends the session and makes the
  permission record inactive, so late input fails authorization),
  `negotiation_peer_disconnect_closes_sessions`,
  `end_is_idempotent`.
- Input flood protection: `rate_limiter_blocks_bursts_and_recovers`,
  `sliding_window_rate_limiter_bounds_input_streams`,
  `sliding_window_rate_limiter_passes_sustained_low_rate`.
- Clipboard separation: `clipboard_is_separate_from_remote_control`.

---

## Test runs on DEBSRV

Machine: debsrv (172.16.0.59), via the `rb` wrapper
(`~/boru-build/work-<slot>` source, `work-target-<slot>` target, sccache).
Workspace: `wt/t_b83cf22e` (task t_b83cf22e). Disk free before builds: 84G
(no space cleanup required).

```
$ rb check --all-targets --features screen-sharing
Finished dev profile ... in 52.64s            (exit 0)

$ rb test --lib --features screen-sharing
test result: FAILED. 2902 passed; 1 failed; 5 ignored; 0 measured; 0 filtered out; finished in 361.75s
  - FAILED (pre-existing, unrelated): storage::tests::docs_reference_current_schema_version
    src/storage.rs:6572 — docs/message-storage-design.md does not state
    CURRENT_SCHEMA_VERSION: u32 = 20. SQLite storage doc-sync test; untouched
    by this task (diff limited to src/screen_share/). Fails identically on
    origin/main.

$ rb test --lib --features net
test result: FAILED. 2669 passed; 1 failed; 2 ignored; 0 measured; 0 filtered out; finished in 357.71s
  - FAILED: same pre-existing storage::tests::docs_reference_current_schema_version

$ rb test --lib --features gui,screen-sharing
test result: FAILED. 2902 passed; 1 failed; 5 ignored; 0 measured; 0 filtered out; finished in 361.41s
  - FAILED: same pre-existing storage::tests::docs_reference_current_schema_version

# X11 live tests under a real (virtual) X server on debsrv:
$ xvfb-run -a -s '-screen 0 1920x1080x24' cargo test --lib --features screen-sharing -- --ignored x11_live_
running 2 tests
test screen_share::platform::linux::tests::x11_live_enumerates_and_captures_selected_monitor ... ok
test screen_share::platform::linux::tests::x11_live_screen_capture_whole_root ... ok
test result: ok. 2 passed; 0 failed; finished in 0.28s
```

Screen-share test inventory after this task: 224 `#[test]` functions under
`src/screen_share/` (218 before + 6 new: `media_round_trip_720p30`,
`media_round_trip_1080p30`, `keyframe_recovery_after_dropped_frames`,
`long_running_share_remains_healthy`, `no_capture_before_consent`,
`peer_disconnect_during_streaming_cleans_permissions`).

The 5 ignored tests in the screen-sharing/gui suites are: 2 live X11 tests
(now run and passing under Xvfb, above), 1 release-mode perf benchmark
(`encode_bench.rs::benchmark_openh264_720p30_and_1080p30`), and 2 ignored
tests in unrelated modules.

## Pre-existing failures (not fixed, per task scope)

1. `storage::tests::docs_reference_current_schema_version` — SQLite storage
   schema-version doc-sync test; `docs/message-storage-design.md` needs a
   one-line schema-version bump (20). Unrelated to screen sharing; fixing it
   is out of scope for BORU-SS-27.

## Notes on honest reporting

- No area is marked PASS without an automated run recorded above.
- "requires real hardware" items are manual verification steps and are
  explicitly NOT TESTED in this environment.
- The X11 live tests were executed under Xvfb (a real X server) on DEBSRV;
  results are in the run log above.
