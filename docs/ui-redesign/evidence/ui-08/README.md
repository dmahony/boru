# UI-08 home evidence

Task: `t_ed8af7fe` (home layout, greeting, and connection hero)

## State mapping

The home hero derives its visual variant from `MeshHealth`, peer counts, and sender/relay reachability in `home_connection_variant`:

| Application state | Hero variant | Header pill | Hero treatment |
| --- | --- | --- | --- |
| `MeshHealth::Good`, no sender/peers | Starting | Starting | Amber retry icon, bootstrap headline |
| `MeshHealth::Good`, sender/relay reachable, no peers | Connecting | Connecting | Amber retry icon, waiting-for-peers headline |
| `MeshHealth::Good`, at least one connected peer | Ready | Connected | Green check icon and soft green surface |
| `MeshHealth::Degraded(reason)` | Degraded | Degraded | Amber mesh icon, reason, Details action |
| `MeshHealth::Offline(reason)` | Offline | Offline | Red offline icon, reason, Retry + Details actions |

Offline and degraded are evaluated before peer counts, so a stale peer count cannot produce a green Ready state.

## Visual evidence

- `t_ed8af7fe_connecting_1280x800.png`: wide connecting state; greeting, amber hero, static node motif, and right rail are visible without clipping.
- `t_ed8af7fe_degraded_1280x800.png`: wide degraded state; reason and Details action are visible.
- `t_ed8af7fe_ready_1280x800.png`: two-instance local discovery produced one direct peer; green Connected/Ready state and Online Now rail are visible.
- `t_ed8af7fe_connecting_600x720.png`: compact connecting state; hero and right rail reflow vertically and the content remains horizontally contained.

The node motif is an embedded static SVG (`assets/icons/network-motif.svg`), rendered only on non-compact layouts. It has no timer, animation, or per-frame state, so it adds no continuous idle animation/CPU loop.

## Verification

- `cargo check --features gui --bin boru` passed.
- `cargo test --features gui --bin boru` passed (558 tests).
- `cargo fmt --check` passed.
- `git diff --check` passed.
