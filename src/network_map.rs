//! Pure derived state for the live Network Status map.
//!
//! The map consumes this projection rather than reaching into the endpoint,
//! persistence, or GeoIP layers while rendering. Presence remains the source
//! of truth for online nodes; a node without coordinates is still counted.

use std::collections::BTreeSet;
use std::time::Instant;

use iroh_base::PublicKey;

use crate::control_plane::privacy::{PeerControlState, PeerControlStateStore};

/// One active node that has a valid coarse coordinate pair.
#[derive(Debug, Clone, PartialEq)]
pub struct NetworkMapPoint {
    /// Stable identity of the node represented by this point.
    pub node_id: PublicKey,
    /// Coarse latitude in degrees.
    pub latitude: f64,
    /// Coarse longitude in degrees.
    pub longitude: f64,
}

/// Derived state consumed by the Network Status card.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct NetworkMapState {
    /// Coordinate-bearing points, one per active node.
    pub points: Vec<NetworkMapPoint>,
    /// Number of active presence records, including records without points.
    pub nodes_online: usize,
    /// Number of distinct non-empty country codes in active presence metadata.
    pub countries: usize,
    /// Number of distinct non-empty ASNs in active presence metadata.
    pub networks: usize,
}

impl NetworkMapState {
    /// Derive the map projection from the active presence records at `now`.
    ///
    /// The presence store can briefly retain a record until its expiry sweep;
    /// stale records are therefore filtered here as well as removed by the
    /// sweep. The supplied time makes this function deterministic and easy to
    /// test.
    pub fn from_presence(store: &PeerControlStateStore, now: Instant) -> Self {
        let mut state = Self::default();
        let mut countries = BTreeSet::new();
        let mut networks = BTreeSet::new();

        for (node_id, record) in store.peers() {
            if record.is_stale(now) {
                continue;
            }
            state.nodes_online += 1;
            add_record(&mut state.points, *node_id, record);
            if let Some(country) = normalized_country(record) {
                countries.insert(country);
            }
            if let Some(asn) = record.coarse.as_ref().and_then(|coarse| coarse.asn) {
                networks.insert(asn);
            }
        }

        state.countries = countries.len();
        state.networks = networks.len();
        state.points.sort_by_key(|point| point.node_id);
        state
    }
}

fn add_record(points: &mut Vec<NetworkMapPoint>, node_id: PublicKey, record: &PeerControlState) {
    let Some(coarse) = record.coarse.as_ref() else {
        return;
    };
    let (Some(latitude), Some(longitude)) = (coarse.latitude, coarse.longitude) else {
        return;
    };
    if latitude.is_finite()
        && longitude.is_finite()
        && (-90.0..=90.0).contains(&latitude)
        && (-180.0..=180.0).contains(&longitude)
    {
        points.push(NetworkMapPoint {
            node_id,
            latitude,
            longitude,
        });
    }
}

fn normalized_country(record: &PeerControlState) -> Option<String> {
    let country = record.coarse.as_ref()?.country_code.as_deref()?.trim();
    (!country.is_empty()).then(|| country.to_ascii_uppercase())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::control_plane::message::{CoarsePresence, ControlEnvelope};
    use std::time::Duration;

    fn key(byte: u8) -> PublicKey {
        let mut seed = [0u8; 32];
        seed[0] = byte;
        iroh_base::SecretKey::from_bytes(&seed).public()
    }

    fn presence(
        node_id: PublicKey,
        sequence: u64,
        country_code: Option<&str>,
        latitude: Option<f64>,
        longitude: Option<f64>,
        asn: Option<u32>,
    ) -> ControlEnvelope {
        ControlEnvelope::presence_with_coarse(
            node_id,
            sequence,
            1_700_000_000,
            None,
            Some(CoarsePresence {
                country_code: country_code.map(str::to_owned),
                latitude,
                longitude,
                asn,
            }),
        )
    }

    #[test]
    fn deduplicates_refreshes_by_node_and_counts_distinct_metadata() {
        let now = Instant::now();
        let mut store = PeerControlStateStore::with_limits(16, Duration::from_secs(60));
        let node = key(1);
        assert_eq!(
            store.record(
                &presence(node, 1, Some("us"), Some(1.0), Some(2.0), Some(42)),
                now
            ),
            crate::control_plane::privacy::StoreOutcome::New
        );
        assert_eq!(
            store.record(
                &presence(node, 2, Some("US"), Some(1.0), Some(2.0), Some(42)),
                now + Duration::from_secs(1)
            ),
            crate::control_plane::privacy::StoreOutcome::Refreshed
        );
        let state = NetworkMapState::from_presence(&store, now + Duration::from_secs(1));
        assert_eq!(state.nodes_online, 1);
        assert_eq!(state.points.len(), 1);
        assert_eq!(state.countries, 1);
        assert_eq!(state.networks, 1);
    }

    #[test]
    fn missing_coordinates_stay_online_without_a_point() {
        let now = Instant::now();
        let mut store = PeerControlStateStore::with_limits(16, Duration::from_secs(60));
        store.record(&presence(key(2), 1, Some("CA"), None, None, Some(7)), now);
        let state = NetworkMapState::from_presence(&store, now);
        assert_eq!(state.nodes_online, 1);
        assert!(state.points.is_empty());
        assert_eq!((state.countries, state.networks), (1, 1));
    }

    #[test]
    fn expired_nodes_are_excluded_from_points_and_statistics() {
        let now = Instant::now();
        let mut store = PeerControlStateStore::with_limits(16, Duration::from_secs(5));
        store.record(
            &presence(key(3), 1, Some("DE"), Some(1.0), Some(2.0), Some(9)),
            now,
        );
        let state = NetworkMapState::from_presence(&store, now + Duration::from_secs(5));
        assert_eq!(state, NetworkMapState::default());
    }

    #[test]
    fn empty_country_and_missing_asn_do_not_count() {
        let now = Instant::now();
        let mut store = PeerControlStateStore::with_limits(16, Duration::from_secs(60));
        store.record(
            &presence(key(4), 1, Some("  "), Some(1.0), Some(2.0), None),
            now,
        );
        store.record(
            &presence(key(5), 1, Some("GB"), Some(3.0), Some(4.0), Some(0)),
            now,
        );
        store.record(
            &presence(key(6), 1, Some("gb"), Some(5.0), Some(6.0), Some(0)),
            now,
        );
        let state = NetworkMapState::from_presence(&store, now);
        assert_eq!(state.nodes_online, 3);
        assert_eq!(state.points.len(), 3);
        assert_eq!(state.countries, 1);
        assert_eq!(state.networks, 1);
    }

    #[test]
    fn identical_coordinates_preserve_one_point_per_online_node() {
        let now = Instant::now();
        let mut store = PeerControlStateStore::with_limits(16, Duration::from_secs(60));
        for byte in 10..=12 {
            store.record(
                &presence(
                    key(byte),
                    1,
                    Some("NL"),
                    Some(52.37),
                    Some(4.90),
                    Some(1234),
                ),
                now,
            );
        }

        let state = NetworkMapState::from_presence(&store, now);
        assert_eq!(state.nodes_online, 3);
        assert_eq!(state.points.len(), 3);
        assert_eq!(
            state
                .points
                .iter()
                .map(|point| (point.latitude, point.longitude))
                .collect::<Vec<_>>(),
            vec![(52.37, 4.90); 3]
        );
        assert_eq!(state.countries, 1);
        assert_eq!(state.networks, 1);
    }

    #[test]
    fn address_change_refreshes_one_presence_record_instead_of_duplicating_it() {
        let now = Instant::now();
        let mut store = PeerControlStateStore::with_limits(16, Duration::from_secs(60));
        let node = key(13);
        assert_eq!(
            store.record(
                &presence(node, 1, Some("US"), Some(40.7), Some(-74.0), Some(701)),
                now,
            ),
            crate::control_plane::privacy::StoreOutcome::New
        );
        assert_eq!(
            store.record(
                &presence(node, 2, Some("DE"), Some(52.5), Some(13.4), Some(3320)),
                now + Duration::from_secs(1),
            ),
            crate::control_plane::privacy::StoreOutcome::Refreshed
        );

        let state = NetworkMapState::from_presence(&store, now + Duration::from_secs(1));
        assert_eq!(state.nodes_online, 1);
        assert_eq!(state.points.len(), 1);
        assert_eq!(
            (state.points[0].latitude, state.points[0].longitude),
            (52.5, 13.4)
        );
        assert_eq!((state.countries, state.networks), (1, 1));
    }

    #[test]
    fn invalid_coordinates_do_not_hide_online_presence() {
        let now = Instant::now();
        let mut store = PeerControlStateStore::with_limits(16, Duration::from_secs(60));
        store.record(
            &presence(
                key(14),
                1,
                Some("AU"),
                Some(f64::NAN),
                Some(151.2),
                Some(1221),
            ),
            now,
        );

        let state = NetworkMapState::from_presence(&store, now);
        assert_eq!(state.nodes_online, 1);
        assert!(state.points.is_empty());
        assert_eq!((state.countries, state.networks), (1, 1));
    }
}
