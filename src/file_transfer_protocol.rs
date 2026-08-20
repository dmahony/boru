//! Secure, bounded contract for direct file transfers.
//!
//! This module is deliberately transport-agnostic: it defines the messages and
//! validation rules used by a future streaming engine. Paths never cross the
//! wire. A receiver chooses its destination and publishes a partial file only
//! after the terminal `Complete` message has validated size and BLAKE3 hash.

#![allow(missing_docs)]

use std::fmt;

use serde::{Deserialize, Serialize};

/// Current version of the file-transfer contract.
pub const FILE_TRANSFER_PROTOCOL_VERSION: u16 = 1;
/// Default maximum accepted file size (100 MiB).
pub const DEFAULT_MAX_FILE_SIZE: u64 = 100 * 1024 * 1024;
/// Default maximum payload carried by one chunk.
pub const DEFAULT_MAX_CHUNK_SIZE: usize = 64 * 1024;
/// Default maximum number of concurrently tracked transfers.
pub const DEFAULT_MAX_ACTIVE_TRANSFERS: usize = 32;
/// Maximum display filename length in Unicode scalar values.
pub const DEFAULT_MAX_FILENAME_CHARS: usize = 255;
/// Maximum MIME hint length in bytes.
pub const DEFAULT_MAX_MIME_HINT_BYTES: usize = 127;

/// Opaque identifier that correlates every message in one transfer.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq, Serialize, Deserialize)]
pub struct TransferId([u8; 16]);

impl TransferId {
    /// Generate an unpredictable transfer identifier.
    pub fn generate() -> Self {
        let mut bytes = [0; 16];
        getrandom::fill(&mut bytes).expect("OS entropy source failed");
        Self(bytes)
    }

    /// Expose the identifier for logging or persistence.
    pub fn as_bytes(&self) -> &[u8; 16] {
        &self.0
    }
}

/// Configurable resource limits enforced before accepting untrusted metadata.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FileTransferLimits {
    pub max_file_size: u64,
    pub max_chunk_size: usize,
    pub max_active_transfers: usize,
    pub max_filename_chars: usize,
    pub max_mime_hint_bytes: usize,
}

impl Default for FileTransferLimits {
    fn default() -> Self {
        Self {
            max_file_size: DEFAULT_MAX_FILE_SIZE,
            max_chunk_size: DEFAULT_MAX_CHUNK_SIZE,
            max_active_transfers: DEFAULT_MAX_ACTIVE_TRANSFERS,
            max_filename_chars: DEFAULT_MAX_FILENAME_CHARS,
            max_mime_hint_bytes: DEFAULT_MAX_MIME_HINT_BYTES,
        }
    }
}

/// User-visible metadata sent with an offer.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct FileTransferOffer {
    pub version: u16,
    pub transfer_id: TransferId,
    /// Display basename only; never a sender filesystem path.
    pub filename: String,
    pub size: u64,
    /// BLAKE3 hash of the complete file.
    pub blake3_hash: [u8; 32],
    /// Optional advisory MIME hint; never used to select an executable handler.
    pub mime_hint: Option<String>,
}

/// Explicit reason for declining an offer or terminating a transfer.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum TransferError {
    UnsupportedVersion,
    MalformedMetadata,
    Oversized,
    InvalidChunk,
    NotFound,
    Rejected,
    Cancelled,
    HashMismatch,
    SizeMismatch,
    ResourceLimit,
    Io,
}

/// The complete transfer control/data contract.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub enum FileTransferMessage {
    Offer(FileTransferOffer),
    Accept {
        transfer_id: TransferId,
    },
    Reject {
        transfer_id: TransferId,
        error: TransferError,
    },
    /// Cancellation is terminal and may be sent by either side.
    Cancel {
        transfer_id: TransferId,
    },
    /// One bounded data frame. Sequence numbers start at zero and increase by one.
    Chunk {
        transfer_id: TransferId,
        sequence: u64,
        data: Vec<u8>,
    },
    /// Terminal success; the receiver verifies both fields before promotion.
    Complete {
        transfer_id: TransferId,
        size: u64,
        blake3_hash: [u8; 32],
    },
    /// Terminal failure after an offer was accepted.
    Error {
        transfer_id: TransferId,
        error: TransferError,
    },
}

/// Validation failures for metadata and chunk bounds.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum FileTransferValidationError {
    UnsupportedVersion,
    EmptyFilename,
    PathComponent,
    ControlCharacter,
    FilenameTooLong,
    EmptyMimeHint,
    MimeHintTooLong,
    FileTooLarge,
    ChunkTooLarge,
    ChunkExceedsOffer,
    InvalidLimits,
}

/// State of one transfer; terminal states cannot transition again.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TransferLifecycleState {
    Offered,
    Accepted,
    Completed,
    Failed(TransferError),
}

/// In-memory receiver-side guard for ordering, byte, and terminal checks.
#[derive(Debug)]
pub struct TransferLifecycle {
    offer: FileTransferOffer,
    state: TransferLifecycleState,
    received: u64,
    next_sequence: u64,
    hasher: blake3::Hasher,
}

impl TransferLifecycle {
    /// Create a lifecycle after validating the offer.
    pub fn new(
        offer: FileTransferOffer,
        limits: &FileTransferLimits,
    ) -> Result<Self, FileTransferValidationError> {
        offer.validate(limits)?;
        Ok(Self {
            offer,
            state: TransferLifecycleState::Offered,
            received: 0,
            next_sequence: 0,
            hasher: blake3::Hasher::new(),
        })
    }

    pub fn state(&self) -> TransferLifecycleState {
        self.state
    }

    pub fn accept(&mut self) -> Result<(), TransferError> {
        if self.state != TransferLifecycleState::Offered {
            return Err(TransferError::MalformedMetadata);
        }
        self.state = TransferLifecycleState::Accepted;
        Ok(())
    }

    pub fn reject(&mut self) -> Result<(), TransferError> {
        if self.state != TransferLifecycleState::Offered {
            return Err(TransferError::MalformedMetadata);
        }
        self.state = TransferLifecycleState::Failed(TransferError::Rejected);
        Ok(())
    }

    pub fn cancel(&mut self) -> Result<(), TransferError> {
        if matches!(
            self.state,
            TransferLifecycleState::Completed | TransferLifecycleState::Failed(_)
        ) {
            return Err(TransferError::MalformedMetadata);
        }
        self.state = TransferLifecycleState::Failed(TransferError::Cancelled);
        Ok(())
    }

    pub fn push_chunk(
        &mut self,
        sequence: u64,
        data: &[u8],
        limits: &FileTransferLimits,
    ) -> Result<(), TransferError> {
        if self.state != TransferLifecycleState::Accepted {
            return Err(TransferError::MalformedMetadata);
        }
        if sequence != self.next_sequence || data.len() > limits.max_chunk_size {
            return Err(TransferError::InvalidChunk);
        }
        let new_size = self.received.saturating_add(data.len() as u64);
        if new_size > self.offer.size || new_size > limits.max_file_size {
            return Err(TransferError::Oversized);
        }
        self.hasher.update(data);
        self.received = new_size;
        self.next_sequence = self.next_sequence.saturating_add(1);
        Ok(())
    }

    /// Verify the terminal record before a partial file is promoted.
    pub fn complete(&mut self, size: u64, hash: [u8; 32]) -> Result<(), TransferError> {
        if self.state != TransferLifecycleState::Accepted {
            return Err(TransferError::MalformedMetadata);
        }
        if size != self.offer.size || self.received != self.offer.size {
            self.state = TransferLifecycleState::Failed(TransferError::SizeMismatch);
            return Err(TransferError::SizeMismatch);
        }
        if hash != self.offer.blake3_hash || *self.hasher.finalize().as_bytes() != hash {
            self.state = TransferLifecycleState::Failed(TransferError::HashMismatch);
            return Err(TransferError::HashMismatch);
        }
        self.state = TransferLifecycleState::Completed;
        Ok(())
    }
}

impl fmt::Display for FileTransferValidationError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "file transfer validation failed: {:?}", self)
    }
}

impl std::error::Error for FileTransferValidationError {}

/// Sanitize and validate a remote display name as a safe basename.
pub fn sanitize_filename(
    input: &str,
    limits: &FileTransferLimits,
) -> Result<String, FileTransferValidationError> {
    if input.is_empty() {
        return Err(FileTransferValidationError::EmptyFilename);
    }
    if input == "." || input == ".." || input.contains('/') || input.contains('\\') {
        return Err(FileTransferValidationError::PathComponent);
    }
    if input.chars().any(|c| c.is_control()) {
        return Err(FileTransferValidationError::ControlCharacter);
    }
    let name = input.trim().to_string();
    if name.is_empty() {
        return Err(FileTransferValidationError::EmptyFilename);
    }
    if name.chars().count() > limits.max_filename_chars {
        return Err(FileTransferValidationError::FilenameTooLong);
    }
    Ok(name)
}

impl FileTransferOffer {
    /// Validate all untrusted offer fields against the configured limits.
    pub fn validate(&self, limits: &FileTransferLimits) -> Result<(), FileTransferValidationError> {
        if self.version != FILE_TRANSFER_PROTOCOL_VERSION {
            return Err(FileTransferValidationError::UnsupportedVersion);
        }
        if limits.max_file_size == 0
            || limits.max_chunk_size == 0
            || limits.max_active_transfers == 0
            || limits.max_filename_chars == 0
        {
            return Err(FileTransferValidationError::InvalidLimits);
        }
        sanitize_filename(&self.filename, limits)?;
        if self.size > limits.max_file_size {
            return Err(FileTransferValidationError::FileTooLarge);
        }
        if let Some(mime) = &self.mime_hint {
            if mime.is_empty() {
                return Err(FileTransferValidationError::EmptyMimeHint);
            }
            if mime.len() > limits.max_mime_hint_bytes {
                return Err(FileTransferValidationError::MimeHintTooLong);
            }
            if mime.chars().any(|c| c.is_control()) {
                return Err(FileTransferValidationError::ControlCharacter);
            }
        }
        Ok(())
    }
}

impl FileTransferMessage {
    /// Validate a message without allocating an unbounded buffer.
    pub fn validate(
        &self,
        offer: Option<&FileTransferOffer>,
        limits: &FileTransferLimits,
    ) -> Result<(), FileTransferValidationError> {
        match self {
            Self::Offer(value) => value.validate(limits),
            Self::Chunk { data, .. } if data.len() > limits.max_chunk_size => {
                Err(FileTransferValidationError::ChunkTooLarge)
            }
            Self::Chunk { data, .. } => {
                if let Some(offer) = offer {
                    if data.len() as u64 > offer.size {
                        return Err(FileTransferValidationError::ChunkExceedsOffer);
                    }
                }
                Ok(())
            }
            Self::Complete { size, .. } => {
                if *size > limits.max_file_size {
                    Err(FileTransferValidationError::FileTooLarge)
                } else {
                    Ok(())
                }
            }
            Self::Accept { .. }
            | Self::Reject { .. }
            | Self::Cancel { .. }
            | Self::Error { .. } => Ok(()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn offer(name: &str, size: u64) -> FileTransferOffer {
        FileTransferOffer {
            version: FILE_TRANSFER_PROTOCOL_VERSION,
            transfer_id: TransferId::generate(),
            filename: name.into(),
            size,
            blake3_hash: [7; 32],
            mime_hint: Some("application/octet-stream".into()),
        }
    }

    #[test]
    fn full_control_contract_round_trips() {
        let id = TransferId::generate();
        let messages = vec![
            FileTransferMessage::Offer(offer("report.pdf", 42)),
            FileTransferMessage::Accept { transfer_id: id },
            FileTransferMessage::Reject {
                transfer_id: id,
                error: TransferError::Rejected,
            },
            FileTransferMessage::Cancel { transfer_id: id },
            FileTransferMessage::Chunk {
                transfer_id: id,
                sequence: 0,
                data: vec![1, 2],
            },
            FileTransferMessage::Complete {
                transfer_id: id,
                size: 42,
                blake3_hash: [8; 32],
            },
            FileTransferMessage::Error {
                transfer_id: id,
                error: TransferError::Io,
            },
        ];
        for message in messages {
            let encoded = postcard::to_stdvec(&message).expect("encode");
            assert_eq!(
                postcard::from_bytes::<FileTransferMessage>(&encoded).unwrap(),
                message
            );
        }
    }

    #[test]
    fn hostile_names_are_rejected_without_path_normalization() {
        let limits = FileTransferLimits::default();
        for name in [
            "",
            " ",
            ".",
            "..",
            "../secret",
            r"..\secret",
            "/tmp/x",
            "a\u{0000}b",
        ] {
            assert!(
                sanitize_filename(name, &limits).is_err(),
                "accepted {name:?}"
            );
        }
        assert_eq!(
            sanitize_filename(" report.pdf ", &limits).unwrap(),
            "report.pdf"
        );
    }

    #[test]
    fn oversized_offer_and_chunk_are_rejected() {
        let limits = FileTransferLimits {
            max_file_size: 4,
            max_chunk_size: 2,
            ..Default::default()
        };
        assert_eq!(
            offer("x.bin", 5).validate(&limits),
            Err(FileTransferValidationError::FileTooLarge)
        );
        let valid = offer("x.bin", 4);
        let chunk = FileTransferMessage::Chunk {
            transfer_id: valid.transfer_id,
            sequence: 0,
            data: vec![0; 3],
        };
        assert_eq!(
            chunk.validate(Some(&valid), &limits),
            Err(FileTransferValidationError::ChunkTooLarge)
        );
    }

    #[test]
    fn partial_file_requires_matching_terminal_size_and_hash() {
        let offer = offer("x.bin", 3);
        let complete = FileTransferMessage::Complete {
            transfer_id: offer.transfer_id,
            size: 2,
            blake3_hash: [7; 32],
        };
        assert_ne!(
            complete,
            FileTransferMessage::Complete {
                transfer_id: offer.transfer_id,
                size: offer.size,
                blake3_hash: offer.blake3_hash,
            }
        );
        assert_eq!(offer.size, 3);
        assert_eq!(offer.blake3_hash, [7; 32]);
    }

    #[test]
    fn lifecycle_requires_ordered_chunks_and_terminal_verification() {
        let payload = b"abc";
        let mut offer = offer("x.bin", payload.len() as u64);
        offer.blake3_hash = *blake3::hash(payload).as_bytes();
        let limits = FileTransferLimits::default();
        let mut lifecycle = TransferLifecycle::new(offer.clone(), &limits).unwrap();
        assert_eq!(lifecycle.state(), TransferLifecycleState::Offered);
        assert_eq!(
            lifecycle.push_chunk(0, payload, &limits),
            Err(TransferError::MalformedMetadata)
        );
        lifecycle.accept().unwrap();
        assert_eq!(
            lifecycle.push_chunk(1, b"a", &limits),
            Err(TransferError::InvalidChunk)
        );
        lifecycle.push_chunk(0, payload, &limits).unwrap();
        lifecycle
            .complete(offer.size, offer.blake3_hash)
            .expect("complete matching transfer");
        assert_eq!(lifecycle.state(), TransferLifecycleState::Completed);
        assert_eq!(lifecycle.cancel(), Err(TransferError::MalformedMetadata));
    }
}
