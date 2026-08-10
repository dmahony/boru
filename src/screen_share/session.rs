//! Screen-sharing session identity, separate from chat conversation identity.

use std::fmt;

/// A randomly generated identifier for one screen-sharing event.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct ScreenShareSessionId([u8; 16]);

impl ScreenShareSessionId {
    /// Generate a fresh session identifier using the operating-system CSPRNG.
    pub fn generate() -> Self {
        let mut bytes = [0; 16];
        getrandom::fill(&mut bytes).expect("OS CSPRNG unavailable for screen-share session");
        Self(bytes)
    }

    /// Return the raw session identifier bytes.
    pub const fn as_bytes(&self) -> &[u8; 16] {
        &self.0
    }
}

impl fmt::Debug for ScreenShareSessionId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_tuple("ScreenShareSessionId")
            .field(&hex::encode(self.0))
            .finish()
    }
}

/// Minimal session record. A conversation can own multiple such records.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ScreenShareSession {
    id: ScreenShareSessionId,
    conversation_id: u64,
}

impl ScreenShareSession {
    /// Create a session with a fresh ID and no protocol-level conversation coupling.
    pub fn new() -> Self {
        Self {
            id: ScreenShareSessionId::generate(),
            conversation_id: 0,
        }
    }

    /// Create a session associated with an application conversation reference.
    pub fn for_conversation(conversation_id: u64) -> Self {
        Self {
            id: ScreenShareSessionId::generate(),
            conversation_id,
        }
    }

    /// Return this screen-share event's identity.
    pub const fn id(&self) -> ScreenShareSessionId {
        self.id
    }
    /// Return the optional application conversation reference.
    pub const fn conversation_id(&self) -> u64 {
        self.conversation_id
    }
}
