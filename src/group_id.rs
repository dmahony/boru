//! Stable, cryptographically random identifiers for groups.

use serde::{de, Deserialize, Deserializer, Serialize, Serializer};
use std::{fmt, str::FromStr};

/// Stable opaque identifier independent from a gossip topic.
#[derive(Clone, Copy, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub struct GroupId([u8; 32]);

impl GroupId {
    /// Generate a cryptographically random group identifier.
    pub fn generate() -> Self {
        let mut bytes = [0u8; 32];
        getrandom::fill(&mut bytes).expect("operating system randomness unavailable");
        Self(bytes)
    }
    /// Construct an identifier from exactly 32 bytes.
    pub const fn from_bytes(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }
    /// Borrow the raw identifier bytes.
    pub const fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }
}
impl fmt::Display for GroupId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&hex::encode(self.0))
    }
}
impl fmt::Debug for GroupId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_tuple("GroupId").field(&self.to_string()).finish()
    }
}
impl FromStr for GroupId {
    type Err = hex::FromHexError;
    fn from_str(value: &str) -> Result<Self, Self::Err> {
        let mut bytes = [0u8; 32];
        hex::decode_to_slice(value, &mut bytes)?;
        Ok(Self(bytes))
    }
}
impl Serialize for GroupId {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&self.to_string())
    }
}
impl<'de> Deserialize<'de> for GroupId {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        String::deserialize(deserializer)?
            .parse()
            .map_err(de::Error::custom)
    }
}
