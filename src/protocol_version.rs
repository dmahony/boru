//! Versioned wire-frame protocol for catalogue retrieval.
//!
//! Provides an ALPN constant, a version marker, and thin async
//! `read_frame` / `write_frame` helpers that wrap a length-prefixed,
//! versioned message format.
//!
//! The client writes: `[version: u16 LE][payload_length: u32 LE][payload bytes]`
//! The server writes the same structure on the response stream.

use std::io;

use bytes::Bytes;
use iroh::endpoint::{RecvStream, SendStream};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tracing::warn;

// ── ALPN ─────────────────────────────────────────────────────────────────

/// ALPN for the catalogue retrieval protocol.
///
/// Matches `net::FILE_CATALOG_ALPN` (`/boru-file-catalog/1`).
pub const CATALOGUE_ALPN: &[u8] = b"/boru-file-catalog/1";

// ── Version constants ─────────────────────────────────────────────────────

/// The current wire version for catalogue frames.
pub const CATALOGUE_RETRIEVAL_V1: u16 = 1;

/// All versions the current code understands.
///
/// Add new versions here when the wire format evolves.
pub const SUPPORTED_CATALOGUE_RETRIEVAL: &[u16] = &[1];

/// Maximum frame payload size (8 MiB).
const MAX_FRAME_PAYLOAD: usize = 8 * 1024 * 1024;

// ── write_frame ───────────────────────────────────────────────────────────

/// Write a versioned, length-prefixed frame to `stream`.
///
/// The frame consists of:
///   - `version` as u16 (little-endian)
///   - `payload.len()` as u32 (little-endian)
///   - The raw payload bytes.
///
/// Returns an `io::Error` on write failure (after wrapping in `n0_error`).
pub async fn write_frame(
    stream: &mut SendStream,
    version: u16,
    payload: &[u8],
) -> std::result::Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let len = payload.len();
    if len > MAX_FRAME_PAYLOAD {
        return Err(Box::new(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("payload too large: {len} > {MAX_FRAME_PAYLOAD}"),
        )));
    }
    stream.write_u16_le(version).await?;
    stream.write_u32_le(len as u32).await?;
    stream.write_all(payload).await?;
    Ok(())
}

// ── read_frame ────────────────────────────────────────────────────────────

/// Read a versioned, length-prefixed frame from `stream`.
///
/// Returns `(version, payload_bytes)` on success.
/// Returns `None` when the stream ends cleanly before any data.
///
/// Rejects frames whose `version` is not in `supported_versions` with an
/// `UnsupportedVersion` error.
pub async fn read_frame(
    stream: &mut RecvStream,
    supported_versions: &[u16],
    protocol_name: &str,
) -> std::result::Result<Option<(u16, Bytes)>, Box<dyn std::error::Error + Send + Sync>> {
    // Read version (2 bytes)
    let version = match stream.read_u16_le().await {
        Ok(v) => v,
        Err(err) if err.kind() == io::ErrorKind::UnexpectedEof => return Ok(None),
        Err(err) => return Err(Box::new(err)),
    };

    // Validate version
    if !supported_versions.contains(&version) {
        warn!(protocol = protocol_name, version, "unsupported version");
        return Err(Box::new(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("unsupported {protocol_name} version {version}"),
        )));
    }

    // Read payload length (4 bytes)
    let len = match stream.read_u32_le().await {
        Ok(l) => l as usize,
        Err(err) if err.kind() == io::ErrorKind::UnexpectedEof => {
            return Err(Box::new(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                format!("{protocol_name}: truncated frame (no length after version)"),
            )));
        }
        Err(err) => return Err(Box::new(err)),
    };

    if len > MAX_FRAME_PAYLOAD {
        return Err(Box::new(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("{protocol_name}: frame payload too large: {len} > {MAX_FRAME_PAYLOAD}"),
        )));
    }

    // Read payload bytes
    let mut buf = vec![0u8; len];
    let mut offset = 0;
    while offset < len {
        let n = stream
            .read(&mut buf[offset..])
            .await
            .map_err(|e| Box::new(e) as Box<dyn std::error::Error + Send + Sync>)?
            .unwrap_or(0);
        if n == 0 {
            return Err(Box::new(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                format!(
                    "{protocol_name}: truncated frame payload (expected {len} bytes, got {offset})"
                ),
            )));
        }
        offset += n;
    }
    let payload = Bytes::from(buf);

    Ok(Some((version, payload)))
}

// ── Tests ─────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    /// Version constant is set correctly.
    #[test]
    fn test_catalogue_retrieval_v1_value() {
        assert_eq!(CATALOGUE_RETRIEVAL_V1, 1);
    }

    /// Supported versions includes the current version.
    #[test]
    fn test_supported_contains_current() {
        assert!(SUPPORTED_CATALOGUE_RETRIEVAL.contains(&CATALOGUE_RETRIEVAL_V1));
    }

    /// Supported versions are sorted and unique.
    #[test]
    fn test_supported_is_sorted_and_unique() {
        let mut sorted = SUPPORTED_CATALOGUE_RETRIEVAL.to_vec();
        sorted.sort();
        sorted.dedup();
        assert_eq!(SUPPORTED_CATALOGUE_RETRIEVAL, sorted.as_slice());
    }

    /// ALPN constant matches expected value.
    #[test]
    fn test_catalogue_alpn() {
        assert_eq!(CATALOGUE_ALPN, b"/boru-file-catalog/1");
    }
}
