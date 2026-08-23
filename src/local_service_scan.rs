//! Local TCP service discovery for the "Share Local Service" dialog.
//!
//! Enumerates loopback-reachable TCP listeners, verifies them with connect
//! tests, fingerprints HTTP services, and labels them with a human-readable
//! name. The GUI calls [`scan_local_services`](crate::local_service_scan::scan_local_services) and renders the returned
//! suggestions; the module itself has no iced dependency so a future
//! non-desktop backend could substitute its own enumeration strategy.
//!
//! ## Pipeline
//!
//! 1. **Enumerate** — [`netstat2`] reads the OS TCP table (no shell-outs, no
//!    admin rights): `GetExtendedTcpTable` on Windows, libproc on macOS,
//!    `/proc/net/tcp` + netlink diag on Linux.
//! 2. **Dedupe** — multiple bind addrs (`0.0.0.0` / `127.0.0.1` / `::1`) for
//!    the same port collapse into one entry per port (loopback preferred).
//! 3. **Exclude self** — the caller's own PID and the LocalTunnelListener
//!    dynamic port are filtered out so Boru never suggests its own listeners.
//! 4. **Verify** — each candidate is connect-tested against `127.0.0.1:port`
//!    with a short timeout. Only loopback-reachable services are shareable
//!    (the tunnel target is loopback-constrained).
//! 5. **Classify** — reachable services get an HTTP probe
//!    (`GET / HTTP/1.1`, `Host: localhost`, `Connection: close`) that yields
//!    an `is_http` flag plus a `Server:` fingerprint.
//! 6. **Label** — process name (from PID via `sysinfo`) > `Server:` header >
//!    static well-known-port table > fallback `"TCP service on :PORT"`.

use std::{
    collections::{BTreeMap, HashMap},
    net::{IpAddr, Ipv4Addr, SocketAddr},
    time::Duration,
};

/// Connect-test timeout per candidate port.
const CONNECT_TIMEOUT: Duration = Duration::from_millis(100);
/// HTTP probe timeout per candidate port.
const HTTP_PROBE_TIMEOUT: Duration = Duration::from_millis(150);
/// Maximum parallel probes (connect + HTTP).
const PROBE_CONCURRENCY: usize = 8;
/// How long a completed scan stays fresh for the dialog reopen cache.
pub const SCAN_CACHE_TTL: Duration = Duration::from_secs(30);

/// A discovered local service suggestion shown in the share dialog.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LocalServiceSuggestion {
    /// Loopback-reachable TCP port.
    pub port: u16,
    /// Human-readable label (process name > Server header > well-known table
    /// > `"TCP service on :PORT"`).
    pub label: String,
    /// Whether the service answered an HTTP probe.
    pub is_http: bool,
}

/// A raw listener entry from the OS TCP table.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ListenerEntry {
    /// Local bind address (any interface).
    pub local_addr: IpAddr,
    /// Local TCP port.
    pub port: u16,
    /// Owning process id when the OS exposes it.
    pub pid: Option<u32>,
}

/// Score how good a bind address is for loopback sharing: explicit loopback
/// beats wildcard, wildcard beats a specific non-loopback interface.
fn loopback_score(addr: IpAddr) -> u8 {
    if addr.is_loopback() {
        2
    } else if addr.is_unspecified() {
        1
    } else {
        0
    }
}

/// Collapse multiple bind addrs for the same port into one entry per port,
/// preferring the loopback/wildcard binding, then sort ascending by port.
///
/// Pure logic — no network access.
pub fn dedupe_and_sort(listeners: Vec<ListenerEntry>) -> Vec<ListenerEntry> {
    let mut by_port: BTreeMap<u16, ListenerEntry> = BTreeMap::new();
    for entry in listeners {
        let better = match by_port.get(&entry.port) {
            Some(existing) => {
                loopback_score(entry.local_addr) > loopback_score(existing.local_addr)
            }
            None => true,
        };
        if better {
            by_port.insert(entry.port, entry);
        }
    }
    by_port.into_values().collect()
}

/// Static well-known-port table used when no process name or Server header is
/// available. Returns `None` for unknown ports.
pub fn well_known_label(port: u16) -> Option<&'static str> {
    Some(match port {
        22 => "SSH",
        80 => "HTTP",
        443 => "HTTPS",
        3000 => "Dev server",
        3306 => "MySQL",
        5000 => "Dev server",
        5432 => "Postgres",
        6379 => "Redis",
        8000 => "HTTP",
        8080 => "HTTP",
        8081 => "HTTP",
        8888 => "HTTP",
        27017 => "MongoDB",
        _ => return None,
    })
}

/// Resolve a display label with priority: process name > Server header >
/// well-known table > fallback.
///
/// Pure logic — no network access.
pub fn resolve_label(port: u16, process_name: Option<&str>, server_header: Option<&str>) -> String {
    if let Some(name) = process_name {
        let name = name.trim();
        if !name.is_empty() {
            return name.to_string();
        }
    }
    if let Some(server) = server_header {
        let server = server.trim();
        if !server.is_empty() {
            return server.to_string();
        }
    }
    if let Some(label) = well_known_label(port) {
        return label.to_string();
    }
    format!("TCP service on :{port}")
}

/// Filter out the caller's own PID and the given excluded ports (e.g. Boru's
/// own LocalTunnelListener dynamic port).
///
/// Pure logic — no network access.
pub fn exclude_self(
    listeners: Vec<ListenerEntry>,
    own_pid: Option<u32>,
    excluded_ports: &[u16],
) -> Vec<ListenerEntry> {
    listeners
        .into_iter()
        .filter(|entry| {
            if excluded_ports.contains(&entry.port) {
                return false;
            }
            if let (Some(own), Some(pid)) = (own_pid, entry.pid) {
                if own == pid {
                    return false;
                }
            }
            true
        })
        .collect()
}

/// Sort suggestions HTTP-first, then ascending by port.
///
/// Pure logic — no network access.
pub fn sort_suggestions(
    mut suggestions: Vec<LocalServiceSuggestion>,
) -> Vec<LocalServiceSuggestion> {
    suggestions.sort_by(|a, b| b.is_http.cmp(&a.is_http).then_with(|| a.port.cmp(&b.port)));
    suggestions
}

/// Parse an HTTP probe response head into `(is_http, server_header)`.
///
/// Pure logic — no network access. The server header is case-insensitive
/// (`Server:` / `server:`).
pub fn parse_http_head(head: &str) -> (bool, Option<String>) {
    let mut lines = head.lines();
    let status_line = lines.next().unwrap_or("");
    let is_http = status_line.starts_with("HTTP/");
    let server = lines
        .find_map(|line| {
            let line = line.trim();
            let lower = line.to_ascii_lowercase();
            lower
                .strip_prefix("server:")
                .map(|value| line[line.len() - value.len()..].trim().to_string())
        })
        .filter(|v| !v.is_empty());
    (is_http, server)
}

/// Enumerate TCP listeners from the OS table (synchronous; run off the async
/// runtime via `spawn_blocking`).
fn enumerate_listeners() -> Vec<ListenerEntry> {
    use netstat2::{
        get_sockets_info, AddressFamilyFlags, ProtocolFlags, ProtocolSocketInfo, TcpState,
    };
    let sockets = match get_sockets_info(
        AddressFamilyFlags::IPV4 | AddressFamilyFlags::IPV6,
        ProtocolFlags::TCP,
    ) {
        Ok(sockets) => sockets,
        Err(_) => return Vec::new(),
    };
    let mut out = Vec::new();
    for socket in sockets {
        if let ProtocolSocketInfo::Tcp(tcp) = socket.protocol_socket_info {
            if tcp.state == TcpState::Listen && tcp.local_port != 0 {
                out.push(ListenerEntry {
                    local_addr: tcp.local_addr,
                    port: tcp.local_port,
                    pid: socket.associated_pids.first().copied(),
                });
            }
        }
    }
    out
}

/// Connect-test one port on loopback with a short timeout.
async fn connect_probe(port: u16) -> bool {
    let addr = SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), port);
    tokio::time::timeout(CONNECT_TIMEOUT, tokio::net::TcpStream::connect(addr))
        .await
        .map(|res| res.is_ok())
        .unwrap_or(false)
}

/// HTTP probe one port on loopback: send a minimal `GET /` request and parse
/// the response head. Returns `(is_http, server_header)`.
async fn http_probe(port: u16) -> (bool, Option<String>) {
    let addr = SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), port);
    let Ok(Ok(mut stream)) =
        tokio::time::timeout(HTTP_PROBE_TIMEOUT, tokio::net::TcpStream::connect(addr)).await
    else {
        return (false, None);
    };
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let request = b"GET / HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n";
    if stream.write_all(request).await.is_err() {
        return (false, None);
    }
    let mut buf = [0u8; 2048];
    let n = tokio::time::timeout(HTTP_PROBE_TIMEOUT, stream.read(&mut buf))
        .await
        .ok()
        .and_then(|res| res.ok())
        .unwrap_or(0);
    let head = String::from_utf8_lossy(&buf[..n]);
    parse_http_head(&head)
}

/// Run a probe across ports with a bounded concurrency cap, preserving order
/// of completion per chunk. The probe is shared via `Arc` so each spawned
/// task can call it without moving the generic closure.
async fn probe_ports<F, Fut, R>(ports: &[u16], probe: std::sync::Arc<F>) -> HashMap<u16, R>
where
    F: Fn(u16) -> Fut + Send + Sync + 'static,
    Fut: std::future::Future<Output = R> + Send + 'static,
    R: Send + 'static,
{
    let mut out = HashMap::new();
    let mut set = tokio::task::JoinSet::new();
    for chunk in ports.chunks(PROBE_CONCURRENCY) {
        for &port in chunk {
            let probe = probe.clone();
            set.spawn(async move {
                let result = probe(port).await;
                (port, result)
            });
        }
        while let Some(res) = set.join_next().await {
            if let Ok((port, result)) = res {
                out.insert(port, result);
            }
        }
    }
    out
}

/// Look up process names for the given PIDs (synchronous; run off the async
/// runtime via `spawn_blocking`).
fn process_names(pids: &[u32]) -> HashMap<u32, String> {
    use sysinfo::{Pid, ProcessRefreshKind, ProcessesToUpdate, System};
    let mut sys = System::new();
    let sys_pids: Vec<Pid> = pids.iter().copied().map(Pid::from_u32).collect();
    if !sys_pids.is_empty() {
        sys.refresh_processes_specifics(
            ProcessesToUpdate::Some(&sys_pids),
            true,
            ProcessRefreshKind::nothing(),
        );
    }
    let mut out = HashMap::new();
    for &pid in pids {
        if let Some(process) = sys.process(Pid::from_u32(pid)) {
            let name = process.name().to_string_lossy().into_owned();
            if !name.is_empty() {
                out.insert(pid, name);
            }
        }
    }
    out
}

/// Scan local TCP listeners and return shareable suggestions.
///
/// - Enumerates the OS TCP table (off-thread), dedupes by port, excludes the
///   caller's own PID and the given excluded ports.
/// - Connect-tests each candidate on loopback (bounded concurrency) and keeps
///   only reachable ports.
/// - HTTP-probes reachable services for `is_http` + `Server:` fingerprint.
/// - Labels via process name > Server header > well-known table > fallback.
/// - Sorts HTTP-first, then by port.
///
/// Designed to run off the UI thread (iced `Task::perform`); the total budget
/// is well under 1s for a handful of local listeners. Errors degrade to an
/// empty list — the dialog stays usable via manual port entry.
pub async fn scan_local_services(
    own_pid: Option<u32>,
    excluded_ports: Vec<u16>,
) -> Vec<LocalServiceSuggestion> {
    let listeners = match tokio::task::spawn_blocking(enumerate_listeners).await {
        Ok(listeners) => listeners,
        Err(_) => return Vec::new(),
    };
    let listeners = dedupe_and_sort(listeners);
    let listeners = exclude_self(listeners, own_pid, &excluded_ports);
    if listeners.is_empty() {
        return Vec::new();
    }

    let ports: Vec<u16> = listeners.iter().map(|e| e.port).collect();

    // Connect-test reachability; keep only loopback-reachable ports.
    let reachable = probe_ports(
        &ports,
        std::sync::Arc::new(|port| async move { connect_probe(port).await }),
    )
    .await
    .into_iter()
    .filter_map(|(port, ok)| ok.then_some(port))
    .collect::<Vec<_>>();
    if reachable.is_empty() {
        return Vec::new();
    }

    // HTTP fingerprint reachable services.
    let http = probe_ports(
        &reachable,
        std::sync::Arc::new(|port| async move { http_probe(port).await }),
    )
    .await;

    // Process names for labelling.
    let pids: Vec<u32> = listeners.iter().filter_map(|e| e.pid).collect();
    let names = tokio::task::spawn_blocking(move || process_names(&pids))
        .await
        .unwrap_or_default();

    let mut suggestions = Vec::with_capacity(reachable.len());
    for port in reachable {
        let process_name = listeners
            .iter()
            .find(|e| e.port == port)
            .and_then(|e| e.pid)
            .and_then(|pid| names.get(&pid))
            .map(String::as_str);
        let (is_http, server) = http.get(&port).cloned().unwrap_or((false, None));
        let label = resolve_label(port, process_name, server.as_deref());
        suggestions.push(LocalServiceSuggestion {
            port,
            label,
            is_http,
        });
    }

    sort_suggestions(suggestions)
}
