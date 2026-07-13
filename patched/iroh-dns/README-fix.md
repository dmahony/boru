# Windows DNS Resolution Fix for Iroh Pkarr

## Problem

On this Windows 11 setup, the iroh pkarr DNS resolution fails because:

1. `hickory-resolver` (used by `iroh-dns`) defaults to **edns0=true**, sending an EDNS0 OPT
   pseudo-record (max_payload=1232) in every DNS query.
2. **pfSense/Unbound at 172.16.0.1 returns REFUSED** for DNS queries containing EDNS0
   OPT records from non-standard clients (confirmed: `nslookup` sends *without* EDNS0
   and succeeds; hickory-resolver sends *with* EDNS0 and gets REFUSED).
3. `nslookup -debug dns.iroh.link` shows `additional = 0` (no EDNS0) — succeeds with NOERROR.
4. hickory-resolver's default queries include an OPT record — pfSense returns rcode=5 (REFUSED).
5. The entire DNS resolution fails after the 5-second timeout, causing `PkarrPublisher`
   to log `"Failed to publish to pkarr"`.

## Fix (in `patched/iroh-dns`)

Three changes were made to the `iroh-dns` crate:

### 1. DNS Timeout Increase (`DNS_TIMEOUT`: 3s → 5s)

`patched/iroh-dns/src/dns.rs`, line 44:
```rust
pub const DNS_TIMEOUT: Duration = Duration::from_secs(5);
```

The hickory-resolver's internal nameserver fallback mechanism needs extra time
when the system nameserver (172.16.0.1) returns REFUSED for EDNS0 queries and
the resolver must fall through to DoH nameservers. The staggered DNS lookup
mechanism already adds delays between retries, so a 5-second per-family timeout
is still aggressive enough for user-facing responsiveness.

### 2. DoH Fallback Nameservers (Cloudflare + Google)

`patched/iroh-dns/src/dns.rs`, in `HickoryResolver::build_resolver()` (lines 750-773):

After adding the system-configured nameservers (and any user-configured ones),
Cloudflare DNS-over-HTTPS and Google DNS-over-HTTPS resolvers are appended as
additional nameservers:

- `1.1.1.1:443` (Cloudflare) — TLS SNI: `cloudflare-dns.com`
- `1.0.0.1:443` (Cloudflare) — TLS SNI: `cloudflare-dns.com`
- `8.8.8.8:443` (Google) — TLS SNI: `dns.google`
- `8.8.4.4:443` (Google) — TLS SNI: `dns.google`

hickory-resolver tries nameservers in order and skips to the next on timeout
or refusal. Since DoH nameservers are added *after* system nameservers, the
resolver will:
1. Try the system nameservers first (fast, local — works on most networks without EDNS0 issues)
2. Receive REFUSED from pfSense/Unbound when EDNS0 is sent
3. Fall through to DoH, which succeeds because HTTPS DNS bypasses the EDNS0 rejection

These are conditionally compiled behind `#[cfg(with_crypto_provider)]` (enabled when
`tls-ring` or `tls-aws-lc-rs` feature is active), which is the case for iroh-gossip
since it depends on iroh with the `tls-ring` feature.

### 3. Additional Hardening (try_tcp_on_error + os_port_selection)

`patched/iroh-dns/src/dns.rs`, in `HickoryResolver::build_resolver()` (after option setup):

```rust
options.try_tcp_on_error = true;
#[cfg(target_os = "windows")]
{
    options.os_port_selection = true;
}
```

- **try_tcp_on_error**: Falls back to TCP DNS when UDP fails. Handles edge cases where
  UDP is entirely blocked or filtered, and provides an alternative transport path.
- **os_port_selection (Windows only)**: Lets the OS select the source port for UDP DNS
  queries. Avoids potential source-port-based firewall filtering that can occur on
  pfSense and other strict DNS environments.

## Integration via Cargo Patch

The fix is integrated through Cargo's `[patch.crates-io]` mechanism in the root
`Cargo.toml`. This overrides the upstream `iroh-dns` crate (v1.0.0) with the
patched local version at `patched/iroh-dns/`:

```toml
[patch.crates-io]
iroh-dns = { path = "patched/iroh-dns" }
```

When the upstream `iroh-dns` crate ships a fix for this issue (e.g., adding
DoH support or improving EDNS0 handling), the patch can be removed.

## What This Fixes

- **Primary**: Pkarr publishing on Windows 11 with pfSense/Unbound rejecting EDNS0 queries
- **Also**: DNS resolution in net_report (relay address resolution) on Windows 11
- **Also**: DNS resolution on networks with slow/filtered UDP DNS but working HTTPS DNS
- **Also**: Reduces timeout-induced failures on congested corporate VPNs

## What This Doesn't Fix

- IPv6 DNS servers with no IPv6 connectivity (the system_config already filters
  fec0::/10 site-local addresses, but link-local fe80::/10 addresses from the wrong
  interface can still cause timeouts on both IPv4 and IPv6 queries)
- Windows Firewall or corporate VPN DNS interception that blocks all outbound DNS
- Users who want to use their own enterprise DoH resolver (would need config support)
- The root issue (pfSense rejecting hickory-resolver EDNS0 queries) upstream in hickory-resolver
