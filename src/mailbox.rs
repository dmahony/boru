//! Encrypted recipient-hosted store-and-forward delivery for direct messages.
//!
//! **DEPRECATED** — mailbox state is stored in the SQLite database via the
//! unified storage layer (Phase 13).  This JSON file is retained only for
//! backward-compatible reads during a transition period.
//!
//! A mailbox stores opaque, authenticated ciphertext.  It never decrypts a
//! message and only accepts envelopes signed by an explicitly authorized
//! sender.  Entries remain until the recipient signs an acknowledgement, or
//! until the configured retention period expires.
//!
//! # Migration
//!
//! New writes go to the SQLite mailbox tables.  The JSON file is no longer
//! written to disk.  Reads from this module are supported for a limited
//! compatibility period.

use std::{
    collections::HashMap,
    fs,
    path::{Path, PathBuf},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use aes_gcm::{
    aead::{Aead, KeyInit, Payload},
    Aes256Gcm, Nonce,
};
use iroh::{PublicKey, SecretKey, Signature};
use n0_error::{Result, StdResultExt};
use serde::{Deserialize, Serialize};
use serde_byte_array::ByteArray;
use x25519_dalek::{PublicKey as EncryptionPublicKey, StaticSecret};

const SCHEMA_VERSION: u32 = 1;
const NONCE_LEN: usize = 12;
const SIGNATURE_LEN: usize = Signature::LENGTH;
/// Canonical protocol/domain tag bound into the V2 signature and the V2
/// AEAD associated data. Domain separation prevents cross-protocol and
/// cross-version signature confusion.
const MAILBOX_DOMAIN: &str = "boru/mailbox";
/// Envelope format version 1 (legacy). Decode-only during the migration
/// window; never emitted by [`seal`] / [`seal_for`].
pub const ENVELOPE_VERSION_V1: u32 = 1;
/// Envelope format version 2 (current). The only format [`seal`] emits.
/// The V2 signature authenticates `created_at` (BORU-AUDIT-02).
pub const ENVELOPE_VERSION_V2: u32 = 2;
/// Maximum allowed forward clock skew for a freshly validated envelope.
const MAX_FUTURE_SKEW_MS: u64 = 60_000;
/// Default retention period for unacknowledged envelopes.
pub const DEFAULT_MAILBOX_TTL: Duration = Duration::from_secs(7 * 24 * 60 * 60);
/// Maximum number of envelopes returned by one reconnect sync response.
pub const MAX_SYNC_ENVELOPES: usize = 64;
/// Maximum postcard-encoded envelope bytes returned by one sync response.
pub const MAX_SYNC_RESPONSE_BYTES: usize = 512 * 1024;
/// A requester cannot force an unbounded historical scan.  The server only
/// serves the mailbox retention window, regardless of the requested cursor.
pub const MAX_SYNC_LOOKBACK: Duration = DEFAULT_MAILBOX_TTL;
/// On-disk mailbox filename.
pub const MAILBOX_FILE_NAME: &str = "mailbox.json";

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

/// Public encryption identity advertised by a recipient.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct MailboxPublicKey {
    /// Identity key used to authenticate envelopes and acknowledgements.
    pub identity: PublicKey,
    /// X25519 public key used for envelope encryption.
    pub encryption: [u8; 32],
}

/// Recipient-side mailbox identity. Keep the secret private and persist it with
/// the same protections as the node's identity key.
#[derive(Clone)]
pub struct MailboxIdentity {
    identity: PublicKey,
    secret: StaticSecret,
}

impl std::fmt::Debug for MailboxIdentity {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MailboxIdentity")
            .field("identity", &self.identity)
            .finish_non_exhaustive()
    }
}

impl MailboxIdentity {
    /// Derive a stable encryption identity from the node identity key.
    pub fn from_secret(secret: &SecretKey) -> Self {
        Self {
            identity: secret.public(),
            secret: StaticSecret::from(secret.to_bytes()),
        }
    }

    /// Return the public key that senders need in order to seal envelopes.
    pub fn public_key(&self) -> MailboxPublicKey {
        MailboxPublicKey {
            identity: self.identity,
            encryption: EncryptionPublicKey::from(&self.secret).to_bytes(),
        }
    }

    /// Encrypt and sign a payload for this recipient.
    pub fn seal(&self, sender: &SecretKey, payload: &[u8]) -> Result<MailboxEnvelope> {
        let recipient = self.public_key();
        seal(sender, recipient, payload)
    }

    /// Encrypt and sign a payload at an explicit creation timestamp.
    ///
    /// Exposed so protocol tests and deterministic integrations can produce
    /// a *validly signed* envelope with a chosen `created_at` (e.g. one that
    /// is genuinely expired or future-dated) without depending on the system
    /// clock. Production code should use [`MailboxIdentity::seal`].
    pub fn seal_at(
        &self,
        sender: &SecretKey,
        payload: &[u8],
        created_at: u64,
    ) -> Result<MailboxEnvelope> {
        let recipient = self.public_key();
        seal_at(sender, recipient, payload, created_at)
    }

    /// Decrypt an envelope addressed to this identity after checking its signature.
    pub fn open(&self, envelope: &MailboxEnvelope) -> Result<Vec<u8>> {
        envelope.open_with(self)
    }
}

/// Legacy mailbox envelope (format V1).
///
/// The V1 signature covers only `(from, recipient, ephemeral, nonce,
/// ciphertext)`; `created_at` was NOT authenticated, which let TTL and
/// replay windows be altered independently of the sender's signature
/// (BORU-AUDIT-02). V1 remains decodable only for the migration window:
/// envelopes persisted before the V2 upgrade are still served with their
/// original semantics until they expire under the mailbox TTL. This layout
/// is never emitted by [`seal`] / [`seal_for`].
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct MailboxEnvelopeV1 {
    /// Authenticated sender identity.
    pub from: PublicKey,
    /// Recipient identity and encryption key.
    pub recipient: MailboxPublicKey,
    /// Ephemeral X25519 public key for this envelope.
    pub ephemeral: [u8; 32],
    /// AES-GCM nonce.
    pub nonce: [u8; NONCE_LEN],
    /// Ciphertext including the AEAD tag.
    pub ciphertext: Vec<u8>,
    /// Creation time in Unix epoch milliseconds. Legacy limitation: NOT
    /// covered by the V1 signature.
    pub created_at: u64,
    /// Sender signature over all preceding fields.
    pub signature: ByteArray<SIGNATURE_LEN>,
}

impl MailboxEnvelopeV1 {
    /// The exact bytes the V1 signature covers.
    fn signing_bytes(&self) -> Vec<u8> {
        postcard::to_stdvec(&(
            self.from,
            self.recipient,
            self.ephemeral,
            self.nonce,
            &self.ciphertext,
        ))
        .expect("postcard encoding cannot fail")
    }
}

/// Current mailbox envelope (format V2).
///
/// Every field used for identity, routing, freshness or interpretation is
/// authenticated. The signature covers
/// `(domain, version, from, recipient, ephemeral, nonce, created_at,
/// ciphertext)` via [`MailboxEnvelopeV2::signing_bytes`], and the same
/// context (minus the ciphertext, which does not exist yet at encryption
/// time) is bound into the AEAD associated data. Mutating `created_at` or
/// any other field therefore invalidates both the signature and the AEAD
/// tag.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct MailboxEnvelopeV2 {
    /// Protocol version — always [`ENVELOPE_VERSION_V2`]. Part of the signed
    /// payload.
    pub version: u32,
    /// Authenticated sender identity.
    pub from: PublicKey,
    /// Recipient identity and encryption key.
    pub recipient: MailboxPublicKey,
    /// Ephemeral X25519 public key for this envelope.
    pub ephemeral: [u8; 32],
    /// AES-GCM nonce.
    pub nonce: [u8; NONCE_LEN],
    /// Ciphertext including the AEAD tag.
    pub ciphertext: Vec<u8>,
    /// Creation time in Unix epoch milliseconds — authenticated by the
    /// signature and the AEAD tag.
    pub created_at: u64,
    /// Sender signature over all preceding fields.
    pub signature: ByteArray<SIGNATURE_LEN>,
}

impl MailboxEnvelopeV2 {
    /// Canonical context bytes (without ciphertext) bound into the AEAD
    /// associated data. Used by BOTH encryption and decryption so the two
    /// can never drift.
    fn context_bytes(&self) -> Vec<u8> {
        postcard::to_stdvec(&(
            MAILBOX_DOMAIN,
            self.version,
            self.from,
            self.recipient,
            self.ephemeral,
            self.nonce,
            self.created_at,
        ))
        .expect("postcard encoding cannot fail")
    }

    /// Canonical bytes that the V2 signature covers. A single serialization
    /// function is used by BOTH [`seal_at`] and [`verify_signature`], so
    /// signing and verification can never drift.
    fn signing_bytes(&self) -> Vec<u8> {
        postcard::to_stdvec(&(
            MAILBOX_DOMAIN,
            self.version,
            self.from,
            self.recipient,
            self.ephemeral,
            self.nonce,
            self.created_at,
            &self.ciphertext,
        ))
        .expect("postcard encoding cannot fail")
    }
}

/// Versioned, encrypted, signed mailbox entry.
///
/// Serialization is externally tagged by serde (postcard writes a variant
/// index), so the version is explicit in every persisted and wire encoding.
/// [`MailboxEnvelope::decode`] additionally accepts the untagged legacy V1
/// layout during the migration window. Every verification path checks the
/// signature first, so any decoding ambiguity still fails closed.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum MailboxEnvelope {
    /// Legacy format. Decode-only during the migration window; never
    /// emitted by [`seal`] / [`seal_for`].
    V1(MailboxEnvelopeV1),
    /// Current format — the only format `seal`/`seal_for` produces.
    V2(MailboxEnvelopeV2),
}

impl MailboxEnvelope {
    /// Envelope format version.
    pub fn version(&self) -> u32 {
        match self {
            MailboxEnvelope::V1(_) => ENVELOPE_VERSION_V1,
            MailboxEnvelope::V2(e) => e.version,
        }
    }

    /// Authenticated sender identity.
    pub fn from(&self) -> PublicKey {
        match self {
            MailboxEnvelope::V1(e) => e.from,
            MailboxEnvelope::V2(e) => e.from,
        }
    }

    /// Recipient identity and encryption key.
    pub fn recipient(&self) -> MailboxPublicKey {
        match self {
            MailboxEnvelope::V1(e) => e.recipient,
            MailboxEnvelope::V2(e) => e.recipient,
        }
    }

    /// Ephemeral X25519 public key for this envelope.
    pub fn ephemeral(&self) -> [u8; 32] {
        match self {
            MailboxEnvelope::V1(e) => e.ephemeral,
            MailboxEnvelope::V2(e) => e.ephemeral,
        }
    }

    /// AES-GCM nonce.
    pub fn nonce(&self) -> [u8; NONCE_LEN] {
        match self {
            MailboxEnvelope::V1(e) => e.nonce,
            MailboxEnvelope::V2(e) => e.nonce,
        }
    }

    /// Ciphertext including the AEAD tag.
    pub fn ciphertext(&self) -> &[u8] {
        match self {
            MailboxEnvelope::V1(e) => &e.ciphertext,
            MailboxEnvelope::V2(e) => &e.ciphertext,
        }
    }

    /// Mutable ciphertext access (used by tamper tests to demonstrate that
    /// ciphertext corruption fails authentication).
    pub fn ciphertext_mut(&mut self) -> Option<&mut Vec<u8>> {
        match self {
            MailboxEnvelope::V1(e) => Some(&mut e.ciphertext),
            MailboxEnvelope::V2(e) => Some(&mut e.ciphertext),
        }
    }

    /// Creation time in Unix epoch milliseconds.
    pub fn created_at(&self) -> u64 {
        match self {
            MailboxEnvelope::V1(e) => e.created_at,
            MailboxEnvelope::V2(e) => e.created_at,
        }
    }

    /// Sender signature.
    pub fn signature(&self) -> &ByteArray<SIGNATURE_LEN> {
        match self {
            MailboxEnvelope::V1(e) => &e.signature,
            MailboxEnvelope::V2(e) => &e.signature,
        }
    }

    /// Stable content identifier used for deduplication and acknowledgements.
    ///
    /// INVARIANT: `message_id()` is the blake3 hash of exactly the bytes the
    /// sender's signature covers. Any mutation that breaks the signature —
    /// including `created_at` for V2 — also changes the id, so an id-stable
    /// envelope has an authentic signature and context. Ack matching relies
    /// on this: a tampered envelope can neither validate nor acknowledge.
    pub fn message_id(&self) -> String {
        match self {
            MailboxEnvelope::V1(e) => blake3::hash(&e.signing_bytes()).to_hex().to_string(),
            MailboxEnvelope::V2(e) => blake3::hash(&e.signing_bytes()).to_hex().to_string(),
        }
    }

    /// Decrypt after checking the sender signature and recipient identity.
    pub fn open(&self, recipient: &SecretKey) -> Result<Vec<u8>> {
        MailboxIdentity::from_secret(recipient).open(self)
    }

    fn open_with(&self, identity: &MailboxIdentity) -> Result<Vec<u8>> {
        verify_signature(self)?;
        let expected = identity.public_key();
        if self.recipient() != expected {
            return Err(n0_error::anyerr!(
                "mailbox envelope is addressed to another recipient"
            ));
        }
        self.decrypt(identity)
    }

    /// Decrypt with the shared-secret derivation and AEAD handling for the
    /// envelope's version. V2 binds the canonical context as associated data.
    fn decrypt(&self, identity: &MailboxIdentity) -> Result<Vec<u8>> {
        match self {
            MailboxEnvelope::V1(e) => {
                let shared = identity
                    .secret
                    .diffie_hellman(&EncryptionPublicKey::from(e.ephemeral));
                let key = derive_key_v1(shared.as_bytes());
                Aes256Gcm::new_from_slice(&key)
                    .expect("32-byte key")
                    .decrypt(Nonce::from_slice(&e.nonce), e.ciphertext.as_ref())
                    .map_err(|_| n0_error::anyerr!("mailbox ciphertext authentication failed"))
            }
            MailboxEnvelope::V2(e) => {
                let shared = identity
                    .secret
                    .diffie_hellman(&EncryptionPublicKey::from(e.ephemeral));
                let key = derive_key_v2(shared.as_bytes());
                let aad = e.context_bytes();
                Aes256Gcm::new_from_slice(&key)
                    .expect("32-byte key")
                    .decrypt(
                        Nonce::from_slice(&e.nonce),
                        Payload {
                            msg: e.ciphertext.as_ref(),
                            aad: &aad,
                        },
                    )
                    .map_err(|_| n0_error::anyerr!("mailbox ciphertext authentication failed"))
            }
        }
    }

    /// Validate authorization, recipient identity, signature, and retention
    /// before handing an incoming replay to the normal message pipeline.
    ///
    /// ORDER (fail closed): the signature is verified FIRST, so for V2 the
    /// clock-skew and TTL rules below run against an attacker-immutable
    /// `created_at`. Only then are sender authorization, recipient identity
    /// and decryption checked. For a legacy V1 envelope the same order
    /// applies, but `created_at` is not covered by the V1 signature — the
    /// acknowledged migration-window exception (bounded by the mailbox TTL).
    pub fn validate_for(
        &self,
        identity: &MailboxIdentity,
        allowed_senders: &[PublicKey],
        ttl: Duration,
    ) -> Result<Vec<u8>> {
        verify_signature(self)?;
        if !allowed_senders.contains(&self.from()) {
            return Err(n0_error::anyerr!("mailbox sender is not authorized"));
        }
        if self.recipient() != identity.public_key() {
            return Err(n0_error::anyerr!(
                "mailbox envelope is addressed to another recipient"
            ));
        }
        let now = now_ms();
        if self.created_at() > now.saturating_add(MAX_FUTURE_SKEW_MS)
            || now.saturating_sub(self.created_at()) > ttl.as_millis() as u64
        {
            return Err(n0_error::anyerr!(
                "mailbox envelope is expired or from the future"
            ));
        }
        self.decrypt(identity)
    }

    /// Decode a persisted or wire envelope blob.
    ///
    /// Accepts the current version-tagged encoding and, for the migration
    /// window, the untagged legacy V1 layout. V1 is never re-emitted by this
    /// crate; a decoded V1 envelope retains its original semantics until it
    /// expires under the mailbox TTL.
    ///
    /// The tagged decode is accepted only when the signature verifies. This
    /// guards against a legacy blob whose leading bytes coincidentally parse
    /// as a tagged enum: such a mis-decode carries a shifted signature and is
    /// rejected, falling through to the legacy layout instead. Every consumer
    /// additionally verifies before acting, so decoding remains fail-closed.
    pub fn decode(bytes: &[u8]) -> Result<MailboxEnvelope> {
        if let Ok(envelope) = postcard::from_bytes::<MailboxEnvelope>(bytes) {
            if verify_signature(&envelope).is_ok() {
                return Ok(envelope);
            }
        }
        let legacy: MailboxEnvelopeV1 =
            postcard::from_bytes(bytes).with_std_context(|_| "decode legacy V1 mailbox envelope")?;
        Ok(MailboxEnvelope::V1(legacy))
    }
}

/// Encrypt and sign a payload for a recipient using the current time.
fn seal(
    sender: &SecretKey,
    recipient: MailboxPublicKey,
    payload: &[u8],
) -> Result<MailboxEnvelope> {
    seal_at(sender, recipient, payload, now_ms())
}

/// Encrypt and sign a payload at an explicit creation timestamp.
///
/// The V2 construction: derive the AEAD key from an ephemeral X25519
/// handshake, encrypt with the canonical context bound as associated data,
/// then sign the canonical payload (which includes `created_at`) so the
/// timestamp is cryptographically immutable.
fn seal_at(
    sender: &SecretKey,
    recipient: MailboxPublicKey,
    payload: &[u8],
    created_at: u64,
) -> Result<MailboxEnvelope> {
    let ephemeral_secret = StaticSecret::random();
    let ephemeral = EncryptionPublicKey::from(&ephemeral_secret);
    let shared = ephemeral_secret.diffie_hellman(&EncryptionPublicKey::from(recipient.encryption));
    let mut nonce = [0u8; NONCE_LEN];
    getrandom::fill(&mut nonce).map_err(|e| n0_error::anyerr!("generate mailbox nonce: {e}"))?;
    let mut envelope = MailboxEnvelopeV2 {
        version: ENVELOPE_VERSION_V2,
        from: sender.public(),
        recipient,
        ephemeral: ephemeral.to_bytes(),
        nonce,
        ciphertext: Vec::new(),
        created_at,
        signature: ByteArray::new([0u8; SIGNATURE_LEN]),
    };
    let aad = envelope.context_bytes();
    let ciphertext = Aes256Gcm::new_from_slice(&derive_key_v2(shared.as_bytes()))
        .expect("32-byte key")
        .encrypt(
            Nonce::from_slice(&nonce),
            Payload {
                msg: payload,
                aad: &aad,
            },
        )
        .map_err(|_| n0_error::anyerr!("encrypt mailbox payload"))?;
    envelope.ciphertext = ciphertext;
    envelope.signature = ByteArray::new(sender.sign(&envelope.signing_bytes()).to_bytes());
    Ok(MailboxEnvelope::V2(envelope))
}

/// V1 key derivation — preserved unchanged so legacy envelopes decrypt.
fn derive_key_v1(shared: &[u8; 32]) -> [u8; 32] {
    *blake3::hash(
        [b"iroh-gossip-chat/mailbox/v1".as_slice(), shared]
            .concat()
            .as_slice(),
    )
    .as_bytes()
}

/// V2 key derivation — domain-separated from V1.
fn derive_key_v2(shared: &[u8; 32]) -> [u8; 32] {
    *blake3::hash(
        [b"boru/mailbox/v2".as_slice(), shared]
            .concat()
            .as_slice(),
    )
    .as_bytes()
}

fn verify_signature(envelope: &MailboxEnvelope) -> Result<()> {
    match envelope {
        MailboxEnvelope::V1(e) => e
            .from
            .verify(
                &e.signing_bytes(),
                &Signature::from_bytes(&e.signature),
            )
            .map_err(|err| n0_error::anyerr!("verify mailbox envelope signature: {err}")),
        MailboxEnvelope::V2(e) => {
            if e.version != ENVELOPE_VERSION_V2 {
                return Err(n0_error::anyerr!(
                    "unsupported mailbox envelope version {}",
                    e.version
                ));
            }
            e.from
                .verify(
                    &e.signing_bytes(),
                    &Signature::from_bytes(&e.signature),
                )
                .map_err(|err| n0_error::anyerr!("verify mailbox envelope signature: {err}"))
        }
    }
}

/// Create an encrypted envelope using a recipient's advertised public key.
///
/// Always emits the current V2 format; V1 is never produced.
pub fn seal_for(
    sender: &SecretKey,
    recipient: MailboxPublicKey,
    payload: &[u8],
) -> Result<MailboxEnvelope> {
    seal(sender, recipient, payload)
}

/// Create an envelope at an explicit creation timestamp.
///
/// Exposed for deterministic protocol tests; production callers use
/// [`seal_for`].
pub fn seal_for_at(
    sender: &SecretKey,
    recipient: MailboxPublicKey,
    payload: &[u8],
    created_at: u64,
) -> Result<MailboxEnvelope> {
    seal_at(sender, recipient, payload, created_at)
}

/// Version of the signed acknowledgement wire contract.
pub const ACKNOWLEDGEMENT_VERSION: u32 = 1;

/// A recipient-signed acknowledgement for one envelope.
///
/// The signature covers every field except `signature`, in this exact order:
/// `(version, message_id, original_sender, recipient, acknowledged_at_ms,
/// status)`, encoded with postcard.  Keeping the field order and encoding
/// explicit makes verification deterministic across implementations.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct MessageAcknowledgement {
    /// Protocol version of the acknowledgement contract.
    pub version: u32,
    /// Envelope identifier being acknowledged.
    pub message_id: String,
    /// Identity that originally authored/sent the envelope.
    pub original_sender: PublicKey,
    /// Recipient identity that signed the acknowledgement.
    pub recipient: PublicKey,
    /// Unix epoch milliseconds when processing completed.
    pub acknowledged_at_ms: u64,
    /// Optional application-level processing result.
    pub status: Option<String>,
    /// Recipient signature over all preceding semantic fields.
    pub signature: ByteArray<SIGNATURE_LEN>,
}

/// Backwards-compatible protocol name used by the inbox and mailbox APIs.
pub type MailboxAck = MessageAcknowledgement;

impl MessageAcknowledgement {
    fn signing_bytes(&self) -> Vec<u8> {
        postcard::to_stdvec(&(
            self.version,
            &self.message_id,
            self.original_sender,
            self.recipient,
            self.acknowledged_at_ms,
            &self.status,
        ))
        .expect("postcard encoding cannot fail")
    }

    /// Sign an accepted acknowledgement after successfully processing a message.
    pub fn sign(
        recipient: &SecretKey,
        message_id: impl Into<String>,
        original_sender: PublicKey,
    ) -> Self {
        Self::sign_at(
            recipient,
            message_id,
            original_sender,
            now_ms(),
            Some("accepted".to_string()),
        )
    }

    /// Construct a signed acknowledgement at a supplied timestamp.
    ///
    /// This is public so protocol tests and deterministic integrations can use
    /// a fixed timestamp without depending on the system clock.
    pub fn sign_at(
        recipient: &SecretKey,
        message_id: impl Into<String>,
        original_sender: PublicKey,
        acknowledged_at_ms: u64,
        status: Option<String>,
    ) -> Self {
        let mut ack = Self {
            version: ACKNOWLEDGEMENT_VERSION,
            message_id: message_id.into(),
            original_sender,
            recipient: recipient.public(),
            acknowledged_at_ms,
            status,
            signature: ByteArray::new([0u8; SIGNATURE_LEN]),
        };
        ack.signature = ByteArray::new(recipient.sign(&ack.signing_bytes()).to_bytes());
        ack
    }

    /// Verify the acknowledgement signature against the expected recipient key.
    pub fn verify(&self, expected: PublicKey) -> Result<()> {
        if self.version != ACKNOWLEDGEMENT_VERSION {
            return Err(n0_error::anyerr!(
                "unsupported mailbox acknowledgement version {}",
                self.version
            ));
        }
        if self.recipient != expected {
            return Err(n0_error::anyerr!("mailbox acknowledgement signer mismatch"));
        }
        self.recipient
            .verify(
                &self.signing_bytes(),
                &Signature::from_bytes(&self.signature),
            )
            .map_err(|e| n0_error::anyerr!("verify mailbox acknowledgement: {e}"))
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
/// Durable encrypted mailbox state.
pub struct MailboxStore {
    #[serde(default = "default_schema")]
    schema_version: u32,
    #[serde(default)]
    recipient: Option<PublicKey>,
    #[serde(default)]
    entries: HashMap<String, MailboxEnvelope>,
    #[serde(skip)]
    data_dir: PathBuf,
    #[serde(skip)]
    ttl: Duration,
}
fn default_schema() -> u32 {
    SCHEMA_VERSION
}

/// Result of accepting an authenticated incoming envelope.
///
/// A duplicate has already been durably retained. Callers must not insert it
/// into user-visible history again, but should still acknowledge it so a lost
/// acknowledgement can be recovered.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IncomingAcceptance {
    /// The envelope was newly retained.
    Inserted,
    /// The envelope was already retained and was not inserted again.
    Duplicate,
}

impl MailboxStore {
    /// Create a mailbox without a preconfigured recipient (useful for first-start).
    pub fn empty_at(data_dir: impl Into<PathBuf>) -> Self {
        Self {
            schema_version: SCHEMA_VERSION,
            recipient: None,
            entries: HashMap::new(),
            data_dir: data_dir.into(),
            ttl: DEFAULT_MAILBOX_TTL,
        }
    }
    /// Create a mailbox bound to one recipient identity; this is the secure production constructor.
    pub fn for_recipient(data_dir: impl Into<PathBuf>, recipient: PublicKey) -> Self {
        let mut s = Self::empty_at(data_dir);
        s.recipient = Some(recipient);
        s
    }
    /// Create a mailbox with a custom retention period.
    pub fn with_ttl(data_dir: impl Into<PathBuf>, ttl: Duration) -> Self {
        let mut s = Self::empty_at(data_dir);
        s.ttl = ttl;
        s
    }
    /// Load a mailbox, returning None when it has not been created yet.
    pub fn load(data_dir: impl AsRef<Path>) -> Result<Option<Self>> {
        let path = data_dir.as_ref().join(MAILBOX_FILE_NAME);
        if !path.exists() {
            return Ok(None);
        }
        let mut store: Self = serde_json::from_str(
            &fs::read_to_string(&path)
                .with_std_context(|_| format!("read mailbox {}", path.display()))?,
        )
        .with_std_context(|_| format!("parse mailbox {}", path.display()))?;
        if store.schema_version != SCHEMA_VERSION {
            return Err(n0_error::anyerr!(
                "unsupported mailbox schema version {}",
                store.schema_version
            ));
        }
        store.data_dir = data_dir.as_ref().to_path_buf();
        store.ttl = DEFAULT_MAILBOX_TTL;
        Ok(Some(store))
    }
    /// Persist atomically and remove expired entries.
    ///
    /// **DEPRECATED**: mailbox state is now in SQLite.  This method logs
    /// a warning and returns the legacy file path without writing to disk.
    #[deprecated(
        since = "0.21.0",
        note = "SQLite mailbox tables replace mailbox.json writes"
    )]
    pub fn save(&self) -> Result<PathBuf> {
        let path = self.data_dir.join(MAILBOX_FILE_NAME);
        tracing::warn!(
            path = %path.display(),
            "save() called on deprecated JSON mailbox store — no data written; \
             use SQLite mailbox tables instead"
        );
        Ok(path)
    }
    fn expire(&mut self) {
        let cutoff = now_ms().saturating_sub(self.ttl.as_millis() as u64);
        self.entries.retain(|_, e| e.created_at() > cutoff);
    }
    /// Enqueue only a valid, authenticated envelope from an allowed sender.
    pub fn enqueue(
        &mut self,
        envelope: MailboxEnvelope,
        allowed_senders: &[PublicKey],
    ) -> Result<String> {
        verify_signature(&envelope)?;
        if !allowed_senders.contains(&envelope.from()) {
            return Err(n0_error::anyerr!("mailbox sender is not authorized"));
        }
        if let Some(recipient) = self.recipient {
            if envelope.recipient().identity != recipient {
                return Err(n0_error::anyerr!("mailbox recipient mismatch"));
            }
        } else {
            self.recipient = Some(envelope.recipient().identity);
        }
        let id = envelope.message_id();
        if self.entries.contains_key(&id) {
            return Err(n0_error::anyerr!("duplicate mailbox message"));
        }
        self.entries.insert(id.clone(), envelope);
        Ok(id)
    }
    /// Store an outgoing envelope without recipient or authorization checks.
    ///
    /// Unlike [`enqueue`], this accepts envelopes addressed to *other* peers
    /// (the sender's own outgoing messages).  Signature verification is
    /// skipped because the envelope was just created locally.  Duplicate
    /// message ids are still rejected.
    pub fn enqueue_outgoing(&mut self, envelope: MailboxEnvelope) -> Result<String> {
        let id = envelope.message_id();
        if self.entries.contains_key(&id) {
            return Err(n0_error::anyerr!("duplicate mailbox message"));
        }
        self.entries.insert(id.clone(), envelope);
        Ok(id)
    }
    /// Return pending opaque envelopes in replay order.
    pub fn pending(&mut self) -> Result<Vec<MailboxEnvelope>> {
        self.expire();
        let mut entries: Vec<_> = self.entries.values().cloned().collect();
        // HashMap iteration order is unstable; deterministic replay order keeps
        // reconnect behavior consistent across restarts.
        entries.sort_by_key(|entry| (entry.created_at(), entry.message_id()));
        Ok(entries)
    }
    /// Remove an entry only after a valid acknowledgement signed by the recipient.
    pub fn acknowledge(&mut self, ack: &MailboxAck) -> Result<bool> {
        let recipient = self
            .recipient
            .ok_or_else(|| n0_error::anyerr!("mailbox recipient is not configured"))?;
        ack.verify(recipient)?;
        Ok(self.entries.remove(&ack.message_id).is_some())
    }

    /// Remove an outgoing envelope after verifying the acknowledgement against
    /// the recipient encoded in that envelope.
    ///
    /// Outgoing stores are not bound to the local identity: their `recipient`
    /// field is either unset or describes an incoming mailbox.  The signer of
    /// an outgoing acknowledgement is the remote envelope recipient, so using
    /// [`acknowledge`] here would verify against the wrong identity.
    pub fn acknowledge_outgoing(&mut self, ack: &MailboxAck) -> Result<bool> {
        let Some(envelope) = self.entries.get(&ack.message_id) else {
            return Ok(false);
        };
        ack.verify(envelope.recipient().identity)?;
        Ok(self.entries.remove(&ack.message_id).is_some())
    }

    /// Authenticate and decrypt an incoming envelope before durably accepting
    /// its opaque ciphertext. The returned plaintext can then be handed to the
    /// normal signed-message pipeline by the application.
    pub fn accept_incoming(
        &mut self,
        identity: &MailboxIdentity,
        envelope: MailboxEnvelope,
        allowed_senders: &[PublicKey],
    ) -> Result<(String, Vec<u8>)> {
        let (id, payload, _) =
            self.accept_incoming_with_status(identity, envelope, allowed_senders)?;
        Ok((id, payload))
    }

    /// Accept an incoming envelope and report whether it was newly retained.
    ///
    /// Validation and decryption happen for every delivery, including
    /// duplicates. If the message id is already present, all immutable
    /// envelope fields are compared before returning `Duplicate`; a mismatch
    /// is rejected rather than allowing an id collision to alter stored state.
    #[allow(deprecated)]
    pub fn accept_incoming_with_status(
        &mut self,
        identity: &MailboxIdentity,
        envelope: MailboxEnvelope,
        allowed_senders: &[PublicKey],
    ) -> Result<(String, Vec<u8>, IncomingAcceptance)> {
        let payload = envelope.validate_for(identity, allowed_senders, self.ttl)?;
        let id = envelope.message_id();
        // Reconnects and restarts may replay an envelope. Idempotent
        // acceptance avoids injecting it twice while still allowing an ack.
        if let Some(existing) = self.entries.get(&id) {
            if existing.from() != envelope.from()
                || existing.recipient() != envelope.recipient()
                || existing.ephemeral() != envelope.ephemeral()
                || existing.nonce() != envelope.nonce()
                || existing.ciphertext() != envelope.ciphertext()
                || existing.created_at() != envelope.created_at()
                || existing.signature() != envelope.signature()
            {
                return Err(n0_error::anyerr!(
                    "conflicting mailbox envelope for message id {id}"
                ));
            }
            return Ok((id, payload, IncomingAcceptance::Duplicate));
        }
        self.enqueue(envelope, allowed_senders)?;
        Ok((id, payload, IncomingAcceptance::Inserted))
    }

    /// Remove an acknowledged outgoing envelope (in-memory only — SQLite is authoritative).
    #[allow(deprecated)]
    pub fn acknowledge_and_save(&mut self, ack: &MailboxAck) -> Result<bool> {
        self.acknowledge(ack)
    }

    /// Remove an acknowledged outgoing envelope (in-memory only — SQLite is authoritative).
    #[allow(deprecated)]
    pub fn acknowledge_outgoing_and_save(&mut self, ack: &MailboxAck) -> Result<bool> {
        self.acknowledge_outgoing(ack)
    }
    /// Return pending envelopes whose recipient identity matches `who`.
    ///
    /// Used by the inbox SyncResponse handler to serve envelopes that
    /// were encrypted for a specific peer and have not yet been
    /// acknowledged.
    pub fn pending_for_recipient(&mut self, who: PublicKey) -> Vec<MailboxEnvelope> {
        self.pending_for_recipient_since(who, 0)
    }

    /// Return a bounded, deterministic sync page for `who`.
    ///
    /// `since_ms` is merely a resume hint supplied by the peer; it is clamped
    /// to the local retention window and never causes an unrestricted scan.
    /// The page is ordered by `(created_at, message_id)` and is bounded by both
    /// envelope count and encoded response size.  Callers can resume with the
    /// last returned envelope's creation time (and rely on idempotent message
    /// acceptance for equal-timestamp boundaries).
    pub fn pending_for_recipient_since(
        &mut self,
        who: PublicKey,
        since_ms: u64,
    ) -> Vec<MailboxEnvelope> {
        self.expire();
        let now = now_ms();
        let floor = now.saturating_sub(MAX_SYNC_LOOKBACK.as_millis() as u64);
        let since_ms = since_ms.max(floor);
        let mut entries: Vec<_> = self
            .entries
            .values()
            .filter(|e| e.recipient().identity == who && e.created_at() >= since_ms)
            .cloned()
            .collect();
        entries.sort_by_key(|entry| (entry.created_at(), entry.message_id()));
        let mut page = Vec::with_capacity(entries.len().min(MAX_SYNC_ENVELOPES));
        let mut encoded_bytes = 0usize;
        for entry in entries {
            if page.len() >= MAX_SYNC_ENVELOPES {
                break;
            }
            let size = postcard::to_stdvec(&entry)
                .map(|bytes| bytes.len())
                .unwrap_or(usize::MAX);
            if encoded_bytes.saturating_add(size) > MAX_SYNC_RESPONSE_BYTES {
                break;
            }
            encoded_bytes += size;
            page.push(entry);
        }
        page
    }

    /// Number of retained entries after applying retention.
    pub fn len(&mut self) -> usize {
        self.expire();
        self.entries.len()
    }
    /// Whether the store is empty (after applying retention).
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Faithfully reproduce the legacy V1 envelope construction (old seal):
    /// AEAD with the V1 key derivation and NO associated data, signature
    /// over `(from, recipient, ephemeral, nonce, ciphertext)` only. Used to
    /// prove the migration-window decoder accepts old blobs.
    fn seal_v1_legacy(
        sender: &SecretKey,
        recipient: MailboxPublicKey,
        payload: &[u8],
    ) -> MailboxEnvelopeV1 {
        let ephemeral_secret = StaticSecret::random();
        let ephemeral = EncryptionPublicKey::from(&ephemeral_secret);
        let shared =
            ephemeral_secret.diffie_hellman(&EncryptionPublicKey::from(recipient.encryption));
        let mut nonce = [0u8; NONCE_LEN];
        getrandom::fill(&mut nonce).unwrap();
        let ciphertext = Aes256Gcm::new_from_slice(&derive_key_v1(shared.as_bytes()))
            .expect("32-byte key")
            .encrypt(Nonce::from_slice(&nonce), payload)
            .expect("encrypt");
        let mut env = MailboxEnvelopeV1 {
            from: sender.public(),
            recipient,
            ephemeral: ephemeral.to_bytes(),
            nonce,
            ciphertext,
            created_at: now_ms(),
            signature: ByteArray::new([0u8; SIGNATURE_LEN]),
        };
        env.signature = ByteArray::new(sender.sign(&env.signing_bytes()).to_bytes());
        env
    }

    fn v2_inner(env: &MailboxEnvelope) -> &MailboxEnvelopeV2 {
        match env {
            MailboxEnvelope::V2(e) => e,
            _ => panic!("expected V2 envelope"),
        }
    }

    #[test]
    fn envelope_is_not_plaintext_and_round_trips() {
        let recipient = SecretKey::generate();
        let sender = SecretKey::generate();
        let id = MailboxIdentity::from_secret(&recipient);
        let env = id.seal(&sender, b"private").unwrap();
        assert!(!env.ciphertext().windows(7).any(|w| w == b"private"));
        assert_eq!(env.open(&recipient).unwrap(), b"private");
    }

    #[test]
    fn seal_emits_v2_and_explicit_version() {
        let recipient = SecretKey::generate();
        let sender = SecretKey::generate();
        let id = MailboxIdentity::from_secret(&recipient);
        let env = id.seal(&sender, b"x").unwrap();
        assert!(matches!(env, MailboxEnvelope::V2(_)));
        assert_eq!(env.version(), ENVELOPE_VERSION_V2);
        assert_eq!(v2_inner(&env).version, ENVELOPE_VERSION_V2);
    }

    #[test]
    fn sync_page_is_bounded_and_recipient_scoped() {
        let dir = tempfile::tempdir().unwrap();
        let recipient = SecretKey::generate();
        let sender = SecretKey::generate();
        let other_recipient = SecretKey::generate();
        let identity = MailboxIdentity::from_secret(&recipient);
        let other_identity = MailboxIdentity::from_secret(&other_recipient);
        let mut store = MailboxStore::for_recipient(dir.path(), recipient.public());

        for i in 0..(MAX_SYNC_ENVELOPES + 8) {
            let env = identity
                .seal_at(&sender, format!("sync-{i}").as_bytes(), now_ms().saturating_sub(i as u64))
                .unwrap();
            store.entries.insert(env.message_id(), env);
        }
        let other = other_identity.seal(&sender, b"not for requester").unwrap();
        store.entries.insert(other.message_id(), other);

        let page = store.pending_for_recipient_since(recipient.public(), 0);
        assert_eq!(page.len(), MAX_SYNC_ENVELOPES);
        assert!(page
            .iter()
            .all(|e| e.recipient().identity == recipient.public()));
        let encoded: usize = page
            .iter()
            .map(|e| postcard::to_stdvec(e).unwrap().len())
            .sum();
        assert!(encoded <= MAX_SYNC_RESPONSE_BYTES);
    }

    #[test]
    fn incoming_acceptance_reports_duplicate_without_reinserting() {
        let dir = tempfile::tempdir().unwrap();
        let recipient = SecretKey::generate();
        let sender = SecretKey::generate();
        let identity = MailboxIdentity::from_secret(&recipient);
        let mut store = MailboxStore::for_recipient(dir.path(), recipient.public());
        let env = identity.seal(&sender, b"signed payload").unwrap();

        let first = store
            .accept_incoming_with_status(&identity, env.clone(), &[sender.public()])
            .unwrap();
        assert_eq!(first.2, IncomingAcceptance::Inserted);
        let second = store
            .accept_incoming_with_status(&identity, env, &[sender.public()])
            .unwrap();
        assert_eq!(second.2, IncomingAcceptance::Duplicate);
        assert_eq!(first.0, second.0);
        assert_eq!(first.1, second.1);
        assert_eq!(store.len(), 1);
    }

    #[test]
    fn incoming_acceptance_legacy_api_remains_idempotent() {
        let dir = tempfile::tempdir().unwrap();
        let recipient = SecretKey::generate();
        let sender = SecretKey::generate();
        let identity = MailboxIdentity::from_secret(&recipient);
        let mut store = MailboxStore::for_recipient(dir.path(), recipient.public());
        let env = identity.seal(&sender, b"signed payload").unwrap();

        let first = store
            .accept_incoming(&identity, env.clone(), &[sender.public()])
            .unwrap();
        let second = store
            .accept_incoming(&identity, env, &[sender.public()])
            .unwrap();
        assert_eq!(first, second);
        assert_eq!(store.len(), 1);
    }

    #[test]
    fn incoming_validation_rejects_unauthorized_sender() {
        let recipient = SecretKey::generate();
        let sender = SecretKey::generate();
        let identity = MailboxIdentity::from_secret(&recipient);
        let env = identity.seal(&sender, b"private").unwrap();
        let result = env.validate_for(&identity, &[], DEFAULT_MAILBOX_TTL);
        assert!(result.is_err());
    }

    #[test]
    fn outgoing_ack_uses_envelope_recipient_when_store_is_unconfigured() {
        let dir = tempfile::tempdir().unwrap();
        let sender = SecretKey::generate();
        let recipient = SecretKey::generate();
        let recipient_identity = MailboxIdentity::from_secret(&recipient);
        let envelope = recipient_identity.seal(&sender, b"outgoing").unwrap();
        let message_id = envelope.message_id();
        let mut store = MailboxStore::empty_at(dir.path());
        store.enqueue_outgoing(envelope).unwrap();

        let ack = MailboxAck::sign(&recipient, message_id, sender.public());
        assert!(store.acknowledge_outgoing(&ack).unwrap());
        assert!(store.is_empty());
    }

    #[test]
    fn acknowledgement_signature_covers_every_semantic_field() {
        let signer = SecretKey::generate();
        let original_sender = SecretKey::generate().public();
        let mut ack = MailboxAck::sign_at(
            &signer,
            "message-1",
            original_sender,
            1_700_000_000_000,
            Some("accepted".to_string()),
        );
        let valid = ack.clone();
        assert!(valid.verify(signer.public()).is_ok());

        ack.version += 1;
        assert!(ack.verify(signer.public()).is_err());
        ack = valid.clone();
        ack.message_id.push('x');
        assert!(ack.verify(signer.public()).is_err());
        ack = valid.clone();
        ack.original_sender = SecretKey::generate().public();
        assert!(ack.verify(signer.public()).is_err());
        ack = valid.clone();
        ack.recipient = SecretKey::generate().public();
        assert!(ack.verify(signer.public()).is_err());
        ack = valid.clone();
        ack.acknowledged_at_ms += 1;
        assert!(ack.verify(signer.public()).is_err());
        ack = valid.clone();
        ack.status = Some("rejected".to_string());
        assert!(ack.verify(signer.public()).is_err());
    }

    // ── BORU-AUDIT-02 regression tests ─────────────────────────────────────
    //
    // These fail on the pre-fix implementation: `created_at` (and any other
    // metadata) could be altered without invalidating the sender signature,
    // so TTL/replay checks ran against attacker-controlled input.

    /// Alter ONLY created_at (e.g. to extend TTL or reset the replay
    /// window). Signature and AEAD must reject the tampered envelope.
    #[test]
    fn v2_created_at_tamper_breaks_authentication() {
        let recipient = SecretKey::generate();
        let sender = SecretKey::generate();
        let identity = MailboxIdentity::from_secret(&recipient);
        let env = identity.seal(&sender, b"tamper me").unwrap();

        let MailboxEnvelope::V2(mut inner) = env.clone() else {
            panic!("expected V2");
        };
        // Rewind the clock to make the envelope look perpetually fresh —
        // the old code accepted this and could keep a message alive forever.
        inner.created_at = now_ms().saturating_sub(1);
        let tampered = MailboxEnvelope::V2(inner);

        // The signature no longer verifies: created_at is part of the
        // signed payload.
        assert!(verify_signature(&tampered).is_err());
        assert!(tampered.validate_for(&identity, &[sender.public()], DEFAULT_MAILBOX_TTL).is_err());
        assert!(tampered.open(&recipient).is_err());
        // The id changes too — ack matching cannot attach to the original.
        assert_ne!(tampered.message_id(), env.message_id());
    }

    /// Alter only created_at the other way (to bypass expiry): also fails.
    #[test]
    fn v2_created_at_tamper_cannot_bypass_expiry() {
        let recipient = SecretKey::generate();
        let sender = SecretKey::generate();
        let identity = MailboxIdentity::from_secret(&recipient);
        let env = identity.seal(&sender, b"tamper me").unwrap();

        let MailboxEnvelope::V2(mut inner) = env.clone() else {
            panic!("expected V2");
        };
        inner.created_at = 1_000_000; // ancient — would exceed any TTL
        let tampered = MailboxEnvelope::V2(inner);

        assert!(verify_signature(&tampered).is_err());
        let err = tampered
            .validate_for(&identity, &[sender.public()], DEFAULT_MAILBOX_TTL)
            .unwrap_err();
        // Rejected at the signature stage, not merely by the expiry check.
        assert!(!err.to_string().contains("expired"));
    }

    /// Alter sender or recipient metadata -> verification fails.
    #[test]
    fn v2_sender_or_recipient_tamper_breaks_authentication() {
        let recipient = SecretKey::generate();
        let sender = SecretKey::generate();
        let identity = MailboxIdentity::from_secret(&recipient);
        let env = identity.seal(&sender, b"tamper me").unwrap();

        let MailboxEnvelope::V2(mut inner) = env.clone() else {
            panic!("expected V2");
        };
        inner.from = SecretKey::generate().public();
        let forged_sender = MailboxEnvelope::V2(inner);
        assert!(verify_signature(&forged_sender).is_err());
        assert!(forged_sender
            .validate_for(&identity, &[sender.public()], DEFAULT_MAILBOX_TTL)
            .is_err());

        let MailboxEnvelope::V2(mut inner) = env.clone() else {
            panic!("expected V2");
        };
        inner.recipient.identity = SecretKey::generate().public();
        let forged_recipient = MailboxEnvelope::V2(inner);
        assert!(verify_signature(&forged_recipient).is_err());
        assert!(forged_recipient
            .validate_for(&identity, &[sender.public()], DEFAULT_MAILBOX_TTL)
            .is_err());
    }

    /// A genuinely expired envelope, VALIDLY SIGNED at an ancient timestamp,
    /// passes signature verification but is rejected by the expiry check.
    #[test]
    fn v2_genuinely_expired_signature_passes_but_expiry_fails() {
        let recipient = SecretKey::generate();
        let sender = SecretKey::generate();
        let identity = MailboxIdentity::from_secret(&recipient);
        let env = identity
            .seal_at(&sender, b"old", 1_000_000)
            .unwrap();
        // The signature is valid — the timestamp is part of it.
        assert!(verify_signature(&env).is_ok());
        let err = env
            .validate_for(&identity, &[sender.public()], Duration::from_secs(3600))
            .unwrap_err();
        assert!(err.to_string().contains("expired"), "got: {err}");
    }

    /// A validly signed future-dated envelope beyond clock skew is rejected.
    #[test]
    fn v2_future_dated_beyond_skew_rejected() {
        let recipient = SecretKey::generate();
        let sender = SecretKey::generate();
        let identity = MailboxIdentity::from_secret(&recipient);
        let future = now_ms().saturating_add(MAX_FUTURE_SKEW_MS + 60_000);
        let env = identity.seal_at(&sender, b"from future", future).unwrap();
        assert!(verify_signature(&env).is_ok());
        let err = env
            .validate_for(&identity, &[sender.public()], DEFAULT_MAILBOX_TTL)
            .unwrap_err();
        assert!(err.to_string().contains("expired"), "got: {err}");
    }

    /// Round-trip a V2 envelope through the exact persistence encoding used
    /// by the SQLite dm_outbox blobs (postcard encode -> decode on restart)
    /// and confirm id, signature and plaintext all survive.
    #[test]
    fn v2_round_trip_through_persistence_and_restart() {
        let recipient = SecretKey::generate();
        let sender = SecretKey::generate();
        let identity = MailboxIdentity::from_secret(&recipient);
        let env = identity.seal(&sender, b"persist me").unwrap();

        let bytes = postcard::to_stdvec(&env).expect("encode");
        let decoded = MailboxEnvelope::decode(&bytes).expect("decode on restart");
        assert!(matches!(decoded, MailboxEnvelope::V2(_)));
        assert_eq!(decoded.version(), ENVELOPE_VERSION_V2);
        assert_eq!(decoded.message_id(), env.message_id());
        assert_eq!(decoded.created_at(), env.created_at());
        assert!(verify_signature(&decoded).is_ok());
        assert_eq!(decoded.open(&recipient).unwrap(), b"persist me");
        // A replayed envelope still validates and is accepted once.
        let mut store = MailboxStore::for_recipient(tempfile::tempdir().unwrap().path(), recipient.public());
        assert_eq!(
            store
                .accept_incoming_with_status(&identity, decoded.clone(), &[sender.public()])
                .unwrap()
                .2,
            IncomingAcceptance::Inserted
        );
        assert_eq!(
            store
                .accept_incoming_with_status(&identity, decoded, &[sender.public()])
                .unwrap()
                .2,
            IncomingAcceptance::Duplicate
        );
    }

    /// The migration window: an untagged legacy V1 blob decodes through
    /// `MailboxEnvelope::decode` (NOT through plain postcard of the enum),
    /// keeps its old message-id derivation, and still verifies/opens with
    /// the original V1 semantics.
    #[test]
    fn v1_legacy_envelope_decodes_with_old_semantics() {
        let recipient = SecretKey::generate();
        let sender = SecretKey::generate();
        let identity = MailboxIdentity::from_secret(&recipient);
        let legacy = seal_v1_legacy(&sender, identity.public_key(), b"legacy hello");
        let legacy_id = blake3::hash(&legacy.signing_bytes()).to_hex().to_string();

        // The untagged legacy blob must decode through the compatibility
        // helper even if its leading bytes happen to look like a tag; the
        // signature guard inside `decode` routes it to the legacy layout.
        let bytes = postcard::to_stdvec(&legacy).expect("encode legacy");

        let decoded = MailboxEnvelope::decode(&bytes).expect("compat decode");
        assert!(matches!(decoded, MailboxEnvelope::V1(_)));
        assert_eq!(decoded.version(), ENVELOPE_VERSION_V1);
        assert_eq!(decoded.message_id(), legacy_id);
        assert!(verify_signature(&decoded).is_ok());
        assert_eq!(decoded.open(&recipient).unwrap(), b"legacy hello");
        assert_eq!(
            decoded
                .validate_for(&identity, &[sender.public()], DEFAULT_MAILBOX_TTL)
                .unwrap(),
            b"legacy hello"
        );
    }

    /// The versioned (tagged) encoding of a V2 envelope round-trips through
    /// the plain serde path used on the wire (whisper / inbox).
    #[test]
    fn v2_tagged_encoding_round_trips_through_serde() {
        let recipient = SecretKey::generate();
        let sender = SecretKey::generate();
        let identity = MailboxIdentity::from_secret(&recipient);
        let env = identity.seal(&sender, b"wire me").unwrap();
        let bytes = postcard::to_stdvec(&env).expect("encode");
        let decoded: MailboxEnvelope = postcard::from_bytes(&bytes).expect("decode tagged");
        assert!(matches!(decoded, MailboxEnvelope::V2(_)));
        assert_eq!(decoded.message_id(), env.message_id());
        assert_eq!(decoded.open(&recipient).unwrap(), b"wire me");
    }
}
