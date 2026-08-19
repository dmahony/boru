//! Relational storage layer with managed migrations.
//!
//! # Schema
//!
//! Version 1 — message delivery (migrated from [`crate::store::MessageStore`]):
//!   - `inbox` / `outbox` / `contacts` / `sync_cursor`
//!   - `schema_version` (single-row meta table introduced in v1)
//!
//! Version 2 — content-addressed file objects and sharing:
//!   - `file_objects`      — content-addressed immutable file data
//!   - `message_attachments` — links a message to one or more file objects
//!   - `shared_files`       — profile-offered files with per-peer visibility
//!   - `file_collections`   — named groups of shared files
//!   - `file_collection_items` — membership in a collection
//!   - `shared_file_permissions` — per-peer grants on individual shared files
//!   - `downloads`          — durable download state machine
//!   - `profile_manifest_state` — manifest revision tracking
//!
//! Version 13 — per-group encryption state (`group_encryption_state` table).
//!
//! # Design rules
//!
//!  1. Chat attachments belong to messages (`message_attachments`).
//!  2. Profile file offers belong to a user profile (`shared_files`).
//!  3. Both reference the same content-addressed `file_objects` store.
//!  4. No local filesystem paths are exposed to remote peers.
//!  5. All large binary data lives in `file_objects`; relationship tables
//!     carry only foreign keys and metadata.

use std::{
    path::Path,
    sync::{
        atomic::{AtomicBool, AtomicUsize, Ordering},
        Arc, Mutex,
    },
    time::Instant,
};

use anyhow::anyhow;
use iroh::{PublicKey, SecretKey};
use n0_error::{Result, StdResultExt};
use rusqlite::{params, Connection, TransactionBehavior};
use serde::{Deserialize, Serialize};
use tracing::{debug, info, warn};

use crate::catalogue_limits::CatalogueLimitsConfig;
use crate::catalogue_model::{CatalogueView, RemoteSharedFile, SignedFileCatalogue};
use crate::diagnostics::TransferLifecycleEvent;
use crate::friends::{FriendRelationship, FriendsStore};
use crate::mailbox::{seal_for, MailboxAck, MailboxEnvelope, MailboxPublicKey};
use crate::proto::TopicId;
use crate::rings::{Ring, RingPermission, RingResourcePermission};
use crate::store::{DeliveryStatus, MessageId, OutboxRow, StoredEnvelope};

// ── Current schema version ────────────────────────────────────────────────

/// Bump every time a new migration is added.
pub const CURRENT_SCHEMA_VERSION: u32 = 21;

/// Maximum number of rows inspected by a single outbox claim query.
pub const MAX_OUTBOX_CLAIM_LIMIT: u32 = 100;
/// Default lease duration for an outbox worker claim.
pub const DEFAULT_OUTBOX_LEASE_MS: u64 = 30_000;

// ── Database file name ────────────────────────────────────────────────────

/// The SQLite database file stored beside the data directory.
pub const DB_FILE_NAME: &str = "boru.db";

// ── File-object types ─────────────────────────────────────────────────────

/// Content hash type — blake3 32-byte output encoded as hex.
pub type ContentHash = [u8; 32];

/// A matched acknowledgement row: (logical_id, sender_id, recipient_id, envelope_bytes).
type AckMatchRow = (MessageId, Vec<u8>, Vec<u8>, Vec<u8>);

/// A content-addressed file object stored locally.
#[derive(Debug, Clone)]
pub struct FileObject {
    /// blake3 hash of the file contents (hex-encoded, 64 chars).
    pub content_hash: String,
    /// Total size in bytes.
    pub size: u64,
    /// MIME type hint (e.g. "image/png", "application/octet-stream").
    pub mime_type: String,
    /// Original filename (no path components).
    pub filename: String,
    /// Created-at timestamp in milliseconds since UNIX epoch.
    pub created_at_ms: u64,
    /// The file data itself. For large files this may be a blob-id
    /// that references an iroh-blobs store.
    pub data: Option<Vec<u8>>,
    /// Optional source path on disk for referenced (non-imported) files.
    pub source_path: Option<String>,
}

/// A file object that has been imported from a remote peer and is
/// referenced by an iroh-blobs hash rather than stored inline.
#[derive(Debug, Clone)]
pub struct ImportedFileObject {
    /// Links to `file_objects.content_hash`.
    pub content_hash: String,
    /// The iroh-blobs hash that can be used to fetch this file.
    pub blob_hash: String,
    /// The peer we imported this from.
    pub source_peer: String,
    /// When the import occurred (ms since UNIX epoch).
    pub imported_at_ms: u64,
}

/// A chat message attachment — links a message to one or more file objects.
#[derive(Debug, Clone)]
pub struct MessageAttachment {
    /// Unique row id.
    pub id: i64,
    /// The local message event-id.
    pub event_id: u64,
    /// Links to `file_objects.content_hash`.
    pub content_hash: String,
    /// Display filename for the recipient.
    pub display_filename: String,
    /// Ordinal position within the message's attachment list.
    pub position: u32,
}

/// A profile-offered shared file.
#[derive(Debug, Clone)]
pub struct SharedFileRow {
    /// Links to `file_objects.content_hash`.
    pub content_hash: String,
    /// The owning profile (hex-encoded public key).
    pub profile_user_id: String,
    /// Stable metadata id (from `crate::user_profile::SharedFile`).
    pub metadata_id: String,
    /// Display filename.
    pub display_filename: String,
    /// Custom description (optional).
    pub description: Option<String>,
    /// Whether this file is currently offered.
    pub offered: bool,
    /// When the offer was created (ms since UNIX epoch).
    pub created_at_ms: u64,
    /// When the offer was last updated.
    pub updated_at_ms: u64,
    /// Monotonically-increasing version number bumped on every metadata
    /// or content change.  Defaults to 0 for new entries.
    pub version: u64,
}

/// A row in the `outgoing_messages` table: tracks delivery state of
/// messages composed locally, mirroring the fields of `OutboxEntry`
/// but stored in SQLite so the GUI never needs to touch `outbox.json`.
#[derive(Debug, Clone)]
pub struct OutgoingMessageRow {
    /// Stable, monotonically-increasing event identifier assigned locally.
    pub event_id: u64,
    /// The gossip topic this message was (or will be) broadcast on.
    pub topic: TopicId,
    /// blake3 hex hash of the raw signed message bytes.
    pub hash: String,
    /// The raw signed message bytes, for replay/retry without touching
    /// the JSON outbox store.
    pub signed_bytes: Vec<u8>,
    /// Current delivery state: "queued", "sent", "delivered", "seen", "failed".
    pub delivery_state: String,
    /// Number of times a send has been attempted (for retry tracking).
    pub retry_count: u32,
    /// Unix-epoch milliseconds when this entry was created.
    pub created_at_ms: u64,
    /// Unix-epoch milliseconds when this entry was last updated.
    pub updated_at_ms: u64,
}

/// A named collection of shared files belonging to a profile.
#[derive(Debug, Clone)]
pub struct FileCollection {
    /// Unique row id.
    pub id: i64,
    /// The owning profile (hex-encoded public key).
    pub profile_user_id: String,
    /// Human-readable collection name (e.g. "photos", "documents").
    pub name: String,
    /// Optional description.
    pub description: Option<String>,
    /// When the collection was created.
    pub created_at_ms: u64,
    /// When the collection was last modified.
    pub updated_at_ms: u64,
}

/// Membership of a shared file in a collection.
#[derive(Debug, Clone)]
pub struct FileCollectionItem {
    /// Links to `file_collections.id`.
    pub collection_id: i64,
    /// Links to `file_objects.content_hash`.
    pub content_hash: String,
    /// Ordinal position within the collection.
    pub position: u32,
    /// When the item was added.
    pub added_at_ms: u64,
}

/// Per-peer permission grant on a shared file.
#[derive(Debug, Clone)]
pub struct SharedFilePermission {
    /// Links to `file_objects.content_hash`.
    pub content_hash: String,
    /// The grantor's hex-encoded public key (the profile owner).
    pub grantor_user_id: String,
    /// The grantee's hex-encoded public key.
    pub grantee_user_id: String,
    /// Allowed operation: "read", "download", etc.
    pub permission: String,
    /// When the grant was created.
    pub created_at_ms: u64,
    /// Optional expiry (ms since UNIX epoch, NULL = never expires).
    pub expires_at_ms: Option<u64>,
}

impl SharedFilePermission {
    /// Whether this grant is active at `now_ms`.
    ///
    /// A grant with no `expires_at_ms` never expires; a grant whose expiry is
    /// in the past is inactive and must not authorize (or deny) access. This
    /// mirrors the expiry filter used by the SQL-level helpers
    /// (`check_permission`, `count_read_grants_for_file`,
    /// `has_active_permissions_for_file`) so in-memory authorization loops
    /// cannot resurrect an expired grant.
    pub fn is_active_at(&self, now_ms: u64) -> bool {
        self.expires_at_ms.is_none_or(|expires| expires > now_ms)
    }
}

/// Durable download state for a file being fetched from a remote peer.
#[derive(Debug, Clone)]
pub struct Download {
    /// Unique row id.
    pub id: i64,
    /// Links to `file_objects.content_hash` (the target).
    pub content_hash: String,
    /// The remote peer we are downloading from.
    pub remote_peer: String,
    /// Current state: "queued", "active", "paused", "completed", "failed".
    pub state: String,
    /// Bytes received so far.
    pub bytes_downloaded: u64,
    /// Total expected bytes.
    pub total_bytes: u64,
    /// When the download was created.
    pub created_at_ms: u64,
    /// When the state last changed.
    pub updated_at_ms: u64,
    /// Last error message (if state == "failed").
    pub last_error: Option<String>,
    /// Retry count.
    pub retry_count: u32,
    /// Next retry timestamp (ms since UNIX epoch).
    pub next_retry_at_ms: Option<u64>,
}

/// A completed download joined with its file-object metadata and the
/// recorded destination path — the durable record behind the Downloaded tab.
///
/// Only dashboard-safe display metadata is exposed: display name, MIME type,
/// size, remote peer, and the destination path needed for safe local actions.
#[derive(Debug, Clone)]
pub struct CompletedDownloadRecord {
    /// Unique download row id (stable history id).
    pub id: i64,
    /// Links to `file_objects.content_hash` (the verified content hash).
    pub content_hash: String,
    /// The remote peer we downloaded from.
    pub remote_peer: String,
    /// Total expected bytes.
    pub total_bytes: u64,
    /// When the download reached the terminal complete state.
    pub completed_at_ms: u64,
    /// Recorded destination path (the file may have moved or been deleted).
    pub destination_path: Option<String>,
    /// Display filename from the file object.
    pub display_filename: String,
    /// MIME type hint from the file object.
    pub mime_type: String,
}

/// Durable, privacy-filtered transfer activity used by the dashboard.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TransferActivityRow {
    /// Opaque event identifier supplied by the lifecycle event contract.
    pub event_id: String,
    /// Short logical transfer identifier.
    pub transfer_id: String,
    /// Stable lifecycle event name.
    pub event_name: String,
    /// Monotonic sequence within the transfer.
    pub sequence: u64,
    /// Local observation timestamp in Unix milliseconds.
    pub occurred_at_ms: u64,
    /// Transfer attempt number.
    pub attempt: u32,
    /// Sanitized JSON payload containing only dashboard-safe counters/status.
    pub payload_json: Option<String>,
    /// Transfer direction: `"inbound"` (downloads to this node) or
    /// `"outbound"` (uploads served to remote peers). Existing rows default
    /// to inbound because outbound recording arrived with schema v17.
    pub direction: String,
}

fn sanitize_activity_payload(value: &serde_json::Value) -> Option<String> {
    let object = value.as_object()?;
    const ALLOWED: &[&str] = &[
        "total_bytes",
        "queue_depth",
        "request_kind",
        "grant_ttl_ms",
        "resumed_bytes",
        "bytes_transferred",
        "bytes_delta",
        "checkpoint_interval_ms",
        "rate_bytes_per_sec",
        "percent_millis",
        "success",
        "duration_ms",
        "error_category",
        "retry_delay_ms",
        "reason",
        // Direction marker set by outbound producers ("inbound"/"outbound").
        "direction",
    ];
    let filtered = object
        .iter()
        .filter(|(key, _)| ALLOWED.contains(&key.as_str()))
        .map(|(key, value)| (key.clone(), value.clone()))
        .collect::<serde_json::Map<_, _>>();
    if filtered.is_empty() {
        None
    } else {
        serde_json::to_string(&filtered).ok()
    }
}

// ── Remote catalogue types ─────────────────────────────────────────

/// Metadata about a remote peer's stored catalogue fetched via the
/// catalogue protocol.
#[derive(Debug, Clone)]
pub struct RemoteCatalogueMeta {
    /// The remote peer's public key (hex-encoded).
    pub peer: String,
    /// Catalogue revision at the time of last fetch.
    pub revision: u64,
    /// When the catalogue was generated on the remote peer.
    pub generated_at_ms: u64,
    /// When we last fetched this catalogue.
    pub fetched_at_ms: u64,
}

/// A file entry from a remote peer's catalogue, stored locally for
/// catalogue reconciliation.
#[derive(Debug, Clone)]
pub struct RemoteSharedFileRow {
    /// Content hash of the file.
    pub content_hash: String,
    /// Display filename from the remote catalogue.
    pub display_filename: String,
    /// MIME type hint.
    pub mime_type: String,
    /// File size in bytes.
    pub size_bytes: u64,
}

/// A collection entry from a remote peer's catalogue.
#[derive(Debug, Clone)]
pub struct RemoteCollectionRow {
    /// Collection row id.
    pub id: i64,
    /// Collection name.
    pub name: String,
}

/// Availability state of a file object — whether the content is
/// present locally and has been verified against expected metadata.
#[derive(Debug, Clone)]
pub struct FileAvailability {
    /// Content hash of the file.
    pub content_hash: String,
    /// The owning profile (hex-encoded public key).
    pub profile_user_id: String,
    /// Availability status: "Available", "Changed", "Missing", etc.
    pub availability: String,
    /// When the file was last verified (ms since UNIX epoch).
    pub verified_at_ms: Option<u64>,
    /// The expected content hash at the time of verification.
    pub expected_content_hash: String,
    /// The expected file size in bytes.
    pub expected_size: u64,
    /// When the availability record was last updated.
    pub updated_at_ms: u64,
}

/// Profile manifest revision tracking for a local user.
#[derive(Debug, Clone)]
pub struct ProfileManifestState {
    /// The hex-encoded user public key (the profile owner).
    pub user_id: String,
    /// Monotonically increasing revision counter.
    pub revision: u64,
    /// blake3 hash of the serialized manifest at this revision.
    pub manifest_hash: String,
    /// When this revision was committed.
    pub created_at_ms: u64,
}

// ── Contact row (from v1 schema) ──────────────────────────────────────────

/// A row from the v1 `contacts` table.
#[derive(Debug, Clone)]
pub struct ContactRow {
    /// Peer user identity (public key bytes).
    pub user_id: Vec<u8>,
    /// Peer device identity (public key bytes).
    pub device_id: Vec<u8>,
    /// Cached endpoint address, if known.
    pub endpoint_addr: Option<Vec<u8>>,
    /// Identity verification key.
    pub identity_key: Vec<u8>,
    /// Last-seen timestamp in milliseconds since UNIX epoch.
    pub last_seen_ms: u64,
    /// Expiry timestamp in milliseconds since UNIX epoch.
    pub expires_at_ms: u64,
}

/// A row from the v1 `sync_cursor` table.
#[derive(Debug, Clone)]
pub struct SyncCursorRow {
    /// Peer device identity.
    pub peer_device_id: Vec<u8>,
    /// Last observed message clock value.
    pub last_seen_msg_clock: Option<Vec<u8>>,
    /// Last-sync timestamp in milliseconds since UNIX epoch.
    pub last_sync_at_ms: u64,
}

/// Durable group metadata.
#[allow(missing_docs)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GroupRow {
    pub group_id: [u8; 32],
    pub name: String,
    pub description: String,
    pub owner_public_key: Vec<u8>,
    pub current_epoch: u64,
    pub created_at_ms: u64,
    pub updated_at_ms: u64,
    pub archived: bool,
}

/// Durable group membership.
#[allow(missing_docs)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GroupMemberRow {
    pub group_id: [u8; 32],
    pub public_key: Vec<u8>,
    pub role: String,
    pub joined_at_ms: u64,
    pub invited_by: Option<Vec<u8>>,
    pub epoch_joined: u64,
    pub state: String,
}

/// Durable group topic/discovery epoch.
#[allow(missing_docs)]
#[derive(Clone, PartialEq, Eq)]
pub struct GroupEpochRow {
    pub group_id: [u8; 32],
    pub epoch: u64,
    pub topic_id: TopicId,
    pub discovery_secret: Vec<u8>,
    pub created_at_ms: u64,
}

impl std::fmt::Debug for GroupEpochRow {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Redact the raw discovery secret (BORU-AUDIT-17); only its length is
        // revealed so logs stay free of key material.
        f.debug_struct("GroupEpochRow")
            .field("group_id", &hex::encode(&self.group_id[..4]))
            .field("epoch", &self.epoch)
            .field("topic_id", &hex::encode(&self.topic_id.as_bytes()[..4]))
            .field(
                "discovery_secret",
                &format!("<redacted {} bytes>", self.discovery_secret.len()),
            )
            .field("created_at_ms", &self.created_at_ms)
            .finish()
    }
}

/// Durable group invitation state.
#[allow(missing_docs)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GroupInviteRow {
    pub invite_id: [u8; 32],
    pub group_id: [u8; 32],
    pub inviter_public_key: Vec<u8>,
    pub recipient_public_key: Vec<u8>,
    pub epoch: u64,
    pub status: String,
    pub created_at_ms: u64,
    pub expires_at_ms: u64,
    pub ticket: String,
    /// Group display name, persisted from the whisper invite so the recipient
    /// can create a ConversationEntry at accept time.
    pub group_name: String,
}

// ── Storage ───────────────────────────────────────────────────────────────

/// Relational storage backed by a single SQLite database.
///
/// Owns the connection, schema migrations, and provides repository-style
/// accessors for each logical group of tables.
#[derive(Debug, Clone)]
pub struct Storage {
    conn: Arc<Mutex<Connection>>,
    catalogue_limits: CatalogueLimitsConfig,
    /// Async-facade activity tracker.  Tracks in-flight blocking operations
    /// so `flush`/`shutdown` can wait for queued writes deterministically.
    /// Only present when the `net` feature (Tokio) is enabled.
    #[cfg(feature = "net")]
    activity: Arc<DbActivity>,
}

/// Threshold (milliseconds) above which a blocking storage operation is
/// logged as slow.  Instrumentation only records the operation label and
/// elapsed time — never message contents.
#[cfg(feature = "net")]
const SLOW_STORAGE_OP_MS: u64 = 100;

/// Tracks in-flight blocking database operations.
///
/// The async facade runs repository work on the Tokio blocking pool via
/// `spawn_blocking`.  This counter lets `flush`/`shutdown` wait until every
/// queued write has completed (or failed explicitly) before returning.
#[cfg(feature = "net")]
#[derive(Debug, Default)]
struct DbActivity {
    /// Number of blocking operations currently queued or executing.
    in_flight: AtomicUsize,
    /// Set by `shutdown`; new operations fail fast once set.
    closed: AtomicBool,
    /// Signalled when `in_flight` drops to zero.
    drained: tokio::sync::Notify,
}

#[cfg(feature = "net")]
impl DbActivity {
    /// Reserve a slot for one blocking operation.
    ///
    /// Returns `false` when the store is shut down; the caller must fail
    /// explicitly instead of queueing more work.
    fn begin(&self) -> bool {
        if self.closed.load(Ordering::SeqCst) {
            return false;
        }
        self.in_flight.fetch_add(1, Ordering::SeqCst);
        true
    }

    /// Release the slot after a blocking operation finished (or panicked).
    fn end(&self) {
        if self.in_flight.fetch_sub(1, Ordering::SeqCst) == 1 {
            self.drained.notify_waiters();
        }
    }

    /// Wait until every in-flight operation has finished.
    async fn wait_idle(&self) {
        loop {
            if self.in_flight.load(Ordering::SeqCst) == 0 {
                return;
            }
            let notified = self.drained.notified();
            // Re-check after creating the future so a decrement between the
            // load and `await` cannot be missed (classic Notify race guard).
            if self.in_flight.load(Ordering::SeqCst) == 0 {
                return;
            }
            notified.await;
        }
    }
}

/// Durable result of creating an outgoing direct message.
#[allow(missing_docs)]
#[derive(Debug, Clone)]
pub struct OutgoingDm {
    pub message_id: MessageId,
    pub sequence: u64,
    pub logical_message: Vec<u8>,
    pub envelope: MailboxEnvelope,
}

/// Deterministic failures used to verify outgoing-DM transaction rollback.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OutgoingDmFault {
    /// Fail while preparing mailbox encryption.
    Encryption,
    /// Fail after durable rows are written but before commit.
    Database,
}

#[allow(missing_docs)]
#[derive(Debug, Clone)]
pub struct DmMessageRow {
    pub message_id: MessageId,
    pub conversation_id: [u8; 32],
    pub sender: PublicKey,
    pub recipient: PublicKey,
    pub sequence: u64,
    pub request_key: String,
    pub plaintext: Vec<u8>,
    /// Local insertion time. This is informational only; it is deliberately
    /// not part of the history ordering key because remote clocks are untrusted.
    pub created_at_ms: u64,
}

#[allow(missing_docs)]
#[derive(Debug, Clone)]
pub struct DmOutboxRow {
    pub message_id: MessageId,
    pub recipient: PublicKey,
    pub envelope: MailboxEnvelope,
}

/// Deterministic failures used to verify acknowledgement rollback.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AckProcessingFault {
    /// Fail after acknowledgement state is written, before commit.
    Database,
}

#[derive(Debug, Serialize, Deserialize)]
struct LogicalDm {
    conversation_id: [u8; 32],
    sender: PublicKey,
    recipient: PublicKey,
    sequence: u64,
    message_id: MessageId,
    plaintext: Vec<u8>,
    signature: Vec<u8>,
}

/// Canonical protocol tag for signed logical DMs (BORU-AUDIT-27).
const LOGICAL_DM_PROTOCOL: &str = "boru/logical-dm";
/// Version of the signed logical-DM layout (BORU-AUDIT-27).
const LOGICAL_DM_VERSION: u16 = 1;

/// A row from the `downloads` table recovered during restart.
struct RecoveryRow {
    id: i64,
    state: String,
    total_bytes: u64,
    content_hash: String,
    temp_path: Option<String>,
    destination_path: Option<String>,
}

impl Storage {
    /// Return the raw [`rusqlite::Connection`] (locked) for advanced use.
    /// Prefer the typed repository methods when possible.
    pub fn with_conn<F, T>(&self, f: F) -> Result<T>
    where
        F: FnOnce(&Connection) -> Result<T>,
    {
        let conn = self.conn.lock().unwrap();
        f(&conn)
    }
    /// Run a blocking storage operation on the Tokio blocking pool.
    ///
    /// The closure receives the [`Storage`] clone owned by the blocking task
    /// and may call any typed repository method (or [`with_conn`](Self::with_conn)
    /// for raw SQL).  The connection is locked *inside* the blocking task —
    /// this facade never holds it across an await point, and transactions
    /// must be started and committed entirely inside the closure.
    ///
    /// Slow operations (>= `SLOW_STORAGE_OP_MS`) are logged with a label
    /// and elapsed time only; message contents are never logged.
    ///
    /// If the store has been shut down, this fails fast instead of queueing.
    ///
    /// Returns `anyhow::Result`; repository errors (n0-error `AnyError`)
    /// are converted so callers in either error convention can use `?`.
    #[cfg(feature = "net")]
    pub async fn run_blocking<F, T, E>(&self, op: &'static str, f: F) -> anyhow::Result<T>
    where
        F: FnOnce(&Storage) -> Result<T, E> + Send + 'static,
        E: Into<anyhow::Error> + Send + 'static,
        T: Send + 'static,
    {
        if !self.activity.begin() {
            return Err(anyhow!(
                "storage is shut down; refusing new operation '{op}'"
            ));
        }
        let storage = self.clone();
        let started = Instant::now();
        let result = tokio::task::spawn_blocking(move || f(&storage)).await;
        self.activity.end();
        let elapsed = started.elapsed();
        if elapsed.as_millis() as u64 >= SLOW_STORAGE_OP_MS {
            warn!(
                op,
                elapsed_ms = elapsed.as_millis() as u64,
                "slow storage operation"
            );
        }
        match result {
            Ok(Ok(r)) => Ok(r),
            Ok(Err(e)) => Err(anyhow!("storage operation '{op}' failed: {:#}", e.into())),
            Err(e) => Err(anyhow!("storage worker task failed for '{op}': {e}")),
        }
    }
    /// Wait until every previously-queued blocking operation has completed.
    ///
    /// A write that was queued before `flush` is either fully applied or has
    /// failed explicitly by the time this returns.
    #[cfg(feature = "net")]
    pub async fn flush(&self) {
        self.activity.wait_idle().await;
    }
    /// Shut the store down: mark it closed so new operations fail fast, then
    /// wait for all in-flight writes to complete or fail explicitly.
    #[cfg(feature = "net")]
    pub async fn shutdown(&self) {
        self.activity.closed.store(true, Ordering::SeqCst);
        self.flush().await;
    }
}

// ── Helpers ───────────────────────────────────────────────────────────────

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

fn row_to_envelope_bare(msg_id: &MessageId, row: &rusqlite::Row) -> Result<StoredEnvelope> {
    let mut conversation_id = [0u8; 32];
    let conv_blob: Vec<u8> = row.get(1).std_context("get conversation_id")?;
    conversation_id.copy_from_slice(&conv_blob);

    let author_user_blob: Vec<u8> = row.get(2).std_context("get author_user_id")?;
    let author_user_id = iroh::PublicKey::try_from(author_user_blob.as_slice())
        .map_err(|e| anyhow!("invalid public key: {}", e))?;

    let author_device_blob: Vec<u8> = row.get(3).std_context("get author_device_id")?;
    let author_device_id = iroh::PublicKey::try_from(author_device_blob.as_slice())
        .map_err(|e| anyhow!("invalid public key: {}", e))?;

    let created_at_ms: i64 = row.get(4).std_context("get created_at_ms")?;
    let expires_at_ms: i64 = row.get(5).std_context("get expires_at_ms")?;
    let ciphertext_blob: Vec<u8> = row.get(6).std_context("get ciphertext")?;
    let ciphertext = bytes::Bytes::from(ciphertext_blob);
    let signature_blob: Vec<u8> = row.get(7).std_context("get signature")?;
    let mut signature = [0u8; 64];
    signature.copy_from_slice(&signature_blob);
    let acked_at_ms: Option<i64> = row.get(8).std_context("get acked_at_ms")?;

    Ok(StoredEnvelope {
        msg_id: *msg_id,
        conversation_id,
        author_user_id,
        author_device_id,
        created_at_ms: created_at_ms as u64,
        expires_at_ms: expires_at_ms as u64,
        ciphertext,
        signature,
        acked_at_ms: acked_at_ms.map(|v| v as u64),
    })
}

fn row_to_envelope(msg_id: &MessageId, row: &rusqlite::Row) -> Result<StoredEnvelope> {
    // Row indices: 0=conversation_id, 1=author_user_id, ...
    let mut conversation_id = [0u8; 32];
    let conv_blob: Vec<u8> = row.get(0).std_context("get conversation_id")?;
    conversation_id.copy_from_slice(&conv_blob);

    let author_user_blob: Vec<u8> = row.get(1).std_context("get author_user_id")?;
    let author_user_id = iroh::PublicKey::try_from(author_user_blob.as_slice())
        .map_err(|e| anyhow!("invalid public key: {}", e))?;

    let author_device_blob: Vec<u8> = row.get(2).std_context("get author_device_id")?;
    let author_device_id = iroh::PublicKey::try_from(author_device_blob.as_slice())
        .map_err(|e| anyhow!("invalid public key: {}", e))?;

    let created_at_ms: i64 = row.get(3).std_context("get created_at_ms")?;
    let expires_at_ms: i64 = row.get(4).std_context("get expires_at_ms")?;
    let ciphertext_blob: Vec<u8> = row.get(5).std_context("get ciphertext")?;
    let ciphertext = bytes::Bytes::from(ciphertext_blob);
    let signature_blob: Vec<u8> = row.get(6).std_context("get signature")?;
    let mut signature = [0u8; 64];
    signature.copy_from_slice(&signature_blob);
    let acked_at_ms: Option<i64> = row.get(7).std_context("get acked_at_ms")?;

    Ok(StoredEnvelope {
        msg_id: *msg_id,
        conversation_id,
        author_user_id,
        author_device_id,
        created_at_ms: created_at_ms as u64,
        expires_at_ms: expires_at_ms as u64,
        ciphertext,
        signature,
        acked_at_ms: acked_at_ms.map(|v| v as u64),
    })
}

fn row_to_outbox(row: &rusqlite::Row) -> Result<OutboxRow> {
    let msg_blob: Vec<u8> = row.get(0).std_context("get msg_id")?;
    let mut msg_id = [0u8; 32];
    msg_id.copy_from_slice(&msg_blob);

    let recipient_blob: Vec<u8> = row.get(1).std_context("get recipient")?;
    let recipient_device_id = iroh::PublicKey::try_from(recipient_blob.as_slice())
        .map_err(|e| anyhow!("invalid public key: {}", e))?;

    let status_code: u8 = row.get(2).std_context("get status")?;
    let status = DeliveryStatus::try_from(status_code)?;

    Ok(OutboxRow {
        msg_id,
        recipient_device_id,
        status,
        attempts: row.get(3).std_context("get attempts")?,
        next_attempt_at_ms: row.get::<_, i64>(4).std_context("get next_attempt")? as u64,
        last_error_code: row.get(5).std_context("get error_code")?,
        last_attempt_at_ms: row
            .get::<_, Option<i64>>(6)
            .std_context("get last_attempt")?
            .map(|v| v as u64),
        lease_owner: row.get(7).std_context("get lease_owner")?,
        locked_until_ms: row
            .get::<_, Option<i64>>(8)
            .std_context("get locked_until")?
            .map(|v| v as u64),
        expires_at_ms: row
            .get::<_, Option<i64>>(9)
            .std_context("get expires_at")?
            .map(|v| v as u64),
    })
}

fn row_to_download(row: &rusqlite::Row) -> Result<Download> {
    Ok(Download {
        id: row.get(0).std_context("get id")?,
        content_hash: row.get(1).std_context("get hash")?,
        remote_peer: row.get(2).std_context("get peer")?,
        state: row.get(3).std_context("get state")?,
        bytes_downloaded: row.get::<_, i64>(4).std_context("get bytes_down")? as u64,
        total_bytes: row.get::<_, i64>(5).std_context("get total_bytes")? as u64,
        created_at_ms: row.get::<_, i64>(6).std_context("get created")? as u64,
        updated_at_ms: row.get::<_, i64>(7).std_context("get updated")? as u64,
        last_error: row.get(8).std_context("get error")?,
        retry_count: row.get::<_, i64>(9).std_context("get retries")? as u32,
        next_retry_at_ms: row
            .get::<_, Option<i64>>(10)
            .std_context("get next_retry")?
            .map(|v| v as u64),
    })
}

trait OptionalExt<T> {
    fn optional(self) -> Result<Option<T>>;
}

impl<T> OptionalExt<T> for Result<T, rusqlite::Error> {
    fn optional(self) -> Result<Option<T>> {
        match self {
            Ok(v) => Ok(Some(v)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(e).std_context("query"),
        }
    }
}

// ── Submodules ───────────────────────────────────────────────────────────

mod conversation;
mod identity;
mod reactions;
pub(crate) mod schema;
#[cfg(test)]
mod tests;
mod transfer;
