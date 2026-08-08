//! Identity types shared by Boru's call-control and media subsystems.

use std::fmt;

use serde::{Deserialize, Serialize};

/// A random identity for one voice/video call.
///
/// Call IDs are transmitted on the wire and are intentionally independent of
/// a peer's long-lived identity.  The call state machine should keep its own
/// local, monotonically increasing `generation: u64` alongside the active
/// call.  Async tasks must capture and check that generation before mutating
/// state, so work from an earlier call cannot affect a later call reusing the
/// same manager.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct CallId([u8; 16]);

impl CallId {
    /// Generate a fresh call identity using the operating system CSPRNG.
    ///
    /// A failure to obtain random bytes means the process cannot safely create
    /// a call identity, so this convenience API panics rather than producing a
    /// predictable identifier.  [`Self::try_generate`] is available to callers
    /// that need to handle that failure explicitly.
    pub fn generate() -> Self {
        Self::try_generate().expect("OS CSPRNG unavailable for CallId")
    }

    /// Generate a fresh call identity, reporting CSPRNG failure to the caller.
    pub fn try_generate() -> Result<Self, getrandom::Error> {
        let mut bytes = [0; 16];
        getrandom::fill(&mut bytes)?;
        Ok(Self(bytes))
    }

    /// Alias for [`Self::generate`] for call sites that construct IDs as a
    /// value without needing to distinguish the generation operation.
    pub fn new() -> Self {
        Self::generate()
    }

    /// Return the raw 128-bit identity.
    pub fn as_bytes(&self) -> &[u8; 16] {
        &self.0
    }
}

impl fmt::Display for CallId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        // Eight bytes (16 hex characters) are enough for diagnostics while
        // avoiding the accidental exposure of the full wire identity in logs.
        for byte in &self.0[..8] {
            write!(formatter, "{byte:02x}")?;
        }
        Ok(())
    }
}

impl fmt::Debug for CallId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_tuple("CallId")
            .field(&self.to_string())
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::CallId;

    #[test]
    fn generated_call_ids_are_distinct() {
        let first = CallId::generate();
        let second = CallId::generate();

        assert_ne!(first, second);
    }

    #[test]
    fn call_id_round_trips_through_postcard() {
        let original = CallId::generate();
        let encoded = postcard::to_stdvec(&original).expect("CallId should serialize");
        let decoded: CallId = postcard::from_bytes(&encoded).expect("CallId should deserialize");

        assert_eq!(original, decoded);
    }
}
