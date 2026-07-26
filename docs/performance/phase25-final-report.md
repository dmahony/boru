# Phase 25: Final Performance Profile and Comparison

**Date:** 2026-07-26
**Commit under test:** `c144b9d545d2b9a804ad082f2f13e84758e659be`
**Host:** Linux 6.8.0-134-generic
**Working tree:** parent phases are present as uncommitted changes; this report does not claim a clean tree.
**Build profile:** test profile for instrumented tests; Criterion bench profile for microbenchmarks.

## Executive summary

The final baseline and stress suites pass. The largest Phase 1 local bottlenecks improved materially: FriendsStore construction fell from 183.536 ms to 101.391 ms (-44.8%), the 1000-message network pipeline fell from 10,613.033 ms to 8,539.563 ms (-19.5%), and blob reads improved by about 14–16%. The local event handler also fell from 0.127 ms to 0.036 ms average in the data-scaling run and from 0.127 ms to 0.022 ms in the download run.

The remaining dominant cost is still network transport: the 1000-message pipeline is 8.54 s, while local signing, decoding, handling, and data-structure operations are sub-millisecond to low-millisecond. Criterion reports broad improvements in the Phase 23 hot-path suite, with one persistence regression at 10,000 records that should be investigated separately.

## Reproducible commands

```sh
cargo fmt --all -- --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test --all-features
BORU_PERF=1 cargo test --test performance_baseline --features net,test-utils -- --nocapture
BORU_PERF=1 cargo test --test stress_test_comprehensive --features net,test-utils -- --nocapture
cargo test --test verify_gui_bootstrap --features gui
cargo test --test test_iced_chat_flow --features gui
cargo bench --bench phase23 -- --noplot
```

The two performance test commands are the Phase 1 baseline commands. They each run against the same named operations and scales, allowing direct comparison with `phase1-baseline-report.md`.

## Phase 1 baseline versus final measurements

All values below are single samples emitted by `PerfTracker` in the test profile. Network values include the test transport path; they are not pure CPU measurements.

| Operation | Phase 1 (ms) | Final (ms) | Change |
|---|---:|---:|---:|
| `friends_store_build_500` | 183.536 | 101.391 | **-44.8%** |
| `conversations_build_100` | 3.103 | 0.780 | **-74.9%** |
| `entry_iteration_5000` | 0.078 | 0.063 | -19.2% |
| `height_estimation_5000` | 3.165 | 0.109 | **-96.6%** |
| `friends_iterate_500` | 15.767 | 10.542 | -33.1% |
| `sequential_read_50_blobs` | 74.177 | 62.398 | -15.9% |
| `sequential_add_50_blobs` | 15.680 | 14.569 | -7.1% |
| `add_100_blobs_16KB` | 22.898 | 19.959 | -12.8% |
| `read_100_blobs_16KB` | 116.520 | 100.159 | -14.0% |
| `startup_endpoint` | 49.561 | 54.015 | +9.0% |
| `startup_gossip_spawn` | 0.193 | 0.126 | -34.7% |
| `startup_subscribe` | 0.023 | 0.017 | -26.1% |
| `gossip_broadcast_100` | 0.313 | 0.266 | -15.0% |
| `sign_encode_100` per message | 0.257 | 0.222 | -13.6% |
| `sign_encode_1000` per message | 0.268 | 0.224 | -16.4% |
| `verify_decode_100` total | 10.626 | 8.238 | -22.5% |
| `handle_net_event_1000` total | 0.183 | 0.083 | **-54.6%** |
| `net_event_pipeline_100` | 1304.446 | 889.116 | **-31.8%** |
| `net_event_pipeline_1000` | 10613.033 | 8539.563 | **-19.5%** |

The startup endpoint bind is the only listed Phase 1 operation that regressed; the 4.45 ms increase is small in absolute terms and remains a one-time cost. The Phase 1 report recorded `handle_net_event` at 0.127 ms average in its aggregate sample; the final run recorded 0.036 ms in data scaling, 0.022 ms in download setup, and 0.044 ms in the 1000-message pipeline aggregate. These are different sample populations, so they should be treated as directional rather than a single strict before/after number.

## Criterion Phase 23 final profile

Command: `cargo bench --bench phase23 -- --noplot`. Criterion used repeated samples and compared against the saved Phase 23 baseline where available.

Representative medians:

| Benchmark | Final median | Criterion result |
|---|---:|---|
| `chat_entries/10000` | 464.06 µs | 13.6% faster |
| `catalogue_and_friends/friends/5000` | 3.434 µs | no significant change |
| `utility_paths/url_parse` | 340.74 ns | improved |
| `utility_paths/blake3/65536` | 18.281 µs | improved |
| `utility_paths/image_resize_256` | 3.7213 ms | 34.1% faster |
| `sqlite_batching/100` | 108.11 µs | 22.9% faster |
| `sqlite_batching/1000` | 525.77 µs | no significant change |
| `sqlite_batching/10000` | 4.7028 ms | no significant change |
| `network_burst/1000` | 81.384 µs | 39.9% faster |
| `network_burst/10000` | 792.41 µs | 42.2% faster |
| `progress_updates/10000` | 190.55 µs | 32.5% faster |
| `watcher_events/10000` | 2.1145 ms | 3.6% faster |
| `persistence/100` | 125.40 µs | 98.3% faster |
| `persistence/1000` | 405.24 µs | 99.0% faster |
| `persistence/10000` | 2.8720 ms | **15.1% slower** |

Criterion's statistical comparison is more useful than a single sample for the microbenchmarks. The persistence results are not directly comparable across all sizes: the small and medium cases improved strongly, while the 10,000-record case regressed and needs profiling before any conclusion about the persistence implementation is made.

## Stress and GUI verification

- `performance_baseline`: **5 passed, 0 failed**, 12.05 s; all five Phase 1 baseline tests completed.
- `stress_test_comprehensive`: **8 passed, 0 failed**, 0.35 s.
- Stress dataset: 256 friends, 100 conversations, 5,000 messages, 200 avatars; search, scrolling, downloads, startup loading, profile opening, and memory-cap paths all completed.
- `verify_gui_bootstrap` with `gui`: **1 passed**, 1.31 s.
- `test_iced_chat_flow` with `gui`: **1 passed**, 14.01 s.
- Tests emitted iroh endpoint-drop error logs during teardown, but the affected tests returned `ok`; this is a cleanup warning to address separately rather than a test failure.

## Verification status and known blockers

`cargo test --all-features` reached test compilation but failed in the existing `tests/test_pairing_integration.rs` target because it imports the nonexistent crate name `boru_chat`; the compiler suggests `boru_core`. This final task did not rewrite those imports because that would be unrelated source repair rather than profiling/reporting.

`cargo clippy --all-targets --all-features -- -D warnings` failed at `build.rs:8` on `clippy::empty-line-after-doc-comments`. It also exposed ordinary warnings in the current tree, including an unnecessary `mut` in `src/chat_core.rs:2332` and an unused `env` import in `examples/setup.rs`.

`cargo fmt --all -- --check` failed because the current parent-phase working tree contains formatting diffs across multiple files, including `tests/test_outbox_throughput.rs` and other existing modified sources. No formatter rewrite was applied in this reporting task.

## Changes represented by the final profile

The measured tree includes the preceding performance phases, notably:

- optimized production and symbol-preserving profiling Cargo profiles;
- expanded Criterion coverage for entries, catalogue/friends, utility paths, SQLite batching, network bursts, progress updates, watcher events, and persistence;
- the Phase 23 implementation changes already present in the working tree;
- the Phase 24 build/profile documentation in `docs/build-release.md`.

## Remaining bottlenecks and follow-up priorities

1. Profile `net_event_pipeline_1000` separately from relay/transport behavior. It remains 8.54 s and dominates the end-to-end test workload.
2. Investigate the 10,000-record persistence regression with Criterion profiling and allocation/I/O tracing.
3. Repair the stale `boru_chat` imports in `tests/test_pairing_integration.rs`, then rerun the all-features suite.
4. Resolve the build-script doc-comment lint and current formatting warnings before treating `-D warnings` as a release gate.
5. Close iroh endpoints explicitly in the affected tests to remove teardown error logs and make lifecycle timing less noisy.

## Raw run artifacts

The raw command output was captured during this run at:

- `/tmp/boru-phase25-performance-baseline.txt`
- `/tmp/boru-phase25-stress.txt`
- `/tmp/boru-phase25-bench.txt`
- `/tmp/boru-phase25-fmt.txt`
- `/tmp/boru-phase25-clippy.txt`
- `/tmp/boru-phase25-test-all-features.txt`
- `/tmp/boru-phase25-gui-bootstrap.txt`
- `/tmp/boru-phase25-gui-flow.txt`
