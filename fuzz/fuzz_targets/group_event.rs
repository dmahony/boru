#![no_main]
//! Fuzz the `GroupEvent` decoder and verifier.
//!
//! Arbitrary bytes can arrive as a group-control event from the gossip mesh;
//! decode + verify against a fresh state must never panic (BORU-AUDIT-28).

use boru_core::group_events::{GroupEvent, GroupState};
use boru_core::TopicId;
use iroh::SecretKey;
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    if let Ok(event) = GroupEvent::decode(data) {
        let owner = SecretKey::generate();
        let state = GroupState::new(TopicId::from_bytes([7u8; 32]), owner.public());
        let _ = event.verify(&state);
    }
});
