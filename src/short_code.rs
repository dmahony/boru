//! PAKE-style short-code file shares (FS-26).
//!
//! A short code is a 7-character alphanumeric code that resolves to a
//! [`iroh_blobs::ticket::BlobTicket`] on the sharing peer.  The sender mints
//! a code bound to a ticket and shows it to the receiver; the receiver types
//! the code and gets the same transfer as pasting the full ticket — without
//! copying a long string.
//!
//! # Rendezvous
//!
//! Both sides derive the same gossip topic from the code
//! ([`derive_shortcode_topic`]), so no prior relationship or address exchange
//! is required (mirroring how the public-room directory derives a topic from
//! the relay URL).  The sender subscribes to the topic and broadcasts a
//! signed [`ShortCodeAnnouncement`] containing the ticket; the receiver
//! subscribes to the same topic and picks the announcement up.
//!
//! # Security properties
//!
//! - **Expiry** — each code carries a `created_at` and `expires_at`; a
//!   resolved code must still be inside its validity window
//!   ([`ShortCodeError::Expired`]).
//! - **Replay rejection** — a code is single-use: [`ShortCodeStore::resolve`]
//!   marks the code `used` on first successful resolution and rejects any
//!   subsequent resolution ([`ShortCodeError::AlreadyUsed`]).
//! - **Authenticity** — announcements are signed with the sender's iroh
//!   secret key; receivers verify before trusting the ticket.
//!
//! Persistence follows the `pairing_service.rs` pattern: a JSON file in the
//! data directory written atomically, so codes survive restarts.

use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use n0_error::{bail_any, Result, StdResultExt};
use serde::{Deserialize, Serialize};

use crate::chat_core::atomic_write::atomic_write_json;
use crate::proto::TopicId;

/// File name for the persisted short-code store.
const SHORT_CODES_FILE: &str = "short_codes.json";

/// Length of a short code in characters.
pub const SHORT_CODE_LEN: usize = 7;

/// Default lifetime of a minted short code.
pub const DEFAULT_SHORT_CODE_TTL: Duration = Duration::from_secs(15 * 60);

/// Alphabet used for short codes.
///
/// Deliberately excludes confusable characters (`0/O`, `1/I/l`) so codes can
/// be read aloud and typed reliably.  Lowercase is normalised to uppercase on
/// resolve.
const CODE_ALPHABET: &[u8] = b"ABCDEFGHJKMNPQRSTUVWXYZ23456789";

/// Domain separator for the short-code rendezvous topic derivation.
const SHORTCODE_DOMAIN_SEPARATOR: &[u8] = b"boru-chat/short-code/v1";

/// A minted short-code grant.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ShortCodeGrant {
    /// The 7-character code.
    pub code: String,
    /// Serialized blob ticket the code resolves to.
    pub ticket: String,
    /// Display file name carried with the ticket.
    pub name: String,
    /// Expected total size in bytes (0 when unknown).
    pub size: u64,
    /// Unix milliseconds when the code was minted.
    pub created_at_ms: u64,
    /// Unix milliseconds after which the code is invalid.
    pub expires_at_ms: u64,
    /// True once the code has been resolved (single-use / replay rejection).
    #[serde(default)]
    pub used: bool,
}

impl ShortCodeGrant {
    /// Whether `now_ms` is past the expiry window.
    pub fn is_expired(&self, now_ms: u64) -> bool {
        now_ms > self.expires_at_ms
    }
}

/// Errors from resolving a short code.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ShortCodeError {
    /// No grant exists for the code.
    UnknownCode,
    /// The code exists but its validity window has passed.
    Expired,
    /// The code was already resolved (replay attempt).
    AlreadyUsed,
}

impl std::fmt::Display for ShortCodeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::UnknownCode => write!(f, "unknown short code"),
            Self::Expired => write!(f, "short code has expired"),
            Self::AlreadyUsed => write!(f, "short code was already used"),
        }
    }
}

/// Result of a successful code resolution.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedShortCode {
    /// The serialized blob ticket.
    pub ticket: String,
    /// Display file name.
    pub name: String,
    /// Expected total size in bytes.
    pub size: u64,
}

/// Generate a random 7-character code from [`CODE_ALPHABET`].
///
/// Uses rejection sampling over [`getrandom`] bytes for a uniform distribution
/// over the alphabet (34 symbols).
pub fn generate_code() -> Result<String> {
    let mut out = String::with_capacity(SHORT_CODE_LEN);
    for _ in 0..SHORT_CODE_LEN {
        let byte = random_alphabet_byte()?;
        out.push(byte as char);
    }
    Ok(out)
}

/// One uniform alphabet byte via rejection sampling.
fn random_alphabet_byte() -> Result<u8> {
    const ALPHABET_LEN: u32 = CODE_ALPHABET.len() as u32;
    // Rejection sampling: 256 = floor(256 / 34) * 34 + 16.  Accept values in
    // [0, 238) so each symbol is equally likely; retry on the 16-byte bias.
    const LIMIT: u32 = (u8::MAX as u32 / ALPHABET_LEN) * ALPHABET_LEN;
    let mut buf = [0u8; 1];
    loop {
        getrandom::fill(&mut buf).map_err(|e| n0_error::anyerr!("short-code rng failure: {e}"))?;
        let v = buf[0] as u32;
        if v < LIMIT {
            return Ok(CODE_ALPHABET[(v % ALPHABET_LEN) as usize]);
        }
    }
}

/// Normalise a user-typed code: trim, uppercase, strip separators.
///
/// Accepts spaces/hyphens between groups (e.g. `ABC-2345` or `ABC 2345`) so
/// codes are easy to read aloud.
pub fn normalise_code(input: &str) -> String {
    input
        .chars()
        .filter(|c| !c.is_whitespace() && *c != '-')
        .collect::<String>()
        .to_uppercase()
}

/// Derive the rendezvous gossip topic for a code.
///
/// ```text
/// TopicId = BLAKE3("boru-chat/short-code/v1" || code)
/// ```
///
/// Both the sender and the receiver derive the same topic from the code, so
/// they rendezvous on the gossip mesh without exchanging addresses first.
pub fn derive_shortcode_topic(code: &str) -> TopicId {
    let mut hasher = blake3::Hasher::new();
    hasher.update(SHORTCODE_DOMAIN_SEPARATOR);
    hasher.update(code.as_bytes());
    TopicId::from_bytes(*hasher.finalize().as_bytes())
}

// ── Persisted store ───────────────────────────────────────────────────────

/// Persistent store of short-code grants.
///
/// The store is a single JSON file ([`SHORT_CODES_FILE`]) in the data
/// directory, loaded lazily and written atomically on every mutation — the
/// same pattern as `pairing_service::PendingPairing`.
#[derive(Debug, Clone)]
pub struct ShortCodeStore {
    data_dir: PathBuf,
    grants: Vec<ShortCodeGrant>,
}

impl ShortCodeStore {
    /// Load the store from `data_dir` (creating an empty store if the file
    /// does not exist).  A corrupt file is treated as an error so callers can
    /// decide how to surface it.
    pub fn load_or_default(data_dir: &Path) -> Result<Self> {
        let path = short_codes_path(data_dir);
        let grants = if path.exists() {
            let raw = std::fs::read_to_string(&path)
                .with_std_context(|_| format!("failed to read {}", path.display()))?;
            serde_json::from_str(&raw)
                .with_std_context(|_| format!("failed to parse {}", path.display()))?
        } else {
            Vec::new()
        };
        Ok(Self {
            data_dir: data_dir.to_path_buf(),
            grants,
        })
    }

    /// Persist the current grant list atomically.
    pub fn save(&self) -> Result<()> {
        atomic_write_json(
            &short_codes_path(&self.data_dir),
            &self.grants,
            "short codes",
        )
    }

    /// Mint a new code bound to `ticket`, persist it, and return the code.
    ///
    /// `ttl` bounds the validity window; `size` is the expected payload size
    /// (0 when unknown).
    pub fn mint(&mut self, ticket: &str, name: &str, size: u64, ttl: Duration) -> Result<String> {
        let now_ms = now_unix_ms();
        let code = loop {
            let candidate = generate_code()?;
            if !self.grants.iter().any(|g| g.code == candidate) {
                break candidate;
            }
        };
        self.grants.push(ShortCodeGrant {
            code: code.clone(),
            ticket: ticket.to_string(),
            name: name.to_string(),
            size,
            created_at_ms: now_ms,
            expires_at_ms: now_ms + ttl.as_millis() as u64,
            used: false,
        });
        self.save()?;
        Ok(code)
    }

    /// Resolve a code to its ticket, enforcing expiry and single-use.
    ///
    /// On success the grant is marked `used` and persisted before the ticket
    /// is returned, so a concurrent or later replay is rejected.
    pub fn resolve(
        &mut self,
        code: &str,
    ) -> std::result::Result<ResolvedShortCode, ShortCodeError> {
        let normalised = normalise_code(code);
        let now_ms = now_unix_ms();
        let idx = self
            .grants
            .iter()
            .position(|g| g.code == normalised)
            .ok_or(ShortCodeError::UnknownCode)?;
        let grant = &self.grants[idx];
        if grant.used {
            return Err(ShortCodeError::AlreadyUsed);
        }
        if grant.is_expired(now_ms) {
            return Err(ShortCodeError::Expired);
        }
        let resolved = ResolvedShortCode {
            ticket: grant.ticket.clone(),
            name: grant.name.clone(),
            size: grant.size,
        };
        self.grants[idx].used = true;
        // Persist the single-use mark before returning so a crash between the
        // mark and the transfer cannot enable replay.
        self.save().map_err(|e| {
            tracing::warn!("short-code: failed to persist used mark: {e}");
            ShortCodeError::UnknownCode
        })?;
        Ok(resolved)
    }

    /// Look up a grant without consuming it (e.g. to show the code again).
    pub fn lookup(&self, code: &str) -> Option<&ShortCodeGrant> {
        let normalised = normalise_code(code);
        self.grants.iter().find(|g| g.code == normalised)
    }

    /// Remove a grant by code (e.g. when the code's transfer is abandoned).
    pub fn remove(&mut self, code: &str) -> Result<()> {
        let normalised = normalise_code(code);
        let before = self.grants.len();
        self.grants.retain(|g| g.code != normalised);
        if self.grants.len() != before {
            self.save()?;
        }
        Ok(())
    }

    /// All grants, newest first (for a UI history if needed).
    pub fn grants(&self) -> impl Iterator<Item = &ShortCodeGrant> {
        self.grants.iter().rev()
    }
}

fn short_codes_path(data_dir: &Path) -> PathBuf {
    data_dir.join(SHORT_CODES_FILE)
}

fn now_unix_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

// ── Signed announcement ───────────────────────────────────────────────────

/// The ticket announcement broadcast on the rendezvous topic.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ShortCodeAnnouncement {
    /// The code being redeemed (receiver verifies it matches its input).
    pub code: String,
    /// Display file name.
    pub name: String,
    /// Serialized blob ticket.
    pub ticket: String,
    /// Expected total size in bytes.
    pub size: u64,
    /// Unix milliseconds when the sender minted the code.  Receivers use this
    /// to bound how old an announcement may be.
    pub created_at_ms: u64,
}

/// Signed envelope for a [`ShortCodeAnnouncement`], following the
/// `SignedContactMessage` wire pattern.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SignedShortCodeAnnouncement {
    /// Identity of the signer.
    pub from: iroh::PublicKey,
    /// Unix seconds when the envelope was signed (replay bound).
    pub sent_at_unix_secs: u64,
    /// Postcard-encoded [`ShortCodeAnnouncement`].
    pub data: Vec<u8>,
    /// Signature over `sent_at_unix_secs || data`.
    pub signature: iroh::Signature,
}

impl SignedShortCodeAnnouncement {
    /// Sign an announcement with the sender's secret key.
    pub fn sign(
        secret_key: &iroh::SecretKey,
        announcement: &ShortCodeAnnouncement,
    ) -> Result<Vec<u8>> {
        let data = postcard::to_stdvec(announcement)
            .map_err(|e| n0_error::anyerr!("encode short-code announcement: {e}"))?;
        let sent_at_unix_secs = now_unix_ms() / 1000;
        let signing_data = signing_bytes(sent_at_unix_secs, &data);
        let signature = secret_key.sign(&signing_data);
        let envelope = Self {
            from: secret_key.public(),
            sent_at_unix_secs,
            data,
            signature,
        };
        postcard::to_stdvec(&envelope)
            .map_err(|e| n0_error::anyerr!("encode signed short-code announcement: {e}"))
    }

    /// Verify the envelope, decode the announcement, and require the
    /// announcement's code to match `expected_code` (normalised).
    pub fn verify(
        bytes: &[u8],
        expected_code: &str,
    ) -> std::result::Result<(iroh::PublicKey, ShortCodeAnnouncement), ShortCodeError> {
        let envelope: Self = postcard::from_bytes(bytes).map_err(|e| {
            tracing::debug!("short-code: decode envelope failed: {e}");
            ShortCodeError::UnknownCode
        })?;
        envelope
            .from
            .verify(
                &signing_bytes(envelope.sent_at_unix_secs, &envelope.data),
                &envelope.signature,
            )
            .map_err(|e| {
                tracing::debug!("short-code: signature verification failed: {e}");
                ShortCodeError::UnknownCode
            })?;
        let announcement: ShortCodeAnnouncement =
            postcard::from_bytes(&envelope.data).map_err(|e| {
                tracing::debug!("short-code: decode announcement failed: {e}");
                ShortCodeError::UnknownCode
            })?;
        if normalise_code(&announcement.code) != normalise_code(expected_code) {
            return Err(ShortCodeError::UnknownCode);
        }
        Ok((envelope.from, announcement))
    }
}

fn signing_bytes(timestamp: u64, data: &[u8]) -> Vec<u8> {
    let mut bytes = timestamp.to_le_bytes().to_vec();
    bytes.extend_from_slice(data);
    bytes
}

// ── Tests ─────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    fn test_store() -> (TempDir, ShortCodeStore) {
        let dir = TempDir::new().unwrap();
        let store = ShortCodeStore::load_or_default(dir.path()).unwrap();
        (dir, store)
    }

    fn sample_ticket() -> String {
        "blob:abcdef0123456789abcdef0123456789abcdef0123456789abcdef0123456789".to_string()
    }

    // ── Code generation ──────────────────────────────────────────────

    #[test]
    fn generated_code_has_expected_length_and_alphabet() {
        for _ in 0..50 {
            let code = generate_code().unwrap();
            assert_eq!(code.len(), SHORT_CODE_LEN);
            assert!(
                code.bytes().all(|b| CODE_ALPHABET.contains(&b)),
                "code {code} contains a char outside the alphabet"
            );
            // No confusables: 0, O, 1, I, l must never appear.
            assert!(!code.contains('0') && !code.contains('O'));
            assert!(!code.contains('1') && !code.contains('I') && !code.contains('l'));
        }
    }

    #[test]
    fn codes_are_mostly_unique() {
        let codes: std::collections::HashSet<String> =
            (0..200).map(|_| generate_code().unwrap()).collect();
        assert!(
            codes.len() > 190,
            "expected high uniqueness, got {}",
            codes.len()
        );
    }

    #[test]
    fn normalise_code_handles_case_and_separators() {
        assert_eq!(normalise_code(" abc-2345 "), "ABC2345");
        assert_eq!(normalise_code("abc 2345"), "ABC2345");
        assert_eq!(normalise_code("ABC2345"), "ABC2345");
    }

    // ── Topic derivation ─────────────────────────────────────────────

    #[test]
    fn topic_is_deterministic_and_code_specific() {
        let a1 = derive_shortcode_topic("ABC2345");
        let a2 = derive_shortcode_topic("ABC2345");
        let b = derive_shortcode_topic("XYZ9876");
        assert_eq!(a1, a2);
        assert_ne!(a1, b);
    }

    // ── Mint / resolve roundtrip ─────────────────────────────────────

    #[test]
    fn mint_resolve_roundtrip_returns_ticket() {
        let (_dir, mut store) = test_store();
        let code = store
            .mint(&sample_ticket(), "report.pdf", 4096, DEFAULT_SHORT_CODE_TTL)
            .unwrap();
        assert_eq!(code.len(), SHORT_CODE_LEN);

        let resolved = store.resolve(&code).unwrap();
        assert_eq!(resolved.ticket, sample_ticket());
        assert_eq!(resolved.name, "report.pdf");
        assert_eq!(resolved.size, 4096);
    }

    #[test]
    fn resolve_accepts_lowercase_and_separators() {
        let (_dir, mut store) = test_store();
        let code = store
            .mint(&sample_ticket(), "a.bin", 1, DEFAULT_SHORT_CODE_TTL)
            .unwrap();
        let lowered = code.to_lowercase();
        let resolved = store.resolve(&lowered).unwrap();
        assert_eq!(resolved.ticket, sample_ticket());
    }

    // ── Expiry ───────────────────────────────────────────────────────

    #[test]
    fn expired_code_is_rejected() {
        let (_dir, mut store) = test_store();
        let code = store
            .mint(&sample_ticket(), "old.bin", 1, Duration::from_secs(1))
            .unwrap();
        // Force expiry by backdating the grant.
        if let Some(g) = store.grants.iter_mut().find(|g| g.code == code) {
            g.expires_at_ms = now_unix_ms() - 1;
        }
        let err = store.resolve(&code).unwrap_err();
        assert_eq!(err, ShortCodeError::Expired);
    }

    // ── Replay rejection ─────────────────────────────────────────────

    #[test]
    fn second_resolve_is_rejected_as_replay() {
        let (_dir, mut store) = test_store();
        let code = store
            .mint(&sample_ticket(), "once.bin", 2, DEFAULT_SHORT_CODE_TTL)
            .unwrap();
        store.resolve(&code).unwrap();
        let err = store.resolve(&code).unwrap_err();
        assert_eq!(err, ShortCodeError::AlreadyUsed);
    }

    #[test]
    fn unknown_code_is_rejected() {
        let (_dir, mut store) = test_store();
        let err = store.resolve("ZZZ9999").unwrap_err();
        assert_eq!(err, ShortCodeError::UnknownCode);
    }

    // ── Persistence across reload ────────────────────────────────────

    #[test]
    fn store_persists_and_reloads_used_mark() {
        let dir = TempDir::new().unwrap();
        let code = {
            let mut store = ShortCodeStore::load_or_default(dir.path()).unwrap();
            let c = store
                .mint(&sample_ticket(), "persist.bin", 3, DEFAULT_SHORT_CODE_TTL)
                .unwrap();
            store.resolve(&c).unwrap();
            c
        };
        // Reload from disk — the used mark must have persisted.
        let mut store = ShortCodeStore::load_or_default(dir.path()).unwrap();
        assert!(store.lookup(&code).unwrap().used);
        assert_eq!(
            store.resolve(&code).unwrap_err(),
            ShortCodeError::AlreadyUsed
        );
    }

    #[test]
    fn remove_drops_grant() {
        let (_dir, mut store) = test_store();
        let code = store
            .mint(&sample_ticket(), "del.bin", 4, DEFAULT_SHORT_CODE_TTL)
            .unwrap();
        store.remove(&code).unwrap();
        assert!(store.lookup(&code).is_none());
        assert_eq!(
            store.resolve(&code).unwrap_err(),
            ShortCodeError::UnknownCode
        );
    }

    // ── Signed announcement ──────────────────────────────────────────

    #[test]
    fn announcement_sign_verify_roundtrip() {
        let sk = iroh::SecretKey::generate();
        let ann = ShortCodeAnnouncement {
            code: "ABC2345".to_string(),
            name: "photo.jpg".to_string(),
            ticket: sample_ticket(),
            size: 8192,
            created_at_ms: now_unix_ms(),
        };
        let bytes = SignedShortCodeAnnouncement::sign(&sk, &ann).unwrap();
        let (from, decoded) = SignedShortCodeAnnouncement::verify(&bytes, "ABC2345").unwrap();
        assert_eq!(from, sk.public());
        assert_eq!(decoded, ann);
    }

    #[test]
    fn announcement_with_wrong_code_is_rejected() {
        let sk = iroh::SecretKey::generate();
        let ann = ShortCodeAnnouncement {
            code: "ABC2345".to_string(),
            name: "photo.jpg".to_string(),
            ticket: sample_ticket(),
            size: 8192,
            created_at_ms: now_unix_ms(),
        };
        let bytes = SignedShortCodeAnnouncement::sign(&sk, &ann).unwrap();
        assert_eq!(
            SignedShortCodeAnnouncement::verify(&bytes, "DIFFERENT").unwrap_err(),
            ShortCodeError::UnknownCode
        );
    }

    #[test]
    fn tampered_announcement_is_rejected() {
        let sk = iroh::SecretKey::generate();
        let ann = ShortCodeAnnouncement {
            code: "ABC2345".to_string(),
            name: "photo.jpg".to_string(),
            ticket: sample_ticket(),
            size: 8192,
            created_at_ms: now_unix_ms(),
        };
        let mut bytes = SignedShortCodeAnnouncement::sign(&sk, &ann).unwrap();
        // Flip a byte in the payload region.
        let mid = bytes.len() / 2;
        bytes[mid] ^= 0x01;
        assert_eq!(
            SignedShortCodeAnnouncement::verify(&bytes, "ABC2345").unwrap_err(),
            ShortCodeError::UnknownCode
        );
    }

    #[test]
    fn corrupt_store_file_is_an_error() {
        let dir = TempDir::new().unwrap();
        fs::write(dir.path().join(SHORT_CODES_FILE), b"{not json").unwrap();
        assert!(ShortCodeStore::load_or_default(dir.path()).is_err());
    }
}
