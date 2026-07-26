# Phase 1: Performance Baseline Report

**Date:** 2026-07-26  
**Commit:** 6f2db267  
**Host:** Linux (6.8.0-134-generic)  
**Profile:** `test` (unoptimized + debuginfo)  

## Test Commands

```sh
# Performance baseline (5 tests)
BORU_PERF=1 cargo test --test performance_baseline --features net,test-utils -- --nocapture

# Stress test suite (8 tests)
BORU_PERF=1 cargo test --test stress_test_comprehensive --features net,test-utils -- --nocapture

# Verify existing behaviour unchanged (no BORU_PERF)
cargo test --test performance_baseline --features net,test-utils
cargo test --test stress_test_comprehensive --features net,test-utils
```

---

## 1. Startup Baseline

Metrics from `baseline_startup_time` (1 iteration each).

| Operation | Total (ms) | Avg (ms) | Min (ms) | Max (ms) |
|---|---|---|---|---|
| startup_endpoint (bind) | 49.561 | 49.561 | 49.561 | 49.561 |
| startup_gossip_spawn | 0.193 | 0.193 | 0.193 | 0.193 |
| startup_subscribe | 0.023 | 0.023 | 0.023 | 0.023 |
| iterate_500_friends | 0.595 | 0.595 | 0.595 | 0.595 |

**Top slowest:** Endpoint bind (`startup_endpoint`, ~50 ms) dwarfs all other startup operations. Gossip spawn + subscribe are sub-millisecond.

---

## 2. Message Latency Baseline

Metrics from `baseline_message_latency`. Per-message averages where applicable.

| Operation | Scale | Total (ms) | Per-msg Avg (ms) | Min (ms) | Max (ms) |
|---|---|---|---|---|---|
| sign_encode_100 | 100 msgs × 10 runs | — | 0.257 | — | — |
| sign_encode_1000 | 1000 msgs × 5 runs | — | 0.268 | — | — |
| gossip_broadcast_100 | 100 msgs | — | 0.313 | — | — |
| handle_net_event_1000 | 1000 msgs | 0.183 | 0.000183 | — | — |
| verify_decode_100 | 100 msgs | 10.626 | 0.106 | — | — |

**Observations:**
- Sign/encode is ~0.26 ms per message, consistent across 100 and 1000 scales.
- Gossip broadcast averages 0.31 ms per message.
- Verify+decode is ~0.106 ms per message.
- All per-message operations are well under 1 ms.

---

## 3. Data Scaling Baseline

Metrics from `baseline_data_scaling`.

| Operation | Scale | Total (ms) | Avg (ms) | Min (ms) | Max (ms) |
|---|---|---|---|---|---|
| friends_store_build_500 | 500 entries | 183.536 | 183.536 | 183.536 | 183.536 |
| conversations_build_100 | 100 entries | 3.103 | 3.103 | 3.103 | 3.103 |
| conversation_switches_100 | 100 switches | 0.002 | 0.002 | 0.002 | 0.002 |
| entry_iteration_100 | 100 entries | 0.002 | 0.002 | 0.002 | 0.002 |
| entry_iteration_1000 | 1000 entries | 0.012 | 0.012 | 0.012 | 0.012 |
| entry_iteration_5000 | 5000 entries | 0.078 | 0.078 | 0.078 | 0.078 |
| height_estimation_100 | 100 entries | 0.004 | 0.004 | 0.004 | 0.004 |
| height_estimation_1000 | 1000 entries | 0.024 | 0.024 | 0.024 | 0.024 |
| height_estimation_5000 | 5000 entries | 3.165 | 3.165 | 3.165 | 3.165 |
| friends_iterate_500 | 500 friends | 15.767 | 15.767 | 15.767 | 15.767 |

**Observations:**
- **FriendsStore build (183.5 ms)** is by far the slowest data operation. This iterates 500 inserts into a `BTreeMap`-backed store with full `FriendRecord` construction.
- Entry iteration is linear and fast: 5000 entries scanned in 78 µs.
- Height estimation (layout calc) scales roughly linearly: 100 → 4 µs, 1000 → 24 µs, 5000 → 3.2 ms (nonlinearity at 5000 suggests additional overhead from date-separator logic).
- `FriendsStore` iteration (friends_iterate_500) at 15.8 ms is notably slower than raw entry iteration, likely due to `BTreeMap` overhead.

---

## 4. Blob / Download Baseline

Metrics from `baseline_simultaneous_downloads`.

| Operation | Scale | Total (ms) | Avg (ms) | Min (ms) | Max (ms) |
|---|---|---|---|---|---|
| sequential_read_50_blobs | 50 × 64 KB | 74.177 | 74.177 | 74.177 | 74.177 |
| sequential_add_50_blobs | 50 × 64 KB | 15.680 | 15.680 | 15.680 | 15.680 |
| add_100_blobs_16KB | 100 × 16 KB | 22.898 | 22.898 | 22.898 | 22.898 |
| read_100_blobs_16KB | 100 × 16 KB | 116.520 | 116.520 | 116.520 | 116.520 |

**Observations:**
- Sequential reads are ~4.7× slower than writes for the same data (74 ms read vs 16 ms write for 50×64KB).
- 100 small blobs (16KB) read in 116.5 ms total, ~1.2 ms per blob.
- All blob operations use in-memory store (`MemStore`); disk-backed `FsStore` would be slower.

---

## 5. Net Event Throughput Baseline

Metrics from `baseline_net_event_throughput` (full pipeline: sign → verify_decode → handle_net_event).

| Operation | Scale | Total (ms) | Avg (ms) | Per-msg Avg (ms) |
|---|---|---|---|---|
| net_event_pipeline_100 | 100 msgs | 1304.446 | 1304.446 | 13.044 |
| net_event_pipeline_1000 | 1000 msgs | 10613.033 | 10613.033 | 10.613 |
| handle_net_event (aggregate) | 1796 samples | 228.4 | 0.127 | — |

**Observations:**
- **Net event pipeline is the single slowest operation at ~10.6 seconds for 1000 messages.** The pipeline includes sign, verify+decode, AND network transport (gossip broadcast).
- The per-msg average (~10.6–13 ms) indicates heavy per-message overhead — likely from the full gossip broadcast + relay round-trip.
- `handle_net_event` itself is fast (0.127 ms avg), so the bottleneck is in gossip broadcast transport, not message processing.

---

## 6. Stress Test Baseline

Metrics from `stress_test_comprehensive` (8 tests). All in-memory, no network.

| Operation | Scale | Total (ms) |
|---|---|---|
| stress_scroll_full_scan_5000 | 5000 entries | 3.409 |
| stress_scroll_height_estimation_5000 | 5000 entries | 3.330 |
| stress_scroll_windowed_iteration | 5000 entries, window 100 | 3.259 |
| stress_search_substring | 5000 entries | 3.204 |
| stress_search_by_author | 5000 entries | 1.734 |
| stress_search_filter_kind | 5000 entries | 1.376 |
| stress_download_setup_50 | 50 downloads | 0.453 |
| stress_download_progress_updates | 50 × 10 updates | 0.347 |
| stress_switch_conversations_100 | 100 conversations | 0.126 |
| stress_download_complete_50 | 50 cleanup | 0.036 |

**Observations:**
- All stress test operations are under 4 ms.
- Scroll operations (full scan, height estimation, windowed iteration) are all ~3.3 ms for 5000 entries — very consistent.
- Substring search over 5000 entries is ~3.2 ms.
- All non-IO stress test operations are comfortably fast.

---

## Summary of Slowest Operations (Cross-Test)

| Rank | Operation | Time (ms) | Concern Level |
|---|---|---|---|
| 1 | net_event_pipeline_1000 | 10613.0 | **HIGH** — 10.6s for 1000 messages |
| 2 | net_event_pipeline_100 | 1304.4 | **HIGH** — 1.3s for 100 messages |
| 3 | friends_store_build_500 | 183.5 | **LOW** — one-time startup cost |
| 4 | read_100_blobs_16KB | 116.5 | **MEDIUM** — batch read path |
| 5 | sequential_read_50_blobs | 74.2 | **LOW** — acceptable at this scale |
| 6 | startup_endpoint | 49.6 | **LOW** — one-time startup cost |

---

## Key Findings

1. **Net event pipeline is the #1 optimization target.** 10.6 seconds for 1000 messages through the gossip broadcast path dwarfs every other metric. This is likely dominated by relay-network round-trips.

2. **Per-message overhead is ~10-13 ms** in the pipeline benchmark. The `handle_net_event` handler itself is only 0.13 ms — the bottleneck is in gossip broadcast/relay, not message processing.

3. **All local operations are fast.** Scrolling (5000 entries: 3.4 ms), search (5000 entries: 3.2 ms), data structure iteration (5000 entries: 78 µs) are all sub-5 ms.

4. **Startup cost is manageable.** Endpoint bind (~50 ms) is the dominant startup cost; Gossip spawn + subscribe + data load are sub-millisecond.

5. **Blob read is slower than blob write** (4.7× ratio). Worth investigating if blobs are stored/read frequently in real usage.

---

## Notes

- All measurements taken with `BORU_PERF=1` (instrumentation enabled).
- Tests run in `test` profile (unoptimized + debuginfo). Release builds will be faster.
- P95 statistics not computed by current `PerfTracker::print_report()` — it reports min/avg/max only. The raw sample data is emitted as `tracing::info!(target: "perf", ...)` events and is available in structured logs for offline P95 computation.
- `net_event_pipeline_100*` tests include actual iroh gossip network traffic (loopback relay), so times include transport overhead.
