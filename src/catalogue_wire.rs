//! Wire serialization for catalogue protocol responses.
//!
//! Serializes [`CatalogResponse`] values, enforces the per-response byte caps
//! from [`crate::catalogue_limits`], and writes versioned frames.  Extracted
//! from `catalogue_handler` so the codec can be tested independently.

use crate::catalogue_limits::{
    check_file_details_payload_size, check_page_payload_size, check_response_payload_size,
};
use crate::catalogue_protocol::{CatalogResponse, CatalogWireResponse};
use crate::protocol_version::{write_frame, CATALOGUE_RETRIEVAL_V1};


/// Serialize a [`CatalogResponse`], check its size against the catalogue
/// response byte limit, and write it to `send` via [`write_frame`].
///
/// Returns an `io::Error` with `InvalidData` when the serialized response
/// exceeds [`MAX_CATALOGUE_RESPONSE_BYTES`].
pub(crate) async fn write_catalogue_response(
    send: &mut iroh::endpoint::SendStream,
    response: CatalogResponse,
) -> std::result::Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let wire_resp = CatalogWireResponse::new(response);
    let resp_bytes = postcard::to_stdvec(&wire_resp)?;
    check_response_payload_size(resp_bytes.len()).map_err(|msg| {
        Box::new(std::io::Error::new(std::io::ErrorKind::InvalidData, msg))
            as Box<dyn std::error::Error + Send + Sync>
    })?;
    write_frame(send, CATALOGUE_RETRIEVAL_V1, &resp_bytes).await?;
    Ok(())
}

/// Serialize and write a paginated response under the stricter page-byte cap.
pub(crate) async fn write_page_response(
    send: &mut iroh::endpoint::SendStream,
    response: CatalogResponse,
) -> std::result::Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let wire_resp = CatalogWireResponse::new(response);
    let resp_bytes = postcard::to_stdvec(&wire_resp)?;
    check_page_payload_size(resp_bytes.len()).map_err(|msg| {
        Box::new(std::io::Error::new(std::io::ErrorKind::InvalidData, msg))
            as Box<dyn std::error::Error + Send + Sync>
    })?;
    write_frame(send, CATALOGUE_RETRIEVAL_V1, &resp_bytes).await?;
    Ok(())
}

/// Serialize a [`CatalogResponse`] that is a single file-details response,
/// check its size, and write it.
///
/// Uses the stricter [`MAX_FILE_DETAILS_PAYLOAD_BYTES`] limit since
/// FileDetails contains a single [`RemoteSharedFile`].
pub(crate) async fn write_file_details_response(
    send: &mut iroh::endpoint::SendStream,
    response: CatalogResponse,
) -> std::result::Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let wire_resp = CatalogWireResponse::new(response);
    let resp_bytes = postcard::to_stdvec(&wire_resp)?;
    check_file_details_payload_size(resp_bytes.len()).map_err(|msg| {
        Box::new(std::io::Error::new(std::io::ErrorKind::InvalidData, msg))
            as Box<dyn std::error::Error + Send + Sync>
    })?;
    write_frame(send, CATALOGUE_RETRIEVAL_V1, &resp_bytes).await?;
    Ok(())
}
