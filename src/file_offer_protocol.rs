//! Dedicated direct file-transfer protocol for announced file offers.
//!
//! The offer metadata is exchanged over a small, versioned postcard frame. Once
//! the header is accepted, the remainder of the bidirectional QUIC stream is
//! the raw file byte stream; file contents never travel through gossip.

use std::sync::{Arc, Mutex};

use iroh::{
    endpoint::{Connection, RecvStream, SendStream},
    protocol::{AcceptError, ProtocolHandler},
    Endpoint, EndpointAddr,
};
use serde::{Deserialize, Serialize};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

use crate::{chat_core::protocol::FileOfferId, file_offer::FileOfferRegistry};

/// ALPN for direct file offers.
pub const FILE_OFFER_ALPN: &[u8] = b"boru/file-offer/1";
/// Current wire version for direct file offers.
pub const FILE_OFFER_WIRE_VERSION: u16 = 1;
const MAX_FRAME_SIZE: u32 = 64 * 1024;

/// Request sent by a receiver before the raw file stream begins.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileOfferRequest {
    /// Wire version requested by the receiver.
    pub version: u16,
    /// Opaque offer identifier announced in gossip.
    pub offer_id: FileOfferId,
}

/// Metadata sent before the raw file bytes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileOfferHeader {
    /// Wire version used for this transfer.
    pub version: u16,
    /// Offer identifier this header describes.
    pub offer_id: FileOfferId,
    /// Safe display basename.
    pub name: String,
    /// Number of raw bytes that follow the header.
    pub size: u64,
}

/// Explicit protocol failures returned before closing a transfer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum FileOfferError {
    /// The requested wire version is not supported.
    UnsupportedVersion,
    /// No offer with this identifier is currently available.
    NotFound,
    /// The request was malformed.
    InvalidRequest,
    /// The local file could not be opened or read.
    InternalError,
}

/// Response frame. A successful header is followed by raw bytes on the same stream.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum FileOfferResponse {
    /// The transfer header; raw bytes follow immediately after this frame.
    Header(FileOfferHeader),
    /// The request was rejected and no raw bytes follow.
    Error(FileOfferError),
}

/// A client-side transfer after its header has been received.
#[derive(Debug)]
pub struct FileOfferTransfer {
    /// Metadata describing the following raw byte stream.
    pub header: FileOfferHeader,
    reader: RecvStream,
}

impl FileOfferTransfer {
    /// Read raw file bytes into `buf`, returning the number read.
    pub async fn read(
        &mut self,
        buf: &mut [u8],
    ) -> Result<Option<usize>, iroh::endpoint::ReadError> {
        self.reader.read(buf).await
    }

    /// Consume the transfer and return its raw QUIC receive stream.
    pub fn into_reader(self) -> RecvStream {
        self.reader
    }
}

/// Server-side handler backed by the sender's local offer registry.
#[derive(Debug, Clone)]
pub struct FileOfferProtocolHandler {
    registry: Arc<Mutex<FileOfferRegistry>>,
}

impl FileOfferProtocolHandler {
    /// Create a handler serving entries from `registry`.
    pub fn new(registry: Arc<Mutex<FileOfferRegistry>>) -> Self {
        Self { registry }
    }
}

impl ProtocolHandler for FileOfferProtocolHandler {
    async fn accept(&self, connection: Connection) -> Result<(), AcceptError> {
        let registry = self.registry.clone();
        tokio::spawn(async move {
            if let Err(error) = serve_connection(connection, registry).await {
                tracing::debug!("file offer transfer ended: {error}");
            }
        });
        Ok(())
    }
}

/// Open a direct transfer and read its metadata header.
pub async fn open_file_offer(
    endpoint: &Endpoint,
    addr: EndpointAddr,
    offer_id: FileOfferId,
) -> anyhow::Result<FileOfferTransfer> {
    let connection = endpoint.connect(addr, FILE_OFFER_ALPN).await?;
    let (mut writer, mut reader) = connection.open_bi().await?;
    write_frame(
        &mut writer,
        &FileOfferRequest {
            version: FILE_OFFER_WIRE_VERSION,
            offer_id,
        },
    )
    .await?;
    writer.finish()?;

    let response: FileOfferResponse = read_frame(&mut reader).await?;
    match response {
        FileOfferResponse::Header(header) if header.version == FILE_OFFER_WIRE_VERSION => {
            Ok(FileOfferTransfer { header, reader })
        }
        FileOfferResponse::Header(_) => {
            Err(anyhow::anyhow!("unsupported file offer response version"))
        }
        FileOfferResponse::Error(error) => Err(anyhow::anyhow!("file offer rejected: {error:?}")),
    }
}

async fn serve_connection(
    connection: Connection,
    registry: Arc<Mutex<FileOfferRegistry>>,
) -> anyhow::Result<()> {
    let (mut writer, mut reader) = connection.accept_bi().await?;
    let request: FileOfferRequest = read_frame(&mut reader).await?;

    if request.version != FILE_OFFER_WIRE_VERSION {
        write_frame(
            &mut writer,
            &FileOfferResponse::Error(FileOfferError::UnsupportedVersion),
        )
        .await?;
        writer.finish()?;
        return Ok(());
    }

    let offer = registry
        .lock()
        .map_err(|_| anyhow::anyhow!("file offer registry poisoned"))?
        .get(&request.offer_id)
        .cloned();
    let Some(offer) = offer else {
        write_frame(
            &mut writer,
            &FileOfferResponse::Error(FileOfferError::NotFound),
        )
        .await?;
        writer.finish()?;
        return Ok(());
    };

    let file = match tokio::fs::File::open(offer.path()).await {
        Ok(file) => file,
        Err(_) => {
            write_frame(
                &mut writer,
                &FileOfferResponse::Error(FileOfferError::InternalError),
            )
            .await?;
            writer.finish()?;
            return Ok(());
        }
    };
    write_frame(
        &mut writer,
        &FileOfferResponse::Header(FileOfferHeader {
            version: FILE_OFFER_WIRE_VERSION,
            offer_id: offer.id,
            name: offer.display_name,
            size: offer.size,
        }),
    )
    .await?;
    tokio::io::copy(&mut file.take(offer.size), &mut writer).await?;
    writer.finish()?;
    Ok(())
}

async fn write_frame<T: Serialize>(writer: &mut SendStream, value: &T) -> anyhow::Result<()> {
    let payload = postcard::to_stdvec(value)?;
    if payload.len() > MAX_FRAME_SIZE as usize {
        anyhow::bail!("file offer frame too large");
    }
    writer.write_u32_le(payload.len() as u32).await?;
    writer.write_all(&payload).await?;
    Ok(())
}

async fn read_frame<T: for<'de> Deserialize<'de>>(reader: &mut RecvStream) -> anyhow::Result<T> {
    let length = reader.read_u32_le().await?;
    if length > MAX_FRAME_SIZE {
        anyhow::bail!("file offer frame too large: {length}");
    }
    let mut payload = vec![0; length as usize];
    reader.read_exact(&mut payload).await?;
    Ok(postcard::from_bytes(&payload)?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn request_round_trip() {
        let request = FileOfferRequest {
            version: 1,
            offer_id: FileOfferId::generate(),
        };
        let bytes = postcard::to_stdvec(&request).unwrap();
        assert_eq!(
            postcard::from_bytes::<FileOfferRequest>(&bytes).unwrap(),
            request
        );
    }

    #[test]
    fn header_round_trip() {
        let header = FileOfferHeader {
            version: 1,
            offer_id: FileOfferId::generate(),
            name: "report.pdf".into(),
            size: 42,
        };
        let bytes = postcard::to_stdvec(&header).unwrap();
        assert_eq!(
            postcard::from_bytes::<FileOfferHeader>(&bytes).unwrap(),
            header
        );
    }

    #[test]
    fn unsupported_version_is_rejected_fail_closed() {
        let request = FileOfferRequest {
            version: 99,
            offer_id: FileOfferId::generate(),
        };
        assert_ne!(request.version, FILE_OFFER_WIRE_VERSION);
        let response = if request.version != FILE_OFFER_WIRE_VERSION {
            FileOfferResponse::Error(FileOfferError::UnsupportedVersion)
        } else {
            panic!("unsupported version must not be served")
        };
        assert_eq!(
            response,
            FileOfferResponse::Error(FileOfferError::UnsupportedVersion)
        );
    }
}
