//! Datagram sizing for call media payloads.
//!
//! QUIC datagram capacity is negotiated per connection and may change when the
//! path changes. Callers must obtain the capacity here rather than assuming a
//! common Ethernet MTU.

use std::time::{Duration, Instant};

/// Bytes reserved at the start of every media datagram.
///
/// This is the media framing budget; fragmentation is deliberately handled by
/// a later phase and is not part of this module.
pub const MEDIA_HEADER_SIZE: usize = 16;

/// Errors encountered while determining media datagram capacity.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MediaDatagramError {
    /// The peer or transport does not support QUIC datagrams.
    DatagramsUnavailable,
    /// The negotiated datagram is too small for the media framing header.
    DatagramTooSmall {
        /// Negotiated maximum datagram size.
        maximum: usize,
        /// Bytes required by the media header.
        header: usize,
    },
}

impl std::fmt::Display for MediaDatagramError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::DatagramsUnavailable => {
                formatter.write_str("connection does not support datagrams")
            }
            Self::DatagramTooSmall { maximum, header } => write!(
                formatter,
                "datagram size {maximum} is smaller than media header {header}"
            ),
        }
    }
}

impl std::error::Error for MediaDatagramError {}

/// Convert a negotiated datagram size into room for an encoded media payload.
///
/// The checked subtraction is intentional: a malformed or unexpectedly small
/// negotiated value must become a typed error, never an integer underflow.
pub fn payload_capacity(datagram_size: usize) -> Result<usize, MediaDatagramError> {
    datagram_size
        .checked_sub(MEDIA_HEADER_SIZE)
        .ok_or(MediaDatagramError::DatagramTooSmall {
            maximum: datagram_size,
            header: MEDIA_HEADER_SIZE,
        })
}

/// A small cache for a connection's current datagram capacity.
///
/// The cache is refreshed on the first request and after `refresh_interval`.
/// A caller that encodes frames less frequently than this interval still gets
/// a current value on the next request; use [`Self::refresh`] to force an
/// immediate path-MTU recheck.
#[derive(Debug, Clone)]
pub struct DatagramSizer {
    refresh_interval: Duration,
    last_refresh: Option<Instant>,
    last_maximum: Option<usize>,
}

impl DatagramSizer {
    /// Create a sizer that rechecks the connection at the given interval.
    pub const fn new(refresh_interval: Duration) -> Self {
        Self {
            refresh_interval,
            last_refresh: None,
            last_maximum: None,
        }
    }

    /// Construct a sizer that checks on every request.
    pub const fn per_frame() -> Self {
        Self::new(Duration::ZERO)
    }

    /// Forget the cached value so the next request re-reads the connection.
    pub fn refresh(&mut self) {
        self.last_refresh = None;
        self.last_maximum = None;
    }

    /// Read the negotiated size from an optional provider value.
    ///
    /// This narrow method keeps the unavailable-datagram behavior testable
    /// without manufacturing a live QUIC connection.
    pub fn payload_capacity_from(
        &mut self,
        maximum: Option<usize>,
    ) -> Result<usize, MediaDatagramError> {
        let now = Instant::now();
        let should_refresh = self
            .last_refresh
            .is_none_or(|at| now.duration_since(at) >= self.refresh_interval);
        if should_refresh {
            let maximum = maximum.ok_or(MediaDatagramError::DatagramsUnavailable)?;
            // Validate before replacing the cached value, so a transiently
            // invalid value cannot make a previous valid cache look usable.
            let capacity = payload_capacity(maximum)?;
            self.last_maximum = Some(maximum);
            self.last_refresh = Some(now);
            Ok(capacity)
        } else {
            // A successful refresh always stores a validated maximum.
            payload_capacity(self.last_maximum.expect("refresh cache invariant"))
        }
    }

    /// Read the current datagram size from an Iroh connection.
    #[cfg(feature = "net")]
    pub fn payload_capacity(
        &mut self,
        connection: &iroh::endpoint::Connection,
    ) -> Result<usize, MediaDatagramError> {
        self.payload_capacity_from(connection.max_datagram_size())
    }
}

#[cfg(test)]
mod tests {
    use super::{payload_capacity, DatagramSizer, MediaDatagramError, MEDIA_HEADER_SIZE};
    use std::time::Duration;

    #[test]
    fn payload_capacity_subtracts_media_header() {
        assert_eq!(payload_capacity(1200), Ok(1200 - MEDIA_HEADER_SIZE));
    }

    #[test]
    fn payload_capacity_rejects_datagrams_smaller_than_header() {
        assert_eq!(
            payload_capacity(MEDIA_HEADER_SIZE - 1),
            Err(MediaDatagramError::DatagramTooSmall {
                maximum: MEDIA_HEADER_SIZE - 1,
                header: MEDIA_HEADER_SIZE,
            })
        );
    }

    #[test]
    fn unavailable_datagrams_are_typed_error() {
        let mut sizer = DatagramSizer::per_frame();
        assert_eq!(
            sizer.payload_capacity_from(None),
            Err(MediaDatagramError::DatagramsUnavailable)
        );
    }

    #[test]
    fn cache_refreshes_after_interval() {
        let mut sizer = DatagramSizer::new(Duration::from_secs(3600));
        assert_eq!(
            sizer.payload_capacity_from(Some(1200)),
            Ok(1200 - MEDIA_HEADER_SIZE)
        );
        // The long interval means this request intentionally uses the cached
        // value, even though the provider reports a changed path MTU.
        assert_eq!(
            sizer.payload_capacity_from(Some(900)),
            Ok(1200 - MEDIA_HEADER_SIZE)
        );
        sizer.refresh();
        assert_eq!(
            sizer.payload_capacity_from(Some(900)),
            Ok(900 - MEDIA_HEADER_SIZE)
        );
    }
}
