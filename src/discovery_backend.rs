//! Discovery backend abstraction for testable DHT operations.
//!
//! Defines the [`TopicDiscoveryBackend`] trait with `publish`, `lookup`, and
//! `shutdown` methods, along with a production (`MainlineDhtBackend`, gated
//! behind the `net` feature) and a deterministic in-memory mock
//! (`InMemoryDiscoveryBackend`) implementation that share the same
//! validation path.

use async_trait::async_trait;
use n0_error::{ensure_any, Result};

/// Maximum number of records returned by a single [`TopicDiscoveryBackend::lookup`] call.
pub const MAX_DISCOVERY_RECORDS: usize = 20;

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

/// A 32-byte namespace identifier derived from a gossip topic.
///
/// This is the key under which discovery records are published and looked up.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct NamespaceId([u8; 32]);

impl NamespaceId {
    /// Create a new [`NamespaceId`] from raw bytes.
    pub fn new(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }

    /// Return a reference to the underlying 32-byte identifier.
    pub fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }
}

impl From<[u8; 32]> for NamespaceId {
    fn from(bytes: [u8; 32]) -> Self {
        Self::new(bytes)
    }
}

/// An opaque encrypted discovery record.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EncryptedDiscoveryRecord {
    /// The encrypted payload bytes.
    pub payload: Vec<u8>,
}

impl EncryptedDiscoveryRecord {
    /// Create a new [`EncryptedDiscoveryRecord`] with the given payload.
    pub fn new(payload: Vec<u8>) -> Self {
        Self { payload }
    }
}

/// Validate a discovery record — rejects empty payloads.
pub fn validate_discovery_record(record: &EncryptedDiscoveryRecord) -> Result<()> {
    ensure_any!(
        !record.payload.is_empty(),
        "discovery record payload must not be empty"
    );
    Ok(())
}

/// Maximum serialized discovery-record payload size (before encryption).
///
/// Native `distributed-topic-tracker::EncryptedRecord` envelopes include an
/// HPKE-wrapped key and are bounded by the crate's 2048-byte wire limit.
pub const MAX_DISCOVERY_PAYLOAD_SIZE: usize = 2048;

/// Trait abstracting DHT-like topic discovery operations.
///
/// Implementations can be backed by a real DHT (e.g. MainlineDht) or
/// by an in-memory store for testing.
#[async_trait]
pub trait TopicDiscoveryBackend: Send + Sync + 'static {
    /// Publish a discovery record under the given namespace.
    async fn publish(
        &self,
        namespace: &NamespaceId,
        record: EncryptedDiscoveryRecord,
    ) -> Result<()>;

    /// Look up discovery records published under the given namespace.
    async fn lookup(&self, namespace: &NamespaceId) -> Result<Vec<EncryptedDiscoveryRecord>>;

    /// Shut down the backend, releasing any resources.
    async fn shutdown(&self) -> Result<()>;
}

use std::collections::HashMap;
use std::sync::{Arc, RwLock};

/// In-memory implementation of [`TopicDiscoveryBackend`] for testing.
///
/// Stores records in a `HashMap` protected by `RwLock`.  All operations
/// are synchronous internally but exposed through the async trait.
#[derive(Debug, Clone, Default)]
pub struct InMemoryDiscoveryBackend {
    records: Arc<RwLock<HashMap<NamespaceId, Vec<EncryptedDiscoveryRecord>>>>,
}

impl InMemoryDiscoveryBackend {
    /// Create an empty in-memory discovery backend.
    pub fn new() -> Self {
        Self::default()
    }

    /// Number of distinct namespaces with stored records.
    pub fn namespace_count(&self) -> usize {
        self.records.read().expect("lock poisoned").len()
    }

    /// Total number of records across all namespaces.
    pub fn total_record_count(&self) -> usize {
        self.records
            .read()
            .expect("lock poisoned")
            .values()
            .map(|v| v.len())
            .sum()
    }

    /// Remove all records stored under the given namespace.
    pub fn clear_namespace(&self, namespace: &NamespaceId) {
        self.records
            .write()
            .expect("lock poisoned")
            .remove(namespace);
    }

    /// Remove all records across every namespace.
    pub fn clear_all(&self) {
        self.records.write().expect("lock poisoned").clear();
    }
}

#[async_trait]
impl TopicDiscoveryBackend for InMemoryDiscoveryBackend {
    async fn publish(
        &self,
        namespace: &NamespaceId,
        record: EncryptedDiscoveryRecord,
    ) -> Result<()> {
        validate_discovery_record(&record)?;
        let mut map = self.records.write().expect("lock poisoned");
        map.entry(*namespace).or_default().push(record);
        Ok(())
    }

    async fn lookup(&self, namespace: &NamespaceId) -> Result<Vec<EncryptedDiscoveryRecord>> {
        let map = self.records.read().expect("lock poisoned");
        let records = map.get(namespace).cloned().unwrap_or_default();
        let mut records = records;
        records.reverse();
        records.truncate(MAX_DISCOVERY_RECORDS);
        Ok(records)
    }

    async fn shutdown(&self) -> Result<()> {
        Ok(())
    }
}

/// DHT-backed implementation of [`TopicDiscoveryBackend`] using the
/// `distributed-topic-tracker` crate.  Only available with the `net` feature.
#[cfg(feature = "net")]
pub struct MainlineDhtBackend {
    dht: distributed_topic_tracker::Dht,
    #[allow(dead_code)]
    default_namespace: distributed_topic_tracker::TopicId,
}

#[cfg(feature = "net")]
impl MainlineDhtBackend {
    /// Create a new DHT-backed discovery backend.
    pub fn new(
        dht: distributed_topic_tracker::Dht,
        default_namespace: distributed_topic_tracker::TopicId,
    ) -> Self {
        Self {
            dht,
            default_namespace,
        }
    }

    fn topic_id_for(&self, namespace: &NamespaceId) -> distributed_topic_tracker::TopicId {
        distributed_topic_tracker::TopicId::from_hash(namespace.as_bytes())
    }
}

#[cfg(feature = "net")]
#[async_trait]
impl TopicDiscoveryBackend for MainlineDhtBackend {
    async fn publish(
        &self,
        namespace: &NamespaceId,
        record: EncryptedDiscoveryRecord,
    ) -> Result<()> {
        validate_discovery_record(&record)?;
        let topic_id = self.topic_id_for(namespace);
        let unix_minute = distributed_topic_tracker::unix_minute(0);
        let signing_key = distributed_topic_tracker::signing_keypair(&topic_id, unix_minute);
        let salt = distributed_topic_tracker::salt(&topic_id, unix_minute);
        self.dht
            .put_mutable(
                signing_key,
                Some(salt.to_vec()),
                record.payload,
                unix_minute as i64,
            )
            .await?;
        Ok(())
    }

    async fn lookup(&self, namespace: &NamespaceId) -> Result<Vec<EncryptedDiscoveryRecord>> {
        let topic_id = self.topic_id_for(namespace);
        let now = distributed_topic_tracker::unix_minute(0);
        let prev = now.saturating_sub(1);
        let mut all_records = Vec::new();
        for unix_minute in [prev, now] {
            let signing_key = distributed_topic_tracker::signing_keypair(&topic_id, unix_minute);
            let pub_key = signing_key.verifying_key();
            let salt = distributed_topic_tracker::salt(&topic_id, unix_minute);
            let items = self.dht.get(pub_key, Some(salt.to_vec()), None).await?;
            for item in items {
                all_records.push(EncryptedDiscoveryRecord::new(item.value().to_vec()));
            }
        }
        all_records.truncate(MAX_DISCOVERY_RECORDS);
        Ok(all_records)
    }

    async fn shutdown(&self) -> Result<()> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_validate_empty_payload_rejected() {
        let record = EncryptedDiscoveryRecord::new(vec![]);
        let result = validate_discovery_record(&record);
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("must not be empty"));
    }

    #[test]
    fn test_validate_non_empty_accepted() {
        let record = EncryptedDiscoveryRecord::new(vec![1, 2, 3]);
        assert!(validate_discovery_record(&record).is_ok());
    }

    fn run_async<F: std::future::Future<Output = ()>>(f: F) {
        tokio::runtime::Runtime::new().unwrap().block_on(f);
    }

    #[test]
    fn test_in_memory_publish_and_lookup() {
        let backend = InMemoryDiscoveryBackend::new();
        let ns = NamespaceId::new([0u8; 32]);
        let record = EncryptedDiscoveryRecord::new(vec![1, 2, 3, 4]);
        run_async(async {
            backend.publish(&ns, record.clone()).await.unwrap();
            let results = backend.lookup(&ns).await.unwrap();
            assert_eq!(results.len(), 1);
            assert_eq!(results[0], record);
        });
    }

    #[test]
    fn test_in_memory_lookup_unknown() {
        let backend = InMemoryDiscoveryBackend::new();
        run_async(async {
            let results = backend.lookup(&NamespaceId::new([0u8; 32])).await.unwrap();
            assert!(results.is_empty());
        });
    }

    #[test]
    fn test_in_memory_multiple_namespaces() {
        let backend = InMemoryDiscoveryBackend::new();
        let ns_a = NamespaceId::new([1u8; 32]);
        let ns_b = NamespaceId::new([2u8; 32]);
        run_async(async {
            backend
                .publish(&ns_a, EncryptedDiscoveryRecord::new(vec![1]))
                .await
                .unwrap();
            backend
                .publish(&ns_b, EncryptedDiscoveryRecord::new(vec![2]))
                .await
                .unwrap();
            backend
                .publish(&ns_b, EncryptedDiscoveryRecord::new(vec![3]))
                .await
                .unwrap();
        });
        assert_eq!(backend.namespace_count(), 2);
        assert_eq!(backend.total_record_count(), 3);
    }

    #[test]
    fn test_in_memory_records_bounded() {
        let backend = InMemoryDiscoveryBackend::new();
        run_async(async {
            for i in 0..MAX_DISCOVERY_RECORDS + 5 {
                backend
                    .publish(
                        &NamespaceId::new([0u8; 32]),
                        EncryptedDiscoveryRecord::new(vec![i as u8]),
                    )
                    .await
                    .unwrap();
            }
            let results = backend.lookup(&NamespaceId::new([0u8; 32])).await.unwrap();
            assert_eq!(results.len(), MAX_DISCOVERY_RECORDS);
        });
    }

    #[test]
    fn test_in_memory_empty_payload_rejected() {
        run_async(async {
            let result = InMemoryDiscoveryBackend::new()
                .publish(
                    &NamespaceId::new([0u8; 32]),
                    EncryptedDiscoveryRecord::new(vec![]),
                )
                .await;
            assert!(result.is_err());
        });
    }

    #[test]
    fn test_in_memory_clear_ops() {
        let backend = InMemoryDiscoveryBackend::new();
        let ns = NamespaceId::new([0u8; 32]);
        run_async(async {
            backend
                .publish(&ns, EncryptedDiscoveryRecord::new(vec![1]))
                .await
                .unwrap();
        });
        assert_eq!(backend.total_record_count(), 1);
        backend.clear_namespace(&ns);
        assert_eq!(backend.total_record_count(), 0);
    }

    #[test]
    fn test_in_memory_clear_all() {
        let backend = InMemoryDiscoveryBackend::new();
        run_async(async {
            backend
                .publish(
                    &NamespaceId::new([1u8; 32]),
                    EncryptedDiscoveryRecord::new(vec![1]),
                )
                .await
                .unwrap();
            backend
                .publish(
                    &NamespaceId::new([2u8; 32]),
                    EncryptedDiscoveryRecord::new(vec![2]),
                )
                .await
                .unwrap();
        });
        assert_eq!(backend.total_record_count(), 2);
        backend.clear_all();
        assert_eq!(backend.total_record_count(), 0);
    }

    #[test]
    fn test_in_memory_shutdown() {
        run_async(async {
            assert!(InMemoryDiscoveryBackend::new().shutdown().await.is_ok());
        });
    }

    #[test]
    fn test_in_memory_send_sync() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<InMemoryDiscoveryBackend>();
    }

    #[test]
    fn test_in_memory_trait_object() {
        let backend: Arc<dyn TopicDiscoveryBackend> = Arc::new(InMemoryDiscoveryBackend::new());
        run_async(async {
            backend
                .publish(
                    &NamespaceId::new([0u8; 32]),
                    EncryptedDiscoveryRecord::new(vec![42]),
                )
                .await
                .unwrap();
            let results = backend.lookup(&NamespaceId::new([0u8; 32])).await.unwrap();
            assert_eq!(results.len(), 1);
        });
    }

    #[test]
    fn test_namespace_id_basics() {
        let bytes = [1u8; 32];
        let ns = NamespaceId::new(bytes);
        assert_eq!(ns.as_bytes(), &bytes);
        let a = NamespaceId::new([0u8; 32]);
        let b = NamespaceId::new([0u8; 32]);
        let c = NamespaceId::new([1u8; 32]);
        assert_eq!(a, b);
        assert_ne!(a, c);
    }

    #[test]
    fn test_encrypted_record_new() {
        let payload = vec![10, 20, 30];
        let record = EncryptedDiscoveryRecord::new(payload.clone());
        assert_eq!(record.payload, payload);
    }
}
