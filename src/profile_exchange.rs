//! Opt-in, recipient-bound public profile exchange.
//!
//! This is deliberately separate from the startup discovery topic. The wire
//! object is suitable for a contact or room's authenticated data path; every
//! receiver still enforces the intended recipient before persisting it.

use std::collections::{HashMap, HashSet, VecDeque};

use iroh::{PublicKey, SecretKey};
use serde::{Deserialize, Serialize};

use crate::{protocol_signing, user_profile::PublicUserProfile};

/// Domain used for profile-exchange signatures.
pub const PROFILE_EXCHANGE_SIGNING_DOMAIN: &str = "boru/profile-exchange";
/// Current profile-exchange wire version.
pub const PROFILE_EXCHANGE_VERSION: u16 = 1;
/// Maximum encoded envelope size accepted from a peer.
pub const MAX_PROFILE_EXCHANGE_BYTES: usize = 4096;
/// Maximum accepted profile exchanges from one sender in a window.
pub const MAX_PROFILE_EXCHANGES_PER_WINDOW: usize = 8;
/// Rate-limit window in seconds.
pub const PROFILE_EXCHANGE_WINDOW_SECS: u64 = 60;
/// Maximum lifetime of an exchange envelope.
pub const MAX_PROFILE_EXCHANGE_TTL_SECS: u64 = 15 * 60;

/// A signed profile update addressed to one node.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProfileExchange {
    /// Wire version, covered by the signature.
    pub version: u16,
    /// Authenticated publisher identity.
    pub sender: PublicKey,
    /// The only node allowed to accept this envelope.
    pub recipient: PublicKey,
    /// Sender-created nonce for replay deduplication.
    pub nonce: u64,
    /// Unix timestamp in seconds when created.
    pub issued_at_secs: u64,
    /// Unix timestamp in seconds after which the envelope is invalid.
    pub expires_at_secs: u64,
    /// Privacy-safe profile fields only.
    pub profile: PublicUserProfile,
    /// Ed25519 signature over all fields above.
    pub signature: Vec<u8>,
}

impl ProfileExchange {
    /// Build and sign an exchange. Publication opt-in and recipient
    /// authorization are intentionally checked by the caller/policy.
    pub fn sign(
        secret: &SecretKey,
        recipient: PublicKey,
        nonce: u64,
        issued_at_secs: u64,
        profile: PublicUserProfile,
    ) -> Result<Self, String> {
        profile.validate().map_err(|e| format!("invalid profile: {e}"))?;
        let expires_at_secs = issued_at_secs
            .checked_add(MAX_PROFILE_EXCHANGE_TTL_SECS)
            .ok_or_else(|| "profile expiry overflow".to_string())?;
        let mut exchange = Self {
            version: PROFILE_EXCHANGE_VERSION,
            sender: secret.public(),
            recipient,
            nonce,
            issued_at_secs,
            expires_at_secs,
            profile,
            signature: Vec::new(),
        };
        exchange.signature = exchange.signing_bytes()?.pipe(|bytes| secret.sign(&bytes).to_bytes().to_vec());
        Ok(exchange)
    }

    fn signing_fields(&self) -> (&u16, &PublicKey, &PublicKey, &u64, &u64, &u64, &PublicUserProfile) {
        (
            &self.version,
            &self.sender,
            &self.recipient,
            &self.nonce,
            &self.issued_at_secs,
            &self.expires_at_secs,
            &self.profile,
        )
    }

    fn signing_bytes(&self) -> Result<Vec<u8>, String> {
        protocol_signing::canonical_signed_bytes(
            PROFILE_EXCHANGE_SIGNING_DOMAIN,
            PROFILE_EXCHANGE_VERSION,
            &self.signing_fields(),
        )
        .map_err(|e| format!("encode profile exchange: {e}"))
    }

    /// Validate authentication, recipient binding, expiry, size, and profile bounds.
    pub fn validate_for(&self, local: PublicKey, now_secs: u64) -> Result<(), String> {
        let encoded = postcard::to_stdvec(self).map_err(|e| format!("decode profile exchange: {e}"))?;
        if encoded.len() > MAX_PROFILE_EXCHANGE_BYTES {
            return Err("profile exchange exceeds size limit".into());
        }
        if self.version != PROFILE_EXCHANGE_VERSION {
            return Err("unsupported profile exchange version".into());
        }
        if self.sender == local {
            return Err("self profile exchange".into());
        }
        if self.recipient != local {
            return Err("profile exchange addressed to another recipient".into());
        }
        if self.expires_at_secs < self.issued_at_secs
            || self.expires_at_secs.saturating_sub(self.issued_at_secs) > MAX_PROFILE_EXCHANGE_TTL_SECS
            || now_secs < self.issued_at_secs
            || now_secs > self.expires_at_secs
        {
            return Err("expired or invalid profile exchange lifetime".into());
        }
        self.profile.validate().map_err(|e| format!("invalid profile: {e}"))?;
        let bytes = self.signing_bytes()?;
        if !protocol_signing::verify(&self.sender, &self.signature, &bytes) {
            return Err("invalid profile exchange signature".into());
        }
        Ok(())
    }
}

/// Local publication and recipient authorization policy. Defaults to deny.
#[derive(Debug, Default, Clone)]
pub struct ProfileExchangePolicy {
    publication_enabled: bool,
    authorized_recipients: HashSet<PublicKey>,
}

impl ProfileExchangePolicy {
    /// Construct the safe default: no publication and no authorized recipients.
    pub fn disabled() -> Self { Self::default() }
    /// Enable or disable local profile publication.
    pub fn set_publication_enabled(&mut self, enabled: bool) { self.publication_enabled = enabled; }
    /// Authorize a contact or room peer explicitly.
    pub fn authorize_recipient(&mut self, peer: PublicKey) { self.authorized_recipients.insert(peer); }
    /// Remove a recipient authorization.
    pub fn revoke_recipient(&mut self, peer: &PublicKey) { self.authorized_recipients.remove(peer); }
    /// Whether publication to this explicitly authorized recipient is allowed.
    pub fn can_publish_to(&self, peer: &PublicKey) -> bool {
        self.publication_enabled && self.authorized_recipients.contains(peer)
    }
}

/// Per-sender replay and rate-limit state for received exchanges.
#[derive(Debug, Default)]
pub struct ProfileExchangeGuard {
    seen: HashMap<(PublicKey, u64), u64>,
    windows: HashMap<PublicKey, VecDeque<u64>>,
}

impl ProfileExchangeGuard {
    /// Admit one already-decoded envelope, or return a reason for dropping it.
    pub fn admit(&mut self, exchange: &ProfileExchange, local: PublicKey, now_secs: u64) -> Result<(), String> {
        exchange.validate_for(local, now_secs)?;
        let key = (exchange.sender, exchange.nonce);
        if self.seen.contains_key(&key) {
            return Err("replayed profile exchange".into());
        }
        let window = self.windows.entry(exchange.sender).or_default();
        while window.front().is_some_and(|timestamp| now_secs.saturating_sub(*timestamp) >= PROFILE_EXCHANGE_WINDOW_SECS) {
            window.pop_front();
        }
        if window.len() >= MAX_PROFILE_EXCHANGES_PER_WINDOW {
            return Err("profile exchange rate limit exceeded".into());
        }
        window.push_back(now_secs);
        self.seen.insert(key, now_secs);
        self.seen.retain(|_, timestamp| now_secs.saturating_sub(*timestamp) < PROFILE_EXCHANGE_WINDOW_SECS);
        Ok(())
    }
}

trait Pipe: Sized { fn pipe<T>(self, f: impl FnOnce(Self) -> T) -> T { f(self) } }
impl<T> Pipe for T {}

#[cfg(test)]
mod tests {
    use super::*;

    fn profile() -> PublicUserProfile {
        PublicUserProfile { display_name: "Alice".into(), bio: "Hi".into(), avatar_identifier: None, shared_files: vec![], revision: 1 }
    }

    #[test]
    fn signed_exchange_requires_intended_recipient_and_verifies() {
        let sender = SecretKey::generate();
        let recipient = SecretKey::generate().public();
        let other = SecretKey::generate().public();
        let exchange = ProfileExchange::sign(&sender, recipient, 1, 100, profile()).unwrap();
        assert!(exchange.validate_for(recipient, 101).is_ok());
        assert!(exchange.validate_for(other, 101).is_err());
    }

    #[test]
    fn default_policy_is_opt_in_and_authorized() {
        let peer = SecretKey::generate().public();
        let mut policy = ProfileExchangePolicy::disabled();
        assert!(!policy.can_publish_to(&peer));
        policy.authorize_recipient(peer);
        assert!(!policy.can_publish_to(&peer));
        policy.set_publication_enabled(true);
        assert!(policy.can_publish_to(&peer));
    }

    #[test]
    fn guard_rejects_replay_expiry_and_rate_flood() {
        let sender = SecretKey::generate();
        let recipient = SecretKey::generate().public();
        let mut guard = ProfileExchangeGuard::default();
        let exchange = ProfileExchange::sign(&sender, recipient, 1, 100, profile()).unwrap();
        assert!(guard.admit(&exchange, recipient, 101).is_ok());
        assert!(guard.admit(&exchange, recipient, 101).is_err());
        for nonce in 2..=MAX_PROFILE_EXCHANGES_PER_WINDOW as u64 { 
            let next = ProfileExchange::sign(&sender, recipient, nonce, 102, profile()).unwrap();
            assert!(guard.admit(&next, recipient, 102).is_ok());
        }
        let flood = ProfileExchange::sign(&sender, recipient, 99, 102, profile()).unwrap();
        assert!(guard.admit(&flood, recipient, 102).is_err());
        let expired = ProfileExchange::sign(&sender, recipient, 100, 100, profile()).unwrap();
        assert!(guard.admit(&expired, recipient, 100 + MAX_PROFILE_EXCHANGE_TTL_SECS + 1).is_err());
    }
}
