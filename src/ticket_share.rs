//! BlobTicket wormhole sharing (SENDME-02).
//!
//! Share a file outside the friend graph by copying a [`BlobTicket`](iroh_blobs::ticket::BlobTicket) string;
//! receive by pasting a ticket.  The ticket grants access to exactly that
//! blob — no friend relationship or download-authorisation is required.
//!
//! This module provides:
//!
//! - `AddrInfoOptions` + [`apply_options`](crate::ticket_share::apply_options) — ticket trimming (Id-only vs
//!   RelayAndAddresses), mirroring sendme's `apply_options`
//!   (`src/main.rs:269-294`).
//! - [`make_share_ticket`](crate::ticket_share::make_share_ticket) — build a share-ticket string from parts with the
//!   selected address info.
//! - [`preflight_ticket`](crate::ticket_share::preflight_ticket) — connect to the ticket's node and read size
//!   information without downloading payload bytes (mirrors sendme's receive
//!   preflight: connect -> `get_hash_seq_and_sizes` / `get_verified_size`).
//!
//! Id-only tickets are supported on the receive side because the boru
//! endpoint registers `DnsAddressLookup::n0_dns()` / `PkarrResolver::n0_dns()`
//! at startup (see `src/bin/boru/main.rs`); connecting to an Id-only
//! addr resolves the node through those lookups.

use anyhow::{anyhow, Result};
use iroh::{Endpoint, EndpointAddr, PublicKey, TransportAddr};
use iroh_blobs::ticket::BlobTicket;
use iroh_blobs::{BlobFormat, Hash};

/// Address information embedded in a share ticket.
///
/// `Id` produces the smallest ticket (node id only); the receiver resolves the
/// node through n0-dns.  `RelayAndAddresses` (the default) is the most robust
/// and matches boru's chat FileShare tickets.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum AddrInfoOptions {
    /// Endpoint id only — receiver resolves via n0-dns address lookup.
    Id,
    /// Endpoint id + relay URL + direct addresses (default).
    #[default]
    RelayAndAddresses,
    /// Endpoint id + relay URL only.
    Relay,
    /// Endpoint id + direct addresses only.
    Addresses,
}

/// Apply address trimming to a ticket addr, mirroring sendme's `apply_options`
/// (`src/main.rs:269-294`).
pub fn apply_options(addr: &mut EndpointAddr, opts: AddrInfoOptions) {
    match opts {
        AddrInfoOptions::Id => {
            addr.addrs = Default::default();
        }
        AddrInfoOptions::RelayAndAddresses => {
            // nothing to do
        }
        AddrInfoOptions::Relay => {
            addr.addrs = addr
                .addrs
                .iter()
                .filter(|addr| matches!(addr, TransportAddr::Relay(_)))
                .cloned()
                .collect();
        }
        AddrInfoOptions::Addresses => {
            addr.addrs = addr
                .addrs
                .iter()
                .filter(|addr| matches!(addr, TransportAddr::Ip(_)))
                .cloned()
                .collect();
        }
    }
}

/// Build a share-ticket string for a blob.
///
/// The ticket grants access to exactly `hash` in `format`, hosted by `addr`
/// (with the selected address info).  Defaults to [`AddrInfoOptions::RelayAndAddresses`]
/// when `opts` is omitted.
pub fn make_share_ticket(
    addr: EndpointAddr,
    hash: Hash,
    format: BlobFormat,
    opts: AddrInfoOptions,
) -> String {
    let mut addr = addr;
    apply_options(&mut addr, opts);
    BlobTicket::new(addr, hash, format).to_string()
}

/// Preflight information for a pasted ticket.
#[derive(Debug, Clone)]
pub struct TicketPreflight {
    /// The content hash.
    pub hash: Hash,
    /// The blob format.
    pub format: BlobFormat,
    /// Provider node id.
    pub node_id: PublicKey,
    /// Total payload bytes.
    pub total_size: u64,
    /// Number of child blobs (1 for Raw; N for HashSeq).
    pub child_count: u64,
}

/// Connect to the ticket's node and read size information without downloading
/// payload bytes.
///
/// - Raw blobs use a verified size probe (`get_verified_size`).
/// - HashSeq collections use `get_hash_seq_and_sizes` (bounded by
///   [`crate::catalogue_limits::MAX_FILE_SIZE_BYTES`]).
///
/// Id-only tickets work here because `endpoint` resolves the node through its
/// registered address lookups (n0-dns).
pub async fn preflight_ticket(endpoint: &Endpoint, ticket: &BlobTicket) -> Result<TicketPreflight> {
    let addr = ticket.addr().clone();
    let hash = ticket.hash();
    let format = ticket.format();
    let node_id = ticket.addr().id;
    let connection = endpoint
        .connect(addr, iroh_blobs::protocol::ALPN)
        .await
        .map_err(|e| anyhow!("ticket preflight: connect failed: {e}"))?;
    match format {
        BlobFormat::Raw => {
            let (size, _stats) = iroh_blobs::get::request::get_verified_size(&connection, &hash)
                .await
                .map_err(|e| anyhow!("ticket preflight: size probe failed: {e}"))?;
            Ok(TicketPreflight {
                hash,
                format,
                node_id,
                total_size: size,
                child_count: 1,
            })
        }
        BlobFormat::HashSeq => {
            let max_size = crate::catalogue_limits::MAX_FILE_SIZE_BYTES;
            let (_hash_seq, sizes) = iroh_blobs::get::request::get_hash_seq_and_sizes(
                &connection,
                &hash,
                max_size,
                None,
            )
            .await
            .map_err(|e| anyhow!("ticket preflight: collection probe failed: {e}"))?;
            let total = sizes.iter().copied().sum::<u64>();
            Ok(TicketPreflight {
                hash,
                format,
                node_id,
                total_size: total,
                child_count: sizes.len() as u64,
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use iroh::RelayUrl;
    use std::net::SocketAddr;
    use std::str::FromStr;

    fn sample_addr() -> EndpointAddr {
        let sk = iroh::SecretKey::generate();
        let mut addr = EndpointAddr::new(sk.public());
        addr = addr.with_relay_url(RelayUrl::from_str("https://relay.example.com").unwrap());
        addr = addr.with_ip_addr("127.0.0.1:8080".parse::<SocketAddr>().unwrap());
        addr
    }

    fn sample_hash() -> Hash {
        Hash::from([7u8; 32])
    }

    /// Parse/serialize round-trip preserves the full ticket parts when the
    /// default (RelayAndAddresses) trimming is used.
    #[test]
    fn ticket_roundtrip_relay_and_addresses() {
        let addr = sample_addr();
        let hash = sample_hash();
        let format = BlobFormat::Raw;
        let ticket_str = make_share_ticket(
            addr.clone(),
            hash,
            format,
            AddrInfoOptions::RelayAndAddresses,
        );

        let parsed = ticket_str.parse::<BlobTicket>().expect("ticket parses");
        assert_eq!(parsed.hash(), hash);
        assert_eq!(parsed.format(), format);
        assert_eq!(parsed.addr().id, addr.id);
        assert_eq!(parsed.addr().addrs, addr.addrs, "addresses preserved");
    }

    /// Round-trip of an Id-only ticket keeps the node id + hash but drops all
    /// transport addresses — the ticket still parses and is usable via n0-dns.
    #[test]
    fn ticket_roundtrip_id_only() {
        let addr = sample_addr();
        let hash = sample_hash();
        let format = BlobFormat::HashSeq;
        let ticket_str = make_share_ticket(addr.clone(), hash, format, AddrInfoOptions::Id);

        let parsed = ticket_str.parse::<BlobTicket>().expect("ticket parses");
        assert_eq!(parsed.hash(), hash);
        assert_eq!(parsed.format(), format);
        assert_eq!(parsed.addr().id, addr.id);
        assert!(
            parsed.addr().addrs.is_empty(),
            "Id-only ticket has no addrs"
        );
    }

    /// Id trimming clears all transport addresses.
    #[test]
    fn apply_options_id_clears_addrs() {
        let mut addr = sample_addr();
        assert!(!addr.addrs.is_empty());
        apply_options(&mut addr, AddrInfoOptions::Id);
        assert!(addr.addrs.is_empty());
    }

    /// RelayAndAddresses is a no-op.
    #[test]
    fn apply_options_relay_and_addresses_keeps_all() {
        let mut addr = sample_addr();
        let before = addr.addrs.clone();
        apply_options(&mut addr, AddrInfoOptions::RelayAndAddresses);
        assert_eq!(addr.addrs, before);
    }

    /// Relay trimming keeps only relay URLs.
    #[test]
    fn apply_options_relay_keeps_only_relays() {
        let mut addr = sample_addr();
        apply_options(&mut addr, AddrInfoOptions::Relay);
        assert!(!addr.addrs.is_empty());
        assert!(addr.addrs.iter().all(|a| a.is_relay()));
    }

    /// Addresses trimming keeps only IP addresses.
    #[test]
    fn apply_options_addresses_keeps_only_ips() {
        let mut addr = sample_addr();
        apply_options(&mut addr, AddrInfoOptions::Addresses);
        assert!(!addr.addrs.is_empty());
        assert!(addr.addrs.iter().all(|a| a.is_ip()));
    }
}
