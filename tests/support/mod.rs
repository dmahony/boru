//! # Shared integration-test support (BORU-TEST-010)
//!
//! Reusable helpers for the very large `tests/` integration suite, reducing
//! per-file boilerplate for multi-peer scenarios:
//!
//! - [`peers`] — peer / node fixture creation (`make_sk`, `spawn_peer_relay`,
//!   [`PeerFixture`]).
//! - [`net`] — relay / network setup (`create_endpoint`, relay server boot).
//! - [`timeout`] — deterministic clock + bounded-wait helpers with rich
//!   failure messages (peer id, state, event context).
//! - [`storage`] — temporary database / storage setup (`temp_dir`, `temp_path`).
//! - [`wait`] — message wait / assertion helpers (`drain_events`,
//!   `wait_for_received`, `wait_until`).
//! - [`fault`] — fault injection + restart guards. Full fault-injection
//!   scenarios (`FaultConfig`, `EventPlan`, `ReproGuard`) intentionally live in
//!   `test_deterministic_harness.rs`; reuse those rather than duplicating them.
//!
//! ## Usage
//!
//! Each integration-test file is its own crate, so shared code is included with
//! a plain `mod support;` declaration (resolves to `tests/support/mod.rs`):
//!
//! ```ignore
//! mod support;
//! use support::peers::spawn_peer_relay;
//! ```
//!
//! Because a test crate only needs a subset of the helpers, the module scopes
//! `dead_code` so unused helpers in a given member do not trip the `-Dwarnings`
//! CI lint.

#![allow(dead_code)]

pub mod fault;
pub mod multinode;
pub mod net;
pub mod peers;
pub mod storage;
pub mod timeout;
pub mod wait;
