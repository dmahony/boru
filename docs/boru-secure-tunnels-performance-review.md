# Boru secure tunnels: Phase 27 performance review

Reviewed the Phase 27 checklist against the tunnel implementation in `src/tunnel.rs`,
`src/tunnel/service.rs`, `src/tunnel/forwarding.rs`, and
`src/tunnel/local_listener.rs`.

## Findings

- **Memory per active tunnel:** bounded. The service caps configured tunnels at 32,
  received streams at 32, and local listener connections at 16 by default. The
  incoming stream queue is bounded to 32 entries and the per-peer attempt history
  is bounded to 256 peers. The service metadata uses one map entry per configured
  tunnel; stream forwarding does not buffer the full payload.
- **Task count:** one handler task is spawned per accepted QUIC connection with a
  service, and one forwarding task is spawned per accepted local TCP connection.
  The two forwarding directions are joined inside that task rather than spawning
  another task per direction. The limits above bound the normal task count.
- **TCP connection overhead:** one owner-side TCP connection is created per
  incoming tunnel stream, which is required to reach the configured local service.
  Failed TCP connection setup releases its reservation. The local listener reuses
  one shared Iroh/QUIC connection rather than opening a QUIC connection for each
  local application socket.
- **QUIC stream reuse:** each application socket gets one bidirectional QUIC stream
  on the shared connection. Sequential and simultaneous stream reuse are covered
  by the existing tunnel tests; no per-request QUIC connection is created.
- **Idle behavior:** there are no polling loops, keepalive loops, or application
  heartbeats. QUIC's transport manages its own protocol traffic. `TUNNEL_IDLE_TIMEOUT`
  is an inactivity deadline, not a lifetime guard: forwarding is activity-aware
  and every successfully transferred chunk in either direction resets the idle
  timer, so a healthy tunnel with ongoing traffic lives indefinitely while an
  inactive tunnel is closed after the configured idle period. The duration is
  configurable per service (bounded to 1 s–24 h), and the idle-close reason is
  logged distinctly from graceful close and I/O errors.
- **Shutdown behavior:** listener shutdown now propagates its cancellation token
  to every in-flight forwarding task, so stopping `LocalTunnelListener::run`
  does not leave active TCP/QUIC forwarding tasks behind. A cached connection is
  also cleared when it is observed closed, allowing a subsequent local connection
  to reconnect instead of repeatedly using a dead transport. Router shutdown
  remains event-driven: the protocol handler exits when the QUIC connection
  accept operation returns an error.

## Verification

`cargo test tunnel::local_listener --lib` passed (3 tests). The command emitted
only pre-existing warnings in unrelated modules. `cargo fmt --check` remains
non-clean because the repository already contains unrelated formatting changes in
other modified files; the tunnel listener change itself is rustfmt-compatible.
