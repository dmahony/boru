//! Global public-room registry on the DHT — relay-independent room discovery.
//!
//! # The gap this fills
//!
//! Boru has two existing discovery surfaces, and both require prior knowledge
//! or a shared relay:
//!
//! * The **gossip directory topic** is derived from the relay URL
//!   ([`directory::directory_topic`](crate::directory::directory_topic)), so
//!   a room advertised there is only visible to peers joined to the **same
//!   relay**.
//! * The **per-room DHT namespace** is derived from the room's *name*
//!   ([`public_room::public_discovery_key`](crate::public_room::public_discovery_key)
//!   → [`discovery_backend::canonical_lobby_key`](crate::discovery_backend::canonical_lobby_key)),
//!   so a peer must already **know a room's name** to derive the lookup key
//!   and find it. There is no way to *enumerate* public rooms over the DHT.
//!
//! Neither surface lets a brand-new peer (with no shared relay and no named
//! room) *browse* the set of public rooms that exist globally.
//!
//! # What this module adds
//!
//! A single **well-known, network-versioned registry namespace**. Every node
//! that owns a `PublicDiscoverable` room publishes a signed
//! [`RoomRegistryEntry`] (room name, gossip topic, join ticket, description,
//! owner) into that one namespace. Any node can then `lookup` the namespace
//! and enumerate the signed entries to build a global, relay-independent room
//! list. This is the "DHT-published room metadata registry" leg of the hybrid
//! discovery design (gossip = fast push-refresh; DHT registry = global
//! fallback/index).
//!
//! # Security
//!
//! * Each entry is a signed [`Record`] bound to the registry namespace and the
//!   coarsely time-boxed unix minute (replay self-limiting).
//! * The entry embeds the publisher's `owner` endpoint id; receivers validate
//!   the signature before trusting any metadata or ticket.
//! * The registry namespace is network-separated ([`PublicNetwork`]), so
//!   mainnet / development / test registries never mix.
//!
//! The registry is **advertisement metadata only** — discovering an entry here
//! never subscribes to a room, downloads history, or grants membership (PDF
//! Core rule). It is purely a browseable index.

use distributed_topic_tracker::Record;
use iroh::{EndpointId, SecretKey};
use n0_error::Result;
use serde::de::{self, SeqAccess, Visitor};
use serde::{Deserialize, Deserializer, Serialize};
use std::collections::{HashMap, HashSet};
use std::fmt;

use crate::discovery_backend::{
    validate_discovery_record, EncryptedDiscoveryRecord, NamespaceId, TopicDiscoveryBackend,
    MAX_DISCOVERY_PAYLOAD_SIZE,
};
use crate::public_room::PublicNetwork;

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// Domain separator for the global public-room registry namespace.
///
/// Deliberately distinct from all other boru-chat domain separators so the
/// registry namespace never collides with a public-room topic, a discovery
/// key, the internal discovery topic, or a relay-scoped directory topic.
pub const ROOM_REGISTRY_DOMAIN: &[u8] = b"boru-chat/public-room-registry/v1";

/// Protocol version of the registry namespace derivation.
///
/// Bump this (and add a new known-answer vector) to change the registry
/// namespace without altering the derivation logic.
pub const ROOM_REGISTRY_PROTOCOL_VERSION: u8 = 1;

/// Wire-format version of [`RoomRegistryEntry`].
///
/// Increment when adding fields to the entry payload.
pub const ROOM_REGISTRY_CONTENT_VERSION: u8 = 1;

/// Maximum encrypted records inspected during one registry lookup.
pub const MAX_ROOM_REGISTRY_RECORDS_EXAMINED: usize = 64;
/// Maximum entries returned to a caller, regardless of backend behavior.
pub const MAX_ROOM_REGISTRY_ENTRIES: usize = 32;
/// Maximum entries admitted from one owner in a lookup result.
pub const MAX_ROOM_REGISTRY_ENTRIES_PER_OWNER: usize = 4;
/// Maximum UTF-8 byte length of a room name.
pub const MAX_ROOM_REGISTRY_ROOM_NAME_BYTES: usize = 128;
/// Maximum UTF-8 byte length of a join ticket.
pub const MAX_ROOM_REGISTRY_TICKET_BYTES: usize = 2048;
/// Maximum UTF-8 byte length of a room description.
pub const MAX_ROOM_REGISTRY_DESCRIPTION_BYTES: usize = 256;

// ---------------------------------------------------------------------------
// Namespace derivation
// ---------------------------------------------------------------------------

/// The well-known global registry namespace for a [`PublicNetwork`].
///
/// `NamespaceId = BLAKE3(ROOM_REGISTRY_DOMAIN || network_byte || version)`
///
/// This is the **single shared rendezvous point** for public-room metadata on
/// that network — independent of relay and independent of any room name — so
/// any node can enumerate all registered rooms by looking it up.
pub fn room_registry_namespace(network: PublicNetwork) -> NamespaceId {
    let key = *blake3::hash(
        &[
            ROOM_REGISTRY_DOMAIN,
            &[network.network_byte()],
            &[ROOM_REGISTRY_PROTOCOL_VERSION],
        ]
        .concat(),
    )
    .as_bytes();
    NamespaceId::new(key)
}

// ---------------------------------------------------------------------------
// Entry
// ---------------------------------------------------------------------------

/// One signed room entry in the global public-room registry.
///
/// Carries just enough metadata to make a room browsable and joinable:
/// the gossip topic (to subscribe), a join ticket (to bootstrap peers), and
/// human-readable name/description/owner for the discover UI.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RoomRegistryEntry {
    /// Wire-format version for forward compatibility.
    version: u8,
    /// 32-byte Ed25519 public key (iroh EndpointId) of the room owner /
    /// publisher.
    owner: [u8; 32],
    /// 32-byte gossip [`TopicId`](crate::proto::TopicId) bytes of the room.
    room_topic: [u8; 32],
    /// Human-readable room name.
    room_name: String,
    /// Join ticket used to bootstrap peers for the room.
    ticket: String,
    /// Optional short description.
    #[serde(default)]
    description: Option<String>,
}

impl<'de> Deserialize<'de> for RoomRegistryEntry {
    fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        struct EntryVisitor;

        impl<'de> Visitor<'de> for EntryVisitor {
            type Value = RoomRegistryEntry;

            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str("a room registry entry")
            }

            fn visit_seq<A>(self, mut seq: A) -> std::result::Result<Self::Value, A::Error>
            where
                A: SeqAccess<'de>,
            {
                let version = seq
                    .next_element()?
                    .ok_or_else(|| de::Error::custom("missing entry version"))?;
                let owner = seq
                    .next_element()?
                    .ok_or_else(|| de::Error::custom("missing owner"))?;
                let room_topic = seq
                    .next_element()?
                    .ok_or_else(|| de::Error::custom("missing room topic"))?;
                let room_name = seq
                    .next_element()?
                    .ok_or_else(|| de::Error::custom("missing room name"))?;
                let ticket = seq
                    .next_element()?
                    .ok_or_else(|| de::Error::custom("missing ticket"))?;
                let description = match seq.next_element::<Option<String>>() {
                    Ok(Some(value)) => value,
                    Ok(None) => None,
                    Err(error) if is_unexpected_end(&error) => None,
                    Err(error) => return Err(error),
                };
                Ok(RoomRegistryEntry {
                    version,
                    owner,
                    room_topic,
                    room_name,
                    ticket,
                    description,
                })
            }
        }

        deserializer.deserialize_tuple(6, EntryVisitor)
    }
}

fn is_unexpected_end<E: fmt::Display>(error: &E) -> bool {
    let text = error.to_string().to_ascii_lowercase();
    text.contains("hit the end") || text.contains("unexpected end")
}

impl RoomRegistryEntry {
    /// Build a new registry entry.
    pub fn new(
        owner: &EndpointId,
        room_topic: [u8; 32],
        room_name: String,
        ticket: String,
        description: Option<String>,
    ) -> Self {
        Self {
            version: ROOM_REGISTRY_CONTENT_VERSION,
            owner: *owner.as_bytes(),
            room_topic,
            room_name,
            ticket,
            description,
        }
    }

    /// Wire-format version of this entry.
    pub fn version(&self) -> u8 {
        self.version
    }

    /// The 32-byte Ed25519 public key of the room owner / publisher.
    pub fn owner(&self) -> [u8; 32] {
        self.owner
    }

    /// The 32-byte gossip topic bytes of the room.
    pub fn room_topic(&self) -> [u8; 32] {
        self.room_topic
    }

    /// Human-readable room name.
    pub fn room_name(&self) -> &str {
        &self.room_name
    }

    /// Join ticket used to bootstrap peers for the room.
    pub fn ticket(&self) -> &str {
        &self.ticket
    }

    /// Optional room description.
    pub fn description(&self) -> Option<&str> {
        self.description.as_deref()
    }
}

// ---------------------------------------------------------------------------
// Publish / lookup
// ---------------------------------------------------------------------------

/// Sign and publish a [`RoomRegistryEntry`] into the global registry namespace
/// for `network`.
///
/// The entry is wrapped in a [`Record`] whose topic is the registry namespace
/// (binding the record to that rendezvous point) and signed with `secret_key`
/// (whose public half is `owner`). The caller must already have validated the
/// metadata against the advertisement bounds before calling this.
pub async fn publish_registry_entry(
    backend: &dyn TopicDiscoveryBackend,
    network: PublicNetwork,
    entry: &RoomRegistryEntry,
    secret_key: &SecretKey,
) -> Result<()> {
    let namespace = room_registry_namespace(network);
    let now = distributed_topic_tracker::unix_minute(0);
    let record = Record::sign(
        *namespace.as_bytes(),
        now,
        entry.clone(),
        secret_key.as_signing_key(),
    )?;
    let encrypted = EncryptedDiscoveryRecord::new(record.to_bytes());
    validate_discovery_record(&encrypted)?;
    backend.publish(&namespace, encrypted).await
}

/// Look up and validate all signed registry entries in the global registry
/// namespace for `network`.
///
/// Decrypted malformed records are skipped (not hard failures). The returned
/// entries have already passed the backend's decryption-and-verification
/// pipeline (signature + topic binding + minute window).
pub async fn lookup_registry(
    backend: &dyn TopicDiscoveryBackend,
    network: PublicNetwork,
) -> Result<Vec<RoomRegistryEntry>> {
    let namespace = room_registry_namespace(network);
    let encrypted = match backend.lookup(&namespace).await {
        Ok(encrypted) => encrypted,
        Err(error) => {
            crate::diagnostics::DHT_COUNTERS.record_lookup_failure();
            return Err(error);
        }
    };
    let examined = encrypted.len().min(MAX_ROOM_REGISTRY_RECORDS_EXAMINED);
    let mut counts = crate::diagnostics::DhtLookupCounts::default();
    let mut entries = Vec::new();
    let mut identities = HashSet::new();
    let mut owner_counts = HashMap::<[u8; 32], usize>::new();
    for er in encrypted
        .into_iter()
        .take(MAX_ROOM_REGISTRY_RECORDS_EXAMINED)
    {
        if er.payload.len() > MAX_DISCOVERY_PAYLOAD_SIZE {
            counts.oversized += 1;
            continue;
        }
        let Ok(record) = Record::from_bytes(er.payload) else {
            counts.decode += 1;
            continue;
        };
        // Only accept records bound to the registry namespace.
        if record.topic() != *namespace.as_bytes() {
            counts.namespace += 1;
            continue;
        }
        let entry = match record.content::<RoomRegistryEntry>() {
            Ok(entry) => entry,
            Err(_) => {
                counts.decode += 1;
                continue;
            }
        };
        if entry.version != ROOM_REGISTRY_CONTENT_VERSION {
            counts.version += 1;
            continue;
        }
        if !valid_registry_metadata(&entry) {
            counts.metadata += 1;
            continue;
        }
        let identity = (entry.owner, entry.room_topic);
        if !identities.insert(identity) {
            counts.duplicate += 1;
            continue;
        }
        let owner_count = owner_counts.entry(entry.owner).or_default();
        if *owner_count >= MAX_ROOM_REGISTRY_ENTRIES_PER_OWNER {
            counts.per_owner += 1;
            continue;
        }
        if entries.len() >= MAX_ROOM_REGISTRY_ENTRIES {
            counts.global_cap += 1;
            continue;
        }
        *owner_count += 1;
        entries.push(entry);
    }
    counts.valid = entries.len() as u64;
    crate::diagnostics::DHT_COUNTERS.record_lookup(examined as u64, counts);
    tracing::debug!(
        target: "boru::room_registry",
        examined,
        returned = entries.len(),
        "global room registry lookup completed"
    );
    Ok(entries)
}

fn valid_registry_metadata(entry: &RoomRegistryEntry) -> bool {
    entry.room_name.as_bytes().len() <= MAX_ROOM_REGISTRY_ROOM_NAME_BYTES
        && entry.ticket.as_bytes().len() <= MAX_ROOM_REGISTRY_TICKET_BYTES
        && entry
            .description
            .as_deref()
            .is_none_or(|description| description.len() <= MAX_ROOM_REGISTRY_DESCRIPTION_BYTES)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use std::sync::Arc;

    #[derive(Clone)]
    struct FixedBackend {
        records: Arc<Vec<EncryptedDiscoveryRecord>>,
    }

    #[async_trait]
    impl TopicDiscoveryBackend for FixedBackend {
        async fn publish(
            &self,
            _namespace: &NamespaceId,
            _record: EncryptedDiscoveryRecord,
        ) -> Result<()> {
            Ok(())
        }

        async fn lookup(&self, _namespace: &NamespaceId) -> Result<Vec<EncryptedDiscoveryRecord>> {
            Ok((*self.records).clone())
        }

        async fn shutdown(&self) -> Result<()> {
            Ok(())
        }
    }

    fn test_identity() -> (SecretKey, EndpointId) {
        let sk = SecretKey::generate();
        let ep = sk.public();
        (sk, ep)
    }

    fn signed_record(
        network: PublicNetwork,
        entry: &RoomRegistryEntry,
        secret_key: &SecretKey,
    ) -> EncryptedDiscoveryRecord {
        let record = Record::sign(
            *room_registry_namespace(network).as_bytes(),
            distributed_topic_tracker::unix_minute(0),
            entry.clone(),
            secret_key.as_signing_key(),
        )
        .unwrap();
        EncryptedDiscoveryRecord::new(record.to_bytes())
    }

    fn fixed_backend(records: Vec<EncryptedDiscoveryRecord>) -> FixedBackend {
        FixedBackend {
            records: Arc::new(records),
        }
    }

    #[test]
    fn lookup_registry_rejects_hostile_records_and_preserves_valid_entries() {
        let (owner_key, owner) = test_identity();
        let (other_key, other_owner) = test_identity();
        let valid = RoomRegistryEntry::new(
            &owner,
            [1; 32],
            "valid".into(),
            "ticket".into(),
            Some("description".into()),
        );
        let duplicate = valid.clone();
        let mut unsupported = valid.clone();
        unsupported.version = ROOM_REGISTRY_CONTENT_VERSION + 1;
        let oversized = EncryptedDiscoveryRecord::new(vec![0; MAX_DISCOVERY_PAYLOAD_SIZE + 1]);
        let wrong_namespace = signed_record(PublicNetwork::Mainnet, &valid, &owner_key);
        let malformed = EncryptedDiscoveryRecord::new(vec![1, 2, 3]);
        let too_long = RoomRegistryEntry::new(
            &other_owner,
            [2; 32],
            "x".repeat(MAX_ROOM_REGISTRY_ROOM_NAME_BYTES + 1),
            "ticket".into(),
            None,
        );
        let records = vec![
            signed_record(PublicNetwork::Test, &valid, &owner_key),
            signed_record(PublicNetwork::Test, &duplicate, &owner_key),
            signed_record(PublicNetwork::Test, &unsupported, &owner_key),
            oversized,
            wrong_namespace,
            malformed,
            signed_record(PublicNetwork::Test, &too_long, &other_key),
        ];
        let backend = fixed_backend(records);
        let rt = tokio::runtime::Runtime::new().unwrap();
        let found = rt
            .block_on(lookup_registry(&backend, PublicNetwork::Test))
            .unwrap();
        assert_eq!(found, vec![valid]);
    }

    #[test]
    fn lookup_registry_bounds_records_results_and_owner_admission() {
        let (owner_key, owner) = test_identity();
        let mut records = Vec::new();
        for topic in 0..(MAX_ROOM_REGISTRY_RECORDS_EXAMINED as u8) {
            let entry = RoomRegistryEntry::new(
                &owner,
                [topic; 32],
                format!("room-{topic}"),
                "ticket".into(),
                None,
            );
            records.push(signed_record(PublicNetwork::Test, &entry, &owner_key));
        }
        let backend = fixed_backend(records);
        let rt = tokio::runtime::Runtime::new().unwrap();
        let found = rt
            .block_on(lookup_registry(&backend, PublicNetwork::Test))
            .unwrap();
        assert_eq!(found.len(), MAX_ROOM_REGISTRY_ENTRIES_PER_OWNER);
        assert!(found
            .windows(2)
            .all(|pair| pair[0].room_topic() < pair[1].room_topic()));
    }

    #[test]
    fn registry_namespace_is_deterministic_and_network_separated() {
        for network in [
            PublicNetwork::Mainnet,
            PublicNetwork::Development,
            PublicNetwork::Test,
        ] {
            let a = room_registry_namespace(network);
            let b = room_registry_namespace(network);
            assert_eq!(a.as_bytes(), b.as_bytes(), "{network:?}");
        }
        let mainnet = room_registry_namespace(PublicNetwork::Mainnet);
        let dev = room_registry_namespace(PublicNetwork::Development);
        let test = room_registry_namespace(PublicNetwork::Test);
        assert_ne!(mainnet.as_bytes(), dev.as_bytes());
        assert_ne!(mainnet.as_bytes(), test.as_bytes());
        assert_ne!(dev.as_bytes(), test.as_bytes());
    }

    #[test]
    fn registry_namespace_is_nonzero() {
        let ns = room_registry_namespace(PublicNetwork::Mainnet);
        assert!(ns.as_bytes().iter().any(|&b| b != 0));
    }

    #[test]
    fn entry_roundtrip_preserves_fields() {
        let (_sk, ep) = test_identity();
        let entry = RoomRegistryEntry::new(
            &ep,
            [7u8; 32],
            "cool-room".to_owned(),
            "ticket-123".to_owned(),
            Some("a description".to_owned()),
        );
        let encoded = postcard::to_allocvec(&entry).unwrap();
        let decoded: RoomRegistryEntry = postcard::from_bytes(&encoded).unwrap();
        assert_eq!(decoded, entry);
        assert_eq!(decoded.room_name(), "cool-room");
        assert_eq!(decoded.ticket(), "ticket-123");
        assert_eq!(decoded.description(), Some("a description"));
        assert_eq!(decoded.room_topic(), [7u8; 32]);
    }

    #[test]
    fn entry_decodes_without_description() {
        let (_sk, ep) = test_identity();
        let entry =
            RoomRegistryEntry::new(&ep, [1u8; 32], "r".to_owned(), "t".to_owned(), None);
        let encoded = postcard::to_allocvec(&entry).unwrap();
        let decoded: RoomRegistryEntry = postcard::from_bytes(&encoded).unwrap();
        assert_eq!(decoded.description(), None);
    }

    #[test]
    fn publish_and_lookup_roundtrip_across_peers() {
        let backend = crate::discovery_backend::InMemoryDiscoveryBackend::new();
        let d: &dyn TopicDiscoveryBackend = &backend;
        let (sk_a, ep_a) = test_identity();

        let entry = RoomRegistryEntry::new(
            &ep_a,
            [42u8; 32],
            "global-room".to_owned(),
            "ticket-live".to_owned(),
            None,
        );
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(async {
            publish_registry_entry(d, PublicNetwork::Test, &entry, &sk_a)
                .await
                .unwrap();
            let found = lookup_registry(d, PublicNetwork::Test).await.unwrap();
            assert_eq!(found.len(), 1);
            assert_eq!(found[0].room_name(), "global-room");
            assert_eq!(found[0].ticket(), "ticket-live");
            assert_eq!(found[0].owner(), *ep_a.as_bytes());
        });
    }

    #[test]
    fn lookup_filters_other_namespaces() {
        let backend = crate::discovery_backend::InMemoryDiscoveryBackend::new();
        let d: &dyn TopicDiscoveryBackend = &backend;
        let (sk_a, ep_a) = test_identity();
        let entry = RoomRegistryEntry::new(
            &ep_a,
            [9u8; 32],
            "only-mine".to_owned(),
            "t".to_owned(),
            None,
        );
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(async {
            // Publish into mainnet namespace.
            publish_registry_entry(d, PublicNetwork::Mainnet, &entry, &sk_a)
                .await
                .unwrap();
            // Test namespace must be empty (network separation).
            let found = lookup_registry(d, PublicNetwork::Test).await.unwrap();
            assert!(found.is_empty());
        });
    }
}
