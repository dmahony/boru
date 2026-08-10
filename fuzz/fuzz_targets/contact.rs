#![no_main]
//! Fuzz the signed contact-message verifier (friend-request / invite flow).

use boru_core::contact::SignedContactMessage;
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let _ = SignedContactMessage::verify(data, None);
});
