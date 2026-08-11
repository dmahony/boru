# UI-09 Mesh Health evidence

Task: `t_2fdb75b2`

## Implementation

The home screen now renders a structured Mesh Health card sourced directly from `IcedChat` mesh state:

- Health badge derives from `MeshHealth::Good`, `Degraded`, or `Offline`.
- Neighbor, direct, and relayed counts are displayed as separate summary cells.
- Mesh events carry their capture `Instant`; rows show semantic icons and relative age (`now`, seconds, or minutes).
- Five newest rows are visible in a fixed 156 px event viewport. The bounded event log remains scrollable, preventing rapid updates from changing card height.
- `View details` opens the existing `OpenConnectionDetails` diagnostic action for the full connection/transport details.

## Visual evidence

- `t_2fdb75b2_connecting_1280x800.png`: initializing/connecting state.
- `t_2fdb75b2_degraded_1280x800.png`: degraded state.
- `t_2fdb75b2_ready_1280x800.png`: healthy/ready state with live peer discovery.
- `t_2fdb75b2_connecting_600x720.png`: compact layout.

The connecting and degraded screenshots were captured by the existing Xvfb/xdotool UI-08 home harness after the UI-09 implementation was built. The healthy screenshot was captured with two isolated Xvfb displays to avoid the harness's same-display collision.

## Verification

- `cargo check --features gui --bin boru` passed.
- `cargo test --features gui --bin boru` passed: 558 tests.
- `cargo fmt --check` passed.
- `git diff --check` passed.
