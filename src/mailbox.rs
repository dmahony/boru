//! Encrypted recipient-hosted store-and-forward delivery for direct messages.
//!
//! A mailbox stores opaque, authenticated ciphertext.  It never decrypts a
//! message and only accepts envelopes signed by an explicitly authorized
//! sender.  Entries remain until the recipient signs an acknowledgement, or
//! until the configured retention period expires.

use std::{
    collections::HashMap,
    fs,
    path::{Path, PathBuf},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use aes_gcm::{
    aead::{Aead, KeyInit},
    Aes256Gcm, Nonce,
};
use iroh::{PublicKey, SecretKey, Signature};
use n0_error::{Result, StdResultExt};
use serde::{Deserialize, Serialize};
use serde_byte_array::ByteArray;
use x25519_dalek::{PublicKey as EncryptionPublicKey, StaticSecret};

use crate::chat_core::atomic_write::atomic_write_json;

const SCHEMA_VERSION: u32 = 1;
const NONCE_LEN: usize = 12;
const SIGNATURE_LEN: usize = Signature::LENGTH;
/// Default retention period for unacknowledged envelopes.
pub const DEFAULT_MAILBOX_TTL: Duration = Duration::from_secs(7 * 24 * 60 * 60);
/// On-disk mailbox filename.
pub const MAILBOX_FILE_NAME: &str = "mailbox.json";

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}
fn signing_bytes(e: &MailboxEnvelope) -> Vec<u8> {
    postcard::to_stdvec(&(e.from, e.recipient, e.ephemeral, e.nonce, &e.ciphertext))
        .expect("postcard encoding cannot fail")
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

    /// Decrypt an envelope addressed to this identity after checking its signature.
    pub fn open(&self, envelope: &MailboxEnvelope) -> Result<Vec<u8>> {
        envelope.open_with(self)
    }
}

/// Opaque encrypted, signed mailbox entry.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct MailboxEnvelope {
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
    /// Creation time in Unix epoch milliseconds.
    pub created_at: u64,
    /// Sender signature over all preceding fields.
    pub signature: ByteArray<SIGNATURE_LEN>,
}

impl MailboxEnvelope {
    /// Stable content identifier used for deduplication and acknowledgements.
    pub fn message_id(&self) -> String {
        blake3::hash(&signing_bytes(self)).to_hex().to_string()
    }

    /// Decrypt after checking the sender signature and recipient identity.
    pub fn open(&self, recipient: &SecretKey) -> Result<Vec<u8>> {
        MailboxIdentity::from_secret(recipient).open(self)
    }

    /// Validate authorization, recipient identity, signature, and retention
    /// before handing an incoming replay to the normal message pipeline.
    pub fn validate_for(
        &self,
        identity: &MailboxIdentity,
        allowed_senders: &[PublicKey],
        ttl: Duration,
    ) -> Result<Vec<u8>> {
        if !allowed_senders.contains(&self.from) {
            return Err(n0_error::anyerr!("mailbox sender is not authorized"));
        }
        let now = now_ms();
        if self.created_at > now.saturating_add(60_000)
            || now.saturating_sub(self.created_at) > ttl.as_millis() as u64
        {
            return Err(n0_error::anyerr!(
                "mailbox envelope is expired or from the future"
            ));
        }
        identity.open(self)
    }

    fn open_with(&self, identity: &MailboxIdentity) -> Result<Vec<u8>> {
        verify_signature(self)?;
        let expected = identity.public_key();
        if self.recipient != expected {
            return Err(n0_error::anyerr!(
                "mailbox envelope is addressed to another recipient"
            ));
        }
        let shared = identity
            .secret
            .diffie_hellman(&EncryptionPublicKey::from(self.ephemeral));
        let key = derive_key(shared.as_bytes());
        Aes256Gcm::new_from_slice(&key)
            .expect("32-byte key")
            .decrypt(Nonce::from_slice(&self.nonce), self.ciphertext.as_ref())
            .map_err(|_| n0_error::anyerr!("mailbox ciphertext authentication failed"))
    }
}

fn seal(
    sender: &SecretKey,
    recipient: MailboxPublicKey,
    payload: &[u8],
) -> Result<MailboxEnvelope> {
    let ephemeral_secret = StaticSecret::random();
    let ephemeral = EncryptionPublicKey::from(&ephemeral_secret);
    let shared = ephemeral_secret.diffie_hellman(&EncryptionPublicKey::from(recipient.encryption));
    let key = derive_key(shared.as_bytes());
    let mut nonce = [0u8; NONCE_LEN];
    getrandom::fill(&mut nonce).map_err(|e| n0_error::anyerr!("generate mailbox nonce: {e}"))?;
    let ciphertext = Aes256Gcm::new_from_slice(&key)
        .expect("32-byte key")
        .encrypt(Nonce::from_slice(&nonce), payload)
        .map_err(|_| n0_error::anyerr!("encrypt mailbox payload"))?;
    let mut envelope = MailboxEnvelope {
        from: sender.public(),
        recipient,
        ephemeral: ephemeral.to_bytes(),
        nonce,
        ciphertext,
        created_at: now_ms(),
        signature: ByteArray::new([0u8; SIGNATURE_LEN]),
    };
    envelope.signature = ByteArray::new(sender.sign(&signing_bytes(&envelope)).to_bytes());
    Ok(envelope)
}

fn derive_key(shared: &[u8; 32]) -> [u8; 32] {
    *blake3::hash(
        [b"iroh-gossip-chat/mailbox/v1".as_slice(), shared]
            .concat()
            .as_slice(),
    )
    .as_bytes()
}

fn verify_signature(envelope: &MailboxEnvelope) -> Result<()> {
    envelope
        .from
        .verify(
            &signing_bytes(envelope),
            &Signature::from_bytes(&envelope.signature),
        )
        .map_err(|e| n0_error::anyerr!("verify mailbox envelope signature: {e}"))
}

/// Create an encrypted envelope using a recipient's advertised public key.
pub fn seal_for(
    sender: &SecretKey,
    recipient: MailboxPublicKey,
    payload: &[u8],
) -> Result<MailboxEnvelope> {
    seal(sender, recipient, payload)
}

/// A recipient-signed acknowledgement for one envelope.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct MailboxAck {
    /// Recipient identity that signed the acknowledgement.
    pub recipient: PublicKey,
    /// Envelope identifier being acknowledged.
    pub message_id: String,
    /// Recipient signature over the identity and message identifier.
    pub signature: ByteArray<SIGNATURE_LEN>,
}

impl MailboxAck {
    /// Sign an acknowledgement after successfully processing a message.
    pub fn sign(recipient: &SecretKey, message_id: impl Into<String>) -> Self {
        let message_id = message_id.into();
        let data = postcard::to_stdvec(&(recipient.public(), &message_id))
            .expect("postcard encoding cannot fail");
        Self {
            recipient: recipient.public(),
            message_id,
            signature: ByteArray::new(recipient.sign(&data).to_bytes()),
        }
    }
    /// Verify the acknowledgement signature against the expected recipient key.
    pub fn verify(&self, expected: PublicKey) -> Result<()> {
        if self.recipient != expected {
            return Err(n0_error::anyerr!("mailbox acknowledgement signer mismatch"));
        }
        let data = postcard::to_stdvec(&(self.recipient, &self.message_id))
            .expect("postcard encoding cannot fail");
        self.recipient
            .verify(&data, &Signature::from_bytes(&self.signature))
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
    pub fn save(&self) -> Result<PathBuf> {
        let mut copy = self.clone();
        copy.expire();
        let path = self.data_dir.join(MAILBOX_FILE_NAME);
        atomic_write_json(&path, &copy, "mailbox store")?;
        Ok(path)
    }
    fn expire(&mut self) {
        let cutoff = now_ms().saturating_sub(self.ttl.as_millis() as u64);
        self.entries.retain(|_, e| e.created_at > cutoff);
    }
    /// Enqueue only a valid, authenticated envelope from an allowed sender.
    pub fn enqueue(
        &mut self,
        envelope: MailboxEnvelope,
        allowed_senders: &[PublicKey],
    ) -> Result<String> {
        verify_signature(&envelope)?;
        if !allowed_senders.contains(&envelope.from) {
            return Err(n0_error::anyerr!("mailbox sender is not authorized"));
        }
        if let Some(recipient) = self.recipient {
            if envelope.recipient.identity != recipient {
                return Err(n0_error::anyerr!("mailbox recipient mismatch"));
            }
        } else {
            self.recipient = Some(envelope.recipient.identity);
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
        entries.sort_by_key(|entry| (entry.created_at, entry.message_id()));
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
        ack.verify(envelope.recipient.identity)?;
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
        let payload = envelope.validate_for(identity, allowed_senders, self.ttl)?;
        let id = envelope.message_id();
        // Reconnects and restarts may replay an envelope.  Idempotent
        // acceptance avoids injecting it twice while still returning the
        // authenticated payload to the caller.
        if !self.entries.contains_key(&id) {
            self.enqueue(envelope, allowed_senders)?;
            self.save()?;
        }
        Ok((id, payload))
    }

    /// Remove an acknowledged outgoing envelope and persist the removal.
    pub fn acknowledge_and_save(&mut self, ack: &MailboxAck) -> Result<bool> {
        let removed = self.acknowledge(ack)?;
        if removed {
            self.save()?;
        }
        Ok(removed)
    }

    /// Remove and persist an acknowledged outgoing envelope.
    pub fn acknowledge_outgoing_and_save(&mut self, ack: &MailboxAck) -> Result<bool> {
        let removed = self.acknowledge_outgoing(ack)?;
        if removed {
            self.save()?;
        }
        Ok(removed)
    }
    /// Return pending envelopes whose recipient identity matches `who`.
    ///
    /// Used by the inbox SyncResponse handler to serve envelopes that
    /// were encrypted for a specific peer and have not yet been
    /// acknowledged.
    pub fn pending_for_recipient(&mut self, who: PublicKey) -> Vec<MailboxEnvelope> {
        self.expire();
        let mut entries: Vec<_> = self
            .entries
            .values()
            .filter(|e| e.recipient.identity == who)
            .cloned()
            .collect();
        entries.sort_by_key(|entry| (entry.created_at, entry.message_id()));
        entries
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
    #[test]
    fn envelope_is_not_plaintext_and_round_trips() {
        let recipient = SecretKey::generate();
        let sender = SecretKey::generate();
        let id = MailboxIdentity::from_secret(&recipient);
        let env = id.seal(&sender, b"private").unwrap();
        assert!(!env.ciphertext.windows(7).any(|w| w == b"private"));
        assert_eq!(env.open(&recipient).unwrap(), b"private");
    }

    #[test]
    fn incoming_acceptance_is_idempotent_after_replay() {
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

        let ack = MailboxAck::sign(&recipient, message_id);
        assert!(store.acknowledge_outgoing(&ack).unwrap());
        assert!(store.is_empty());
    }
}
