//! Per-connection request dispatch for the catalogue retrieval protocol.
//!
//! Contains the `serve_catalogue` worker that reads a
//! [`CatalogRequest`](crate::catalogue_protocol::CatalogRequest) from an
//! already-accepted bi-directional stream, builds the requester-specific
//! signed response using the [`CatalogueHandler`](super::CatalogueHandler)
//! helpers, and writes it back.

use std::time::{SystemTime, UNIX_EPOCH};

use iroh::endpoint::Connection;
use tracing::{error, warn};

use super::CatalogueHandler;
use crate::catalogue_limits::{MAX_CATALOGUE_PAGE_SIZE, MAX_CATALOGUE_REQUEST_BYTES};
use crate::catalogue_model::{
    CatalogueView, FileCatalogueCollection, RemoteCollection, SignedCatalogueCursor,
    SignedFileCatalogue,
};
use crate::catalogue_policy::{is_requester_blocked, validate_catalogue_view};
use crate::catalogue_protocol::{
    CatalogErrorCode, CatalogRequest, CatalogResponse, CatalogWireRequest, CatalogWireResponse,
};
use crate::catalogue_rate_limits::{write_rate_limited_response, CatalogueAdmission};
use crate::catalogue_wire::{
    write_catalogue_response, write_file_details_response, write_page_response,
};
use crate::chat_core::DIAGNOSTICS;
use crate::diagnostics::DiagnosticEventKind;
use crate::friends::FriendId;
use crate::protocol_version::{read_frame, SUPPORTED_CATALOGUE_RETRIEVAL};

/// Serve a single catalogue request on an already-accepted connection.
///
/// Reads a [`CatalogRequest`] from the bi-directional stream, builds a
/// signed catalogue for the authenticated remote peer, and writes the
/// response back.
pub(super) async fn serve_catalogue(
    connection: &Connection,
    handler: &CatalogueHandler,
) -> std::result::Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let remote_id = connection.remote_id();

    // ── Per-peer abuse limiter check ─────────────────────────────────
    let peer_key = remote_id.to_string();
    if !matches!(
        handler.abuse_limiter.admit(&peer_key),
        CatalogueAdmission::Allowed
    ) {
        warn!(peer = %remote_id.fmt_short(), "catalogue: blocked peer request by abuse limit");
        let (mut send, mut recv) = connection.accept_bi().await?;
        // Drain the request data so the stream closes cleanly.
        let _ = tokio::io::copy(&mut recv, &mut tokio::io::sink()).await;
        write_rate_limited_response(&mut send).await?;
        send.finish()?;
        return Ok(());
    }

    // Accept the bi-directional stream opened by the client.
    let (mut send, mut recv) = connection.accept_bi().await?;

    // Read the versioned request frame.
    let (_version, payload) =
        match read_frame(&mut recv, SUPPORTED_CATALOGUE_RETRIEVAL, "catalogue").await? {
            Some(result) => result,
            None => {
                // Clean end-of-stream — nothing to do.
                return Ok(());
            }
        };

    // ── Reject oversized request payloads ────────────────────────────
    if payload.len() > MAX_CATALOGUE_REQUEST_BYTES {
        // Oversized payloads are malformed protocol attempts: count them in
        // the same budget as failed postcard decoding so an attacker cannot
        // bypass malformed-attempt blocking by sending oversized frames.
        let remains_unblocked = handler.abuse_limiter.record_invalid(&peer_key);
        let response = CatalogResponse::error(
            if remains_unblocked {
                CatalogErrorCode::InvalidRequest
            } else {
                CatalogErrorCode::PermissionDenied
            },
            format!(
                "request payload too large ({} > {MAX_CATALOGUE_REQUEST_BYTES})",
                payload.len()
            ),
        );
        // Use write_catalogue_response here even though it's an error
        // response — the limit check is against MAX_CATALOGUE_RESPONSE_BYTES
        // and error frames are tiny.
        write_catalogue_response(&mut send, response).await?;
        send.finish()?;
        return Ok(());
    }

    // Deserialize the inner request.
    let wire_req: CatalogWireRequest = match postcard::from_bytes(&payload) {
        Ok(request) => request,
        Err(error) => {
            let remains_unblocked = handler.abuse_limiter.record_invalid(&peer_key);
            warn!(
                peer = %remote_id.fmt_short(),
                blocked = !remains_unblocked,
                "catalogue: malformed request rejected: {error}"
            );
            let code = if remains_unblocked {
                CatalogErrorCode::InvalidRequest
            } else {
                CatalogErrorCode::PermissionDenied
            };
            write_catalogue_response(
                &mut send,
                CatalogResponse::error(code, "malformed catalogue request"),
            )
            .await?;
            send.finish()?;
            return Ok(());
        }
    };
    let request = wire_req.inner;

    match request {
        CatalogRequest::GetCataloguePage {
            known_revision: _known_revision,
            cursor,
            page_size,
        } => {
            // A zero-sized page is a valid probe: return no items and no cursor.
            let page_size = page_size.min(MAX_CATALOGUE_PAGE_SIZE);

            // ── Blocked check (early) ──────────────────────────────────
            let requester_id = FriendId::from_public_key(remote_id);
            let is_blocked = is_requester_blocked(&handler.friends, &requester_id);

            if is_blocked {
                let response = CatalogResponse::Error {
                    code: CatalogErrorCode::PermissionDenied,
                    message: "You are blocked from viewing this catalogue".to_string(),
                };
                write_catalogue_response(&mut send, response).await?;
                send.finish()?;
                return Ok(());
            }

            // ── Build the signed catalogue for this requester ──────────
            // `build_catalogue_for_requester` runs SQLite queries; execute
            // on the blocking pool so the QUIC accept worker is never
            // stalled (BORU-AUDIT-18).
            let handler_clone = handler.clone();
            let remote_id_clone = remote_id;
            let catalogue = match tokio::task::spawn_blocking(move || {
                handler_clone.build_catalogue_for_requester(&remote_id_clone)
            })
            .await
            .map_err(|join_err| {
                Box::new(std::io::Error::new(
                    std::io::ErrorKind::Other,
                    format!("catalogue worker panicked: {join_err}"),
                )) as Box<dyn std::error::Error + Send + Sync>
            })? {
                Ok(cat) => cat,
                Err(code) => {
                    let response = CatalogResponse::error(code, "request denied");
                    write_catalogue_response(&mut send, response).await?;
                    send.finish()?;
                    return Ok(());
                }
            };

            // ── Validate catalogue item limits ────────────────────────
            {
                let collections: Vec<RemoteCollection> = catalogue
                    .collections
                    .iter()
                    .map(|c| RemoteCollection {
                        collection_id: c.collection_id.clone(),
                        name: c.name.clone(),
                        description: c.description.clone(),
                        sort_order: 0,
                    })
                    .collect();
                let view = CatalogueView {
                    collections,
                    files: catalogue.files.clone(),
                };
                if let Some(msg) = validate_catalogue_view(&view) {
                    let response = CatalogResponse::error(CatalogErrorCode::InvalidRequest, &msg);
                    write_catalogue_response(&mut send, response).await?;
                    send.finish()?;
                    return Ok(());
                }
            }

            // ── Decode and validate the signed cursor ──────────────────
            let start_index: usize = if let Some(cursor_str) = &cursor {
                let decoded = SignedCatalogueCursor::decode(cursor_str).ok_or_else(|| {
                    Box::new(std::io::Error::new(
                        std::io::ErrorKind::InvalidData,
                        "invalid cursor encoding",
                    )) as Box<dyn std::error::Error + Send + Sync>
                })?;

                // Verify the cursor's signature.
                if let Err(e) = decoded.verify() {
                    warn!(
                        peer = %remote_id.fmt_short(),
                        "GetCataloguePage: invalid cursor signature: {e:#}"
                    );
                    let response = CatalogResponse::error(
                        CatalogErrorCode::InvalidRequest,
                        "invalid cursor signature",
                    );
                    write_catalogue_response(&mut send, response).await?;
                    send.finish()?;
                    return Ok(());
                }

                // Verify the cursor owner matches this server.
                if decoded.owner_id != handler.secret_key.public() {
                    let response = CatalogResponse::error(
                        CatalogErrorCode::InvalidRequest,
                        "cursor owner does not match server",
                    );
                    write_catalogue_response(&mut send, response).await?;
                    send.finish()?;
                    return Ok(());
                }

                // Verify the cursor was issued for the requesting peer.
                if decoded.requester != remote_id {
                    let response = CatalogResponse::error(
                        CatalogErrorCode::PermissionDenied,
                        "cursor was issued for a different peer",
                    );
                    write_catalogue_response(&mut send, response).await?;
                    send.finish()?;
                    return Ok(());
                }

                // Verify the cursor revision matches the current catalogue revision.
                // When the revision changed, signal RevisionChanged so the
                // client restarts pagination rather than receiving pages from
                // two different revisions.
                if decoded.revision != catalogue.revision {
                    let response = CatalogResponse::RevisionChanged {
                        new_revision: catalogue.revision,
                    };
                    write_catalogue_response(&mut send, response).await?;
                    send.finish()?;
                    return Ok(());
                }

                // Find the position of the cursor's (last_updated_at_ms, last_file_id)
                // in the sorted file list.  Files are sorted by updated_at_ms DESC.
                // When the target file no longer exists, the catalogue changed
                // since the cursor was issued — signal RevisionChanged.
                let pos = match catalogue.files.iter().position(|f| {
                    f.updated_at_ms == decoded.last_updated_at_ms
                        && f.shared_file_id == decoded.last_file_id
                }) {
                    Some(p) => p,
                    None => {
                        let response = CatalogResponse::RevisionChanged {
                            new_revision: catalogue.revision,
                        };
                        write_catalogue_response(&mut send, response).await?;
                        send.finish()?;
                        return Ok(());
                    }
                };

                // The next page starts after the cursor's target file.
                pos + 1
            } else {
                0 // First page — start from the beginning.
            };

            // ── Paginate from start_index ──────────────────────────────
            let total_files = catalogue.files.len();
            let end = (start_index + page_size as usize).min(total_files);
            let page_items: Vec<_> = catalogue.files[start_index..end].to_vec();

            // ── Build the signed next cursor ───────────────────────────
            let next_cursor = if end < total_files && !page_items.is_empty() {
                let last = &page_items[page_items.len() - 1];
                let signed_cursor = SignedCatalogueCursor::sign(
                    &handler.secret_key,
                    catalogue.revision,
                    last.updated_at_ms,
                    &last.shared_file_id,
                    remote_id,
                );
                Some(signed_cursor.encode())
            } else {
                None
            };

            // ── Return a CataloguePage (paginated) ─────────────────────
            let page = crate::catalogue_protocol::CataloguePage {
                revision: catalogue.revision,
                items: page_items,
                next_cursor,
            };
            let response = CatalogResponse::CataloguePage(page);
            handler.abuse_limiter.record_response_bytes(
                &peer_key,
                postcard::to_stdvec(&CatalogWireResponse::new(response.clone()))?.len(),
            );
            write_page_response(&mut send, response).await?;
            send.finish()?;
        }
        CatalogRequest::GetCatalogue { known_revision } => {
            // ── Blocked check ──────────────────────────────────────────
            let requester_id = FriendId::from_public_key(remote_id);
            let is_blocked = is_requester_blocked(&handler.friends, &requester_id);

            if is_blocked {
                let response = CatalogResponse::Error {
                    code: CatalogErrorCode::PermissionDenied,
                    message: "You are blocked from viewing this catalogue".to_string(),
                };
                write_catalogue_response(&mut send, response).await?;
                send.finish()?;
                return Ok(());
            }

            // ── Get manifest revision for early check ──────────────────
            let current_revision = handler
                .storage
                .run_blocking("catalogue.get_manifest_state", {
                    let profile_user_id = handler.profile_user_id.clone();
                    move |s| s.get_manifest_state(&profile_user_id)
                })
                .await
                .ok()
                .flatten()
                .map(|m| m.revision)
                .unwrap_or(0);

            // ── Build the requester-specific view ──────────────────────
            // `catalogue_entries_for_peer` runs a SQLite query; run it on the
            // blocking pool so the QUIC accept worker is never stalled
            // (BORU-AUDIT-18).
            let view = match handler
                .storage
                .run_blocking("catalogue.entries_for_peer", {
                    let profile_user_id = handler.profile_user_id.clone();
                    let remote_id = remote_id;
                    let friends = handler.friends.clone();
                    move |s| s.catalogue_entries_for_peer(&profile_user_id, &remote_id, &friends)
                })
                .await
            {
                Ok(v) => v,
                Err(e) => {
                    error!(
                        peer = %remote_id.fmt_short(),
                        "GetCatalogue: catalogue_entries_for_peer: {e:#}"
                    );
                    let response = CatalogResponse::internal_error();
                    write_catalogue_response(&mut send, response).await?;
                    send.finish()?;
                    return Ok(());
                }
            };

            // ── Validate catalogue view against limits ────────────────
            if let Some(msg) = validate_catalogue_view(&view) {
                error!(
                    peer = %remote_id.fmt_short(),
                    "GetCatalogue: validation failed: {msg}"
                );
                let response = CatalogResponse::error(CatalogErrorCode::InvalidRequest, &msg);
                write_catalogue_response(&mut send, response).await?;
                send.finish()?;
                return Ok(());
            }

            // ── Compute content hash for NotModified detection ─────────
            let view_hash = CatalogueHandler::compute_view_hash(&view);

            // ── Check for NotModified (content-aware) ──────────────────
            if let Some(known) = known_revision {
                if known == current_revision
                    && handler.is_view_unchanged(&requester_id, known, view_hash)
                {
                    DIAGNOSTICS.record_with_peer(
                        None,
                        Some(remote_id.to_string()),
                        DiagnosticEventKind::CatalogueCachedDataUsed {
                            cached_revision: current_revision,
                        },
                    );
                    let response = CatalogResponse::NotModified {
                        revision: current_revision,
                    };
                    write_catalogue_response(&mut send, response).await?;
                    send.finish()?;
                    return Ok(());
                }
            }

            // ── Build and sign the full catalogue ──────────────────────
            let now_ms = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis() as u64;

            let collections: Vec<FileCatalogueCollection> = view
                .collections
                .iter()
                .map(|c| FileCatalogueCollection {
                    collection_id: c.collection_id.clone(),
                    name: c.name.clone(),
                    description: c.description.clone(),
                })
                .collect();
            let catalogue = SignedFileCatalogue::sign(
                &handler.secret_key,
                current_revision,
                now_ms,
                collections,
                view.files.clone(),
            );

            // Cache the view hash for future NotModified checks.
            handler.cache_view_hash(&requester_id, current_revision, view_hash);

            let response = CatalogResponse::SignedCatalogue(catalogue);
            handler.abuse_limiter.record_response_bytes(
                &peer_key,
                postcard::to_stdvec(&CatalogWireResponse::new(response.clone()))?.len(),
            );
            write_catalogue_response(&mut send, response).await?;
            send.finish()?;
        }
        CatalogRequest::GetFileDetails { shared_file_id } => {
            // `get_file_details_for_requester` runs SQLite queries; execute
            // on the blocking pool so the QUIC accept worker is never
            // stalled (BORU-AUDIT-18).
            let handler_clone = handler.clone();
            let shared_file_id_clone = shared_file_id.clone();
            match tokio::task::spawn_blocking(move || {
                handler_clone.get_file_details_for_requester(&remote_id, &shared_file_id_clone)
            })
            .await
            .map_err(|join_err| {
                Box::new(std::io::Error::new(
                    std::io::ErrorKind::Other,
                    format!("catalogue file-details worker panicked: {join_err}"),
                )) as Box<dyn std::error::Error + Send + Sync>
            })? {
                Ok(Some(file)) => {
                    // ── Validate the file entry before sending ────────
                    if let Err(e) = file.validate() {
                        error!(
                            peer = %remote_id.fmt_short(),
                            "GetFileDetails: validation failed: {e}"
                        );
                        let response = CatalogResponse::error(
                            CatalogErrorCode::InternalError,
                            "invalid file metadata",
                        );
                        write_catalogue_response(&mut send, response).await?;
                        send.finish()?;
                        return Ok(());
                    }
                    let response = CatalogResponse::FileDetails(file);
                    handler.abuse_limiter.record_response_bytes(
                        &peer_key,
                        postcard::to_stdvec(&CatalogWireResponse::new(response.clone()))?.len(),
                    );
                    write_file_details_response(&mut send, response).await?;
                    send.finish()?;
                }
                Ok(None) => {
                    let response = CatalogResponse::error(
                        CatalogErrorCode::NotFound,
                        "file not found or not visible",
                    );
                    write_catalogue_response(&mut send, response).await?;
                    send.finish()?;
                }
                Err(code) => {
                    let response = CatalogResponse::error(code, "request denied");
                    write_catalogue_response(&mut send, response).await?;
                    send.finish()?;
                }
            }
        }
    }

    Ok(())
}
