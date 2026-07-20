//! Catalogue retrieval client — fetches and verifies a signed catalogue
//! from a remote peer over the `/boru-file-catalog/1` QUIC protocol.
//!
//! # Usage
//!
//! ```ignore
//! let catalogue = fetch_paginated_remote_catalogue(&client_ep, server_pk, 500).await?;
//! if catalogue.verify().is_ok() {
//!     for file in &catalogue.files {
//!         println!("{} — {}", file.content_hash, file.display_name);
//!     }
//! }
//! ```

use std::time::Duration;

use iroh::{Endpoint, EndpointAddr, PublicKey};

use crate::catalogue_limits::{
    check_page_payload_size, check_response_payload_size, MAX_CATALOGUE_FILES,
    MAX_CATALOGUE_PAGE_SIZE, MAX_INVALID_RESPONSE_ATTEMPTS,
};
use crate::catalogue_model::SignedFileCatalogue;
use crate::catalogue_protocol::{
    CatalogRequest, CatalogResponse, CatalogWireRequest, CatalogWireResponse,
};
use crate::chat_core::DIAGNOSTICS;
use crate::diagnostics::DiagnosticEventKind;
use crate::protocol_version::{
    read_frame, write_frame, CATALOGUE_RETRIEVAL_V1, SUPPORTED_CATALOGUE_RETRIEVAL,
};
use crate::storage::Storage;

/// Default timeout for a catalogue fetch operation.
const FETCH_TIMEOUT: Duration = Duration::from_secs(30);

/// Default page size for paginated fetches.
const DEFAULT_PAGE_SIZE: u32 = MAX_CATALOGUE_PAGE_SIZE;

/// Error returned by [`fetch_remote_catalogue`] and
/// [`fetch_paginated_remote_catalogue`].
#[derive(Debug, Clone)]
pub enum RemoteCatalogueFetchError {
    /// The remote peer denied the request (typically a blocked peer).
    PermissionDenied,

    /// The catalogue has not changed since `known_revision`.
    NotModified,

    /// The paginated catalogue is incomplete — the server sent fewer than
    /// the expected number of pages or the stream ended before the final
    /// page with `next_cursor == None` was received.
    IncompleteCatalogue {
        /// Human-readable details about what was missing.
        details: String,
    },

    /// The server's revision changed during the exchange;
    /// the caller should re-fetch from the beginning.
    RevisionChanged {
        /// The new revision the server reported.
        new_revision: u64,
    },

    /// The connection to the remote peer failed.
    ConnectionFailed {
        /// Human-readable failure details.
        details: String,
    },

    /// The operation timed out.
    Timeout,

    /// The catalogue signature verification failed.
    SignatureInvalid {
        /// Human-readable verification failure details.
        details: String,
    },

    /// The request was invalid or the response was malformed.
    ProtocolError {
        /// Human-readable error details.
        details: String,
    },
}

impl std::fmt::Display for RemoteCatalogueFetchError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::PermissionDenied => f.write_str("permission denied"),
            Self::NotModified => f.write_str("not modified"),
            Self::IncompleteCatalogue { details } => {
                write!(f, "incomplete catalogue: {details}")
            }
            Self::RevisionChanged { new_revision } => {
                write!(f, "revision changed to {new_revision}")
            }
            Self::ConnectionFailed { details } => {
                write!(f, "connection failed: {details}")
            }
            Self::Timeout => f.write_str("timeout"),
            Self::SignatureInvalid { details } => {
                write!(f, "signature verification failed: {details}")
            }
            Self::ProtocolError { details } => {
                write!(f, "protocol error: {details}")
            }
        }
    }
}

impl std::error::Error for RemoteCatalogueFetchError {}

impl From<Box<dyn std::error::Error + Send + Sync>> for RemoteCatalogueFetchError {
    fn from(e: Box<dyn std::error::Error + Send + Sync>) -> Self {
        RemoteCatalogueFetchError::ProtocolError {
            details: format!("{e:#}"),
        }
    }
}

/// Validate a collection of catalogue pages: check revision consistency,
/// verify the final page was received (next_cursor is None), and flatten
/// items into a single vector.
///
/// Returns `(items, revision, collections_from_signed)`.
fn validate_pages(
    pages: &[crate::catalogue_protocol::CataloguePage],
) -> std::result::Result<
    (Vec<crate::catalogue_model::RemoteSharedFile>, u64),
    RemoteCatalogueFetchError,
> {
    if pages.is_empty() {
        return Err(RemoteCatalogueFetchError::IncompleteCatalogue {
            details: "no pages received".to_string(),
        });
    }

    let revision = pages[0].revision;

    // Verify all pages share the same revision.
    for (i, page) in pages.iter().enumerate() {
        if page.revision != revision {
            return Err(RemoteCatalogueFetchError::RevisionChanged {
                new_revision: page.revision,
            });
        }
        // Verify each page's items individually.
        if !page.items.is_empty() {
            // The final page must have next_cursor == None.
            if i == pages.len() - 1 && page.next_cursor.is_some() {
                return Err(RemoteCatalogueFetchError::IncompleteCatalogue {
                    details: format!(
                        "final page (page {}) still has a next cursor — more pages expected",
                        i
                    ),
                });
            }
        }
    }

    // Verify the last page has no cursor (otherwise it's incomplete).
    let last = pages.last().unwrap();
    if last.next_cursor.is_some() {
        return Err(RemoteCatalogueFetchError::IncompleteCatalogue {
            details: "last page has a next cursor — catalogue may be incomplete".to_string(),
        });
    }

    // Flatten items from all pages.
    let total_items: usize = pages.iter().map(|p| p.items.len()).sum();
    let mut all_items = Vec::with_capacity(total_items);
    for page in pages {
        all_items.extend(page.items.clone());
    }

    Ok((all_items, revision))
}

/// Validate a fully assembled [`SignedFileCatalogue`] against limits,
/// duplicate checks, field validation, owner matching, and signature verification.
///
/// Emits [`DiagnosticEventKind::CatalogueSignatureRejected`](crate::diagnostics::DiagnosticEventKind::CatalogueSignatureRejected)
/// on signature or owner mismatch.  Returns
/// [`RemoteCatalogueFetchError::SignatureInvalid`] for signature/owner
/// errors and [`RemoteCatalogueFetchError::ProtocolError`] for structural
/// issues.
pub fn validate_complete_catalogue(
    catalogue: &SignedFileCatalogue,
    server_pk: PublicKey,
) -> std::result::Result<(), RemoteCatalogueFetchError> {
    // Signature verification must come first — a tampered signature is
    // a distinct error variant from structural issues.
    catalogue.verify().map_err(|e| {
        let details = format!("catalogue signature verification failed: {e}");
        DIAGNOSTICS.record_with_peer(
            None,
            Some(server_pk.to_string()),
            DiagnosticEventKind::CatalogueSignatureRejected {
                error: details.clone(),
            },
        );
        RemoteCatalogueFetchError::SignatureInvalid { details }
    })?;

    // Structural metadata validation (including timestamp, files, collections,
    // and duplicate checks).  The redundant verify() inside validate() is
    // harmless since we already verified the signature above.
    catalogue
        .validate()
        .map_err(|e| RemoteCatalogueFetchError::ProtocolError {
            details: format!("invalid catalogue metadata: {e}"),
        })?;

    // Verify the owner matches who we connected to.
    if catalogue.owner_id != server_pk {
        DIAGNOSTICS.record_with_peer(
            None,
            Some(server_pk.to_string()),
            DiagnosticEventKind::CatalogueSignatureRejected {
                error: "catalogue owner_id does not match server public key".to_string(),
            },
        );
        return Err(RemoteCatalogueFetchError::SignatureInvalid {
            details: "catalogue owner_id does not match server public key".to_string(),
        });
    }
    Ok(())
}

// ── Single-page fetch (internal) ───────────────────────────────────────────

/// Send a single `GetCataloguePage` request on an already-connected bi-stream
/// and return the parsed response.
///
/// The `(send, recv)` pair is consumed — each page request uses its own
/// bi-directional stream.
async fn do_fetch_page(
    send: &mut iroh::endpoint::SendStream,
    recv: &mut iroh::endpoint::RecvStream,
    known_revision: Option<u64>,
    cursor: Option<String>,
    page_size: u32,
) -> std::result::Result<CatalogResponse, RemoteCatalogueFetchError> {
    let request = CatalogRequest::GetCataloguePage {
        known_revision,
        cursor,
        page_size,
    };
    let wire_req = CatalogWireRequest::new(request);
    let payload =
        postcard::to_stdvec(&wire_req).map_err(|e| RemoteCatalogueFetchError::ProtocolError {
            details: format!("encode request: {e}"),
        })?;

    write_frame(send, CATALOGUE_RETRIEVAL_V1, &payload)
        .await
        .map_err(|e| RemoteCatalogueFetchError::ProtocolError {
            details: format!("write frame: {e}"),
        })?;

    send.finish()
        .map_err(|e| RemoteCatalogueFetchError::ProtocolError {
            details: format!("finish send: {e}"),
        })?;

    // Read the response.
    let (_version, resp_bytes) = tokio::time::timeout(
        FETCH_TIMEOUT,
        read_frame(recv, SUPPORTED_CATALOGUE_RETRIEVAL, "catalogue"),
    )
    .await
    .map_err(|_| RemoteCatalogueFetchError::Timeout)?
    .map_err(|e| RemoteCatalogueFetchError::ProtocolError {
        details: format!("read frame: {e}"),
    })?
    .ok_or_else(|| RemoteCatalogueFetchError::ProtocolError {
        details: "server closed connection without response".to_string(),
    })?;

    check_page_payload_size(resp_bytes.len())
        .map_err(|msg| RemoteCatalogueFetchError::ProtocolError { details: msg })?;

    let wire_resp: CatalogWireResponse = postcard::from_bytes(&resp_bytes).map_err(|e| {
        RemoteCatalogueFetchError::ProtocolError {
            details: format!("decode response: {e}"),
        }
    })?;

    Ok(wire_resp.inner)
}

// ── Full signed catalogue fetch on a bi-stream (internal) ───────────────────

/// Send a `GetCatalogue` request on an already-paired `(send, recv)` and
/// return the parsed signed catalogue response.
async fn do_fetch_signed_catalogue(
    send: &mut iroh::endpoint::SendStream,
    recv: &mut iroh::endpoint::RecvStream,
    known_revision: Option<u64>,
) -> std::result::Result<CatalogResponse, RemoteCatalogueFetchError> {
    let request = CatalogRequest::GetCatalogue { known_revision };
    let wire_req = CatalogWireRequest::new(request);
    let payload =
        postcard::to_stdvec(&wire_req).map_err(|e| RemoteCatalogueFetchError::ProtocolError {
            details: format!("encode request: {e}"),
        })?;

    write_frame(send, CATALOGUE_RETRIEVAL_V1, &payload)
        .await
        .map_err(|e| RemoteCatalogueFetchError::ProtocolError {
            details: format!("write frame: {e}"),
        })?;

    send.finish()
        .map_err(|e| RemoteCatalogueFetchError::ProtocolError {
            details: format!("finish send: {e}"),
        })?;

    let (_version, resp_bytes) = tokio::time::timeout(
        FETCH_TIMEOUT,
        read_frame(recv, SUPPORTED_CATALOGUE_RETRIEVAL, "catalogue"),
    )
    .await
    .map_err(|_| RemoteCatalogueFetchError::Timeout)?
    .map_err(|e| RemoteCatalogueFetchError::ProtocolError {
        details: format!("read frame: {e}"),
    })?
    .ok_or_else(|| RemoteCatalogueFetchError::ProtocolError {
        details: "server closed connection without response".to_string(),
    })?;

    check_response_payload_size(resp_bytes.len())
        .map_err(|msg| RemoteCatalogueFetchError::ProtocolError { details: msg })?;

    let wire_resp: CatalogWireResponse = postcard::from_bytes(&resp_bytes).map_err(|e| {
        RemoteCatalogueFetchError::ProtocolError {
            details: format!("decode response: {e}"),
        }
    })?;

    Ok(wire_resp.inner)
}

// ── Public API ─────────────────────────────────────────────────────────────

/// Fetch a signed, requester-filtered file catalogue from a remote peer.
///
/// # Arguments
///
/// * `client_ep` — The local endpoint (authenticated with the caller's
///   [`SecretKey`]).
/// * `server_pk` — The [`PublicKey`] of the peer whose catalogue we want.
/// * `known_revision` — If `Some(r)`, the server may return
///   [`RemoteCatalogueFetchError::NotModified`] when `r` matches the
///   current catalogue revision.
///
/// # Returns
///
/// A fully verified [`SignedFileCatalogue`] on success, or an error from
/// [`RemoteCatalogueFetchError`].
async fn fetch_remote_catalogue_inner(
    client_ep: &Endpoint,
    server_pk: PublicKey,
    known_revision: Option<u64>,
) -> std::result::Result<SignedFileCatalogue, RemoteCatalogueFetchError> {
    let addr = EndpointAddr::new(server_pk);

    let conn = tokio::time::timeout(
        FETCH_TIMEOUT,
        client_ep.connect(addr, crate::protocol_version::CATALOGUE_ALPN),
    )
    .await
    .map_err(|_| RemoteCatalogueFetchError::Timeout)?
    .map_err(|e| RemoteCatalogueFetchError::ConnectionFailed {
        details: format!("connect: {e}"),
    })?;

    let (mut send, mut recv) =
        conn.open_bi()
            .await
            .map_err(|e| RemoteCatalogueFetchError::ConnectionFailed {
                details: format!("open_bi: {e}"),
            })?;

    let response = do_fetch_signed_catalogue(&mut send, &mut recv, known_revision).await?;

    // Keep the connection alive until the response is fully read.
    drop(send);
    drop(recv);
    drop(conn);

    match_signed_catalogue_response(response, server_pk, known_revision)
}

/// Fetch a signed catalogue while recording its observable lifecycle.
pub async fn fetch_remote_catalogue(
    client_ep: &Endpoint,
    server_pk: PublicKey,
    known_revision: Option<u64>,
) -> std::result::Result<SignedFileCatalogue, RemoteCatalogueFetchError> {
    DIAGNOSTICS.record_with_peer(
        None,
        Some(server_pk.to_string()),
        DiagnosticEventKind::CatalogueFetchStarted { known_revision },
    );
    let result = fetch_remote_catalogue_inner(client_ep, server_pk, known_revision).await;
    record_fetch_result(server_pk, result)
}

fn record_fetch_result(
    server_pk: PublicKey,
    result: std::result::Result<SignedFileCatalogue, RemoteCatalogueFetchError>,
) -> std::result::Result<SignedFileCatalogue, RemoteCatalogueFetchError> {
    match &result {
        Ok(catalogue) => DIAGNOSTICS.record_with_peer(
            None,
            Some(server_pk.to_string()),
            DiagnosticEventKind::CatalogueFetchCompleted {
                revision: catalogue.revision,
                file_count: catalogue.files.len(),
                collection_count: catalogue.collections.len(),
            },
        ),
        Err(error) => {
            // Only emit CatalogueFetchFailed for transport/protocol errors.
            // Signature rejection and permission denial have their own events
            // emitted by the validation layer (CatalogueSignatureRejected).
            let is_transport_error = matches!(
                error,
                RemoteCatalogueFetchError::ConnectionFailed { .. }
                    | RemoteCatalogueFetchError::Timeout
                    | RemoteCatalogueFetchError::ProtocolError { .. }
            );
            if is_transport_error {
                DIAGNOSTICS.record_with_peer(
                    None,
                    Some(server_pk.to_string()),
                    DiagnosticEventKind::CatalogueFetchFailed {
                        error: error.to_string(),
                    },
                );
            }
        }
    }
    result
}

/// Store a fetched and validated [`SignedFileCatalogue`] to local storage,
/// emitting a [`CatalogueRevisionInstalled`] event on success.
///
/// This is the idiomatic way to persist a remote catalogue after a
/// successful fetch.  The function calls [`Storage::replace_remote_catalogue`]
/// and records the installation event with the catalogue's revision, file
/// count, and collection count.
///
/// # Errors
///
/// Propagates any storage error from `replace_remote_catalogue`.
pub fn process_and_store_remote_catalogue(
    storage: &Storage,
    catalogue: &SignedFileCatalogue,
) -> std::result::Result<(), String> {
    storage
        .replace_remote_catalogue(catalogue)
        .map_err(|e| format!("store remote catalogue: {e:#}"))?;

    DIAGNOSTICS.record_with_peer(
        None,
        Some(catalogue.owner_id.to_string()),
        DiagnosticEventKind::CatalogueRevisionInstalled {
            revision: catalogue.revision,
            file_count: catalogue.files.len(),
            collection_count: catalogue.collections.len(),
        },
    );

    Ok(())
}

/// Handle a catalogue notice/advertisement from a remote peer.
///
/// Orchestrates the full lifecycle:
/// 1. Emits [`CatalogueNoticeReceived`] with the advertised revision.
/// 2. Fetches the remote catalogue via [`fetch_remote_catalogue`].
/// 3. On success, persists the catalogue via [`process_and_store_remote_catalogue`],
///    which emits [`CatalogueRevisionInstalled`].
///
/// The peer identity for all events is sourced from `server_pk`.
///
/// # Errors
///
/// Returns [`RemoteCatalogueFetchError`] if the fetch or store fails.
pub async fn handle_catalogue_notice(
    client_ep: &Endpoint,
    server_pk: PublicKey,
    known_revision: Option<u64>,
    storage: &Storage,
) -> std::result::Result<SignedFileCatalogue, RemoteCatalogueFetchError> {
    DIAGNOSTICS.record_with_peer(
        None,
        Some(server_pk.to_string()),
        DiagnosticEventKind::CatalogueNoticeReceived { known_revision },
    );

    let catalogue = fetch_remote_catalogue(client_ep, server_pk, known_revision).await?;

    process_and_store_remote_catalogue(storage, &catalogue)
        .map_err(|e| RemoteCatalogueFetchError::ProtocolError { details: e })?;

    Ok(catalogue)
}

/// Parse a `CatalogResponse` from a `GetCatalogue` request and apply
/// all validation checks.
fn match_signed_catalogue_response(
    response: CatalogResponse,
    server_pk: PublicKey,
    known_revision: Option<u64>,
) -> std::result::Result<SignedFileCatalogue, RemoteCatalogueFetchError> {
    match response {
        CatalogResponse::SignedCatalogue(catalogue) => {
            validate_complete_catalogue(&catalogue, server_pk)?;
            Ok(catalogue)
        }
        CatalogResponse::CataloguePage(_page) => Err(RemoteCatalogueFetchError::ProtocolError {
            details: "server returned a page instead of a signed catalogue".to_string(),
        }),
        CatalogResponse::NotModified { revision: _ } => Err(RemoteCatalogueFetchError::NotModified),
        CatalogResponse::FileDetails(_) => Err(RemoteCatalogueFetchError::ProtocolError {
            details: "server returned FileDetails instead of a signed catalogue".to_string(),
        }),
        CatalogResponse::RevisionChanged { new_revision } => {
            if known_revision.is_some() && Some(new_revision) == known_revision {
                Err(RemoteCatalogueFetchError::NotModified)
            } else {
                Err(RemoteCatalogueFetchError::RevisionChanged { new_revision })
            }
        }
        CatalogResponse::Error { code, message } => match code {
            crate::catalogue_protocol::CatalogErrorCode::PermissionDenied => {
                Err(RemoteCatalogueFetchError::PermissionDenied)
            }
            _ => Err(RemoteCatalogueFetchError::ProtocolError {
                details: format!("server error: {code} — {message}"),
            }),
        },
    }
}

/// Connect to a server and fetch a single catalogue page.
async fn connect_and_fetch_page(
    client_ep: &Endpoint,
    server_pk: PublicKey,
    known_revision: Option<u64>,
    cursor: Option<String>,
    page_size: u32,
) -> std::result::Result<CatalogResponse, RemoteCatalogueFetchError> {
    let addr = EndpointAddr::new(server_pk);

    let conn = tokio::time::timeout(
        FETCH_TIMEOUT,
        client_ep.connect(addr, crate::protocol_version::CATALOGUE_ALPN),
    )
    .await
    .map_err(|_| RemoteCatalogueFetchError::Timeout)?
    .map_err(|e| RemoteCatalogueFetchError::ConnectionFailed {
        details: format!("connect: {e}"),
    })?;

    let (mut send, mut recv) =
        conn.open_bi()
            .await
            .map_err(|e| RemoteCatalogueFetchError::ConnectionFailed {
                details: format!("open_bi: {e}"),
            })?;

    let response = do_fetch_page(&mut send, &mut recv, known_revision, cursor, page_size).await?;

    drop(send);
    drop(recv);
    drop(conn);

    Ok(response)
}

/// Connect to a server and fetch the signed catalogue.
async fn connect_and_fetch_signed(
    client_ep: &Endpoint,
    server_pk: PublicKey,
    known_revision: Option<u64>,
) -> std::result::Result<CatalogResponse, RemoteCatalogueFetchError> {
    let addr = EndpointAddr::new(server_pk);

    let conn = tokio::time::timeout(
        FETCH_TIMEOUT,
        client_ep.connect(addr, crate::protocol_version::CATALOGUE_ALPN),
    )
    .await
    .map_err(|_| RemoteCatalogueFetchError::Timeout)?
    .map_err(|e| RemoteCatalogueFetchError::ConnectionFailed {
        details: format!("connect: {e}"),
    })?;

    let (mut send, mut recv) =
        conn.open_bi()
            .await
            .map_err(|e| RemoteCatalogueFetchError::ConnectionFailed {
                details: format!("open_bi: {e}"),
            })?;

    let response = do_fetch_signed_catalogue(&mut send, &mut recv, known_revision).await?;

    drop(send);
    drop(recv);
    drop(conn);

    Ok(response)
}

/// Fetch a complete, verified catalogue from a remote peer using paginated
/// retrieval.
///
/// This function walks all pages via `GetCataloguePage` requests, collects
/// the items in memory, then fetches the signed catalogue (with collections
/// and signature) for verification.  The full signed catalogue is **not**
/// installed until every page has been collected and every check has passed.
///
/// Each page and the final verification use a fresh QUIC connection since
/// the handler serves a single request per connection.
///
/// # Arguments
///
/// * `client_ep` — The local endpoint (authenticated with the caller's
///   [`SecretKey`]).
/// * `server_pk` — The [`PublicKey`] of the peer whose catalogue we want.
/// * `page_size` — Number of items per page (clamped server-side if
///   too large, use 500 for the default server maximum).
///
/// # Returns
///
/// A fully verified [`SignedFileCatalogue`] on success, or an error from
/// [`RemoteCatalogueFetchError`].
async fn fetch_paginated_remote_catalogue_inner(
    client_ep: &Endpoint,
    server_pk: PublicKey,
    page_size: u32,
) -> std::result::Result<SignedFileCatalogue, RemoteCatalogueFetchError> {
    let page_size = if page_size == 0 {
        DEFAULT_PAGE_SIZE
    } else {
        page_size.min(MAX_CATALOGUE_PAGE_SIZE)
    };

    // Bound pagination even if a peer keeps issuing valid-looking cursors.
    let max_pages = (MAX_CATALOGUE_FILES as u32 / page_size.max(1) + 2) as usize;
    let mut invalid_response_attempts = 0usize;
    // ── Phase 1: Walk all pages ──────────────────────────────────────
    let mut pages: Vec<crate::catalogue_protocol::CataloguePage> = Vec::new();
    let mut cursor: Option<String> = None;
    let mut known_revision: Option<u64> = None;

    loop {
        if pages.len() >= max_pages {
            return Err(RemoteCatalogueFetchError::ProtocolError {
                details: format!("catalogue pagination exceeded {max_pages} pages"),
            });
        }
        let response =
            connect_and_fetch_page(client_ep, server_pk, known_revision, cursor, page_size).await?;

        match response {
            CatalogResponse::CataloguePage(page) => {
                // Track revision from first page onward.
                if known_revision.is_none() {
                    known_revision = Some(page.revision);
                } else if Some(page.revision) != known_revision {
                    return Err(RemoteCatalogueFetchError::RevisionChanged {
                        new_revision: page.revision,
                    });
                }

                let has_next = page.next_cursor.is_some();
                pages.push(page);

                if has_next {
                    cursor = pages.last().unwrap().next_cursor.clone();
                    continue;
                }
                // No next cursor — we have all pages.
                break;
            }
            CatalogResponse::RevisionChanged { new_revision } => {
                return Err(RemoteCatalogueFetchError::RevisionChanged { new_revision });
            }
            CatalogResponse::Error { code, message } => match code {
                crate::catalogue_protocol::CatalogErrorCode::PermissionDenied => {
                    return Err(RemoteCatalogueFetchError::PermissionDenied);
                }
                _ => {
                    return Err(RemoteCatalogueFetchError::ProtocolError {
                        details: format!("server error: {code} — {message}"),
                    });
                }
            },
            other => {
                invalid_response_attempts += 1;
                if invalid_response_attempts >= MAX_INVALID_RESPONSE_ATTEMPTS {
                    return Err(RemoteCatalogueFetchError::ProtocolError {
                        details: format!(
                            "peer returned {invalid_response_attempts} invalid catalogue responses"
                        ),
                    });
                }
                return Err(RemoteCatalogueFetchError::ProtocolError {
                    details: format!("unexpected response during pagination: {other:?}"),
                });
            }
        }
    }

    // ── Validate pages: revision consistency, completeness ────────────
    let (_all_items, revision) = validate_pages(&pages)?;

    // ── Phase 2: Fetch the signed catalogue for verification ─────────
    let signed_response = connect_and_fetch_signed(client_ep, server_pk, Some(revision)).await?;

    let signed_catalogue = match signed_response {
        CatalogResponse::SignedCatalogue(cat) => {
            // Cross-check that page items match the signed catalogue items.
            let page_items: Vec<_> = pages.iter().flat_map(|p| p.items.clone()).collect();
            let page_hashes: std::collections::BTreeSet<&str> =
                page_items.iter().map(|f| f.content_hash.as_str()).collect();
            let signed_hashes: std::collections::BTreeSet<&str> =
                cat.files.iter().map(|f| f.content_hash.as_str()).collect();
            if page_hashes != signed_hashes {
                return Err(RemoteCatalogueFetchError::SignatureInvalid {
                    details: format!(
                        "page item hashes do not match signed catalogue (pages: {}, signed: {})",
                        page_hashes.len(),
                        signed_hashes.len()
                    ),
                });
            }
            cat
        }
        CatalogResponse::NotModified { .. } => {
            // Server confirms content is unchanged at this revision.
            // Re-fetch without known_revision to force the full signed
            // catalogue response (carries collections + signature).
            let forced_response = connect_and_fetch_signed(client_ep, server_pk, None).await?;

            let cat = match forced_response {
                CatalogResponse::SignedCatalogue(c) => c,
                other => {
                    return Err(RemoteCatalogueFetchError::ProtocolError {
                        details: format!("expected SignedCatalogue on re-fetch, got {other:?}"),
                    });
                }
            };

            // Cross-check page items against the forced signed catalogue.
            let page_items: Vec<_> = pages.iter().flat_map(|p| p.items.clone()).collect();
            let page_hashes: std::collections::BTreeSet<&str> =
                page_items.iter().map(|f| f.content_hash.as_str()).collect();
            let signed_hashes: std::collections::BTreeSet<&str> =
                cat.files.iter().map(|f| f.content_hash.as_str()).collect();
            if page_hashes != signed_hashes {
                return Err(RemoteCatalogueFetchError::SignatureInvalid {
                    details: format!(
                        "page item hashes do not match signed catalogue (pages: {}, signed: {})",
                        page_hashes.len(),
                        signed_hashes.len()
                    ),
                });
            }

            cat
        }
        CatalogResponse::RevisionChanged { new_revision } => {
            return Err(RemoteCatalogueFetchError::RevisionChanged { new_revision });
        }
        CatalogResponse::Error { code, message } => match code {
            crate::catalogue_protocol::CatalogErrorCode::PermissionDenied => {
                return Err(RemoteCatalogueFetchError::PermissionDenied);
            }
            _ => {
                return Err(RemoteCatalogueFetchError::ProtocolError {
                    details: format!("server error on verification fetch: {code} — {message}"),
                });
            }
        },
        other => {
            return Err(RemoteCatalogueFetchError::ProtocolError {
                details: format!("unexpected response on verification fetch: {other:?}"),
            });
        }
    };

    // ── Apply all validation on the signed catalogue ─────────────────
    validate_complete_catalogue(&signed_catalogue, server_pk)?;

    Ok(signed_catalogue)
}

/// Fetch a paginated signed catalogue while recording its observable lifecycle.
pub async fn fetch_paginated_remote_catalogue(
    client_ep: &Endpoint,
    server_pk: PublicKey,
    page_size: u32,
) -> std::result::Result<SignedFileCatalogue, RemoteCatalogueFetchError> {
    DIAGNOSTICS.record_with_peer(
        None,
        Some(server_pk.to_string()),
        DiagnosticEventKind::CatalogueFetchStarted {
            known_revision: None,
        },
    );
    let result = fetch_paginated_remote_catalogue_inner(client_ep, server_pk, page_size).await;
    record_fetch_result(server_pk, result)
}
