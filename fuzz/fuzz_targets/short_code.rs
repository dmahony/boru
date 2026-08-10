#![no_main]
//! Fuzz the short-code announcement verifier.
//!
//! Announcements are broadcast on the rendezvous topic to any peer; decode +
//! freshness + signature verification must never panic (BORU-AUDIT-28).

use boru_core::short_code::{ShortCodeFreshnessPolicy, SignedShortCodeAnnouncement};
use libfuzzer_sys::fuzz_target;
use std::time::{Duration, SystemTime};

fuzz_target!(|data: &[u8]| {
    let policy = ShortCodeFreshnessPolicy::new(Duration::from_secs(300), Duration::from_secs(60));
    let _ = SignedShortCodeAnnouncement::verify_at(SystemTime::now(), &policy, data, "any-code");
});
