//! Adversarial, property and fuzz coverage (BORU-AUDIT-28).
//!
//! Boru is a peer-to-peer application: every byte that arrives on the wire is
//! untrusted.  This test crate treats decoders, parsers, canonical encoders,
//! persistence transactions and authorization checks as hostile-input
//! surfaces and verifies the security invariants:
//!
//! - **No panic / no unwrap** on malformed peer input — decoders return
//!   typed rejection errors instead of crashing.
//! - **No unbounded allocation** from malicious advertised sizes.
//! - **Property tests** for the pure parsers and canonical encoders (HTTP
//!   Range, protocol frame lengths, canonical signed bytes, group event IDs).
//! - **Mutation tests** that flip / truncate / extend every byte of every
//!   signed protocol object and assert clean rejection.
//! - **Authorization matrix** — stranger, member, removed member, wrong peer,
//!   stale capability, replayed capability.
//! - **Restart** — replay state, mailbox state, group epoch state, migration
//!   completion all survive a reopen.
//! - **Failure injection** around multi-step persistence transactions.
//! - **Concurrency / stress** — inbox channel saturation, duplicate
//!   concurrent events, simultaneous writers.
//! - A deterministic **fuzz smoke** harness that runs in CI on every change
//!   (the long-running libFuzzer session lives in `fuzz/` for nightly runs).
//!
//! The deterministic fuzz smoke iteration count is controlled by the
//! `BORU_FUZZ_ITERATIONS` environment variable (default 2000) so CI can keep
//! it short while a nightly job can run it hotter.

#![cfg(feature = "net")]

// Module files live in tests/security/ (a subdirectory so cargo does not
// auto-discover them as separate test targets). The `#[path]` attribute is
// required: for an integration-test crate root at tests/security.rs, plain
// `mod x;` resolves relative to tests/ (rustc E0583), not tests/security/.
#[path = "security/authorization.rs"]
mod authorization;
#[path = "security/failure_injection.rs"]
mod failure_injection;
#[path = "security/fuzz_smoke.rs"]
mod fuzz_smoke;
#[path = "security/mutation.rs"]
mod mutation;
#[path = "security/oversized.rs"]
mod oversized;
#[path = "security/property.rs"]
mod property;
#[path = "security/restart.rs"]
mod restart;
#[path = "security/stress.rs"]
mod stress;

// Re-export the shared test helpers so each module can use them without a
// `mod common;` cycle (integration-test crates share the crate root).
#[path = "security/common.rs"]
pub(crate) mod common;
