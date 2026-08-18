//! Path resolution/containment and I/O preparation for file transfer.
//!
//! Owns [`prepare_imported_file`] (confirm/import content-addressed bytes)
//! and [`prepare_referenced_file`] (validate the on-disk `source_path` —
//! regular file, not symlink/directory, expected size — then import it by
//! reference).  Both return safe [`PreparedFile`] metadata with no local paths.

use iroh_blobs::api::blobs::{AddPathOptions, BlobStatus, ImportMode};
use rusqlite::params;

use crate::file_access_protocol::{BlobFormat, PreparedFile};
use crate::storage::Storage;

/// Prepare an imported file for transfer — confirm managed bytes exist,
/// optionally verify content integrity, and return safe transfer metadata.
///
/// # Steps
///
/// 1. **Look up** the file object by `content_hash` in local storage.
/// 2. **Import check** — if the file has a `blob_hash` (imported from a
///    remote peer), confirm the blob exists in the `blob_store`.  If the
///    file has inline `data`, import those bytes into the blob store so
///    the file can be served by hash.
/// 3. **Optional verification** — if `verify_hash` and/or `verify_size`
///    are provided, verify that the stored metadata matches.
/// 4. **Return** a [`PreparedFile`] with safe metadata (no local paths).
///
/// # Errors
///
/// Returns an error if the file object is not found in the database, if
/// the blob store does not contain the expected blob, or if optional
/// hash/size verification fails.
pub async fn prepare_imported_file(
    storage: &Storage,
    blob_store: &iroh_blobs::api::Store,
    content_hash: &str,
    verify_hash: Option<&str>,
    verify_size: Option<u64>,
) -> Result<PreparedFile, anyhow::Error> {
    // ── 1. Look up the file object ───────────────────────────────────
    let file_obj = storage
        .run_blocking("file_access.prepare_imported.get_file_object", {
            let content_hash = content_hash.to_owned();
            move |s| {
                s.get_file_object(&content_hash)
                    .map_err(|e| anyhow::anyhow!("db lookup failed: {e:#}"))
            }
        })
        .await?
        .ok_or_else(|| anyhow::anyhow!("file not found: {content_hash}"))?;

    // ── 2. Check / import blob availability ─────────────────────────
    // Check if the file has a blob_hash (imported from a remote peer).
    let blob_hash_str: Option<String> = storage
        .run_blocking("file_access.prepare_imported.blob_hash", {
            let content_hash = content_hash.to_owned();
            move |s| {
                s.with_conn(|conn| {
                    let mut stmt = conn
                        .prepare(
                            "SELECT blob_hash FROM file_objects \
                             WHERE content_hash = ?1 AND blob_hash IS NOT NULL",
                        )
                        .map_err(|e| anyhow::anyhow!("prepare blob_hash query: {e}"))?;
                    let result: Option<String> =
                        stmt.query_row(params![content_hash], |row| row.get(0)).ok();
                    Ok(result)
                })
            }
        })
        .await
        .unwrap_or(None);

    if let Some(ref hash_str) = blob_hash_str {
        // Imported file — confirm the blob exists in the iroh-blobs store
        // and that the content-addressed store's size matches the DB
        // record.  The blob store is authoritative for imported content
        // (BORU-AUDIT-07); a mismatch means the record is stale or corrupt
        // and must not be signed under guessed metadata.
        let blob_hash: iroh_blobs::Hash = hash_str
            .parse()
            .map_err(|e| anyhow::anyhow!("invalid blob hash {hash_str}: {e}"))?;

        let blob_present = blob_store
            .blobs()
            .has(blob_hash)
            .await
            .map_err(|e| anyhow::anyhow!("blob_store.has failed: {e:#}"))?;

        if !blob_present {
            return Err(anyhow::anyhow!(
                "imported blob missing from store: {hash_str}"
            ));
        }

        match blob_store
            .blobs()
            .status(blob_hash)
            .await
            .map_err(|e| anyhow::anyhow!("blob_store.status failed: {e:#}"))?
        {
            BlobStatus::Complete { size } if size == file_obj.size => {}
            BlobStatus::Complete { size } => {
                return Err(anyhow::anyhow!(
                    "imported blob size mismatch: store has {size}, DB records {}",
                    file_obj.size
                ));
            }
            BlobStatus::Partial { .. } => {
                return Err(anyhow::anyhow!(
                    "imported blob only partially stored: {hash_str}"
                ));
            }
            BlobStatus::NotFound => {
                return Err(anyhow::anyhow!(
                    "imported blob missing from store: {hash_str}"
                ));
            }
        }
    } else if let Some(ref data) = file_obj.data {
        // Inline file — import into blob store if not already present.
        let hash_for_inline = blake3::hash(data);
        let blob_hash = iroh_blobs::Hash::from(hash_for_inline);
        let already_present = blob_store
            .blobs()
            .has(blob_hash)
            .await
            .map_err(|e| anyhow::anyhow!("blob_store.has failed: {e:#}"))?;
        if !already_present {
            let progress = blob_store.blobs().add_slice(data);
            let _tag = progress
                .await
                .map_err(|e| anyhow::anyhow!("add_slice failed: {e:#}"))?;
        }
    } else {
        // File exists in DB but has neither blob_hash nor inline data.
        return Err(anyhow::anyhow!(
            "file object {content_hash} has no data and no blob hash"
        ));
    }

    // ── 3. Optional verification ─────────────────────────────────────
    if let Some(expected_size) = verify_size {
        if file_obj.size != expected_size {
            return Err(anyhow::anyhow!(
                "size mismatch: expected {expected_size}, got {}",
                file_obj.size
            ));
        }
    }

    if let Some(expected_hash) = verify_hash {
        let expected = expected_hash.to_ascii_lowercase();
        if file_obj.content_hash != expected {
            return Err(anyhow::anyhow!(
                "hash mismatch: expected {expected}, got {}",
                file_obj.content_hash
            ));
        }
    }

    // ── 4. Return safe transfer metadata ────────────────────────────
    // Determine blob format — imported files are always Raw; inline
    // files are also Raw by default.
    let blob_format = BlobFormat::Raw;

    Ok(PreparedFile {
        content_hash: file_obj.content_hash,
        size_bytes: file_obj.size,
        blob_format,
        mime_type: file_obj.mime_type,
        filename: file_obj.filename,
    })
}

/// Prepare a referenced file for transfer — confirm the source still
/// exists on the local filesystem, verify integrity, and import it into
/// the iroh-blobs store so it can be served by hash.
///
/// # Steps
///
/// 1. **Look up** the file object by `content_hash` in local storage.
/// 2. **Path validation** — confirm the `source_path` exists, is a
///    regular file (not a directory, not a symlink), and has the
///    expected size.
/// 3. **Content verification (optional)** — when `verify_hash` is
///    provided, read the file from disk and verify its blake3 content
///    hash matches the expected value.  The live file-access path
///    defers hash verification to descriptor verification and passes
///    `None`, so the common case skips the read entirely.
/// 4. **Import by reference** — import the on-disk file via
///    `add_path_with_opts` with [`ImportMode::TryReference`] so the
///    store references the file in place instead of copying its bytes
///    (no double storage for files that already live on disk).
/// 5. **Return** a [`PreparedFile`] with safe metadata (no local paths).
///
/// # Errors
///
/// Returns an error if the file object does not exist in the database,
/// has no `source_path` field, the source file is missing or has been
/// replaced (by a directory, symlink, or different content), or the
/// file cannot be read.
pub async fn prepare_referenced_file(
    storage: &Storage,
    blob_store: &iroh_blobs::api::Store,
    content_hash: &str,
    verify_hash: Option<&str>,
    verify_size: Option<u64>,
) -> Result<PreparedFile, anyhow::Error> {
    use std::fs;
    use std::io;

    // ── 1. Look up the file object ───────────────────────────────────
    let file_obj = storage
        .run_blocking("file_access.prepare_referenced.get_file_object", {
            let content_hash = content_hash.to_owned();
            move |s| {
                s.get_file_object(&content_hash)
                    .map_err(|e| anyhow::anyhow!("db lookup failed: {e:#}"))
            }
        })
        .await?
        .ok_or_else(|| anyhow::anyhow!("file not found: {content_hash}"))?;

    // ── 2. Get and validate the source path ──────────────────────────
    let src = file_obj
        .source_path
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("file object {content_hash} has no source path"))?
        .to_owned();

    // Confirm the path exists and is a regular file (not a directory,
    // not a symlink). stat + canonicalise are blocking filesystem calls;
    // run them on the blocking pool so they never occupy a Tokio worker
    // (BORU-AUDIT-12).
    let (metadata, abs_path) = {
        let src_for_worker = src.clone();
        tokio::task::spawn_blocking(move || {
            let metadata = fs::symlink_metadata(&src_for_worker).map_err(|e| {
                if e.kind() == io::ErrorKind::NotFound {
                    anyhow::anyhow!("referenced source file not found: {src_for_worker}")
                } else {
                    anyhow::anyhow!("failed to stat referenced source {src_for_worker}: {e:#}")
                }
            })?;
            let abs_path = fs::canonicalize(&src_for_worker).map_err(|e| {
                anyhow::anyhow!("failed to resolve referenced source {src_for_worker}: {e:#}")
            })?;
            Ok::<_, anyhow::Error>((metadata, abs_path))
        })
        .await
        .map_err(|join_err| anyhow::anyhow!("referenced-source stat task panicked: {join_err}"))??
    };

    if metadata.is_dir() {
        return Err(anyhow::anyhow!(
            "referenced source is a directory, not a regular file: {src}"
        ));
    }

    if metadata.file_type().is_symlink() {
        return Err(anyhow::anyhow!(
            "referenced source is a symlink, not a regular file: {src}"
        ));
    }

    // ── 3. Size verification ─────────────────────────────────────────
    if let Some(expected_size) = verify_size {
        let actual_size = metadata.len();
        if actual_size != expected_size {
            return Err(anyhow::anyhow!(
                "size mismatch: expected {expected_size}, got {actual_size}"
            ));
        }
    }

    // ── 4. Content verification (only when explicitly requested) ─────
    // The live file-access path defers hash verification to descriptor
    // verification (passes `None` here), so the common case does not
    // read the file at all — `add_path_with_opts` below reads and hashes
    // it internally during import. When a caller *does* request hash
    // verification, read the file on a blocking worker and compare the
    // blake3 hash before importing, so a mismatched file is never
    // imported into the blob store.
    if let Some(expected_hash) = verify_hash {
        let src_for_worker = src.clone();
        let actual_hex = tokio::task::spawn_blocking(move || {
            let data = fs::read(&src_for_worker).map_err(|e| {
                if e.kind() == io::ErrorKind::PermissionDenied {
                    anyhow::anyhow!("permission denied reading referenced source: {src_for_worker}")
                } else {
                    anyhow::anyhow!("failed to read referenced source {src_for_worker}: {e:#}")
                }
            })?;
            let hash = blake3::hash(&data);
            Ok::<_, anyhow::Error>(hex::encode(hash.as_bytes()))
        })
        .await
        .map_err(|join_err| anyhow::anyhow!("referenced-file read task panicked: {join_err}"))??;

        let expected = expected_hash.to_ascii_lowercase();
        if actual_hex != expected {
            return Err(anyhow::anyhow!(
                "hash mismatch: expected {expected}, got {actual_hex}"
            ));
        }
    }

    // ── 5. Import into iroh-blobs store by reference ────────────────
    // The source is a real file on disk — import it with
    // `ImportMode::TryReference` so the store references the file in
    // place instead of copying its bytes (avoids double-storing files
    // that already live on disk). Stores that cannot reference files
    // (e.g. the in-memory store) fall back to copying internally.
    // `add_path_with_opts` requires an absolute path — canonicalised on
    // the blocking pool above.
    let progress = blob_store.blobs().add_path_with_opts(AddPathOptions {
        path: abs_path,
        mode: ImportMode::TryReference,
        format: iroh_blobs::BlobFormat::Raw,
    });
    let _tag = progress
        .await
        .map_err(|e| anyhow::anyhow!("add_path failed: {e:#}"))?;

    // ── 6. Return safe transfer metadata ─────────────────────────────
    Ok(PreparedFile {
        content_hash: file_obj.content_hash,
        size_bytes: metadata.len(),
        blob_format: BlobFormat::Raw,
        mime_type: file_obj.mime_type,
        filename: file_obj.filename,
    })
}
