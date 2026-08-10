#![no_main]
//! Fuzz the wire deflate decompressor (peer-controlled compressed payloads).
//! A panic, unbounded allocation, or hang here is a security bug
//! (BORU-AUDIT-28).

use boru_core::wire_compression::decompress;
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let _ = decompress(data);
});
