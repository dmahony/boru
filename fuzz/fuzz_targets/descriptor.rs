#![no_main]
//! Fuzz the signed download descriptor decoder + verifier.
//!
//! Descriptors arrive from the file-access peer; decode and verify must
//! never panic (BORU-AUDIT-28).

use boru_core::file_access_protocol::{verify_download_descriptor, SignedDownloadDescriptor};
use iroh::SecretKey;
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    if let Ok(descriptor) = postcard::from_bytes::<SignedDownloadDescriptor>(data) {
        let owner = SecretKey::generate().public();
        let requester = SecretKey::generate().public();
        let _ = verify_download_descriptor(&descriptor, &owner, &requester, 0);
    }
});
