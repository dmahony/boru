# Experimental VNC-over-Boru-Tunnel (Phase A)

This prototype reuses Boru's existing authenticated TCP tunnel. It does not
implement VNC, capture pixels, or transport credentials. The host must run a
TigerVNC-compatible server on `127.0.0.1:<port>` (normally `5900`); Boru
rejects every other source address. The viewer binds the received tunnel to
`127.0.0.1:<ephemeral-port>` and uses the VNC client's normal authentication.

Build with `--features gui,video-playback,terminal,experimental-vnc`, pair two
Boru instances, then choose **Share desktop using VNC Tunnel** from a friend's
profile. Start VNC on loopback, send the offer, and connect the viewer's VNC
client to the displayed loopback endpoint. Stop/disconnect when finished.

## A2 measurement procedure

For 720p and 1080p, run 60 seconds each of desktop motion, scrolling, and
terminal typing. Record VNC encoding, Direct/Relay route, client/server CPU,
and tunnel live byte counters. Measure median/p95 input-to-visible latency with
a timestamped on-screen timer; repeat with the direct path disabled to measure
relay overhead. This code-only milestone was not run against TigerVNC on the
LAN VMs in this workspace, so measurements are **verified-pending**.

Known failure modes: VNC not listening, expired/revoked offers, viewer port
collision (ephemeral fallback), peer disconnect, sleep/network-path changes
(reconnect policy retries), and explicit stop (cancellation/revocation closes
the mapped listener). A server bound to `0.0.0.0` or a LAN address is refused.