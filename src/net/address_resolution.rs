//! Deterministic address-resolution policy used before dialing a peer.
//!
//! The resolver deliberately keeps identity (`EndpointId`) separate from
//! transport addresses.  A peer may change addresses without changing its
//! identity, and stale candidates can therefore be replaced safely.

use std::collections::HashSet;

use iroh::{EndpointAddr, EndpointId};

/// Ordered source of an address candidate.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum ResolutionSource {
    Current,
    Persisted,
    Mdns,
    Configured,
    Relay,
    Dht,
    TrustedPeer,
}

/// Stable diagnostic category for resolution failures.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum ResolutionFailureCategory {
    NoCandidates,
    IdentityMismatch,
}

impl ResolutionFailureCategory {
    pub(crate) const fn code(self) -> &'static str {
        match self {
            Self::NoCandidates => "address_resolution.no_candidates",
            Self::IdentityMismatch => "address_resolution.identity_mismatch",
        }
    }
}

/// A candidate together with its provenance.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct AddressCandidate {
    pub(crate) addr: EndpointAddr,
    pub(crate) source: ResolutionSource,
}

/// Result of address resolution. The identity is never inferred from an
/// address; it is always the requested peer id.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct AddressResolution {
    pub(crate) peer: EndpointId,
    pub(crate) candidates: Vec<AddressCandidate>,
    pub(crate) failure: Option<ResolutionFailureCategory>,
}

/// Injected results from each backend. `resolve_candidates` consumes these in
/// policy order rather than backend completion order.
#[derive(Debug, Default, Clone)]
pub(crate) struct ResolutionInputs {
    pub(crate) current: Option<EndpointAddr>,
    pub(crate) persisted: Option<EndpointAddr>,
    pub(crate) mdns: Option<EndpointAddr>,
    pub(crate) configured: Option<EndpointAddr>,
    pub(crate) relay: Option<EndpointAddr>,
    pub(crate) dht: Option<EndpointAddr>,
    pub(crate) trusted_peer: Option<EndpointAddr>,
}

/// Build the address sequence in the documented order. Each input is an
/// injected lookup result, which makes the policy deterministic in tests and
/// keeps the resolver independent of a particular discovery backend.
pub(crate) fn resolve_candidates(peer: EndpointId, inputs: ResolutionInputs) -> AddressResolution {
    let sources = [
        (ResolutionSource::Current, inputs.current),
        (ResolutionSource::Persisted, inputs.persisted),
        (ResolutionSource::Mdns, inputs.mdns),
        (ResolutionSource::Configured, inputs.configured),
        (ResolutionSource::Relay, inputs.relay),
        (ResolutionSource::Dht, inputs.dht),
        (ResolutionSource::TrustedPeer, inputs.trusted_peer),
    ];
    let mut seen = HashSet::new();
    let mut candidates = Vec::new();

    for (source, addr) in sources {
        let Some(addr) = addr else { continue };
        if addr.id != peer {
            return AddressResolution {
                peer,
                candidates,
                failure: Some(ResolutionFailureCategory::IdentityMismatch),
            };
        }
        if seen.insert(addr.clone()) {
            candidates.push(AddressCandidate { addr, source });
        }
    }

    let failure = if candidates.is_empty() {
        Some(ResolutionFailureCategory::NoCandidates)
    } else {
        None
    };
    AddressResolution {
        peer,
        candidates,
        failure,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use iroh::{RelayUrl, SecretKey};
    use std::net::SocketAddr;

    fn peer() -> EndpointId {
        SecretKey::from_bytes(&[7; 32]).public()
    }

    #[test]
    fn follows_resolution_order_and_deduplicates() {
        let peer = peer();
        let direct: SocketAddr = "192.0.2.10:1234".parse().unwrap();
        let relay: RelayUrl = "https://relay.example.test./".parse().unwrap();
        let direct_addr = EndpointAddr::new(peer).with_ip_addr(direct);
        let result = resolve_candidates(
            peer,
            ResolutionInputs {
                current: Some(direct_addr.clone()),
                persisted: Some(direct_addr),
                relay: Some(EndpointAddr::new(peer).with_relay_url(relay)),
                ..Default::default()
            },
        );
        assert_eq!(result.failure, None);
        assert_eq!(result.candidates.len(), 2);
        assert_eq!(result.candidates[0].source, ResolutionSource::Current);
        assert_eq!(result.candidates[1].source, ResolutionSource::Relay);
        assert_eq!(result.peer, peer);
    }

    #[test]
    fn stale_address_can_be_replaced_without_changing_identity() {
        let peer = peer();
        let old: SocketAddr = "192.0.2.1:1".parse().unwrap();
        let new: SocketAddr = "192.0.2.2:2".parse().unwrap();
        let result = resolve_candidates(
            peer,
            ResolutionInputs {
                current: Some(EndpointAddr::new(peer).with_ip_addr(new)),
                persisted: Some(EndpointAddr::new(peer).with_ip_addr(old)),
                ..Default::default()
            },
        );
        assert_eq!(result.peer, peer);
        assert_eq!(result.candidates[0].addr.ip_addrs().next(), Some(&new));
        assert_eq!(result.candidates[0].source, ResolutionSource::Current);
    }

    #[test]
    fn failures_have_stable_categories() {
        let result = resolve_candidates(peer(), ResolutionInputs::default());
        assert_eq!(
            result.failure,
            Some(ResolutionFailureCategory::NoCandidates)
        );
        assert_eq!(
            ResolutionFailureCategory::NoCandidates.code(),
            "address_resolution.no_candidates"
        );
    }

    #[test]
    fn mismatched_candidate_is_rejected() {
        let requested = peer();
        let other = SecretKey::from_bytes(&[8; 32]).public();
        let result = resolve_candidates(
            requested,
            ResolutionInputs {
                configured: Some(EndpointAddr::new(other)),
                ..Default::default()
            },
        );
        assert_eq!(
            result.failure,
            Some(ResolutionFailureCategory::IdentityMismatch)
        );
        assert!(result.candidates.is_empty());
    }
}
