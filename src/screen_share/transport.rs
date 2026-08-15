//! Iroh/QUIC transport for screen-share control messages and disposable media units.
//!
//! Control messages use one reliable length-delimited bidirectional stream per
//! exchange. Encoded frames use short-lived streams: an old in-flight frame is
//! less useful than the newest frame, so callers can bound work without making
//! the control stream head-of-line blocked by video data.
#![allow(missing_docs)]

use std::sync::atomic::{AtomicU64, Ordering};

use serde::{Deserialize, Serialize};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

use super::{codec::EncodedFrame, protocol::{self, ControlMessage, ScreenShareMessage, SCREEN_SHARE_PROTOCOL_VERSION}, ScreenShareError};

/// Synchronous boundary used by capture/codec pipelines that do not know about QUIC.
pub trait ScreenTransport: Send {
    fn send(&mut self, frame: EncodedFrame) -> Result<(), ScreenShareError>;
}

/// Maximum encoded media unit accepted from an untrusted peer.
pub const MAX_MEDIA_FRAME: usize = 4 * 1024 * 1024;
/// Maximum media header size.
pub const MAX_MEDIA_HEADER: usize = 256;
/// Frames older than this many sequence numbers than the playout point are dropped.
pub const MAX_LATE_SEQUENCE_DISTANCE: u64 = 120;
const CONTROL_KIND: u8 = 0x01;
const MEDIA_KIND: u8 = 0x02;
/// Versioned protocol message (negotiation and lifecycle) frame kind.
const SCREEN_SHARE_KIND: u8 = 0x03;

/// Diagnostics describing the selected QUIC path. Transport behavior never
/// depends on this value.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PathKind { Unknown, Direct, Relay }

/// Classify the SELECTED path of a live iroh connection (BORU-SS-39).
///
/// iroh reports one path per address family; the selected path is the one
/// actually carrying traffic. A relay path (via the configured relay server)
/// is `Relay`; an IP path is `Direct`; anything else (not yet selected,
/// pathless) is `Unknown`. Used by the host to pick the initial quality
/// preset and to detect mid-session Direct↔Relay switches.
pub fn selected_path_kind(connection: &iroh::endpoint::Connection) -> PathKind {
    connection
        .paths()
        .iter()
        .find(|path| path.is_selected())
        .map(|path| {
            if path.is_relay() {
                PathKind::Relay
            } else if path.is_ip() {
                PathKind::Direct
            } else {
                PathKind::Unknown
            }
        })
        .unwrap_or(PathKind::Unknown)
}

/// Counters useful to a viewer/debug harness. They are monotonic and contain no peer-identifying data.
#[derive(Debug, Default)]
pub struct TransportCounters {
    pub bytes_sent: AtomicU64,
    pub frames_sent: AtomicU64,
    pub media_streams_reset: AtomicU64,
    pub frames_received: AtomicU64,
    pub late_frames_dropped: AtomicU64,
    pub decode_errors: AtomicU64,
    pub bytes_in_flight: AtomicU64,
}

/// Compact media header. Payload bytes are kept outside the postcard header.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct MediaHeader {
    pub version: u16,
    pub session_id: [u8; 16],
    pub sequence: u64,
    pub timestamp_us: u64,
    /// Encode-stage timestamp (PDF Task 7.2) so the viewer can measure
    /// end-to-end latency: capture→encode (encode_timestamp - timestamp) and
    /// encode→receive (receive time - encode_timestamp).
    pub encode_timestamp_us: u64,
    pub codec: u8,
    pub flags: u8,
    pub width: u16,
    pub height: u16,
    pub config_generation: u64,
    pub payload_len: u32,
}

impl MediaHeader {
    pub const FLAG_KEYFRAME: u8 = 1;
    pub const FLAG_CODEC_CONFIG: u8 = 2;
    pub fn validate(&self) -> Result<(), ScreenShareError> {
        if self.version != SCREEN_SHARE_PROTOCOL_VERSION { return Err(ScreenShareError::new("unsupported media protocol version")); }
        if self.session_id == [0; 16] { return Err(ScreenShareError::new("media session id is empty")); }
        if self.codec == 0 || self.codec > 8 { return Err(ScreenShareError::new("invalid media codec")); }
        if self.width == 0 || self.height == 0 || self.width > 16_384 || self.height > 16_384 { return Err(ScreenShareError::new("invalid media dimensions")); }
        if self.payload_len == 0 || self.payload_len as usize > MAX_MEDIA_FRAME { return Err(ScreenShareError::new("media payload exceeds limit")); }
        if self.flags & !(Self::FLAG_KEYFRAME | Self::FLAG_CODEC_CONFIG) != 0 { return Err(ScreenShareError::new("invalid media flags")); }
        Ok(())
    }
}

/// Encode a bounded media unit for a disposable QUIC stream.
pub fn encode_media(session_id: [u8; 16], frame: &EncodedFrame) -> Result<Vec<u8>, ScreenShareError> {
    if frame.bytes.is_empty() || frame.bytes.len() > MAX_MEDIA_FRAME { return Err(ScreenShareError::new("encoded frame exceeds limit")); }
    if frame.width == 0 || frame.height == 0 || frame.width > u16::MAX as u32 || frame.height > u16::MAX as u32 { return Err(ScreenShareError::new("encoded frame dimensions are invalid")); }
    let header = MediaHeader { version: SCREEN_SHARE_PROTOCOL_VERSION, session_id, sequence: frame.sequence, timestamp_us: frame.timestamp_us, encode_timestamp_us: frame.encode_timestamp_us, codec: 1, flags: if frame.keyframe { MediaHeader::FLAG_KEYFRAME } else { 0 }, width: frame.width as u16, height: frame.height as u16, config_generation: frame.config_generation, payload_len: frame.bytes.len() as u32 };
    let header_bytes = postcard::to_stdvec(&header).map_err(|e| ScreenShareError::new(e.to_string()))?;
    if header_bytes.len() > MAX_MEDIA_HEADER { return Err(ScreenShareError::new("media header exceeds limit")); }
    let mut out = Vec::with_capacity(1 + 2 + header_bytes.len() + frame.bytes.len());
    out.push(MEDIA_KIND);
    out.extend_from_slice(&(header_bytes.len() as u16).to_be_bytes());
    out.extend_from_slice(&header_bytes);
    out.extend_from_slice(&frame.bytes);
    Ok(out)
}

/// Decode and validate one complete media unit before allocating based on its header.
pub fn decode_media(bytes: &[u8]) -> Result<(MediaHeader, Vec<u8>), ScreenShareError> {
    if bytes.len() < 3 || bytes[0] != MEDIA_KIND { return Err(ScreenShareError::new("invalid media unit")); }
    let header_len = u16::from_be_bytes([bytes[1], bytes[2]]) as usize;
    if header_len == 0 || header_len > MAX_MEDIA_HEADER || bytes.len() < 3 + header_len { return Err(ScreenShareError::new("invalid media header length")); }
    let header: MediaHeader = postcard::from_bytes(&bytes[3..3 + header_len]).map_err(|e| ScreenShareError::new(e.to_string()))?;
    header.validate()?;
    let payload = &bytes[3 + header_len..];
    if payload.len() != header.payload_len as usize { return Err(ScreenShareError::new("media payload length mismatch")); }
    Ok((header, payload.to_vec()))
}

/// A bounded latest-frame queue. It intentionally discards stale non-keyframes.
#[derive(Debug)]
pub struct LatestFrameQueue { latest: Option<(MediaHeader, Vec<u8>)>, max_depth: usize }
impl LatestFrameQueue {
    pub fn new(max_depth: usize) -> Result<Self, ScreenShareError> { if max_depth == 0 { return Err(ScreenShareError::new("queue depth must be non-zero")); } Ok(Self { latest: None, max_depth: max_depth.min(2) }) }
    pub fn push(&mut self, header: MediaHeader, payload: Vec<u8>) {
        if self.latest.as_ref().is_some_and(|(old, _)| old.sequence > header.sequence && header.flags & MediaHeader::FLAG_KEYFRAME == 0) { return; }
        self.latest = Some((header, payload));
    }
    pub fn take_latest(&mut self) -> Option<(MediaHeader, Vec<u8>)> { self.latest.take() }
    pub fn len(&self) -> usize { usize::from(self.latest.is_some()).min(self.max_depth) }
}

/// Async QUIC transport handle. It can be cloned for control/media producers.
#[derive(Debug, Clone)]
pub struct QuicScreenTransport { connection: iroh::endpoint::Connection, pub counters: std::sync::Arc<TransportCounters>, session_id: [u8; 16] }

impl QuicScreenTransport {
    pub fn new(connection: iroh::endpoint::Connection, session_id: [u8; 16]) -> Result<Self, ScreenShareError> { if session_id == [0; 16] { return Err(ScreenShareError::new("session id is empty")); } Ok(Self { connection, counters: std::sync::Arc::new(TransportCounters::default()), session_id }) }
    pub fn session_id(&self) -> [u8; 16] { self.session_id }
    pub fn path_kind(&self) -> PathKind { selected_path_kind(&self.connection) }
    pub async fn send_control(&self, message: &ControlMessage) -> Result<(), ScreenShareError> {
        let bytes = protocol::encode(message).map_err(|e| ScreenShareError::new(e.to_string()))?;
        let (mut send, _) = self.connection.open_bi().await.map_err(|e| ScreenShareError::new(e.to_string()))?;
        send.write_u8(CONTROL_KIND).await.map_err(|e| ScreenShareError::new(e.to_string()))?;
        send.write_u32(bytes.len() as u32).await.map_err(|e| ScreenShareError::new(e.to_string()))?;
        send.write_all(&bytes).await.map_err(|e| ScreenShareError::new(e.to_string()))?;
        send.finish().map_err(|e| ScreenShareError::new(e.to_string()))?;
        Ok(())
    }
    /// Send one versioned protocol message (negotiation/lifecycle) on a fresh
    /// reliable stream, using the versioned message framing.
    pub async fn send_screen_share(&self, message: &ScreenShareMessage) -> Result<(), ScreenShareError> {
        let bytes = message.encode().map_err(|e| ScreenShareError::new(e.to_string()))?;
        let (mut send, _) = self.connection.open_bi().await.map_err(|e| ScreenShareError::new(e.to_string()))?;
        send.write_u8(SCREEN_SHARE_KIND).await.map_err(|e| ScreenShareError::new(e.to_string()))?;
        send.write_u32(bytes.len() as u32).await.map_err(|e| ScreenShareError::new(e.to_string()))?;
        send.write_all(&bytes).await.map_err(|e| ScreenShareError::new(e.to_string()))?;
        send.finish().map_err(|e| ScreenShareError::new(e.to_string()))?;
        Ok(())
    }
    pub async fn send_frame(&self, frame: &EncodedFrame) -> Result<(), ScreenShareError> {
        let unit = encode_media(self.session_id, frame)?;
        self.counters.bytes_in_flight.fetch_add(unit.len() as u64, Ordering::Relaxed);
        let (mut send, _) = self.connection.open_bi().await.map_err(|e| ScreenShareError::new(e.to_string()))?;
        send.write_all(&unit).await.map_err(|e| ScreenShareError::new(e.to_string()))?;
        send.finish().map_err(|e| ScreenShareError::new(e.to_string()))?;
        self.counters.bytes_in_flight.fetch_sub(unit.len() as u64, Ordering::Relaxed);
        self.counters.bytes_sent.fetch_add(unit.len() as u64, Ordering::Relaxed);
        self.counters.frames_sent.fetch_add(1, Ordering::Relaxed);
        Ok(())
    }
}

/// Read a bounded control or media stream. The caller owns session lifecycle decisions.
pub async fn read_unit(mut recv: iroh::endpoint::RecvStream) -> Result<ReadUnit, ScreenShareError> {
    let kind = recv.read_u8().await.map_err(|e| ScreenShareError::new(e.to_string()))?;
    match kind {
        CONTROL_KIND => {
            let len = recv.read_u32().await.map_err(|e| ScreenShareError::new(e.to_string()))? as usize;
            if len == 0 || len > super::protocol::MAX_CONTROL_FRAME { return Err(ScreenShareError::new("control frame exceeds limit")); }
            let mut bytes = vec![0; len]; recv.read_exact(&mut bytes).await.map_err(|e| ScreenShareError::new(e.to_string()))?;
            protocol::decode(&bytes).map(ReadUnit::Control).map_err(|e| ScreenShareError::new(e.to_string()))
        }
        SCREEN_SHARE_KIND => {
            let len = recv.read_u32().await.map_err(|e| ScreenShareError::new(e.to_string()))? as usize;
            if len == 0 || len > super::protocol::MAX_SCREEN_SHARE_MESSAGE { return Err(ScreenShareError::new("screen-share message exceeds limit")); }
            let mut bytes = vec![0; len]; recv.read_exact(&mut bytes).await.map_err(|e| ScreenShareError::new(e.to_string()))?;
            ScreenShareMessage::decode(&bytes).map(ReadUnit::ScreenShare).map_err(|e| ScreenShareError::new(e.to_string()))
        }
        MEDIA_KIND => {
            let rest = recv.read_to_end(MAX_MEDIA_FRAME + MAX_MEDIA_HEADER + 3).await.map_err(|e| ScreenShareError::new(e.to_string()))?;
            let mut unit = Vec::with_capacity(1 + rest.len()); unit.push(MEDIA_KIND); unit.extend_from_slice(&rest);
            decode_media(&unit).map(|(header, payload)| ReadUnit::Media(header, payload))
        }
        _ => Err(ScreenShareError::new("unknown screen-share stream kind")),
    }
}

#[derive(Debug)]
pub enum ReadUnit { Control(ControlMessage), ScreenShare(ScreenShareMessage), Media(MediaHeader, Vec<u8>) }

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn media_round_trip_and_bounds() {
        let frame = EncodedFrame { timestamp_us: 7, encode_timestamp_us: 9, sequence: 9, keyframe: true, config_generation: 2, width: 1280, height: 720, bytes: vec![4, 5, 6] };
        let bytes = encode_media([1; 16], &frame).unwrap();
        let (header, payload) = decode_media(&bytes).unwrap();
        assert_eq!(header.sequence, 9); assert_eq!(payload, frame.bytes);
        assert_eq!(header.encode_timestamp_us, 9, "encode timestamp must ride the wire");
        assert!(decode_media(&bytes[..bytes.len() - 1]).is_err());
    }
    #[test]
    fn queue_keeps_current_state_bounded() {
        let mut q = LatestFrameQueue::new(8).unwrap();
        let mut h = MediaHeader { version: 1, session_id: [1;16], sequence: 2, timestamp_us: 0, encode_timestamp_us: 0, codec: 1, flags: 0, width: 1, height: 1, config_generation: 0, payload_len: 1 };
        q.push(h, vec![2]); h.sequence = 1; q.push(h, vec![1]); assert_eq!(q.take_latest().unwrap().0.sequence, 2);
    }
    #[test]
    fn hostile_header_is_rejected_before_payload_use() {
        let mut bytes = vec![MEDIA_KIND, 1, 0, 0]; bytes.extend_from_slice(&[0; 32]);
        assert!(decode_media(&bytes).is_err());
    }
}
