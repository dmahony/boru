//! Bootstrap peer resolution helpers.
//!
//! Collects and deduplicates endpoint addresses from tickets and room stores,
//! seeds the endpoint's memory lookup, and refreshes stored peers after a
//! successful join.  Extracted from `chat_core`.

use std::collections::HashSet;

use iroh::{EndpointAddr, EndpointId};


/// Collect unique bootstrap peer IDs from multiple address sources, preserving
/// the EndpointAddr information for seeding the endpoint address lookup.
///
/// Takes multiple slices of [`EndpointAddr`] values (e.g. from a ticket and
/// from a RoomStore), deduplicates them, and returns the peer IDs (for
/// `subscribe_and_join`) plus the full addresses (for seeding a MemoryLookup).
pub fn collect_bootstrap_peers(
    sources: impl IntoIterator<Item = impl AsRef<[EndpointAddr]>>,
) -> (Vec<EndpointId>, Vec<EndpointAddr>) {
    let mut seen_ids = HashSet::new();
    let mut peer_ids = Vec::new();
    let mut all_addrs = Vec::new();
    let mut seen_addrs = HashSet::new();

    for source in sources {
        for addr in source.as_ref() {
            if seen_ids.insert(addr.id) {
                peer_ids.push(addr.id);
            }
            if seen_addrs.insert(addr.id) {
                all_addrs.push(addr.clone());
            }
        }
    }

    (peer_ids, all_addrs)
}

/// Merge bootstrap peer addresses from a new invitation with any addresses we
/// already know for the peer, deduplicating by endpoint id.
///
/// This keeps relay-only invitations usable: if the incoming invitation has no
/// hints, we retain the previously stored peer metadata instead of replacing it
/// with an empty list.
pub fn merge_bootstrap_peer_addrs(
    existing: &[EndpointAddr],
    incoming: &[EndpointAddr],
) -> Vec<EndpointAddr> {
    collect_bootstrap_peers([incoming, existing]).1
}

/// Seed an [`iroh::address_lookup::memory::MemoryLookup`] with every
/// [`EndpointAddr`] from a deduplicated address list, so that
/// `endpoint.connect()` can resolve the peers by their addresses.
///
/// Call this **before** `subscribe_and_join()` so the address resolution
/// chain has the ticket/room-store peer addresses available.
pub fn seed_memory_lookup(
    memory_lookup: &iroh::address_lookup::memory::MemoryLookup,
    addrs: &[EndpointAddr],
) {
    for addr in addrs {
        memory_lookup.set_endpoint_info(addr.clone());
    }
}

/// Refresh the stored bootstrap peers in a [`RoomStore`](crate::room::RoomStore) using the
/// endpoint's current remote info for a set of known peer IDs.
///
/// Call this **after** joining a room so that future reconnections
/// have up-to-date address information, even if the original ticket
/// creator is offline.
///
/// Returns `true` if the peers list changed.
pub async fn refresh_bootstrap_peers(
    room_store: &mut crate::room::RoomStore,
    peer_ids: &std::collections::HashSet<iroh::PublicKey>,
    endpoint: &iroh::Endpoint,
) -> bool {
    let mut refreshed: Vec<iroh::EndpointAddr> = Vec::new();
    let mut seen = std::collections::HashSet::new();

    for pk in peer_ids {
        if !seen.insert(*pk) {
            continue;
        }
        if let Some(info) = endpoint.remote_info(*pk).await {
            let addr =
                iroh::EndpointAddr::from_parts(info.id(), info.into_addrs().map(|a| a.into_addr()));
            refreshed.push(addr);
        }
    }

    if refreshed.is_empty() {
        return false;
    }

    let changed = room_store.peers != refreshed;
    if changed {
        room_store.peers = refreshed;
    }
    changed
}

