#![no_main]
//! Fuzz the tunnel-capability verifier.
//!
//! Capabilities are exchanged over whisper; a malformed token must never
//! panic the receiver (BORU-AUDIT-28).

use boru_core::tunnel::{TunnelCapability, TunnelId};
use iroh::SecretKey;
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    if let Ok(cap) = postcard::from_bytes::<TunnelCapability>(data) {
        let owner = SecretKey::generate().public();
        let peer = SecretKey::generate().public();
        let _ = cap.verify_for(&owner, &peer, TunnelId([0x11; 32]), 0, true);
    }
});
