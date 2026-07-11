# Windows 11 DNS Resolution Fix for Iroh Pkarr

## Problem

On Windows 11, the iroh pkarr DNS resolution fails because:

1. Windows 11 configures DNS over HTTPS (DoH) by default for compatible DNS servers
2. The `hickory-resolver` library (used by `iroh-dns`) sends **plain UDP DNS queries** (port 53)
3. When Windows has configured a DNS server with DoH, plain UDP queries are silently dropped
4. The hickory-resolver gets no response within the 3-second timeout
5. The entire DNS resolution fails, causing `PkarrPublisher` to fail with `"Failed to publish to pkarr"`

## Fix (in `patched/iroh-dns`)

Two changes were made to the `iroh-dns` crate:

### 1. DNS Timeout Increase (`DNS_TIMEOUT`: 3s → 5s)

`patched/iroh-dns/src/dns.rs`, line 40:
```rust
pub const DNS_TIMEOUT: Duration = Duration::from_secs(5);
```

The hickory-resolver's internal retry/fallback mechanism benefits from extra time,
especially when the resolver tries both UDP system nameservers and DoH fallback.
The staggered DNS lookup mechanism (`DNS_STAGGERING_MS` in iroh's `DnsAddressLookup`)
already adds delays between retries, so a 5-second per-family timeout is still
aggressive enough for user-facing responsiveness.

### 2. DoH Fallback Nameservers (Cloudflare + Google)

`patched/iroh-dns/src/dns.rs`, in `HickoryResolver::build_resolver()`:

After adding the system-configured nameservers (and any user-configured ones),
Cloudflare DNS-over-HTTPS and Google DNS-over-HTTPS resolvers are appended as
additional nameservers:

- `1.1.1.1:443` (Cloudflare) — TLS SNI: `cloudflare-dns.com`
- `1.0.0.1:443` (Cloudflare) — TLS SNI: `cloudflare-dns.com`
- `8.8.8.8:443` (Google) — TLS SNI: `dns.google`
- `8.8.4.4:443` (Google) — TLS SNI: `dns.google`

hickory-resolver tries nameservers in order and skips to the next on timeout.
Since DoH nameservers are added *after* system nameservers, the resolver will:
1. Try the system nameservers first (fast, local — works on most networks)
2. Fall through to DoH if system nameservers time out (silent UDP drop on Windows 11)
3. The DoH query succeeds because it uses HTTPS, which Windows 11 DNS infrastructure respects

These are conditionally compiled behind `#[cfg(with_crypto_provider)]` (enabled when
`tls-ring` or `tls-aws-lc-rs` feature is active), which is the case for iroh-gossip
since it depends on iroh with the `tls-ring` feature.

## Integration via Cargo Patch

The fix is integrated through Cargo's `[patch.crates-io]` mechanism in the root
`Cargo.toml`. This overrides the upstream `iroh-dns` crate (v1.0.0) with the
patched local version at `patched/iroh-dns/`:

```toml
[patch.crates-io]
iroh-dns = { path = "patched/iroh-dns" }
```

When the upstream `iroh-dns` crate ships a fix for this issue (e.g., adding
DoH support or switching to the Windows native DNS API), the patch can be
removed.

## What This Fixes

- **Primary**: Pkarr publishing on Windows 11 with DoH enabled
- **Also**: DNS resolution in net_report (relay address resolution) on Windows 11
- **Also**: DNS resolution on networks with slow/filtered UDP DNS but working HTTPS DNS
- **Also**: Reduces timeout-induced failures on congested corporate VPNs

## What This Doesn't Fix

- IPv6 DNS servers with no IPv6 connectivity (the system_config already filters
  fec0::/10 site-local addresses, but link-local fe80::/10 addresses from the wrong
  interface can still cause timeouts on both IPv4 and IPv6 queries)
- Windows Firewall or corporate VPN DNS interception that blocks all outbound DNS
- Users who want to use their own enterprise DoH resolver (would need config support)
