//! Media datagram framing and datagram capacity sizing for call media.
//!
//! The media packet header is deliberately encoded by hand: media packets are
//! frequent and small, so serializing each one with postcard would add
//! avoidable work and make the wire format less explicit.

use std::fmt;
use std::time::{Duration, Instant};

use super::CallId;

#[cfg(feature = "net")]
use tokio::sync::mpsc;

// ── Fixed media datagram header (BCL1) ─────────────────────────────────────

/// Number of bytes in the fixed media datagram header.
pub const MEDIA_HEADER_LEN: usize = 40;
/// Current media datagram protocol version.
pub const MEDIA_VERSION: u8 = 1;
/// Bit indicating that a video fragment starts a keyframe.
pub const FLAG_KEYFRAME: u16 = 1 << 0;
/// Bit indicating a discontinuity in the media sequence.
pub const FLAG_DISCONTINUITY: u16 = 1 << 1;
const KNOWN_FLAGS: u16 = FLAG_KEYFRAME | FLAG_DISCONTINUITY;

/// The media pipeline to which a datagram belongs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MediaKind {
    /// Audio media.
    Audio,
    /// Video media.
    Video,
}

impl MediaKind {
    fn to_wire(self) -> u8 {
        match self {
            Self::Audio => 1,
            Self::Video => 2,
        }
    }

    fn from_wire(value: u8) -> Result<Self, MediaDatagramError> {
        match value {
            1 => Ok(Self::Audio),
            2 => Ok(Self::Video),
            kind => Err(MediaDatagramError::UnknownKind(kind)),
        }
    }
}

/// A parsed media datagram, including its unmodified payload.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MediaDatagram {
    /// Audio or video routing key.
    pub kind: MediaKind,
    /// Reserved protocol flags, including [`FLAG_KEYFRAME`].
    pub flags: u16,
    /// Call to which this packet belongs.
    pub call_id: CallId,
    /// Codec track identifier within the call.
    pub track_id: u32,
    /// Monotonically increasing packet sequence number.
    pub sequence: u32,
    /// Codec timestamp (units are negotiated by the media pipeline).
    pub timestamp: u32,
    /// Zero-based fragment number.
    pub fragment_index: u16,
    /// Number of fragments in this media sample.
    pub fragment_count: u16,
    /// Encoded media bytes.
    pub payload: Vec<u8>,
}

impl MediaDatagram {
    /// Encode a datagram using the fixed version-1 wire header.
    pub fn encode(&self) -> Vec<u8> {
        let mut encoded = Vec::with_capacity(MEDIA_HEADER_LEN + self.payload.len());
        encoded.extend_from_slice(b"BCL1");
        encoded.push(MEDIA_VERSION);
        encoded.push(self.kind.to_wire());
        encoded.extend_from_slice(&self.flags.to_be_bytes());
        encoded.extend_from_slice(self.call_id.as_bytes());
        encoded.extend_from_slice(&self.track_id.to_be_bytes());
        encoded.extend_from_slice(&self.sequence.to_be_bytes());
        encoded.extend_from_slice(&self.timestamp.to_be_bytes());
        encoded.extend_from_slice(&self.fragment_index.to_be_bytes());
        encoded.extend_from_slice(&self.fragment_count.to_be_bytes());
        encoded.extend_from_slice(&self.payload);
        encoded
    }

    /// Parse and validate a complete media datagram.
    pub fn parse(input: &[u8]) -> Result<Self, MediaDatagramError> {
        if input.len() < MEDIA_HEADER_LEN {
            return Err(MediaDatagramError::TruncatedHeader {
                actual: input.len(),
            });
        }
        if input[..4] != *b"BCL1" {
            return Err(MediaDatagramError::BadMagic);
        }
        if input[4] != MEDIA_VERSION {
            return Err(MediaDatagramError::BadVersion(input[4]));
        }
        let kind = MediaKind::from_wire(input[5])?;
        let flags = u16::from_be_bytes([input[6], input[7]]);
        if flags & !KNOWN_FLAGS != 0 {
            return Err(MediaDatagramError::InvalidFlags(flags));
        }
        let mut call_bytes = [0u8; 16];
        call_bytes.copy_from_slice(&input[8..24]);
        if call_bytes.iter().all(|byte| *byte == 0) {
            return Err(MediaDatagramError::InvalidCallId);
        }
        let call_id = CallId::from_bytes(call_bytes);
        let track_id = u32::from_be_bytes(input[24..28].try_into().unwrap());
        let sequence = u32::from_be_bytes(input[28..32].try_into().unwrap());
        let timestamp = u32::from_be_bytes(input[32..36].try_into().unwrap());
        let fragment_index = u16::from_be_bytes(input[36..38].try_into().unwrap());
        let fragment_count = u16::from_be_bytes(input[38..40].try_into().unwrap());
        if fragment_count == 0 {
            return Err(MediaDatagramError::InvalidFragmentCount);
        }
        if fragment_index >= fragment_count {
            return Err(MediaDatagramError::FragmentIndexOutOfBounds {
                index: fragment_index,
                count: fragment_count,
            });
        }

        Ok(Self {
            kind,
            flags,
            call_id,
            track_id,
            sequence,
            timestamp,
            fragment_index,
            fragment_count,
            payload: input[MEDIA_HEADER_LEN..].to_vec(),
        })
    }
}

/// Item emitted by the single media reader task for a call connection.
#[cfg(feature = "net")]
#[derive(Debug)]
pub enum MediaReaderEvent {
    /// A validated packet ready for routing to audio or video.
    Packet(MediaDatagram),
    /// A malformed packet. The reader remains alive for subsequent packets.
    Malformed(MediaDatagramError),
}

/// Read and parse every datagram on one call connection.
///
/// This is intentionally the only code path that calls
/// `Connection::read_datagram`. Audio and video consumers receive parsed
/// events from `events`; they must never read from the connection themselves.
#[cfg(feature = "net")]
pub async fn media_reader(
    connection: iroh::endpoint::Connection,
    events: mpsc::Sender<MediaReaderEvent>,
) {
    while let Ok(datagram) = connection.read_datagram().await {
        let event = match MediaDatagram::parse(datagram.as_ref()) {
            Ok(packet) => MediaReaderEvent::Packet(packet),
            Err(error) => MediaReaderEvent::Malformed(error),
        };
        if events.send(event).await.is_err() {
            break;
        }
    }
}

// ── Datagram capacity sizing ───────────────────────────────────────────────

/// Bytes reserved at the start of every media datagram.
///
/// This is the media framing budget; fragmentation is deliberately handled by
/// a later phase and is not part of this module.
pub const MEDIA_HEADER_SIZE: usize = 16;

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

/// Errors encountered while framing or sizing media datagrams.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MediaDatagramError {
    // Header validation errors.
    /// The input was shorter than the fixed header.
    TruncatedHeader {
        /// Number of bytes received.
        actual: usize,
    },
    /// The four-byte protocol marker did not match.
    BadMagic,
    /// The protocol version is not supported.
    BadVersion(u8),
    /// The media kind byte is not defined by this protocol.
    UnknownKind(u8),
    /// One or more reserved flag bits were set.
    InvalidFlags(u16),
    /// The call identity was all zeroes.
    InvalidCallId,
    /// A datagram must contain at least one fragment.
    InvalidFragmentCount,
    /// The fragment index must be less than the fragment count.
    FragmentIndexOutOfBounds {
        /// Zero-based fragment index received.
        index: u16,
        /// Number of fragments declared by the packet.
        count: u16,
    },
    // Datagram capacity errors.
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

impl fmt::Display for MediaDatagramError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::TruncatedHeader { actual } => {
                write!(formatter, "truncated media header ({actual} bytes)")
            }
            Self::BadMagic => formatter.write_str("invalid media datagram magic"),
            Self::BadVersion(version) => {
                write!(formatter, "unsupported media datagram version {version}")
            }
            Self::UnknownKind(kind) => write!(formatter, "unknown media kind {kind}"),
            Self::InvalidFlags(flags) => write!(formatter, "unknown media flags 0x{flags:04x}"),
            Self::InvalidCallId => formatter.write_str("media datagram has an all-zero call id"),
            Self::InvalidFragmentCount => formatter.write_str("media datagram has zero fragments"),
            Self::FragmentIndexOutOfBounds { index, count } => {
                write!(
                    formatter,
                    "media fragment index {index} is outside count {count}"
                )
            }
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn sample() -> MediaDatagram {
        MediaDatagram {
            kind: MediaKind::Video,
            flags: FLAG_KEYFRAME,
            call_id: CallId::generate(),
            track_id: 7,
            sequence: 0x0102_0304,
            timestamp: 48_000,
            fragment_index: 1,
            fragment_count: 3,
            payload: vec![0, 1, 2, 255],
        }
    }

    #[test]
    fn header_and_payload_round_trip() {
        let original = sample();
        let encoded = original.encode();
        assert_eq!(encoded.len(), MEDIA_HEADER_LEN + original.payload.len());
        assert_eq!(MediaDatagram::parse(&encoded), Ok(original));
    }

    #[test]
    fn rejects_each_invalid_header_case() {
        let valid = sample().encode();
        let mut bad = valid.clone();
        bad[..4].copy_from_slice(b"NOPE");
        assert_eq!(
            MediaDatagram::parse(&bad),
            Err(MediaDatagramError::BadMagic)
        );

        let mut bad = valid.clone();
        bad[4] = 9;
        assert_eq!(
            MediaDatagram::parse(&bad),
            Err(MediaDatagramError::BadVersion(9))
        );

        let mut bad = valid.clone();
        bad[5] = 9;
        assert_eq!(
            MediaDatagram::parse(&bad),
            Err(MediaDatagramError::UnknownKind(9))
        );

        assert!(matches!(
            MediaDatagram::parse(&valid[..MEDIA_HEADER_LEN - 1]),
            Err(MediaDatagramError::TruncatedHeader { .. })
        ));

        let mut bad = valid.clone();
        bad[38..40].copy_from_slice(&0u16.to_be_bytes());
        assert_eq!(
            MediaDatagram::parse(&bad),
            Err(MediaDatagramError::InvalidFragmentCount)
        );

        let mut bad = valid;
        bad[36..38].copy_from_slice(&3u16.to_be_bytes());
        assert!(matches!(
            MediaDatagram::parse(&bad),
            Err(MediaDatagramError::FragmentIndexOutOfBounds { index: 3, count: 3 })
        ));
    }

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
