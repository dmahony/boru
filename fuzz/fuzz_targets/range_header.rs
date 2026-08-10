#![no_main]
//! Fuzz the HTTP Range header parser (pure function, BORU-AUDIT-28).

use boru_core::streaming_server::parse_range_header;
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    // Arbitrary bytes may contain a Range header line (possibly with an
    // interior NUL). Convert lossily; the parser must never panic.
    let header = String::from_utf8_lossy(data);
    let _ = parse_range_header(&header, 1_000_000);
});
