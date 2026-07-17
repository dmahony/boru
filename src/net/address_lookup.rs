//! An address lookup service to gather addressing info collected from gossip Join and ForwardJoin messages.

use std::{
    collections::{BTreeMap, btree_map::Entry},
    sync::{Arc, Mutex, RwLock},
    time::Duration,
};

use iroh::EndpointAddr;
use iroh::address_lookup::{self, AddressLookup, EndpointData, EndpointInfo};
use iroh_base::{EndpointId, TransportAddr};
use n0_future::{
    boxed::BoxStream,
    stream::{self, StreamExt},
    task::AbortOnDropHandle,
    time::SystemTime,
};

use crate::friends::{FriendId, FriendsStore};

pub(crate) struct RetentionOpts {
    /// How long to keep received endpoint info records alive before pruning them
    retention: Duration,
    /// How often to check for expired entries
    evict_interval: Duration,
}

impl Default for RetentionOpts {
    fn default() -> Self {
        Self {
            retention: Duration::from_secs(60 * 5),
            evict_interval: Duration::from_secs(30),
        }
    }
}

/// An address lookup service that expires endpoints after some time.
#[derive(Debug, Clone)]
pub(crate) struct GossipAddressLookup {
    endpoints: NodeMap,
    friends: Option<Arc<Mutex<FriendsStore>>>,
    _task_handle: Arc<AbortOnDropHandle<()>>,
}

type NodeMap = Arc<RwLock<BTreeMap<EndpointId, StoredEndpointInfo>>>;

#[derive(Debug)]
struct StoredEndpointInfo {
    data: EndpointData,
    last_updated: SystemTime,
}

impl Default for GossipAddressLookup {
    fn default() -> Self {
        Self::new()
    }
}

impl GossipAddressLookup {
    const PROVENANCE: &'static str = "gossip";

    pub(crate) fn new() -> Self {
        Self::with_opts(Default::default())
    }

    pub(crate) fn with_opts(opts: RetentionOpts) -> Self {
        let endpoints: NodeMap = Default::default();
        let task = {
            let endpoints = Arc::downgrade(&endpoints);
            n0_future::task::spawn(async move {
                let mut interval = n0_future::time::interval(opts.evict_interval);
                loop {
                    interval.tick().await;
                    let Some(endpoints) = endpoints.upgrade() else {
                        break;
                    };
                    let now = SystemTime::now();
                    endpoints.write().expect("poisoned").retain(|_k, v| {
                        let age = now.duration_since(v.last_updated).unwrap_or(Duration::MAX);
                        age <= opts.retention
                    });
                }
            })
        };
        Self {
            endpoints,
            friends: None,
            _task_handle: Arc::new(AbortOnDropHandle::new(task)),
        }
    }

    pub(crate) fn with_friends(opts: RetentionOpts, friends: Arc<Mutex<FriendsStore>>) -> Self {
        let mut lookup = Self::with_opts(opts);
        lookup.friends = Some(friends);
        lookup
    }

    pub(crate) fn add(&self, endpoint_info: impl Into<EndpointInfo>) {
        let last_updated = SystemTime::now();
        let EndpointInfo { endpoint_id, data } = endpoint_info.into();
        if let Some(friends) = &self.friends {
            let mut addr = EndpointAddr::new(endpoint_id);
            for transport_addr in data.addrs() {
                match transport_addr {
                    TransportAddr::Ip(ip) => addr = addr.with_ip_addr(*ip),
                    TransportAddr::Relay(relay) => addr = addr.with_relay_url(relay.clone()),
                    _ => {}
                }
            }
            let mut friends = friends.lock().expect("poisoned");
            let id = FriendId::from_public_key(endpoint_id);
            let changed = friends
                .get_mut(&id)
                .map(|record| {
                    let before = record.known_addrs.clone();
                    record.record_addrs([addr]);
                    before != record.known_addrs
                })
                .unwrap_or(false);
            if changed {
                let _ = friends.save();
            }
        }
        let mut guard = self.endpoints.write().expect("poisoned");
        match guard.entry(endpoint_id) {
            Entry::Occupied(mut entry) => {
                let existing = entry.get_mut();
                existing.data.add_addrs(data.addrs().cloned());
                existing.data.set_user_data(data.user_data().cloned());
                existing.last_updated = last_updated;
            }
            Entry::Vacant(entry) => {
                entry.insert(StoredEndpointInfo { data, last_updated });
            }
        }
    }

    pub(crate) fn endpoint_addr(&self, endpoint_id: EndpointId) -> Option<EndpointAddr> {
        let guard = self.endpoints.read().expect("poisoned");
        let info = guard.get(&endpoint_id)?;
        let mut addr = EndpointAddr::new(endpoint_id);
        for transport_addr in info.data.addrs() {
            match transport_addr {
                TransportAddr::Ip(ip) => addr = addr.with_ip_addr(*ip),
                TransportAddr::Relay(relay) => addr = addr.with_relay_url(relay.clone()),
                _ => {}
            }
        }
        Some(addr)
    }
}

impl AddressLookup for GossipAddressLookup {
    fn resolve(
        &self,
        endpoint_id: EndpointId,
    ) -> Option<BoxStream<Result<address_lookup::Item, address_lookup::Error>>> {
        let guard = self.endpoints.read().expect("poisoned");
        let info = guard.get(&endpoint_id)?;
        let last_updated = info
            .last_updated
            .duration_since(SystemTime::UNIX_EPOCH)
            .expect("time drift")
            .as_micros() as u64;
        let item = address_lookup::Item::new(
            EndpointInfo::from_parts(endpoint_id, info.data.clone()),
            Self::PROVENANCE,
            Some(last_updated),
        );
        Some(stream::iter(Some(Ok(item))).boxed())
    }
}

#[cfg(test)]
mod tests {
    use super::{GossipAddressLookup, RetentionOpts};
    use crate::friends::{FriendId, FriendRecord, FriendsStore};
    use iroh::{EndpointAddr, SecretKey, address_lookup::AddressLookup};
    use n0_future::StreamExt;
    use rand::{RngExt, SeedableRng};
    use std::sync::{Arc, Mutex};
    use std::time::Duration;

    #[tokio::test]
    async fn known_friend_address_is_written_through() {
        let dir = tempfile::tempdir().expect("tempdir");
        let peer = SecretKey::generate().public();
        let mut store = FriendsStore::empty_at(dir.path());
        store.upsert(FriendId::from_public_key(peer), FriendRecord::default());
        let shared = Arc::new(Mutex::new(store));
        let lookup = GossipAddressLookup::with_friends(Default::default(), Arc::clone(&shared));
        let addr = EndpointAddr::new(peer).with_ip_addr("127.0.0.1:1234".parse().unwrap());
        lookup.add(addr.clone());
        let store = shared.lock().unwrap();
        assert_eq!(
            store
                .get(&FriendId::from_public_key(peer))
                .unwrap()
                .known_addrs,
            vec![addr]
        );
        assert!(store.file_path().exists());
    }

    #[tokio::test]
    async fn unknown_peer_is_not_added_to_friends_store() {
        let dir = tempfile::tempdir().expect("tempdir");
        let peer = SecretKey::generate().public();
        let shared = Arc::new(Mutex::new(FriendsStore::empty_at(dir.path())));
        let lookup = GossipAddressLookup::with_friends(Default::default(), Arc::clone(&shared));
        lookup.add(EndpointAddr::new(peer));
        let store = shared.lock().unwrap();
        assert!(store.is_empty());
        assert!(!store.file_path().exists());
    }

    #[tokio::test]
    async fn test_retention() {
        let opts = RetentionOpts {
            evict_interval: Duration::from_millis(100),
            retention: Duration::from_millis(500),
        };
        let disco = GossipAddressLookup::with_opts(opts);
        let rng = &mut rand::rngs::ChaCha12Rng::seed_from_u64(1);
        let k1 = SecretKey::from_bytes(&rng.random());
        let a1 = EndpointAddr::new(k1.public());
        disco.add(a1);
        assert!(matches!(
            disco.resolve(k1.public()).unwrap().next().await,
            Some(Ok(_))
        ));
        tokio::time::sleep(Duration::from_millis(200)).await;
        assert!(matches!(
            disco.resolve(k1.public()).unwrap().next().await,
            Some(Ok(_))
        ));
        tokio::time::sleep(Duration::from_millis(700)).await;
        assert!(disco.resolve(k1.public()).is_none());
    }
}
