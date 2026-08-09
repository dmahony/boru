# BORU-CALL-10 validation report

Date: 2026-08-08 UTC
Commit under test: `8936476e` (this validation branch, including the merged call changes)

This report separates automated/synthetic checks from tests that require a human-operated
camera, microphone, speakers, and visible GUI. A passing synthetic test is not presented as
proof of real device behavior.

## Execution summary

| Area | Result | Evidence |
|---|---|---|
| Call manager, shutdown, stats, generation and policy unit tests | PASS (41 tests) | `rb test --lib --features net,voice-calls,video-calls -- manager` |
| Synthetic Opus encode/jitter/decode | PASS (2 tests) | `rb test --test call_audio_integration --features voice-calls` |
| Synthetic H.264 encode/fragment/reorder/reassemble/decode | PASS (2 tests) | `rb test --test call_video_integration --features video-calls` |
| Headless two-endpoint voice acceptance flow | PASS (1 test) | `rb test --test voice_acceptance --features net,voice-calls` |
| Feature-gated compile check | PASS | `rb check --features net,voice-calls,video-calls` |
| Application-level call encryption audit | PASS | `scripts/audit-call-crypto.sh` |
| Local host hardware inventory | PARTIAL | `evidence/cross-machine-probes.txt` |
| Real GUI/audio/video call | NOT EXECUTED | Requires operator-controlled device/GUI session |

The targeted test builds reported pre-existing warnings (unused imports/variables and an
unfulfilled lint expectation); no command failed because of them.

## Available test machines

SSH reachability was verified with BatchMode key authentication on all three Linux hosts:

| Host | OS/architecture | Audio nodes | Camera nodes | Intended path |
|---|---|---:|---:|---|
| local host | Ubuntu, x86_64 | present | none | build/test host |
| 172.16.0.54 | Linux, x86_64 | present | none detected | LAN Linux peer |
| 172.16.0.55 | Linux, x86_64 | present | none detected | LAN Linux peer |
| 172.16.0.118 (`dragon`) | Linux, aarch64, VPN route | present | `/dev/video0`, `/dev/video1` | remote/VPN Linux peer |

`ffmpeg` and `gst-inspect-1.0` are installed on all three remote hosts. `arecord` is
available on dragon only. The local host has `/dev/snd` nodes but no `/dev/video*`; `arecord`
and `v4l2-ctl` are not installed locally. These observations are recorded verbatim in
`evidence/cross-machine-probes.txt`.

No Windows machine or Windows build/runtime was available in this execution environment.

## Cross-machine matrix

Status meanings: PASS means the complete real-device scenario was performed and observed;
NOT RUN means hardware or an operator session was unavailable; PARTIAL means infrastructure
was checked but the user-facing call was not completed.

| Scenario | Same LAN | Relay/different network | Status | Reason/evidence |
|---|---|---|---|---|
| Windows ↔ Windows | NOT RUN | NOT RUN | BLOCKED ON HARDWARE | No Windows endpoints available |
| Linux x86_64 VM-A (172.16.0.54) ↔ Linux x86_64 VM-B (172.16.0.55) | NOT RUN | NOT RUN | PARTIAL | SSH and media runtime probes pass; neither VM exposes a camera, and no real GUI call was operated |
| Linux x86_64 VM ↔ dragon aarch64 (172.16.0.118) | NOT RUN | NOT RUN | PARTIAL | SSH and dragon camera/audio probes pass; no deployed compatible binary and operator GUI session were available |
| Windows ↔ Linux | NOT RUN | NOT RUN | BLOCKED ON HARDWARE | No Windows endpoint available |

For the Linux LAN row, use direct/mDNS discovery first. For the relay row, stop or firewall
one side's direct LAN path and start both endpoints with the configured relay URL; do not mark
a relay test successful merely because the endpoint is online.

## Real-device checklist

Run each item for every available endpoint pair. Record the pair, path (LAN or relay), time,
Boru version/hash, and the observation in the result column before calling the scenario PASS.
The same checklist applies to Windows ↔ Windows, Linux ↔ Linux, and Windows ↔ Linux.

### Setup and identity

- [ ] Start exactly one Boru process per endpoint with a fresh test data directory, unless
      preserving identity is intentional.
- [ ] Verify the displayed version/hash matches the binary built for this validation branch.
- [ ] Confirm both peers are authorized/friends and appear as discovered/connected.
- [ ] For LAN: confirm the peer route is direct/mDNS where diagnostics expose it.
- [ ] For relay: disable direct reachability (or use separate networks), confirm both peers
      use the same configured relay, and record the relay path in diagnostics.
- [ ] Capture endpoint status and relevant `logs/boru.log` excerpts into the evidence folder.
- [ ] Confirm there is one Boru process and one GUI window per endpoint; stale processes can
      invalidate all observations.

### Voice

For each direction, use a spoken phrase or a known tone and have the receiving operator
confirm audibility without relying on the sender's local monitor.

- [ ] Caller hears callee.
- [ ] Callee hears caller.
- [ ] Toggle mute on caller: remote audio stops, then resumes after unmute.
- [ ] Toggle mute on callee: remote audio stops, then resumes after unmute.
- [ ] Unplug/replug the output device while active; Boru remains alive and the call can
      continue or reports a bounded device error instead of crashing.
- [ ] Hang up from caller; both sides leave the active state promptly (target: immediate,
      not a multi-second UI stall).
- [ ] Start a second call immediately using the same peers; audio works without restarting
      Boru.
- [ ] Repeat at least once over the relay path.

### Video

- [ ] Local camera preview appears with the expected orientation and dimensions.
- [ ] Remote video appears on the receiving endpoint.
- [ ] Turn camera off; remote side shows the off state and audio remains usable.
- [ ] Turn camera on again; remote video returns without restarting the call.
- [ ] Use a non-16:9 source and verify it is contained/aspect-ratio preserving (no stretching).
- [ ] Apply controlled packet loss/jitter or a constrained link. Verify video drops frames or
      degrades quality instead of accumulating multi-second latency.
- [ ] Restore the link and verify keyframe recovery produces a current picture.
- [ ] During video degradation, verify the voice path remains intelligible and does not
      starve behind video traffic.
- [ ] Hang up and repeat a second video call.

### Failure and cleanup

- [ ] Close the call with the normal hangup control and verify both endpoints emit one terminal
      call event, with no stale active call remaining.
- [ ] Kill/unplug a peer and verify the surviving endpoint reports connection loss without
      wedging the GUI.
- [ ] Save the endpoint logs and screenshots before cleaning test data.
- [ ] Stop all test Boru instances and verify no stale process or SSH tunnel remains.

## Exact Linux execution procedure

1. Build the Linux GUI binary with all call/video features using the repository's remote build
   wrapper: `rb build --example boru --features gui,video-playback,terminal`.
2. Deploy the matching binary and `scripts/boru-test-instance.sh` to each selected Linux host.
   For dragon, use an `aarch64-unknown-linux-gnu` build and launch on its xrdp display (`:10`),
   as described in the iroh-gossip-chat deployment reference.
3. Use fresh data directories for a clean pair. Start one instance per host; record the
   version/hash from the node status and verify one process per host.
4. Establish the LAN pair first. Wait for reciprocal discovery, then perform the Voice and
   Video checklists above. Capture GUI screenshots and each endpoint's `logs/boru.log`.
5. For relay validation, place the endpoints on different networks or block the direct LAN
   route, configure the relay explicitly, wait for relay connectivity, and repeat the full
   checklist. A diagnostic probe alone is insufficient: it does not exercise the normal call
   media path.
6. For device recovery, begin an active call, unplug/replug the selected output and camera,
   and record whether the call remains usable, terminates with a bounded error, or crashes.
7. Repeat the second-call checks after every terminal path, then archive logs/screenshots under
   `evidence/<pair>/<lan-or-relay>/`.

For the existing x86_64 VMs, the deployment scripts use VM-A MCP port 9054/display 98 and
VM-B MCP port 9055/display 99 for headless runs; visible GUI checks require desktop mode and
an active X session. Do not treat an xvfb process as proof that a physical camera or speaker
worked.

## Evidence produced in this run

- `evidence/cross-machine-probes.txt`: timestamped local and remote OS, runtime, audio,
  and camera device inventory.
- The command outputs listed in the execution summary are the reproducible automated evidence.
  They exercise the media codecs, packetization/reassembly, jitter path, actor lifecycle, and
  two-endpoint signalling, but intentionally do not claim physical-device behavior.

## Final disposition

Automated call-path validation is PASS. Real-device cross-machine validation remains OPEN and
requires a human with at least one Windows endpoint for the Windows rows, plus a visible GUI
session and actual microphone/camera/speaker for the Linux rows. The checklist and exact
procedure above are ready for that operator run; no real-device PASS is claimed here.
