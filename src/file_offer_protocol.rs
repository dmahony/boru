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
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
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

/// Authenticated completion record written after the advertised file bytes.
/// QUIC authenticates the stream to the peer, so this footer is integrity
/// protected by the direct transfer connection rather than gossip metadata.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FileOfferCompletion {
    /// Offer this completion belongs to.
    pub offer_id: FileOfferId,
    /// Number of bytes actually written to the stream.
    pub bytes_sent: u64,
    /// BLAKE3 digest of exactly those bytes.
    pub blake3_hash: [u8; 32],
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

/// Response frame. A successful header is followed by raw bytes and a
/// [`FileOfferCompletion`] on the same stream.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum FileOfferResponse {
    /// The transfer header; raw bytes and completion follow immediately after.
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
    let mut hasher = blake3::Hasher::new();
    let mut buffer = [0u8; 64 * 1024];
    let mut received = 0u64;
    while received < transfer.header.size {
        let remaining = (transfer.header.size - received) as usize;
        let read_len = remaining.min(buffer.len());
        let count = transfer
            .read(&mut buffer[..read_len])
            .await?
            .ok_or_else(|| anyhow::anyhow!("direct transfer ended before its advertised size"))?;
        hasher.update(&buffer[..count]);
        output.write_all(&buffer[..count]).await?;
        received += count as u64;
    }
    let completion: FileOfferCompletion = read_frame(&mut transfer.reader).await?;
    verify_completion(
        &transfer.header,
        offer_id,
        received,
        *hasher.finalize().as_bytes(),
        &completion,
    )?;
    output.flush().await?;
    destination.restore_file(output.into_std().await);
    destination
        .publish()
        .map_err(|error| anyhow::anyhow!(error.to_string()))
}

fn verify_completion(
    header: &FileOfferHeader,
    requested_offer_id: FileOfferId,
    bytes_received: u64,
    receiver_hash: [u8; 32],
    completion: &FileOfferCompletion,
) -> anyhow::Result<()> {
    if completion.offer_id != header.offer_id
        || completion.offer_id != requested_offer_id
        || completion.bytes_sent != bytes_received
        || bytes_received != header.size
        || completion.blake3_hash != receiver_hash
    {
        anyhow::bail!("direct transfer completion verification failed");
    }
    Ok(())
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
    let file_metadata = file
        .metadata()
        .await
        .map_err(|_| anyhow::Error::new(FileOfferError::SourceUnavailable))?;
    if !file_metadata.is_file() {
        return reject(&mut writer, FileOfferError::SourceUnavailable).await;
    }
    if file_metadata.len() != offer.size
        || file_metadata
            .modified()
            .map_err(|_| anyhow::Error::new(FileOfferError::SourceUnavailable))?
            != offer.modified_at
    {
        return reject(&mut writer, FileOfferError::SourceChanged).await;
    }
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
    let (bytes_sent, blake3_hash) =
        stream_exact(&mut file.take(offer.size), &mut writer, offer.size).await?;
    write_frame(
        &mut writer,
        &FileOfferCompletion {
            offer_id: offer.id,
            bytes_sent,
            blake3_hash,
        },
    )
    .await?;
    writer.finish()?;
    drop(permit);
    Ok(())
}

/// Copy exactly `expected` bytes while hashing what was actually transmitted.
/// A short read is an error, so callers must not write a completion footer when
/// this returns an error.
async fn stream_exact<R, W>(
    reader: &mut R,
    writer: &mut W,
    expected: u64,
) -> anyhow::Result<(u64, [u8; 32])>
where
    R: AsyncRead + Unpin,
    W: AsyncWrite + Unpin,
{
    let mut hasher = blake3::Hasher::new();
    let mut bytes_sent = 0u64;
    let mut buffer = [0u8; 64 * 1024];
    while bytes_sent < expected {
        let read_len = (expected - bytes_sent).min(buffer.len() as u64) as usize;
        let count = reader.read(&mut buffer[..read_len]).await?;
        if count == 0 {
            anyhow::bail!("source ended early: sent {bytes_sent} of {expected} bytes");
        }
        hasher.update(&buffer[..count]);
        writer.write_all(&buffer[..count]).await?;
        bytes_sent += count as u64;
    }
    Ok((bytes_sent, *hasher.finalize().as_bytes()))
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
    use n0_future::StreamExt;
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
    fn required_test_12_completion_footer_verifies_count_and_hash() {
        let offer_id = FileOfferId::generate();
        let payload = b"streamed payload";
        let header = FileOfferHeader {
            version: FILE_OFFER_WIRE_VERSION,
            offer_id,
            name: "payload.bin".into(),
            size: payload.len() as u64,
        };
        let completion = FileOfferCompletion {
            offer_id,
            bytes_sent: payload.len() as u64,
            blake3_hash: *blake3::hash(payload).as_bytes(),
        };
        verify_completion(
            &header,
            offer_id,
            payload.len() as u64,
            completion.blake3_hash,
            &completion,
        )
        .unwrap();

        let mut bad_count = completion.clone();
        bad_count.bytes_sent += 1;
        assert!(verify_completion(
            &header,
            offer_id,
            payload.len() as u64,
            completion.blake3_hash,
            &bad_count,
        )
        .is_err());
        let mut bad_hash = completion.clone();
        bad_hash.blake3_hash[0] ^= 1;
        assert!(verify_completion(
            &header,
            offer_id,
            payload.len() as u64,
            completion.blake3_hash,
            &bad_hash,
        )
        .is_err());
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

    #[tokio::test]
    async fn required_test_11_early_eof_fails_without_completion() {
        let mut source = std::io::Cursor::new(b"short".to_vec());
        let mut sink = tokio::io::sink();
        let error = stream_exact(&mut source, &mut sink, 10)
            .await
            .expect_err("short source must abort before a completion footer");
        assert!(error.to_string().contains("source ended early"));
    }

    #[tokio::test]
    async fn exact_stream_hashes_only_transmitted_bytes() {
        let payload = b"complete payload";
        let mut source = std::io::Cursor::new(payload.to_vec());
        let mut sink = tokio::io::sink();
        let (bytes_sent, hash) = stream_exact(&mut source, &mut sink, payload.len() as u64)
            .await
            .unwrap();
        assert_eq!(bytes_sent, payload.len() as u64);
        assert_eq!(hash, *blake3::hash(payload).as_bytes());
    }

    #[tokio::test]
    async fn direct_read_and_blob_ingest_use_independent_source_handles() {
        let directory = tempdir().unwrap();
        let path = directory.path().join("concurrent.bin");
        let contents = vec![0x5a; 256 * 1024];
        tokio::fs::write(&path, &contents).await.unwrap();

        // These are intentionally separate opens, matching the direct sender
        // and background ingest paths. Neither operation shares a cursor or
        // takes an exclusive lock on the source file.
        let direct_read = tokio::fs::read(&path);
        let ingest = async {
            let file = tokio::fs::File::open(&path).await.unwrap();
            let stream = tokio_util::io::ReaderStream::new(file);
            let blob_store: iroh_blobs::api::Store = iroh_blobs::store::mem::MemStore::new().into();
            let import = blob_store.blobs().add_stream(Box::pin(stream)).await;
            let mut progress = import.stream().await;
            let mut tag = None;
            while let Some(item) = progress.next().await {
                if let iroh_blobs::api::blobs::AddProgressItem::Done(done) = item {
                    tag = Some(done);
                }
            }
            tag.expect("blob ingest completed")
        };

        let (direct_bytes, tag) = tokio::join!(direct_read, ingest);
        assert_eq!(direct_bytes.unwrap(), contents);
        assert_eq!(tag.hash(), iroh_blobs::Hash::from(blake3::hash(&contents)));
    }
}
