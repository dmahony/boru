//! Shared helpers for the adversarial test suite.
//!
//! Everything here is test-only; nothing is exported from the library crate.

use std::panic::{catch_unwind, AssertUnwindSafe};

/// Iterate every single-byte mutation of `sample`:
/// - flip each byte to `0x00` and `0xFF`
/// - XOR each byte with `0x01`
/// - truncate at every prefix
/// - append one garbage byte after every position
///
/// `f` is called for each mutated buffer and must return `true` when the
/// decoder *accepted* the input.  The harness asserts that:
///
/// 1. no mutation panics (the decoder must fail closed, not crash), and
/// 2. no bit-flip / truncation of a valid sample is accepted.
///
/// Append-extensions are permitted to be accepted (postcard decoders ignore
/// trailing bytes); the invariant there is only "no panic".
pub fn sweep_mutations<F>(name: &str, sample: &[u8], mut f: F)
where
    F: FnMut(&[u8]) -> bool,
{
    assert!(!sample.is_empty(), "{name}: sample must be non-empty");
    let n = sample.len();

    // 1. Byte flips: 0x00, 0xFF, XOR 0x01 at every position.
    for i in 0..n {
        for target in [0x00u8, 0xFF, sample[i] ^ 0x01] {
            if target == sample[i] {
                continue;
            }
            let mut mutated = sample.to_vec();
            mutated[i] = target;
            assert_no_panic(name, &mutated, &mut f);
            assert!(
                !f(&mutated),
                "{name}: byte-flip at offset {i} (-> {target:#04x}) was accepted"
            );
        }
    }

    // 2. Truncations: every strict prefix.
    for len in 0..n {
        let mutated = sample[..len].to_vec();
        assert_no_panic(name, &mutated, &mut f);
        assert!(
            !f(&mutated),
            "{name}: truncation to {len} bytes was accepted"
        );
    }

    // 3. Append-extension with garbage after every position (also at the end).
    for i in 0..=n {
        for garbage in [0x00u8, 0xFF] {
            let mut mutated = sample.to_vec();
            mutated.insert(i, garbage);
            assert_no_panic(name, &mutated, &mut f);
            // Append-extension may be accepted; only no-panic is required.
        }
    }
}

/// Assert that calling `f` on `data` does not panic.
fn assert_no_panic<F>(name: &str, data: &[u8], f: &mut F)
where
    F: FnMut(&[u8]) -> bool,
{
    let result = catch_unwind(AssertUnwindSafe(|| f(data)));
    assert!(
        result.is_ok(),
        "{name}: decoder panicked on {}-byte input starting {:02x?}",
        data.len(),
        &data[..data.len().min(16)]
    );
}

/// Deterministic pseudo-random byte buffer for the fuzz smoke.
///
/// A tiny xorshift64* so the harness needs no extra dependency surface and the
/// sequence is fully reproducible across platforms.
pub struct MiniRng(pub u64);

impl MiniRng {
    pub fn new(seed: u64) -> Self {
        Self(seed.max(1))
    }

    pub fn next_u64(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        x.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }

    /// Fill `buf` with pseudo-random bytes.
    pub fn fill(&mut self, buf: &mut [u8]) {
        for chunk in buf.chunks_mut(8) {
            let v = self.next_u64().to_le_bytes();
            chunk.copy_from_slice(&v[..chunk.len()]);
        }
    }

    /// Random byte length in `[0, max]`.
    pub fn len(&mut self, max: usize) -> usize {
        (self.next_u64() as usize) % (max + 1)
    }
}

/// Current UNIX seconds (test helper mirroring `now_secs` in the crate).
pub fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}
