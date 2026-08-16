//! Dedicated direct file-transfer protocol for announced file offers.
//!
//! The offer metadata is exchanged over a small, versioned postcard frame. Once
//! the header is accepted, the remainder of the bidirectional QUIC stream is
//! the raw file byte stream; file contents never travel through gossip.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use iroh::{
    endpoint::{Connection, RecvStream, SendStream},
    protocol::{AcceptError, ProtocolHandler},
    Endpoint, EndpointAddr,
};
use serde::{Deserialize, Serialize};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::Semaphore;

use crate::safe_destination::{reserve_download_destination, OverwritePolicy, Reservation};
use crate::{chat_core::protocol::FileOfferId, file_offer::FileOfferRegistry};

/// ALPN for direct file offers.
pub const FILE_OFFER_ALPN: &[u8] = b"boru/file-offer/1";
/// Current wire version for direct file offers.
pub const FILE_OFFER_WIRE_VERSION: u16 = 1;
const MAX_FRAME_SIZE: u32 = 64 * 1024;
const MAX_CONCURRENT_TRANSFERS: usize = 32;

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
    /// The requesting authenticated peer is not authorized for this offer.
    PermissionDenied,
    /// The offer's TTL has elapsed.
    Expired,
    /// The source file no longer exists or cannot be opened.
    SourceUnavailable,
    /// The source file metadata no longer matches the announced offer.
    SourceChanged,
    /// The server has reached its transfer concurrency limit.
    Busy,
}

impl std::fmt::Display for FileOfferError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::UnsupportedVersion => "unsupported_version",
            Self::NotFound => "not_found",
            Self::PermissionDenied => "permission_denied",
            Self::Expired => "expired",
            Self::SourceUnavailable => "source_unavailable",
            Self::SourceChanged => "source_changed",
            Self::Busy => "busy",
        })
    }
}

impl std::error::Error for FileOfferError {}

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
    transfers: Arc<Semaphore>,
}

impl FileOfferProtocolHandler {
    /// Create a handler serving entries from `registry`.
    pub fn new(registry: Arc<Mutex<FileOfferRegistry>>) -> Self {
        Self {
            registry,
            transfers: Arc::new(Semaphore::new(MAX_CONCURRENT_TRANSFERS)),
        }
    }
}

impl ProtocolHandler for FileOfferProtocolHandler {
    async fn accept(&self, connection: Connection) -> Result<(), AcceptError> {
        let registry = self.registry.clone();
        let transfers = self.transfers.clone();
        tokio::spawn(async move {
            if let Err(error) = serve_connection(connection, registry, transfers).await {
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
        FileOfferResponse::Error(error) => Err(anyhow::Error::new(error)),
    }
}

/// Download an announced file offer into Boru's managed downloads directory.
///
/// The destination is reserved before any bytes are written. The reservation
/// owns the output file and removes it on every error path; publication happens
/// only after the stream contains exactly the number of bytes advertised by
/// the authenticated header. This gives direct transfers the same traversal,
/// symlink, collision, and partial-file guarantees as BlobTicket downloads.
pub async fn download_file_offer(
    endpoint: &Endpoint,
    addr: EndpointAddr,
    offer_id: FileOfferId,
    download_dir: &Path,
) -> anyhow::Result<PathBuf> {
    let mut transfer = open_file_offer(endpoint, addr, offer_id).await?;
    let fallback = format!("{offer_id:?}");
    let mut destination = match reserve_download_destination(
        download_dir,
        &transfer.header.name,
        &fallback,
        OverwritePolicy::KeepBoth,
    )? {
        Reservation::Use(destination) => destination,
        Reservation::Skip => anyhow::bail!("download destination already exists"),
    };

    let std_file = destination
        .take_file()
        .ok_or_else(|| anyhow::anyhow!("download destination handle is unavailable"))?;
    let mut output = tokio::fs::File::from_std(std_file);
    let mut buffer = [0u8; 64 * 1024];
    let mut received = 0u64;
    while received < transfer.header.size {
        let remaining = (transfer.header.size - received) as usize;
        let read_len = remaining.min(buffer.len());
        let count = transfer.read(&mut buffer[..read_len]).await?;
        let Some(count) = count else {
            anyhow::bail!(
                "direct transfer ended early: received {received} of {} bytes",
                transfer.header.size
            );
        };
        output.write_all(&buffer[..count]).await?;
        received += count as u64;
    }
    if transfer.read(&mut [0u8; 1]).await?.is_some() {
        anyhow::bail!("direct transfer exceeded announced size");
    }
    output.flush().await?;
    destination.restore_file(output.into_std().await);
    destination
        .publish()
        .map_err(|error| anyhow::anyhow!(error.to_string()))
}

async fn serve_connection(
    connection: Connection,
    registry: Arc<Mutex<FileOfferRegistry>>,
    transfers: Arc<Semaphore>,
) -> anyhow::Result<()> {
    let (mut writer, mut reader) = connection.accept_bi().await?;
    let permit = match transfers.try_acquire_owned() {
        Ok(permit) => permit,
        Err(_) => return reject(&mut writer, FileOfferError::Busy).await,
    };
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

    let offer = match authorize_offer(connection.remote_id(), request.offer_id, &registry).await {
        Ok(offer) => offer,
        Err(error) => return reject(&mut writer, error).await,
    };
    let file = tokio::fs::File::open(offer.path())
        .await
        .map_err(|_| anyhow::Error::new(FileOfferError::SourceUnavailable))?;
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
    drop(permit);
    Ok(())
}

/// Authenticate and validate an offer immediately before serving it.
pub async fn authorize_offer(
    requester: iroh::PublicKey,
    offer_id: FileOfferId,
    registry: &Arc<Mutex<FileOfferRegistry>>,
) -> Result<crate::file_offer::FileOffer, FileOfferError> {
    let (offer, expired) = {
        let registry = registry
            .lock()
            .map_err(|_| FileOfferError::SourceUnavailable)?;
        let offer = registry
            .get(&offer_id)
            .cloned()
            .ok_or(FileOfferError::NotFound)?;
        (offer.clone(), registry.is_expired(&offer))
    };
    if offer.authorized_peer != requester {
        return Err(FileOfferError::PermissionDenied);
    }
    if expired {
        return Err(FileOfferError::Expired);
    }
    let metadata = tokio::fs::metadata(offer.path())
        .await
        .map_err(|_| FileOfferError::SourceUnavailable)?;
    if !metadata.is_file() {
        return Err(FileOfferError::SourceUnavailable);
    }
    if metadata.len() != offer.size {
        return Err(FileOfferError::SourceChanged);
    }
    let modified_at = metadata
        .modified()
        .map_err(|_| FileOfferError::SourceUnavailable)?;
    if modified_at != offer.modified_at {
        return Err(FileOfferError::SourceChanged);
    }
    Ok(offer)
}

async fn reject(writer: &mut SendStream, error: FileOfferError) -> anyhow::Result<()> {
    write_frame(writer, &FileOfferResponse::Error(error)).await?;
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
    use tempfile::tempdir;

    fn registry_with_offer(
        registry: &mut FileOfferRegistry,
        id: FileOfferId,
        peer: iroh::PublicKey,
        path: &std::path::Path,
        size: u64,
        modified_at: std::time::SystemTime,
    ) {
        registry.register(crate::file_offer::FileOffer::new(
            id,
            peer,
            path,
            "payload.bin".into(),
            size,
            modified_at,
        ));
    }

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

    #[tokio::test]
    async fn unauthorized_peer_cannot_request_an_offer() {
        let directory = tempdir().unwrap();
        let path = directory.path().join("payload.bin");
        std::fs::write(&path, b"payload").unwrap();
        let metadata = std::fs::metadata(&path).unwrap();
        let authorized = iroh::SecretKey::generate().public();
        let stranger = iroh::SecretKey::generate().public();
        let id = FileOfferId::generate();
        let registry = Arc::new(Mutex::new(FileOfferRegistry::new()));
        registry_with_offer(
            &mut registry.lock().unwrap(),
            id,
            authorized,
            &path,
            metadata.len(),
            metadata.modified().unwrap(),
        );

        assert_eq!(
            authorize_offer(stranger, id, &registry).await.unwrap_err(),
            FileOfferError::PermissionDenied
        );
    }

    #[tokio::test]
    async fn unknown_offer_is_not_found() {
        let registry = Arc::new(Mutex::new(FileOfferRegistry::new()));
        let peer = iroh::SecretKey::generate().public();
        assert_eq!(
            authorize_offer(peer, FileOfferId::generate(), &registry)
                .await
                .unwrap_err(),
            FileOfferError::NotFound
        );
    }

    #[tokio::test]
    async fn expired_offer_is_rejected() {
        let directory = tempdir().unwrap();
        let path = directory.path().join("payload.bin");
        std::fs::write(&path, b"payload").unwrap();
        let metadata = std::fs::metadata(&path).unwrap();
        let peer = iroh::SecretKey::generate().public();
        let id = FileOfferId::generate();
        let mut offers = FileOfferRegistry::with_ttl(std::time::Duration::ZERO);
        registry_with_offer(
            &mut offers,
            id,
            peer,
            &path,
            metadata.len(),
            metadata.modified().unwrap(),
        );
        let registry = Arc::new(Mutex::new(offers));
        assert_eq!(
            authorize_offer(peer, id, &registry).await.unwrap_err(),
            FileOfferError::Expired
        );
    }

    #[tokio::test]
    async fn missing_source_is_unavailable() {
        let directory = tempdir().unwrap();
        let path = directory.path().join("gone.bin");
        let peer = iroh::SecretKey::generate().public();
        let id = FileOfferId::generate();
        let mut offers = FileOfferRegistry::new();
        registry_with_offer(
            &mut offers,
            id,
            peer,
            &path,
            7,
            std::time::SystemTime::UNIX_EPOCH,
        );
        let registry = Arc::new(Mutex::new(offers));
        assert_eq!(
            authorize_offer(peer, id, &registry).await.unwrap_err(),
            FileOfferError::SourceUnavailable
        );
    }

    #[tokio::test]
    async fn changed_source_is_rejected() {
        let directory = tempdir().unwrap();
        let path = directory.path().join("changed.bin");
        std::fs::write(&path, b"original").unwrap();
        let metadata = std::fs::metadata(&path).unwrap();
        let peer = iroh::SecretKey::generate().public();
        let id = FileOfferId::generate();
        let mut offers = FileOfferRegistry::new();
        registry_with_offer(
            &mut offers,
            id,
            peer,
            &path,
            metadata.len(),
            metadata.modified().unwrap(),
        );
        std::fs::write(&path, b"changed-size").unwrap();
        let registry = Arc::new(Mutex::new(offers));
        assert_eq!(
            authorize_offer(peer, id, &registry).await.unwrap_err(),
            FileOfferError::SourceChanged
        );
    }
}
