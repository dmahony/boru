//! Home network information: bounded HTTPS IP fallback, offline geolocation.
mod public_ip;
use crate::control_plane::message::CoarsePresence;
use iroh::{Endpoint, EndpointAddr, TransportAddr, Watcher};
use maxminddb::Reader;
use n0_future::StreamExt;
use serde::Deserialize;
use std::{
    collections::BTreeMap,
    io::Read,
    sync::{Arc, Mutex},
};

#[derive(Debug, Clone, Hash, PartialEq, Eq)]
/// Local display details. Only separately derived coarse coordinates are shared.
pub struct Snapshot {
    /// Endpoint addresses and, when needed, HTTPS-observed public addresses.
    pub addresses: String,
    /// Approximate IP-based location or an explicit unavailable message.
    pub location: String,
}
impl Default for Snapshot {
    fn default() -> Self {
        Self {
            addresses: "IP: discovering…".into(),
            location: "Approximate location: loading local database…".into(),
        }
    }
}

impl Snapshot {
    /// Compact public-address line for the everyday Home summary.
    pub fn public_address_summary(&self) -> String {
        self.addresses.lines().next().unwrap_or("Public IP: unavailable").to_string()
    }

    /// Address diagnostics intentionally kept behind Home's Details disclosure.
    pub fn address_diagnostics(&self) -> String {
        self.addresses.lines().skip(1).collect::<Vec<_>>().join("\n")
    }
}

#[derive(Deserialize)]
struct Place {
    names: Option<BTreeMap<String, String>>,
    iso_code: Option<String>,
}
#[derive(Deserialize)]
struct Coordinates {
    latitude: Option<f64>,
    longitude: Option<f64>,
}
#[derive(Deserialize)]
struct Record {
    city: Option<Place>,
    subdivisions: Option<Vec<Place>>,
    country: Option<Place>,
    location: Option<Coordinates>,
}
fn english(place: Option<Place>) -> Option<String> {
    place?.names?.remove("en")
}
fn bundled_reader() -> Result<Reader<Vec<u8>>, String> {
    let mut bytes = Vec::new();
    flate2::read::GzDecoder::new(&include_bytes!("../assets/geoip/dbip-city-lite.mmdb.gz")[..])
        .read_to_end(&mut bytes)
        .map_err(|error| error.to_string())?;
    Reader::from_source(bytes).map_err(|error| error.to_string())
}
fn snapshot(address: &EndpointAddr, reader: Option<&Reader<Vec<u8>>>) -> Snapshot {
    let mut public = Vec::new();
    let mut local = Vec::new();
    for transport in &address.addrs {
        if let TransportAddr::Ip(addr) = transport {
            let ip = addr.ip();
            if crate::network_location::is_public_ip(ip) {
                public.push(ip);
            } else {
                local.push(ip);
            }
        }
    }
    public.sort();
    public.dedup();
    local.sort();
    local.dedup();
    let display = |ips: &[std::net::IpAddr]| {
        ips.iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .join(", ")
    };
    let addresses = format!(
        "Public IP: {}\nLocal IP: {}",
        if public.is_empty() {
            "not available".into()
        } else {
            display(&public)
        },
        if local.is_empty() {
            "not available".into()
        } else {
            display(&local)
        }
    );
    let location = public
        .iter()
        .find_map(|ip| {
            let record = reader?.lookup(*ip).ok()?.decode::<Record>().ok()??;
            let mut parts = Vec::new();
            if let Some(city) = english(record.city) {
                parts.push(city);
            }
            if let Some(regions) = record.subdivisions {
                for region in regions {
                    if let Some(name) = english(Some(region)) {
                        parts.push(name);
                    }
                }
            }
            if let Some(country) = english(record.country) {
                parts.push(country);
            }
            (!parts.is_empty())
                .then(|| format!("Approximate location ({ip}): {}", parts.join(", ")))
        })
        .unwrap_or_else(|| {
            if public.is_empty() {
                "Approximate location: unavailable until a public IP is discovered".into()
            } else {
                "Approximate location: unavailable in the local database".into()
            }
        });
    Snapshot {
        addresses,
        location,
    }
}

/// Receiver for coarse map metadata on the existing presence heartbeat.
pub type PresenceSink = Arc<dyn Fn(Option<CoarsePresence>) + Send + Sync>;

fn sharing_enabled(value: Option<&str>) -> bool {
    !value.is_some_and(|value| {
        matches!(
            value.trim().to_ascii_lowercase().as_str(),
            "0" | "false" | "off" | "no"
        )
    })
}

fn coarse_location(
    address: &EndpointAddr,
    reader: Option<&Reader<Vec<u8>>>,
    fallback: &[std::net::IpAddr],
) -> Option<CoarsePresence> {
    let mut ips = address
        .addrs
        .iter()
        .filter_map(|addr| match addr {
            TransportAddr::Ip(addr) => Some(addr.ip()),
            _ => None,
        })
        .chain(fallback.iter().copied());
    ips.find_map(|ip| {
        if !crate::network_location::is_public_ip(ip) {
            return None;
        }
        let record = reader?.lookup(ip).ok()?.decode::<Record>().ok()??;
        let location = record.location?;
        let latitude = location.latitude?;
        let longitude = location.longitude?;
        if !latitude.is_finite()
            || !longitude.is_finite()
            || !(-90.0..=90.0).contains(&latitude)
            || !(-180.0..=180.0).contains(&longitude)
        {
            return None;
        }
        CoarsePresence {
            country_code: record.country.and_then(|country| country.iso_code),
            latitude: Some((latitude * 10.0).round() / 10.0),
            longitude: Some((longitude * 10.0).round() / 10.0),
            asn: None,
        }
        .sanitized()
    })
}

/// Resolve geography off the GUI thread and share coarse map metadata by
/// default. `BORU_SHARE_MAP_LOCATION=0` opts out without disabling local details.
pub fn start(
    runtime: &tokio::runtime::Handle,
    endpoint: Endpoint,
    sink: Option<PresenceSink>,
) -> Arc<Mutex<Snapshot>> {
    let state = Arc::new(Mutex::new(Snapshot::default()));
    let output = Arc::clone(&state);
    let share = sharing_enabled(std::env::var("BORU_SHARE_MAP_LOCATION").ok().as_deref());
    runtime.spawn(async move {
        let publish = |address: &EndpointAddr,
                       reader: Option<&Reader<Vec<u8>>>,
                       fallback: &[std::net::IpAddr]| {
            if let Some(sink) = &sink {
                sink(if share {
                    coarse_location(address, reader, fallback)
                } else {
                    None
                });
            }
        };
        // Decompression and file IO must not block the GUI or networking workers.
        let reader = tokio::task::spawn_blocking(|| {
            if let Some(path) = std::env::var_os("BORU_GEOIP_CITY") {
                if let Ok(reader) = Reader::open_readfile(path) {
                    return Ok(reader);
                }
                tracing::warn!("Configured GeoIP database unavailable; using bundled DB-IP Lite");
            }
            bundled_reader()
        })
        .await
        .ok()
        .and_then(|result| {
            result
                .map_err(|error| tracing::warn!(%error, "Local GeoIP unavailable"))
                .ok()
        });
        let mut addresses = endpoint.watch_addr().stream();
        let mut current = endpoint.addr();
        let mut delay = std::time::Duration::ZERO;
        let mut retry_secs = 30u64;
        publish(&current, reader.as_ref(), &[]);
        if let Ok(mut value) = output.lock() {
            *value = snapshot(&current, reader.as_ref());
        }
        loop {
            // Selecting the address stream against the entire refresh cancels
            // obsolete lookups, so an old network can never overwrite a new one.
            tokio::select! {
                address = addresses.next() => {
                    let Some(address) = address else { break; };
                    if address == current { continue; }
                    current = address;
                    publish(&current, reader.as_ref(), &[]);
                    if let Ok(mut value) = output.lock() {
                        *value = snapshot(&current, reader.as_ref());
                    }
                    delay = std::time::Duration::from_secs(2);
                    retry_secs = 30;
                }
                fallback = async {
                    tokio::time::sleep(delay).await;
                    let (v4, v6) = tokio::join!(
                        discover_missing_family(&current, false),
                        discover_missing_family(&current, true),
                    );
                    [v4, v6].into_iter().flatten().collect::<Vec<_>>()
                } => {
                    let resolved = snapshot_with_fallback(&current, reader.as_ref(), &fallback);
                    publish(&current, reader.as_ref(), &fallback);
                    if let Ok(mut value) = output.lock() { *value = resolved; }
                    // A failed refresh removes stale HTTPS results rather than
                    // presenting an old ISP/VPN exit as current indefinitely.
                    if fallback.is_empty() {
                        delay = std::time::Duration::from_secs(retry_secs);
                        retry_secs = (retry_secs * 2).min(600);
                    } else {
                        delay = std::time::Duration::from_secs(300);
                        retry_secs = 30;
                    }
                }
            }
        }
        if let Some(sink) = &sink {
            sink(None);
        }
    });
    state
}

async fn discover_missing_family(address: &EndpointAddr, ipv6: bool) -> Option<std::net::IpAddr> {
    let available = address.addrs.iter().any(|addr| match addr {
        TransportAddr::Ip(addr) => {
            addr.is_ipv6() == ipv6 && crate::network_location::is_public_ip(addr.ip())
        }
        _ => false,
    });
    if available {
        None
    } else {
        public_ip::discover(ipv6).await
    }
}

fn snapshot_with_fallback(
    address: &EndpointAddr,
    reader: Option<&Reader<Vec<u8>>>,
    fallback: &[std::net::IpAddr],
) -> Snapshot {
    // Display-only copy: HTTPS results are NOT usable QUIC socket addresses.
    let mut display_address = address.clone();
    for ip in fallback {
        display_address
            .addrs
            .insert(TransportAddr::Ip(std::net::SocketAddr::new(*ip, 0)));
    }
    let mut value = snapshot(&display_address, reader);
    if !fallback.is_empty() {
        value
            .addresses
            .push_str("\nHTTPS-observed IP (may be a VPN exit): ");
        value.addresses.push_str(
            &fallback
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
                .join(", "),
        );
    }
    value
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn map_sharing_defaults_on_and_supports_opt_out() {
        assert!(sharing_enabled(None));
        assert!(sharing_enabled(Some("1")));
        for value in ["0", "false", "OFF", "no"] {
            assert!(!sharing_enabled(Some(value)));
        }
    }

    #[test]
    fn shared_fallback_location_reaches_map_and_clears() {
        use crate::control_plane::{message::ControlEnvelope, privacy::PeerControlStateStore};
        let reader = bundled_reader().unwrap();
        let endpoint = address(&["192.168.1.20:1234"]);
        let coarse =
            coarse_location(&endpoint, Some(&reader), &["8.8.8.8".parse().unwrap()]).unwrap();
        for coordinate in [coarse.latitude.unwrap(), coarse.longitude.unwrap()] {
            assert!((coordinate * 10.0 - (coordinate * 10.0).round()).abs() < 1e-8);
        }
        let node = iroh_base::SecretKey::from_bytes(&[3; 32]).public();
        let now = std::time::Instant::now();
        let mut store = PeerControlStateStore::with_limits(16, std::time::Duration::from_secs(60));
        store.record(
            &ControlEnvelope::presence_with_coarse(node, 1, 1_700_000_000, None, Some(coarse)),
            now,
        );
        assert_eq!(
            crate::network_map::NetworkMapState::from_presence(&store, now)
                .points
                .len(),
            1
        );
        let unavailable = coarse_location(&endpoint, Some(&reader), &[]);
        assert!(unavailable.is_none());
        store.record(
            &ControlEnvelope::presence_with_coarse(node, 2, 1_700_000_001, None, unavailable),
            now,
        );
        let cleared = crate::network_map::NetworkMapState::from_presence(&store, now);
        assert!(cleared.points.is_empty());
        assert_eq!(cleared.nodes_online, 1);
        assert_eq!(endpoint.addrs.len(), 1);
    }
    #[test]
    fn fallback_resolves_private_only_endpoint_without_mutating_it() {
        let reader = bundled_reader().unwrap();
        let endpoint = address(&["192.168.1.20:1234"]);
        let result =
            snapshot_with_fallback(&endpoint, Some(&reader), &["8.8.8.8".parse().unwrap()]);
        assert!(result.addresses.contains("Public IP: 8.8.8.8"));
        assert!(result.location.contains("Approximate location (8.8.8.8):"));
        assert_eq!(endpoint.addrs.len(), 1);
        let expired = snapshot_with_fallback(&endpoint, Some(&reader), &[]);
        assert!(!expired.location.contains("8.8.8.8"));
        assert!(expired.addresses.contains("Public IP: not available"));
    }

    #[test]
    fn bundled_database_resolves_public_ip_offline() {
        let reader = bundled_reader().expect("bundled database must decode");
        let record = reader
            .lookup("8.8.8.8".parse().unwrap())
            .unwrap()
            .decode::<Record>()
            .unwrap()
            .unwrap();
        assert!(english(record.country).is_some());
    }

    fn address(ips: &[&str]) -> EndpointAddr {
        let mut address = EndpointAddr::new(iroh::SecretKey::from_bytes(&[7; 32]).public());
        for ip in ips {
            address.addrs.insert(TransportAddr::Ip(ip.parse().unwrap()));
        }
        address
    }

    #[test]
    fn private_ip_is_not_geolocated() {
        let state = snapshot(&address(&["192.168.1.20:1234"]), None);
        assert!(state.addresses.contains("Public IP: not available"));
        assert!(state.addresses.contains("Local IP: 192.168.1.20"));
        assert!(state.location.contains("until a public IP is discovered"));
        assert_eq!(state.public_address_summary(), "Public IP: not available");
        assert_eq!(state.address_diagnostics(), "Local IP: 192.168.1.20");
    }

    #[test]
    fn default_snapshot_keeps_loading_address_summary_and_no_diagnostics() {
        let state = Snapshot::default();
        assert_eq!(state.public_address_summary(), "IP: discovering…");
        assert!(state.address_diagnostics().is_empty());
        assert!(state.location.contains("loading"));
    }

    #[test]
    fn location_resets_when_public_address_disappears() {
        let reader = bundled_reader().unwrap();
        let public = snapshot(&address(&["8.8.8.8:1234"]), Some(&reader));
        assert!(public.location.contains("Approximate location (8.8.8.8):"));
        let gone = snapshot(&address(&[]), Some(&reader));
        assert!(!gone.location.contains("8.8.8.8"));
        assert!(gone.location.contains("unavailable"));
    }

    #[test]
    fn missing_database_keeps_addresses_and_reports_unavailable() {
        let state = snapshot(
            &address(&[
                "8.8.8.8:1234",
                "8.8.8.8:5678",
                "[2001:4860:4860::8888]:1234",
            ]),
            None,
        );
        assert_eq!(state.addresses.matches("8.8.8.8").count(), 1);
        assert!(state.addresses.contains("2001:4860:4860::8888"));
        assert!(state.location.contains("unavailable in the local database"));
    }
}
