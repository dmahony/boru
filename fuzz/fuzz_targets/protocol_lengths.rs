#![no_main]
//! Fuzz the protocol frame-length gate (the pure predicate behind
//! `read_frame` / `write_frame` allocation caps).  A malicious advertised
//! length must be rejected before any buffer is allocated (BORU-AUDIT-28).

use boru_core::protocol_version::frame_payload_len_ok;
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    // The first four bytes (if present) are a little-endian u32 length; feed
    // every prefix to the gate.  The gate is a pure boolean — this target
    // exists to keep the boundary predicate under fuzz and to catch accidental
    // panics/overflow in its arithmetic.
    for len in 0..=data.len().min(4) {
        let mut buf = [0u8; 4];
        buf[..len].copy_from_slice(&data[..len]);
        let advertised = u32::from_le_bytes(buf) as usize;
        let _ = frame_payload_len_ok(advertised);
    }
    let _ = frame_payload_len_ok(usize::MAX);
    let _ = frame_payload_len_ok(0);
});
