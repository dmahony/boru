//! Fail-soft local public-address and GeoIP resolution.
//!
//! `Endpoint::watch_addr()` is the authoritative Iroh API for this state: an
//! endpoint address changes as interfaces, NAT mappings, and relays change.
//! Call [`GeoIpResolver::resolve_endpoint_addr`] for each watcher notification.
//!
//! GeoIP databases are deliberately runtime inputs. Install MaxMind's
//! GeoLite2 City database (and, optionally, GeoLite2 ASN) under a
//! user-controlled path and pass those paths to [`GeoIpResolver::from_paths`].
//! The databases must be obtained and updated under MaxMind's applicable
//! GeoLite2 licence and account terms; Boru does not bundle them or contact a
//! hosted lookup service. Missing, unreadable, or malformed files simply make
//! resolution return `None`.

use serde::Deserialize;
use std::{collections::HashMap, net::IpAddr, path::Path};

use crate::control_plane::message::CoarsePresence;
use iroh::{Endpoint, EndpointAddr, TransportAddr, Watcher};
use maxminddb::Reader;

#[derive(Debug, Deserialize)]
struct CityRecord {
    country: Option<CountryRecord>,
    location: Option<LocationRecord>,
}

#[derive(Debug, Deserialize)]
struct CountryRecord {
    iso_code: Option<String>,
}

#[derive(Debug, Deserialize)]
struct LocationRecord {
    latitude: Option<f64>,
    longitude: Option<f64>,
}

#[derive(Debug, Deserialize)]
struct AsnRecord {
    autonomous_system_number: Option<u32>,
}

/// A local-only resolver with an address-keyed cache.
#[derive(Debug)]
pub struct GeoIpResolver {
    city: Option<Reader<Vec<u8>>>,
    asn: Option<Reader<Vec<u8>>>,
    cache: HashMap<IpAddr, Option<CoarsePresence>>,
    lookups: u64,
}

impl GeoIpResolver {
    /// Open local databases. Any individual open failure is treated as a
    /// missing database; startup and networking do not depend on GeoIP.
    pub fn from_paths(city_path: Option<&Path>, asn_path: Option<&Path>) -> Self {
        let city = city_path.and_then(|path| Reader::open_readfile(path).ok());
        let asn = asn_path.and_then(|path| Reader::open_readfile(path).ok());
        Self {
            city,
            asn,
            cache: HashMap::new(),
            lookups: 0,
        }
    }

    /// Resolve the first public/reflexive address with available local data.
    /// Relays, custom transports, and non-public IPs never reach a database.
    pub fn resolve_endpoint_addr(
        &mut self,
        endpoint_addr: &EndpointAddr,
    ) -> Option<CoarsePresence> {
        endpoint_addr
            .addrs
            .iter()
            .filter_map(|transport| match transport {
                TransportAddr::Ip(addr) if is_public_ip(addr.ip()) => Some(addr.ip()),
                _ => None,
            })
            .find_map(|ip| self.resolve_ip(ip))
    }

    /// Resolve one address, using the address itself as the cache key (the
    /// transport port is intentionally not relevant to GeoIP).
    pub fn resolve_ip(&mut self, ip: IpAddr) -> Option<CoarsePresence> {
        if !is_public_ip(ip) {
            return None;
        }
        if let Some(value) = self.cache.get(&ip) {
            return value.clone();
        }
        self.lookups += 1;
        let value = self.lookup(ip);
        self.cache.insert(ip, value.clone());
        value
    }

    /// Number of uncached database lookup attempts; useful for diagnostics and
    /// proving unchanged watcher notifications are deduplicated.
    pub fn lookup_count(&self) -> u64 {
        self.lookups
    }

    fn lookup(&mut self, ip: IpAddr) -> Option<CoarsePresence> {
        let city = self.city.as_mut().and_then(|reader| {
            reader
                .lookup(ip)
                .ok()?
                .decode::<CityRecord>()
                .ok()
                .flatten()
        });
        let asn = self
            .asn
            .as_mut()
            .and_then(|reader| reader.lookup(ip).ok()?.decode::<AsnRecord>().ok().flatten());
        let (country_code, latitude, longitude) = city
            .map(|record| {
                let country = record.country.and_then(|country| country.iso_code);
                let coordinates = record
                    .location
                    .map(|location| (location.latitude, location.longitude));
                (
                    country,
                    coordinates.as_ref().and_then(|x| x.0),
                    coordinates.and_then(|x| x.1),
                )
            })
            .unwrap_or((None, None, None));
        let value = CoarsePresence {
            country_code: country_code.map(|code| code.to_ascii_uppercase()),
            latitude: latitude.map(coarse_coordinate),
            longitude: longitude.map(coarse_coordinate),
            asn: asn.and_then(|record| record.autonomous_system_number),
        };
        value.sanitized()
    }
}

/// Spawn an off-render-path watcher for live Iroh address changes.
pub fn spawn_endpoint_watcher<F>(
    endpoint: Endpoint,
    resolver: std::sync::Arc<std::sync::Mutex<GeoIpResolver>>,
    mut on_change: F,
) -> tokio::task::JoinHandle<()>
where
    F: FnMut(Option<CoarsePresence>) + Send + 'static,
{
    tokio::spawn(async move {
        use n0_future::StreamExt;
        let mut addresses = endpoint.watch_addr().stream();
        while let Some(address) = addresses.next().await {
            let value = resolver
                .lock()
                .ok()
                .and_then(|mut resolver| resolver.resolve_endpoint_addr(&address));
            on_change(value);
        }
    })
}

fn coarse_coordinate(value: f64) -> f64 {
    (value * 10.0).round() / 10.0
}

/// Conservative public-address filter. In particular, RFC1918, CGNAT,
/// loopback, link-local, documentation, multicast, and IPv6 ULA are rejected.
pub fn is_public_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(ip) => {
            let octets = ip.octets();
            !(ip.is_private()
                || ip.is_loopback()
                || ip.is_link_local()
                || ip.is_unspecified()
                || ip.is_broadcast()
                || ip.is_multicast()
                || (octets[0] == 100 && (64..=127).contains(&octets[1]))
                || (octets[0] == 192 && octets[1] == 0 && octets[2] == 0)
                || (octets[0] == 192 && octets[1] == 0 && octets[2] == 2)
                || (octets[0] == 198 && (18..=19).contains(&octets[1]))
                || (octets[0] == 198 && octets[1] == 51 && octets[2] == 100)
                || (octets[0] == 203 && octets[1] == 0 && octets[2] == 113))
        }
        IpAddr::V6(ip) => {
            let segments = ip.segments();
            !(ip.is_loopback()
                || ip.is_unspecified()
                || ip.is_multicast()
                || (segments[0] & 0xfe00) == 0xfc00
                || (segments[0] & 0xffc0) == 0xfe80
                || (segments[0] == 0x2001 && segments[1] == 0x0db8))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::{Ipv4Addr, Ipv6Addr};

    #[test]
    fn filters_private_and_special_addresses() {
        for ip in [
            IpAddr::V4(Ipv4Addr::new(10, 1, 2, 3)),
            IpAddr::V4(Ipv4Addr::new(100, 64, 1, 1)),
            IpAddr::V4(Ipv4Addr::LOCALHOST),
            IpAddr::V6(Ipv6Addr::LOCALHOST),
            IpAddr::V6(Ipv6Addr::new(0xfc00, 0, 0, 0, 0, 0, 0, 1)),
        ] {
            assert!(!is_public_ip(ip));
        }
        assert!(is_public_ip(IpAddr::V4(Ipv4Addr::new(8, 8, 8, 8))));
    }

    #[test]
    fn missing_database_is_normal_none_and_cached() {
        let mut resolver = GeoIpResolver::from_paths(None, None);
        let ip = IpAddr::V4(Ipv4Addr::new(8, 8, 8, 8));
        assert_eq!(resolver.resolve_ip(ip), None);
        assert_eq!(resolver.resolve_ip(ip), None);
        assert_eq!(resolver.lookup_count(), 1);
        assert_eq!(resolver.resolve_ip(IpAddr::V4(Ipv4Addr::LOCALHOST)), None);
        assert_eq!(resolver.lookup_count(), 1);
    }

    #[test]
    fn coarse_rounding_is_tenth_degree() {
        assert_eq!(coarse_coordinate(51.50741), 51.5);
        assert_eq!(coarse_coordinate(-0.12782), -0.1);
    }
}
