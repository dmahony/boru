//! Whole-directory (HashSeq collection) transfer — import a folder tree into
//! iroh-blobs as a single collection, and export a received collection back
//! to disk as a folder tree.
//!
//! # Import
//!
//! [`import_collection`] walks a directory with `walkdir`, flattens it into
//! `(relative_name, path)` pairs (validated with
//! [`canonicalized_path_to_string`] so names never contain path separators or
//! traversal components), imports each file in parallel with
//! [`ImportMode::TryReference`] (copy-on-write when the store supports it),
//! keeps the per-file [`TempTag`]s alive while building the
//! [`Collection`], then stores the collection and drops the tags.  The
//! returned tag references the HashSeq root, which is what a
//! [`BlobTicket`] carries for a folder share.
//!
//! # Export
//!
//! [`export_collection`] mirrors sendme's export loop: for each entry it
//! builds a traversal-safe target under a caller-chosen root with
//! [`get_export_path`] (which rejects `/`, `\`, `..` and `.` components),
//! refuses to overwrite an existing target, and copies the blob out with
//! [`ExportMode::Copy`].
//!
//! # Security
//!
//! Collection entry names are peer-controlled.  The single
//! `validate_path_component` gate used at both import and export time keeps
//! the wire names confined to safe relative components, and
//! [`get_export_path`] additionally verifies the joined path stays under the
//! export root (belt-and-suspenders, matching the stronger guarantees in
//! [`crate::safe_destination`]).

use std::path::{Component, Path, PathBuf};

use anyhow::{anyhow, ensure, Context};
use n0_future::BufferedStreamExt;
use n0_future::StreamExt;

use iroh_blobs::api::blobs::{
    AddPathOptions, AddProgressItem, ExportMode, ExportOptions, ExportProgressItem, ImportMode,
};
use iroh_blobs::api::{Store, TempTag};
use iroh_blobs::format::collection::Collection;
use iroh_blobs::BlobFormat;

/// Validate a single path component of a collection entry name.
///
/// Rejects anything that could act as a path separator or traversal
/// reference: `/`, `\`, `..`, `.`, and the empty string.  This is the single
/// gate applied to every component at both import (name construction) and
/// export (path construction) time.
pub fn validate_path_component(component: &str) -> anyhow::Result<()> {
    ensure!(!component.is_empty(), "path component must not be empty");
    ensure!(
        !component.contains('/'),
        "path components must not contain the path separator '/'"
    );
    ensure!(
        !component.contains('\\'),
        "path components must not contain the path separator '\\'"
    );
    ensure!(
        component != "." && component != "..",
        "path component must not be '.' or '..'"
    );
    Ok(())
}

/// Convert an already-canonicalised, relative path to a `/`-joined string,
/// validating every component.
///
/// Mirrors sendme's `canonicalized_path_to_string` with `must_be_relative =
/// true`, but additionally rejects `.` and `..` components (sendme relies on
/// the caller to have canonicalised; here we enforce it defensively).
///
/// # Errors
///
/// Fails if the path is absolute, contains any non-normal component
/// (`ParentDir`/`CurDir`/`RootDir`/`Prefix`), or a component contains `/` or
/// `\` (impossible for a real path on Unix, but the string can be hostile on
/// Windows).
pub fn canonicalized_path_to_string(path: impl AsRef<Path>) -> anyhow::Result<String> {
    let mut parts = Vec::new();
    for c in path.as_ref().components() {
        match c {
            Component::Normal(x) => {
                let c = x
                    .to_str()
                    .ok_or_else(|| anyhow!("invalid character in path component"))?;
                validate_path_component(c)?;
                parts.push(c.to_string());
            }
            other => {
                return Err(anyhow!(
                    "invalid path component {:?}: path must be a relative canonical path",
                    other
                ));
            }
        }
    }
    Ok(parts.join("/"))
}

/// Import a file or directory into the blob store as a collection.
///
/// The returned tag always references a collection:
/// - if `path` is a directory, the collection contains every file under it
///   (recursively), named by its `/`-joined relative path;
/// - if `path` is a single file, the collection contains one entry named
///   after the file.
///
/// Files are imported with [`ImportMode::TryReference`] so the store can use
/// copy-on-write / reflink when available.  Per-file temp tags are kept alive
/// until the collection itself is stored, then dropped — the collection
/// protects the children.
pub async fn import_collection(
    db: &Store,
    path: &Path,
    parallelism: usize,
) -> anyhow::Result<(TempTag, u64, Collection)> {
    let parallelism = parallelism.max(1);
    let path = path.canonicalize().context("canonicalize import path")?;
    ensure!(path.exists(), "path {} does not exist", path.display());
    // For a directory, names are relative to the directory itself (the root
    // folder name travels in the FileShare `name` field, not in the entry
    // names).  For a single file, names are relative to its parent so the
    // file keeps its own basename.
    let root = if path.is_dir() {
        path.as_path()
    } else {
        path.parent().context("import path has no parent directory")?
    };

    // Flatten the directory structure into a list of (name, path) pairs.
    // Symlinks are skipped; directories are handled by WalkDir.
    let files = walkdir::WalkDir::new(path.clone()).into_iter();
    let data_sources: Vec<(String, PathBuf)> = files
        .map(|entry| {
            let entry = entry?;
            if !entry.file_type().is_file() {
                // Skip symlinks and directories (WalkDir descends into the
                // latter on its own).
                return Ok(None);
            }
            let path = entry.into_path();
            let relative = path
                .strip_prefix(root)
                .context("file escaped import root")?;
            let name = canonicalized_path_to_string(relative)?;
            anyhow::Ok(Some((name, path)))
        })
        .filter_map(std::result::Result::transpose)
        .collect::<anyhow::Result<Vec<_>>>()?;

    // Import all the files in parallel, keeping (name, temp_tag, size) triples.
    let mut names_and_tags = n0_future::stream::iter(data_sources)
        .map(|(name, path)| {
            let db = db.clone();
            async move {
                let import = db.blobs().add_path_with_opts(AddPathOptions {
                    path,
                    mode: ImportMode::TryReference,
                    format: BlobFormat::Raw,
                });
                let mut stream = import.stream().await;
                let mut item_size = 0u64;
                let temp_tag = loop {
                    let item = stream
                        .next()
                        .await
                        .context("import stream ended without a tag")?;
                    match item {
                        AddProgressItem::Size(size) => {
                            item_size = size;
                        }
                        AddProgressItem::CopyProgress(_) | AddProgressItem::OutboardProgress(_) => {}
                        AddProgressItem::CopyDone => {}
                        AddProgressItem::Error(cause) => {
                            anyhow::bail!("error importing {}: {}", name, cause);
                        }
                        AddProgressItem::Done(tt) => break tt,
                    }
                };
                anyhow::Ok((name, temp_tag, item_size))
            }
        })
        .buffered_unordered(parallelism)
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .collect::<anyhow::Result<Vec<_>>>()?;

    names_and_tags.sort_by(|(a, _, _), (b, _, _)| a.cmp(b));
    let size = names_and_tags.iter().map(|(_, _, s)| *s).sum::<u64>();

    // Collect the (name, hash) tuples into a collection.  The per-file tags
    // must stay alive until the collection is stored, then they can be
    // dropped — the collection protects the children.
    let (collection, tags) = names_and_tags
        .into_iter()
        .map(|(name, tag, _)| ((name, tag.hash()), tag))
        .unzip::<_, _, Collection, Vec<_>>();
    let temp_tag = collection.clone().store(db).await?;
    drop(tags);

    Ok((temp_tag, size, collection))
}

/// Build the export target path for a collection entry name under `root`.
///
/// Splits `name` on `/` and validates every component with
/// [`validate_path_component`], then joins them onto `root`.  As a final
/// belt-and-suspenders check the resulting path is verified to still start
/// with `root`.
pub fn get_export_path(root: &Path, name: &str) -> anyhow::Result<PathBuf> {
    let mut path = root.to_path_buf();
    for part in name.split('/') {
        validate_path_component(part)?;
        path.push(part);
    }
    // Belt-and-suspenders: the joined path must stay inside `root`.
    let canonical_root = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());
    let canonical_path = path.canonicalize().unwrap_or_else(|_| path.clone());
    ensure!(
        canonical_path.starts_with(&canonical_root),
        "export path escapes root directory: {}",
        path.display()
    );
    Ok(path)
}

/// Export every entry of a collection under `root`, mirroring sendme's
/// per-file export loop.
///
/// Each entry is written with [`ExportMode::Copy`] (never modified in place).
/// If a target already exists the export stops with an error, exactly like
/// sendme — the caller can remove the conflicting path and retry without
/// re-downloading (blobs stay in the local store).
pub async fn export_collection(
    db: &Store,
    collection: &Collection,
    root: &Path,
) -> anyhow::Result<Vec<PathBuf>> {
    let mut exported = Vec::with_capacity(collection.len());
    for (name, hash) in collection.iter() {
        let target = get_export_path(root, name)?;
        ensure!(
            !target.exists(),
            "target {} already exists; remove it and retry (the download is not repeated)",
            target.display()
        );
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent).with_context(|| {
                format!("create parent directory {}", parent.display())
            })?;
        }
        let progress = db
            .blobs()
            .export_with_opts(ExportOptions {
                hash: *hash,
                target: target.clone(),
                mode: ExportMode::Copy,
            });
        let mut stream = progress.stream().await;
        while let Some(item) = stream.next().await {
            match item {
                ExportProgressItem::Size(_) | ExportProgressItem::CopyProgress(_) => {}
                ExportProgressItem::Done => break,
                ExportProgressItem::Error(cause) => {
                    anyhow::bail!("error exporting {}: {}", name, cause);
                }
            }
        }
        exported.push(target);
    }
    Ok(exported)
}

/// Download a whole-directory share (HashSeq collection) and expand it under
/// `target_root` as a folder tree named after `root_name`.
///
/// The collection root hash is downloaded through the store's existing
/// downloader (which fetches the HashSeq root and every child), then the
/// collection is loaded and each entry is exported with the same
/// traversal-safe [`get_export_path`] + exists-check used by
/// [`export_collection`].
///
/// Returns the path of the created root folder.
pub async fn download_collection_to_dir(
    blob_store: &Store,
    endpoint: &iroh::Endpoint,
    root_hash: iroh_blobs::Hash,
    candidates: Vec<iroh::PublicKey>,
    root_name: &str,
    target_root: &Path,
) -> anyhow::Result<PathBuf> {
    // Phase 1: download the collection root + all children into the local store.
    let downloader = blob_store.downloader(endpoint);
    let progress = downloader.download(
        iroh_blobs::HashAndFormat::hash_seq(root_hash),
        candidates,
    );
    let mut stream = progress.stream().await?;
    use iroh_blobs::api::downloader::DownloadProgressItem;
    loop {
        match stream.next().await {
            Some(DownloadProgressItem::Progress(_))
            | Some(DownloadProgressItem::TryProvider { .. })
            | Some(DownloadProgressItem::ProviderFailed { .. })
            | Some(DownloadProgressItem::PartComplete { .. }) => {}
            Some(DownloadProgressItem::Error(e)) => {
                anyhow::bail!("collection download failed: {e}");
            }
            Some(DownloadProgressItem::DownloadError) => {
                anyhow::bail!("collection download failed");
            }
            None => break,
        }
    }

    // Phase 2: load the collection and export each entry under the root dir.
    let collection = Collection::load(root_hash, blob_store).await?;
    let root_dir = target_root.join(root_name);
    if root_dir.exists() {
        anyhow::bail!(
            "target folder {} already exists; remove it and retry (the download is not repeated)",
            root_dir.display()
        );
    }
    std::fs::create_dir_all(&root_dir)
        .with_context(|| format!("create folder {}", root_dir.display()))?;
    export_collection(blob_store, &collection, &root_dir).await?;
    Ok(root_dir)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn test_collection_entry(root: &Path, rel: &str) -> anyhow::Result<PathBuf> {
        get_export_path(root, rel)
    }

    #[test]
    fn validate_path_component_rejects_separators_and_traversal() {
        for bad in ["/", "\\", "..", ".", "", "a/b", "a\\b"] {
            assert!(
                validate_path_component(bad).is_err(),
                "component {:?} should be rejected",
                bad
            );
        }
        for good in ["a", "report.pdf", "sub dir", "-", "_"] {
            assert!(
                validate_path_component(good).is_ok(),
                "component {:?} should be accepted",
                good
            );
        }
    }

    #[test]
    fn canonicalized_path_to_string_rejects_unsafe_paths() {
        assert!(canonicalized_path_to_string(Path::new("a/b/c")).is_ok());
        assert_eq!(
            canonicalized_path_to_string(Path::new("a/b/c")).unwrap(),
            "a/b/c"
        );
        // Absolute paths are rejected.
        assert!(canonicalized_path_to_string(Path::new("/etc/passwd")).is_err());
        // Traversal components are rejected.
        assert!(canonicalized_path_to_string(Path::new("../secret")).is_err());
        assert!(canonicalized_path_to_string(Path::new("./secret")).is_err());
        assert!(canonicalized_path_to_string(Path::new("a/../b")).is_err());
    }

    #[test]
    fn get_export_path_rejects_traversal_and_joins_components() {
        let tmp = TempDir::new().unwrap();
        let root = tmp.path().canonicalize().unwrap();

        for bad in ["../escape", "a/../../escape", "/absolute", "a//b"] {
            assert!(
                test_collection_entry(&root, bad).is_err(),
                "name {:?} should be rejected",
                bad
            );
        }

        let ok = test_collection_entry(&root, "sub/dir/file.txt").unwrap();
        assert!(ok.starts_with(&root));
        assert_eq!(ok, root.join("sub").join("dir").join("file.txt"));
    }

    #[tokio::test]
    async fn import_collection_roundtrip_single_file() -> anyhow::Result<()> {
        let tmp = TempDir::new()?;
        let file = tmp.path().join("hello.txt");
        std::fs::write(&file, b"hello collection")?;

        let store: iroh_blobs::api::Store =
            iroh_blobs::store::mem::MemStore::new().into();
        let (tag, size, collection) =
            import_collection(&store, &file, 4).await?;
        assert_eq!(size, b"hello collection".len() as u64);
        assert_eq!(collection.len(), 1);
        assert_eq!(collection.iter().next().unwrap().0, "hello.txt");
        assert!(tag.hash() != iroh_blobs::Hash::from_bytes([0u8; 32]));

        // Export back to a fresh directory.
        let out = tmp.path().join("out");
        std::fs::create_dir_all(&out)?;
        let exported = export_collection(&store, &collection, &out).await?;
        assert_eq!(exported.len(), 1);
        let written = std::fs::read(out.join("hello.txt"))?;
        assert_eq!(written, b"hello collection");
        Ok(())
    }

    #[tokio::test]
    async fn import_collection_roundtrip_directory_tree() -> anyhow::Result<()> {
        let tmp = TempDir::new()?;
        let src = tmp.path().join("src");
        std::fs::create_dir_all(src.join("sub/deep"))?;
        std::fs::write(src.join("root.txt"), b"root")?;
        std::fs::write(src.join("sub/child.txt"), b"child")?;
        std::fs::write(src.join("sub/deep/leaf.bin"), b"leaf")?;

        let store: iroh_blobs::api::Store =
            iroh_blobs::store::mem::MemStore::new().into();
        let (_tag, size, collection) = import_collection(&store, &src, 4).await?;
        assert_eq!(size, 4 + 5 + 4); // root + child + leaf
        let mut names: Vec<_> = collection.iter().map(|(n, _)| n.clone()).collect();
        names.sort();
        assert_eq!(
            names,
            vec!["root.txt", "sub/child.txt", "sub/deep/leaf.bin"]
        );

        let out = tmp.path().join("out");
        std::fs::create_dir_all(&out)?;
        let exported = export_collection(&store, &collection, &out).await?;
        assert_eq!(exported.len(), 3);
        assert_eq!(std::fs::read(out.join("sub/deep/leaf.bin"))?, b"leaf");
        assert_eq!(std::fs::read(out.join("sub/child.txt"))?, b"child");
        assert_eq!(std::fs::read(out.join("root.txt"))?, b"root");
        Ok(())
    }

    #[tokio::test]
    async fn export_collection_refuses_existing_target() -> anyhow::Result<()> {
        let tmp = TempDir::new()?;
        let file = tmp.path().join("file.txt");
        std::fs::write(&file, b"data")?;

        let store: iroh_blobs::api::Store =
            iroh_blobs::store::mem::MemStore::new().into();
        let (_tag, _size, collection) = import_collection(&store, &file, 4).await?;

        let out = tmp.path().join("out");
        std::fs::create_dir_all(&out)?;
        std::fs::write(out.join("file.txt"), b"existing")?;
        let result = export_collection(&store, &collection, &out).await;
        assert!(result.is_err(), "existing target should abort the export");
        Ok(())
    }
}
