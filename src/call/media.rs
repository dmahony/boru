//! Compact datagrams used by the call audio and video receive paths.
//!
//! The header is deliberately encoded by hand.  Media packets are frequent and
//! small, so serializing each one with postcard would add avoidable work and
//! make the wire format less explicit.

use std::fmt;

use super::CallId;

#[cfg(feature = "net")]
use tokio::sync::mpsc;

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

/// Validation errors for a media datagram header.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MediaDatagramError {
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
        }
    }
}

impl std::error::Error for MediaDatagramError {}

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

#[cfg(test)]
mod tests {
    use super::*;

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
}
