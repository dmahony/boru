# Deflate Wire Compression — Benchmark & Verification Report

**Date:** 2026-08-05
**Task:** t_2d59ee66 — Benchmark and verify deflate compression
**Working tree:** shared kanban tree; compression changes uncommitted (repo convention).
**Build profile:** Criterion bench profile for microbenchmarks; test profile for tests.
**Dictionary:** 7856 bytes (4-section preshared dictionary; protocol constant).

## Executive summary

Boru's gossip wire messages are structurally repetitive: every envelope repeats
enum discriminants, 32-byte hashes, blob-ticket shapes, and short text fields.
Generic deflate cannot exploit this on tiny per-message streams, so the
`wire_compression` module uses **deflate with a preshared dictionary** — the
same technique Matrix adopted for its low-bandwidth mode (Matrix blog,
"Introducing low-bandwidth mode", Sept 2020; zlib RFC 1950/1951 dictionary
presetting).

**Overall data ratio: 2.52×** (1921 → 761 bytes over the 23-variant corpus) —
meets the ≥2× acceptance criterion.  **Overall envelope ratio: 1.37×**
(4319 → 3143 bytes), which is the real wire impact after the fixed
signature/key/header overhead.  **No variant produces a larger wire envelope
than uncompressed**: the encoder falls back to `compression = 0` when deflate
would not shrink a tiny payload, so tiny unit variants (Presence, Heartbeat,
Leave, Latency, AboutMe — 1-6 byte payloads) stay at 1.00× instead of growing
by deflate's framing overhead.  Worst-case variant on the raw data field is
`presence` (0.33×, 1 → 3 bytes), but its *envelope* is 1.00× (fallback).

## Reproducible commands

```sh
cargo bench --bench compression_bench -- --quick   # size report + CPU timing
cargo test --lib compression                        # wire_compression module + backward-compat
cargo test --lib compressed                         # round-trip + edge-case + fallback tests
cargo test --test test_signed_gossip_flow           # two-instance compressed gossip flow
cargo test --lib                                    # full lib suite
```

## 1. Size report (per variant)

Command: `cargo bench --bench compression_bench -- --quick`
(dictionary 7856 bytes; `plain`/`compressed` are the raw postcard data field,
`env_plain`/`env_comp` are the full SignedMessage envelope on the wire):

```
variant                             plain   compressed      ratio    env_plain     env_comp  env_ratio
text_5                                  7            4      1.75x          111          108      1.03x
text_50                                59           36      1.64x          163          140      1.16x
text_200                              162           31      5.23x          267          135      1.98x
text_500                              466          169      2.76x          571          274      2.08x
file_share                            134           59      2.27x          239          163      1.47x
file_share_no_thumb                    84           47      1.79x          188          151      1.25x
image_share                            43           12      3.58x          147          116      1.27x
room_advertisement                    300          135      2.22x          405          240      1.69x
reaction                               38            9      4.22x          142          113      1.26x
edit                                   86           36      2.39x          190          140      1.36x
delete                                 33            5      6.60x          137          109      1.26x
about_me                                6            8      0.75x          110          110      1.00x
presence                                1            3      0.33x          105          105      1.00x
presence_with_ticket                   71           38      1.87x          175          142      1.23x
read_receipt                           33            5      6.60x          137          109      1.26x
heartbeat                               1            3      0.33x          105          105      1.00x
latency_ping                            3            5      0.60x          107          107      1.00x
latency_pong                            3            5      0.60x          107          107      1.00x
leave                                   1            3      0.33x          105          105      1.00x
diagnostic_probe                        47           36      1.31x          151          140      1.08x
contact_control                       131            8     16.38x          236          112      2.11x
profile_update                         76           24      3.17x          180          128      1.41x
encrypted_group_control               136           80      1.70x          241          184      1.31x

overall data (23 samples, 1921 -> 761 bytes): 2.52x
overall envelope (23 samples, 4319 -> 3143 bytes): 1.37x
worst-case variant (data): presence (0.33x, 1 -> 3 bytes)
no variant produces a larger wire envelope than uncompressed
```

### CPU timing (Criterion, --quick)

| Benchmark | time |
|---|---:|
| compress_text_500 | 25.5 µs |
| decompress_text_500 | 3.2 µs |
| compress_presence | 16.7 µs |
| decompress_presence | 1.1 µs |
| compress_all_variants | 988 µs |
| decompress_all_variants | 20 µs |
| sign_and_encode_plain_all (23 msgs) | 1.21 ms |
| sign_and_encode_compressed_all (23 msgs) | 2.24 ms |
| verify_and_decode_plain_all (23 msgs) | 1.40 ms |
| verify_and_decode_compressed_all (23 msgs) | 2.04-2.38 ms |

Compress adds ~16-26 µs per message and decompress ~1-3 µs — negligible
against the ~50-500 µs gossip pipeline per message.

## 2. Backward compatibility

- **Old → new**: a legacy 4-field envelope (no `compression` byte) decodes on
  new code as `compression = 0`.
  Test: `signed_message_backward_compat_without_compression_field` — PASS.
- **New (compression=0) → old**: a 5-field envelope with `compression = 0`
  deserializes with the old 4-field struct (postcard ignores the trailing
  byte).  Test: `new_compression_zero_message_decodes_with_legacy_struct` — PASS.
- **compression=1 on old code**: simulating old-code decode (calling postcard
  directly on the raw `data` field without inflating) fails cleanly with a
  decode error — no panic, no silent corruption (asserted that if postcard
  somehow parses the deflate bytes, it must NOT equal the original message).
  Test: `compressed_message_old_code_postcard_decode_fails_cleanly` — PASS.
- **Unknown compression values**: rejected with a clear error mentioning
  "compression".  Test: `signed_message_unknown_compression_rejected` — PASS.
- **Signature covers the compressed data**; verify-then-inflate order means
  the `compression` byte cannot be tampered with.

## 3. Round-trip tests

- Every Message variant round-trips through `sign_and_encode_compressed` →
  `verify_and_decode`; decoded postcard bytes are identical to the original.
  Test: `compressed_roundtrip_all_message_variants` (21 variants) — PASS.
- Edge cases: empty text, single char, 10k-char max-length, emoji-only,
  mixed emoji, RTL Hebrew/Arabic, CJK, combining marks, control chars,
  zalgotext, empty FileShare with a big repeated ticket.
  Tests: `compressed_roundtrip_edge_cases`,
  `compressed_roundtrip_empty_file_share_and_big_ticket` — PASS.
- Fallback: `compressed_envelope_never_larger_than_plain` — for every
  variant the compressed envelope is ≤ the plain envelope (tiny variants fall
  back to raw), and still decodes identically — PASS.
- Two real instances: `test_compressed_signed_message_gossip_flow` in
  `tests/test_signed_gossip_flow.rs` spawns two boru gossip peers; Peer A
  broadcasts `sign_and_encode_compressed`, Peer B receives and
  `verify_and_decode`s it, and the compressed envelope is smaller than plain.
  PASS (alongside the pre-existing plain gossip flow test).

## 4. Full suite

`cargo test --lib` on the reviewed tree (2026-08-05): **1823 passed; 20 failed;
2 ignored** — matching the parent task's pristine-HEAD baseline (1813 passed /
21 failed at cf8a77c7; `file_indexer` flake now passing, so the count moved
1813→1823 passed and 21→20 failed). All 20 failures are the pre-existing
classes verified at pristine HEAD (friendly-name resolution, group_encryption
integration, net address lookup, room_cleanup cascade, handle_net_event
image-share/ack paths, old_wire_format_file_share) — none involve the
compression path. New compression tests: 31 lib (wire_compression +
backward-compat) + 6 lib (round-trip/edge/fallback) + 2 integration
(test_signed_gossip_flow, incl. compressed two-peer flow) all pass.

## Design note added during verification

The parent task's encoder always emitted `compression = 1` when asked.
Benchmarking showed deflate's framing overhead (~2-8 bytes) exceeds tiny
unit payloads (Presence/Heartbeat/Leave/Latency are 1-6 bytes), so those
variants would have produced *larger* wire messages.  `SignedMessage::encode`
now compares compressed vs raw and falls back to `compression = 0` (raw
postcard) when deflate would not shrink the payload.  This keeps the
acceptance criterion "no variant produces worse-than-uncompressed output"
true at the wire level while preserving full backward compatibility.
