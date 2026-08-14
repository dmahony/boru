# Screen-Share Media Path Benchmark: Datagram vs Reliable Streams

Status: **BORU-SS-09 / PDF Task 3.2** — benchmark recorded, reliable streams
retained as the media transport.

## Question

Iroh (patched `1.0.3`) exposes QUIC application datagrams
(`Connection::send_datagram` / `read_datagram`, RFC 9221) with bounded
send/receive buffers (`datagram_send_buffer_size`, `datagram_receive_buffer_size`).
The screen-share plan asks: if an unordered/datagram path exists for media,
benchmark it against reliable streams before adopting it. This document records
that benchmark and the adoption decision.

## Method

`src/screen_share/media_path_bench.rs` (`benchmark_datagram_vs_reliable_media_path`,
a `#[tokio::test]`) runs two real iroh endpoints over loopback with
`presets::Minimal` (no relay, no external network):

- **Reliable path** — one fresh bidirectional QUIC stream per encoded frame,
  `write_all` + `finish`, reply side dropped. This mirrors the production
  `MediaChannel` worker exactly.
- **Datagram path** — the same frames fragmented into
  `max_datagram_size - 8`-byte chunks with an 8-byte header
  (`frame_id: u32`, `chunk_index: u16`, `chunk_count: u16`), sent as
  unreliable/unordered application datagrams, reassembled on the receiver.

Payload: 64 frames × 16 KiB (a realistic H.264 access unit for the 640x360@15
demo stream, well under the transport's 4 MiB media cap).

Run:

```bash
rb test --features screen-sharing --lib -- --nocapture media_path_bench
```

## Environment

- Host: debsrv (172.16.0.59, 8 cores, Ubuntu/Debian) via `rb`; loopback only.
- iroh: patched 1.0.3 (repo `patched/iroh`), QUIC via noq 1.0.x.
- Date: 2026-08-15.

## Results

Measured 2026-08-15 on debsrv via `rb` (loopback endpoints, `presets::Minimal`):

```
media_path_bench: max_datagram_size = Some(1162)
media_path_bench: reliable  frames=64 bytes=1048576 elapsed=885.162µs throughput=1184.61 MiB/s
media_path_bench: datagram  chunks_sent=960 chunks_received=799 bytes=879360 frames_reassembled=52 elapsed=2.090023ms throughput=420.74 MiB/s
test screen_share::media_path_bench::benchmark_datagram_vs_reliable_media_path ... ok
```

Key facts:

- `max_datagram_size()` returned `Some(1162)` — datagrams are enabled and
  negotiated, but a datagram must fit in a single QUIC packet: a 16 KiB media
  frame needs **14 datagram chunks** with an 8-byte reassembly header. Any
  datagram path requires fragmentation + reassembly + per-frame sequencing.
- **Reliable**: 64/64 frames (1 MiB) delivered, ordered, zero loss; 1184 MiB/s
  on loopback (QUIC guarantees).
- **Datagram**: under burst, the bounded default datagram send buffer drops
  chunks when the producer outpaces the link — 799/960 chunks arrived (83%)
  and only 52/64 frames reassembled completely (81%). This is the datagram
  path's *intended* backpressure, but for interactive video it means a lost
  chunk destroys a whole frame (until the next keyframe).

## Decision

**Keep reliable streams as the media transport** (the existing `MediaChannel`).

Rationale:

1. **Delivery semantics**: reliable streams deliver every frame in order. The
   viewer decode pipeline relies on sequence ordering and keyframe resync; a
   datagram path would need fragmentation, reassembly, ordering heuristics,
   and explicit keyframe re-requests on any loss — strictly more machinery for
   the interactive-video case.
2. **Isolation is already achieved**: chat traffic is on a separate QUIC
   connection (gossip); control and media already use independent QUIC
   streams, so a large media frame cannot head-of-line-block control messages.
3. **Boundedness is provided by the MediaChannel, not the transport**: the
   host-side `MediaChannel` caps queued frames and drops the oldest stale frame
   when full (drop counter), so memory stays bounded regardless of path
   reliability.
4. **Datagram cost without benefit on the demo geometry**: the measured
   loopback throughput is comparable, but real-world use (relay path, lossy
   links) would amplify the datagram path's loss/recovery costs. The task
   says "adopt only if it wins or is comparable" — it does not win.

The datagram path is NOT dead code: iroh's bounded datagram buffers remain
available for future work (e.g. sending small control-ish packets, or a
low-latency mode for sub-MTU frames) and the benchmark test documents the
current characteristics. Revisit if/when a future task adds MTU-aware frame
pacing that could exploit it (see PDF Phase 7.2).

## Acceptance

- Benchmark recorded: this document (committed with BORU-SS-09).
- Datagram-vs-reliable comparison measured over real QUIC endpoints.
- Decision documented: reliable streams retained; bounded queues + drop-oldest
  provide the required backpressure.
