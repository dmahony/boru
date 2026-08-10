#![no_main]
//! Fuzz the `SignedMessage` envelope decoder/verifier.
//!
//! In production this bytes slice arrives from an untrusted gossip peer; a
//! panic here is a security bug (BORU-AUDIT-28).

use boru_core::chat_core::SignedMessage;
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let _ = SignedMessage::verify_and_decode(data);
});
