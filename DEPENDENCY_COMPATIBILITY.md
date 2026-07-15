# Dependency Compatibility Summary

## Current dependencies

| Dependency | Version | Feature | Compat Notes |
|---|---|---|---|
| `iroh` | `= "1"` | net, examples, gui | Stable, dual MIT/Apache-2.0 |
| `iroh-base` | `= "1"` | key | Stable |
| `iroh-blobs` | `= "0.103"` | net, gui | Stable |
| `iroh-mdns-address-lookup` | `= "0.4"` | net | Stable |
| `iroh-mainline-address-lookup` | `= "0.4"` | net | Uses `n0-mainline` DHT internally |
| `tokio` | `= "1"` | net, gui | Stable, `resolver = "2"` |
| `postcard` | `= "1"` | experimental-derive | 1:2021 edition compatible |
| `serde` | `= "1.0.164"` | derive | Compatible |
| `blake3` | `= "1.8"` | — | CC0-1.0 / Apache-2.0 |
| `aes-gcm` | `= "0.10.3"` | — | Compatible TLS versions |
| `x25519-dalek` | `= "2.0.1"` | getrandom, static_secrets | BSD-3-Clause |
| `ed25519-dalek` | _(transitive via iroh)_ | — | BSD-3-Clause, version 2.x |
| `iced` | `= "0.14"` | gui | MIT, GUI-only |
| Rust edition | `2021` | — | MSRV 1.91 |
| `getrandom` | `= "0.3"` | — | Includes `getrandom` feature flag |
| `rand` | `= "0.10.1"` | std_rng | Compatible |

## If adding distributed-topic-tracker v0.3.5 (default-features = false)

| Dep in d-t-t | d-t-t version | boru-chat version | Compat? |
|---|---|---|---|
| `tokio` | `= "1"` | `= "1"` | ✓ |
| `serde` | `= "1"` | `= "1.0.164"` | ✓ |
| `rand` | `= "0.10"` | `= "0.10.1"` | ✓ |
| `getrandom` | `= "0.4"` | `= "0.3"` | **Minor mismatch** — both coexist via resolver=2 |
| `ed25519-dalek` | `= "3.0.0-rc.0"` | _(transitive, 2.x via iroh)_ | **Two versions coexist** — no API conflict at crate boundary |
| `mainline` | `= "7"` | _(not used)_ | New dep, no conflict |
| `iroh` (optional) | `= "1"` | `= "1"` | ✓ (not pulled in with `default-features = false`) |

**Verdict:** No compatibility blockers. Two minor version divergences (`getrandom 0.3 ↔ 0.4`, `ed25519-dalek 2.x ↔ 3.x`) are safely resolved by Cargo's `resolver = "2"` since neither crate exposes the conflicting types publicly through the API boundary. The d-t-t crate's `ed25519-dalek` is used only internally for DHT signing — our code never calls it directly.
