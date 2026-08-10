#![no_main]
//! Fuzz the signed inbox-message verifier (delivery / ack / sync / delete).

use boru_core::inbox::SignedInboxMessage;
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let _ = SignedInboxMessage::verify(data, None);
});
