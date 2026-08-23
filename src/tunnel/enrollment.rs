//! One-time enrollment tokens for Boru's secure tunnel transport.
//!
//! Mirrors the out-of-band `PairingInvitation` pattern from
//! `src/pairing_service.rs`: the tunnel owner mints a short-lived token, a
//! headless peer presents it on its first connection, and on success the peer's
//! key is pinned for that tunnel. The pin is persisted so a restart does not
//! orphan an already-enrolled peer.
//!
//! # Security properties
//!
//! - Tokens are one-time: a redeemed token is rejected forever afterwards
//!   (replay rejection).
//! - Tokens expire: a token cannot be redeemed after `expires_at_ms`.
//! - Pins are per-tunnel and per-peer: possession of a pin authorises exactly
//!   one peer key for exactly one tunnel, never arbitrary destinations.
//! - Only SHA-256 hashes of tokens are stored in memory and on disk; the raw
//!   token value exists only in the `mint` return value and the peer's copy.
//! - The store is additive: existing capability-based tunnel creation flows
//!   never touch the enrollment store.

use std::{
    path::{Path, PathBuf},
    sync::RwLock,
    time::{SystemTime, UNIX_EPOCH},
};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::TunnelId;

/// File name for the persisted enrollment token store.
const ENROLLMENT_STORE_FILE: &str = "tunnel_enrollments.json";

/// Default lifetime for a minted enrollment token (ten minutes).
pub const DEFAULT_ENROLLMENT_TTL_SECS: u64 = 10 * 60;

/// Error returned by enrollment-token operations.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EnrollmentError {
    /// The presented token does not match any minted token.
    UnknownToken,
    /// The token has expired and can no longer be redeemed.
    TokenExpired,
    /// The token was already redeemed; a token is one-time.
    TokenAlreadyUsed,
    /// The token was minted for a different tunnel.
    TunnelMismatch,
    /// Persisting or loading the store failed.
    Store(String),
}

impl std::fmt::Display for EnrollmentError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::UnknownToken => f.write_str("unknown enrollment token"),
            Self::TokenExpired => f.write_str("enrollment token expired"),
            Self::TokenAlreadyUsed => f.write_str("enrollment token already used"),
            Self::TunnelMismatch => f.write_str("enrollment token tunnel mismatch"),
            Self::Store(message) => write!(f, "enrollment store: {message}"),
        }
    }
}

impl std::error::Error for EnrollmentError {}

/// A minted, not-yet-redeemed enrollment token.
///
/// The raw token string is returned once by [`EnrollmentTokenStore::mint`] and
/// never stored; the store keeps only its SHA-256 hash.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EnrollmentToken {
    /// The raw token value the peer presents on its first connection.
    pub token: String,
    /// Unix epoch milliseconds at which the token becomes invalid.
    pub expires_at_ms: u64,
}

/// On-disk representation of a minted token (hash only, never the raw value).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct StoredToken {
    /// SHA-256 of the raw token, hex-encoded.
    token_hash: String,
    /// Tunnel this token authorises.
    tunnel_id: TunnelId,
    /// Unix epoch milliseconds at which the token was minted.
    created_at_ms: u64,
    /// Unix epoch milliseconds at which the token expires.
    expires_at_ms: u64,
    /// Whether the token has already been redeemed.
    used: bool,
}

/// On-disk representation of a pinned peer.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct StoredPin {
    /// Tunnel the peer is pinned to.
    tunnel_id: TunnelId,
    /// Peer public key, hex-encoded.
    peer: String,
}

/// Whole-store JSON shape.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct StoreState {
    #[serde(default)]
    tokens: Vec<StoredToken>,
    #[serde(default)]
    pins: Vec<StoredPin>,
}

/// One-time enrollment token store with optional on-disk persistence.
///
/// All mutations are protected by an internal lock, so the store is safe to
/// share behind an `Arc` with the tunnel service and the GUI.
#[derive(Debug)]
pub struct EnrollmentTokenStore {
    state: RwLock<StoreState>,
    path: Option<PathBuf>,
}

impl Default for EnrollmentTokenStore {
    fn default() -> Self {
        Self::new()
    }
}

impl EnrollmentTokenStore {
    /// Construct an in-memory store with no persistence.
    pub fn new() -> Self {
        Self {
            state: RwLock::new(StoreState::default()),
            path: None,
        }
    }

    /// Load a store from `data_dir/tunnel_enrollments.json`, or start empty
    /// when the file does not exist. A corrupt file starts empty (the store is
    /// best-effort state; a corrupt file should not prevent tunnel startup).
    pub fn load_or_default(data_dir: &Path) -> Self {
        let path = data_dir.join(ENROLLMENT_STORE_FILE);
        let state = match std::fs::read_to_string(&path) {
            Ok(raw) => serde_json::from_str(&raw).unwrap_or_default(),
            Err(_) => StoreState::default(),
        };
        Self {
            state: RwLock::new(state),
            path: Some(path),
        }
    }

    /// Mint a fresh one-time enrollment token for a tunnel.
    ///
    /// Returns the raw token (to be shared out-of-band with the headless peer)
    /// and its expiry. The store keeps only the SHA-256 hash.
    pub fn mint(
        &self,
        tunnel_id: TunnelId,
        ttl_secs: u64,
    ) -> Result<EnrollmentToken, EnrollmentError> {
        let now = unix_epoch_ms();
        let raw: [u8; 32] = rand::random();
        let token = hex::encode(raw);
        let token_hash = sha256_hex(token.as_bytes());
        let expires_at_ms = now.saturating_add(ttl_secs.saturating_mul(1_000));
        let stored = StoredToken {
            token_hash,
            tunnel_id,
            created_at_ms: now,
            expires_at_ms,
            used: false,
        };
        self.state
            .write()
            .expect("enrollment store lock poisoned")
            .tokens
            .push(stored);
        self.persist();
        Ok(EnrollmentToken {
            token,
            expires_at_ms,
        })
    }

    /// Redeem a token on a peer's first tunnel connection.
    ///
    /// On success the token is marked used (one-time) and the peer key is
    /// pinned for the tunnel. Replaying the same token later returns
    /// [`EnrollmentError::TokenAlreadyUsed`].
    pub fn redeem(
        &self,
        token: &str,
        tunnel_id: TunnelId,
        peer: &iroh::PublicKey,
        now_ms: u64,
    ) -> Result<(), EnrollmentError> {
        let token_hash = sha256_hex(token.as_bytes());
        let mut state = self.state.write().expect("enrollment store lock poisoned");
        let stored = state
            .tokens
            .iter_mut()
            .find(|stored| stored.token_hash == token_hash)
            .ok_or(EnrollmentError::UnknownToken)?;
        if stored.tunnel_id != tunnel_id {
            return Err(EnrollmentError::TunnelMismatch);
        }
        if now_ms > stored.expires_at_ms {
            return Err(EnrollmentError::TokenExpired);
        }
        if stored.used {
            return Err(EnrollmentError::TokenAlreadyUsed);
        }
        stored.used = true;
        let peer_hex = peer.to_string();
        if !state
            .pins
            .iter()
            .any(|pin| pin.tunnel_id == tunnel_id && pin.peer == peer_hex)
        {
            state.pins.push(StoredPin {
                tunnel_id,
                peer: peer_hex,
            });
        }
        drop(state);
        self.persist();
        Ok(())
    }

    /// Return whether a peer key is already pinned for a tunnel.
    pub fn is_pinned(&self, tunnel_id: TunnelId, peer: &iroh::PublicKey) -> bool {
        let peer_hex = peer.to_string();
        self.state
            .read()
            .expect("enrollment store lock poisoned")
            .pins
            .iter()
            .any(|pin| pin.tunnel_id == tunnel_id && pin.peer == peer_hex)
    }

    /// Remove the pin for a peer on a tunnel (used when a tunnel is revoked).
    pub fn unpin(&self, tunnel_id: TunnelId, peer: &iroh::PublicKey) {
        let peer_hex = peer.to_string();
        let mut state = self.state.write().expect("enrollment store lock poisoned");
        state
            .pins
            .retain(|pin| !(pin.tunnel_id == tunnel_id && pin.peer == peer_hex));
        drop(state);
        self.persist();
    }

    /// Remove all tokens and pins for a tunnel (used when a tunnel is revoked).
    pub fn remove_tunnel(&self, tunnel_id: TunnelId) {
        let mut state = self.state.write().expect("enrollment store lock poisoned");
        state.tokens.retain(|token| token.tunnel_id != tunnel_id);
        state.pins.retain(|pin| pin.tunnel_id != tunnel_id);
        drop(state);
        self.persist();
    }

    fn persist(&self) {
        let Some(path) = &self.path else {
            return;
        };
        let state = self.state.read().expect("enrollment store lock poisoned");
        let raw = serde_json::to_string_pretty(&*state)
            .expect("enrollment store serialization cannot fail");
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        if let Err(error) = std::fs::write(path, raw) {
            tracing::warn!(%error, path = %path.display(), "failed to persist enrollment store");
        }
    }
}

fn unix_epoch_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock before Unix epoch")
        .as_millis() as u64
}

fn sha256_hex(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    hex::encode(hasher.finalize())
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use iroh::SecretKey;
    use tempfile::TempDir;

    use super::{EnrollmentError, EnrollmentTokenStore};
    use crate::tunnel::TunnelId;

    fn fixture() -> (EnrollmentTokenStore, TunnelId, iroh::PublicKey) {
        let store = EnrollmentTokenStore::new();
        let tunnel = TunnelId([7; 32]);
        let peer = SecretKey::generate().public();
        (store, tunnel, peer)
    }

    #[test]
    fn mint_returns_token_and_persists_hash_only() {
        let (store, tunnel, _peer) = fixture();
        let minted = store.mint(tunnel, 600).expect("mint");
        assert_eq!(minted.token.len(), 64);
        assert!(minted.expires_at_ms > 0);
        // The raw token value must never be stored.
        let state = store.state.read().unwrap();
        assert!(state
            .tokens
            .iter()
            .all(|t| !t.token_hash.contains(&minted.token)));
    }

    #[test]
    fn redeem_succeeds_once_then_rejects_replay() {
        let (store, tunnel, peer) = fixture();
        let now = super::unix_epoch_ms();
        let minted = store.mint(tunnel, 600).expect("mint");
        store
            .redeem(&minted.token, tunnel, &peer, now)
            .expect("first redeem");
        assert!(store.is_pinned(tunnel, &peer));
        assert_eq!(
            store.redeem(&minted.token, tunnel, &peer, now),
            Err(EnrollmentError::TokenAlreadyUsed),
            "replaying the same token must be rejected"
        );
    }

    #[test]
    fn expired_token_is_rejected() {
        let (store, tunnel, peer) = fixture();
        let minted = store.mint(tunnel, 1).expect("mint");
        // Advance past expiry.
        let later = super::unix_epoch_ms() + Duration::from_secs(10).as_millis() as u64;
        assert_eq!(
            store.redeem(&minted.token, tunnel, &peer, later),
            Err(EnrollmentError::TokenExpired)
        );
        assert!(!store.is_pinned(tunnel, &peer));
    }

    #[test]
    fn unknown_token_is_rejected() {
        let (store, tunnel, peer) = fixture();
        assert_eq!(
            store.redeem("not-a-real-token", tunnel, &peer, super::unix_epoch_ms()),
            Err(EnrollmentError::UnknownToken)
        );
    }

    #[test]
    fn token_is_bound_to_its_tunnel() {
        let (store, tunnel, peer) = fixture();
        let other = TunnelId([8; 32]);
        let minted = store.mint(tunnel, 600).expect("mint");
        assert_eq!(
            store.redeem(&minted.token, other, &peer, super::unix_epoch_ms()),
            Err(EnrollmentError::TunnelMismatch)
        );
    }

    #[test]
    fn store_survives_restart_with_pin() {
        let dir = TempDir::new().expect("tempdir");
        let tunnel = TunnelId([9; 32]);
        let peer = SecretKey::generate().public();

        let store = EnrollmentTokenStore::load_or_default(dir.path());
        let minted = store.mint(tunnel, 600).expect("mint");
        store
            .redeem(&minted.token, tunnel, &peer, super::unix_epoch_ms())
            .expect("redeem");

        // A fresh store instance (simulating restart) must still know the pin.
        let reloaded = EnrollmentTokenStore::load_or_default(dir.path());
        assert!(reloaded.is_pinned(tunnel, &peer));
        // The used token is still rejected on the reloaded store.
        assert_eq!(
            reloaded.redeem(&minted.token, tunnel, &peer, super::unix_epoch_ms()),
            Err(EnrollmentError::TokenAlreadyUsed)
        );
    }

    #[test]
    fn remove_tunnel_clears_tokens_and_pins() {
        let (store, tunnel, peer) = fixture();
        let minted = store.mint(tunnel, 600).expect("mint");
        store
            .redeem(&minted.token, tunnel, &peer, super::unix_epoch_ms())
            .expect("redeem");
        store.remove_tunnel(tunnel);
        assert!(!store.is_pinned(tunnel, &peer));
        assert_eq!(
            store.redeem(&minted.token, tunnel, &peer, super::unix_epoch_ms()),
            Err(EnrollmentError::UnknownToken)
        );
    }
}
