//! Cargo-fuzz targets for Boru's peer-controlled decoders (BORU-AUDIT-28).
//!
//! These targets feed arbitrary bytes (which in production arrive from
//! untrusted peers) to every binary decoder / signature verifier in the
//! crate.  A crash (panic, unwrap, OOM, hang) is a fuzzer finding.
//!
//! Run locally with a nightly toolchain:
//!
//! ```text
//! cargo +nightly install cargo-fuzz
//! cargo +nightly fuzz run signed_message
//! cargo +nightly fuzz run group_event
//! cargo +nightly fuzz run mailbox
//! cargo +nightly fuzz run descriptor
//! cargo +nightly fuzz run range_header
//! cargo +nightly fuzz run wire_compression
//! cargo +nightly fuzz run short_code
//! cargo +nightly fuzz run contact
//! cargo +nightly fuzz run inbox
//! cargo +nightly fuzz run tunnel
//! cargo +nightly fuzz run protocol_lengths
//! ```
//!
//! Persist any crashing corpus input under `tests/download-fixtures/` (or a
//! `fuzz/corpus/` sibling) as a regression fixture so the deterministic
//! `tests/security` suite reproduces it without a fuzzer.
//!
//! CI runs the bounded, deterministic smoke variant (`tests/security/
//! fuzz_smoke.rs`) on every change; these targets are for the longer
//! local/nightly sessions.
