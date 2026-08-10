#![no_main]
//! Fuzz the mailbox envelope decoder.
//!
//! Envelopes arrive from peers over the inbox protocol and from persisted
//! state; decode must never panic (BORU-AUDIT-28).

use boru_core::mailbox::MailboxEnvelope;
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let _ = MailboxEnvelope::decode(data);
});
