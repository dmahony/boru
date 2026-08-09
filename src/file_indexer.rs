//! Local index and filesystem monitor for the Boru Shared folder.
//!
//! The index contains metadata only. Files remain on disk and are never
//! uploaded by this module. Hashes are computed only when explicitly requested.

use std::{
    collections::{BTreeSet, HashMap},
    path::{Path, PathBuf},
    sync::{mpsc, Arc, RwLock},
    time::{Duration, UNIX_EPOCH},
};

use blake3::Hasher;
use n0_error::{Result, StdResultExt};
use notify::{
    event::{CreateKind, ModifyKind, RemoveKind},
    Config, Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher,
};
use tracing::warn;

use crate::file_hasher::FileHasher;
use crate::user_profile::SharedFile;

/// Default debounce window in milliseconds — events arriving within this
/// window are batched together so we process them as a single batch instead
/// of triggering a full rescan per event.
const DEBOUNCE_MS: u64 = 200;

/// Maximum number of unique changed paths before we fall back to a full
/// recursive rescan.  Beyond this threshold the per-path overhead of
/// incremental updates adds up and a full scan is cheaper.
const FULL_RESCAN_PATH_THRESHOLD: usize = 50;

/// File-name suffixes that indicate temporary or intermediate files.
/// Events touching these paths are discarded; the real file will emit
/// a follow-up event when the temp file is renamed to its final name.
const TEMP_SUFFIXES: &[&str] = &[
    ".tmp",
    ".temp",
    ".swp",
    ".swx",
    "~",
    ".part",
    ".crdownload",
    ".bak",
];

/// Filesystem changes reported by [`FileIndexer::watch`].
#[derive(Clone, Debug)]
pub enum FileChangeEvent {
    /// A file was added.
    Added(SharedFile),
    /// A file was removed.
    Removed {
        /// Stable metadata id of the removed file.
        id: String,
        /// Last known local path of the removed file.
        path: PathBuf,
    },
    /// A file's metadata changed.
    Modified(SharedFile),
}

#[derive(Debug)]
struct IndexState {
    files: HashMap<String, SharedFile>,
}

/// Indexes files below one local shared folder and monitors it for changes.
#[derive(Clone, Debug)]
pub struct FileIndexer {
    shared_folder: PathBuf,
    state: Arc<RwLock<IndexState>>,
    /// Bounded blocking hasher for content-hash computation.
    hasher: FileHasher,
}

impl FileIndexer {
    /// Create an indexer. The shared folder is created on the first scan or watch.
    ///
    /// Uses a default [`FileHasher`] with 4 concurrent hashing slots.
    pub fn new(shared_folder: impl Into<PathBuf>) -> Self {
        Self {
            shared_folder: shared_folder.into(),
            state: Arc::new(RwLock::new(IndexState {
                files: HashMap::new(),
            })),
            hasher: FileHasher::new(4),
        }
    }

    /// Create an indexer with a custom [`FileHasher`].
    pub fn with_hasher(shared_folder: impl Into<PathBuf>, hasher: FileHasher) -> Self {
        Self {
            shared_folder: shared_folder.into(),
            state: Arc::new(RwLock::new(IndexState {
                files: HashMap::new(),
            })),
            hasher,
        }
    }

    /// Return the folder this indexer is allowed to expose.
    pub fn shared_folder(&self) -> &Path {
        &self.shared_folder
    }

    /// Ensure the folder exists and replace the index with a recursive scan.
    pub fn scan(&self) -> Result<Vec<SharedFile>> {
        ensure_shared_folder(&self.shared_folder)?;
        let files = scan_folder(&self.shared_folder)?;
        let mut state = self.state.write().expect("file index lock poisoned");
        state.files = files
            .iter()
            .cloned()
            .map(|file| (file.id.clone(), file))
            .collect();
        Ok(files)
    }

    /// Same as [`scan`](Self::scan) but applies profile-based filtering flags
    /// (`over_limit`, `extension_blocked`) to each file.  Still indexes all
    /// files locally — callers decide which to announce via [`SharedFile::is_announceable`].
    pub fn scan_with_profile(
        &self,
        profile: &crate::user_profile::UserProfile,
    ) -> Result<Vec<SharedFile>> {
        ensure_shared_folder(&self.shared_folder)?;
        let files = scan_folder_with_profile(&self.shared_folder, profile)?;
        let mut state = self.state.write().expect("file index lock poisoned");
        state.files = files
            .iter()
            .cloned()
            .map(|file| (file.id.clone(), file))
            .collect();
        Ok(files)
    }

    /// Return a snapshot of all indexed files.
    pub fn list_shared_files(&self) -> Vec<SharedFile> {
        let state = self.state.read().expect("file index lock poisoned");
        let mut files: Vec<_> = state.files.values().cloned().collect();
        files.sort_by(|a, b| a.path.cmp(&b.path));
        files
    }

    /// Find an indexed file by its metadata id or by its computed content hash.
    pub fn get_shared_file(&self, hash: &str) -> Option<SharedFile> {
        let state = self.state.read().expect("file index lock poisoned");
        state
            .files
            .values()
            .find(|file| {
                file.id == hash
                    || file
                        .hash
                        .as_ref()
                        .is_some_and(|value| hex::encode(value) == hash)
            })
            .cloned()
    }

    /// Compute and cache a file's content hash. This is the only operation here
    /// that reads file contents; scanning and watching remain metadata-only.
    pub fn hash_for_transfer(&self, id: &str) -> Option<[u8; 32]> {
        let path = {
            let state = self.state.read().ok()?;
            state.files.get(id)?.path.clone()
        };
        let mut file = std::fs::File::open(&path).ok()?;
        let mut hasher = Hasher::new();
        std::io::copy(&mut file, &mut hasher).ok()?;
        let hash = *hasher.finalize().as_bytes();
        if let Ok(mut state) = self.state.write() {
            if let Some(entry) = state.files.get_mut(id) {
                entry.hash = Some(hash);
            }
        }
        Some(hash)
    }

    /// Compute and cache a file's content hash on a blocking worker thread.
    ///
    /// This is the async non-blocking equivalent of [`hash_for_transfer`].
    ///
    /// 1. Reads the file path under the index read lock, then releases it.
    /// 2. Delegates the actual file I/O + blake3 to a blocking worker
    ///    (via [`FileHasher`] which uses [`tokio::task::spawn_blocking`]).
    /// 3. Re-acquires the write lock and verifies the file's metadata
    ///    (mtime/size) hasn't changed before caching the hash.
    /// 4. If the file changed during hashing (`Ok(None)`), the result is
    ///    discarded — the caller should retry.
    ///
    /// # Returns
    ///
    /// * `Ok(Some(hash))` — hash computed and cached.
    /// * `Ok(None)` — file changed during read; caller should retry.
    /// * `Err(...)` — I/O or database error.
    pub async fn hash_for_transfer_async(&self, id: &str) -> Result<Option<[u8; 32]>> {
        // ── 1. Read lock: get path + metadata ────────────────────────
        let (path, expected_size, expected_mtime) = {
            let state = self.state.read().expect("file index lock poisoned");
            let entry = match state.files.get(id) {
                Some(e) => e,
                None => return Ok(None),
            };
            let mtime = match std::fs::metadata(&entry.path) {
                Ok(m) => m.modified().unwrap_or(std::time::UNIX_EPOCH),
                Err(_) => return Ok(None),
            };
            (entry.path.clone(), Some(entry.size), Some(mtime))
        };

        // ── 2. Hash on a blocking thread (lock released) ─────────────
        let result = self
            .hasher
            .hash_file(path.clone(), expected_size, expected_mtime)
            .await?;

        // ── 3. Re-acquire write lock to cache the hash ───────────────
        match result {
            Some(hash) => {
                let mut state = self.state.write().expect("file index lock poisoned");
                if let Some(entry) = state.files.get_mut(id) {
                    // Post-hash verification: check mtime again.
                    let current_mtime = match std::fs::metadata(&entry.path) {
                        Ok(m) => m.modified().unwrap_or(std::time::UNIX_EPOCH),
                        Err(_) => return Ok(None),
                    };
                    if expected_mtime.is_none_or(|exp| current_mtime == exp) {
                        entry.hash = Some(hash);
                        Ok(Some(hash))
                    } else {
                        // File changed during hashing — discard.
                        Ok(None)
                    }
                } else {
                    // Entry vanished from index.
                    Ok(None)
                }
            }
            None => {
                // File changed during hashing (detected by FileHasher).
                Ok(None)
            }
        }
    }

    /// Watch recursively and update this index as files change.
    ///
    /// Incoming `notify` events are batched inside a 200 ms debounce window
    /// so that bursts (e.g. a multi-file copy or a bulk save) are collapsed
    /// into a single update.  The update is performed *incrementally* using
    /// `event.paths` — only the affected files are re-statted, avoiding a
    /// full recursive scan.
    ///
    /// A full recursive scan is triggered when:
    ///
    /// *   The batch contains directory-level events (create/remove/rename)
    ///     that could change the tree structure.
    /// *   The number of unique changed paths exceeds
    ///     [`FULL_RESCAN_PATH_THRESHOLD`] (50).
    pub fn watch(&self) -> Result<mpsc::Receiver<FileChangeEvent>> {
        ensure_shared_folder(&self.shared_folder)?;
        let (notify_tx, notify_rx) = mpsc::channel();
        let (event_tx, event_rx) = mpsc::channel();
        let folder = self.shared_folder.clone();
        let indexer = self.clone();

        std::thread::Builder::new()
            .name("boru-shared-folder-watch".into())
            .spawn(move || {
                let callback = move |result: notify::Result<Event>| {
                    if let Ok(event) = result {
                        let _ = notify_tx.send(event);
                    }
                };
                let mut watcher = match RecommendedWatcher::new(callback, Config::default()) {
                    Ok(watcher) => watcher,
                    Err(error) => {
                        warn!("failed to create shared-folder watcher: {error}");
                        return;
                    }
                };
                if let Err(error) = watcher.watch(&folder, RecursiveMode::Recursive) {
                    warn!(
                        "failed to watch shared folder {}: {error}",
                        folder.display()
                    );
                    return;
                }

                // ── Debounced event loop ────────────────────────────────
                let mut collector = DebounceCollector::new(DEBOUNCE_MS);

                loop {
                    let batch = collector.collect(&notify_rx);
                    if batch.is_empty() {
                        // Sender disconnected — thread should exit.
                        break;
                    }

                    // Collapse the batch into unique, non-temp paths.
                    let unique_paths = collect_unique_paths(&batch);

                    if affects_directories(&batch)
                        || unique_paths.len() > FULL_RESCAN_PATH_THRESHOLD
                    {
                        // ── Full recursive rescan ───────────────────────
                        let previous = indexer.list_shared_files();
                        if indexer.scan().is_err() {
                            continue;
                        }
                        let current = indexer.list_shared_files();
                        // Use the last event's kind as a heuristic for the
                        // Added vs Modified classification.
                        if let Some(last) = batch.last() {
                            emit_changes(&previous, &current, last, &event_tx);
                        }
                    } else if !unique_paths.is_empty() {
                        // ── Incremental per-path update ──────────────────
                        process_event_batch(&indexer, &unique_paths, &event_tx);
                    }
                }
            })
            .with_std_context(|_| "failed to spawn shared-folder watcher")?;
        Ok(event_rx)
    }
}

fn emit_changes(
    previous: &[SharedFile],
    current: &[SharedFile],
    event: &Event,
    tx: &mpsc::Sender<FileChangeEvent>,
) {
    let old: HashMap<_, _> = previous
        .iter()
        .map(|file| (file.id.as_str(), file))
        .collect();
    let new: HashMap<_, _> = current
        .iter()
        .map(|file| (file.id.as_str(), file))
        .collect();
    for file in current {
        if !old.contains_key(file.id.as_str()) {
            let _ = tx.send(if matches!(event.kind, EventKind::Create(_)) {
                FileChangeEvent::Added(file.clone())
            } else {
                FileChangeEvent::Modified(file.clone())
            });
        }
    }
    for file in previous {
        if !new.contains_key(file.id.as_str()) {
            let _ = tx.send(FileChangeEvent::Removed {
                id: file.id.clone(),
                path: file.path.clone(),
            });
        }
    }
}

// ── Debounce collector ────────────────────────────────────────────────────

/// Collects raw `notify::Event` items into batches using a debounce window.
///
/// The first event blocks until at least one item arrives.  After that,
/// subsequent events are accumulated as long as they arrive within
/// `debounce_ms` of the previous one.  Once the window expires (a
/// `recv_timeout` times out), the batch is returned.
struct DebounceCollector {
    buf: Vec<Event>,
    debounce: Duration,
}

impl DebounceCollector {
    fn new(debounce_ms: u64) -> Self {
        Self {
            buf: Vec::new(),
            debounce: Duration::from_millis(debounce_ms),
        }
    }

    /// Block until at least one event is available, then collect arrival
    /// bursts for up to `debounce` milliseconds of silence.
    fn collect(&mut self, rx: &mpsc::Receiver<Event>) -> Vec<Event> {
        self.buf.clear();

        // Block for the first event.
        let first = match rx.recv() {
            Ok(e) => e,
            Err(mpsc::RecvError) => return std::mem::take(&mut self.buf),
        };
        self.buf.push(first);

        // Accumulate while events keep arriving within the debounce window.
        loop {
            match rx.recv_timeout(self.debounce) {
                Ok(event) => self.buf.push(event),
                Err(mpsc::RecvTimeoutError::Timeout) => break,
                Err(mpsc::RecvTimeoutError::Disconnected) => break,
            }
        }

        std::mem::take(&mut self.buf)
    }
}

// ── Event-path helpers ────────────────────────────────────────────────────

/// Returns `true` if the file name indicates a temporary or intermediate file
/// that should be ignored by the watcher.
fn is_temp_file(path: &Path) -> bool {
    let name = match path.file_name().and_then(|n| n.to_str()) {
        Some(n) => n,
        None => return false,
    };
    TEMP_SUFFIXES.iter().any(|suffix| name.ends_with(suffix))
}

/// Extract unique (non-temp) paths from a batch of events, ordered
/// deterministically by path.
fn collect_unique_paths(events: &[Event]) -> Vec<PathBuf> {
    let mut seen = BTreeSet::new();
    for e in events {
        for p in &e.paths {
            if !is_temp_file(p) {
                seen.insert(p.clone());
            }
        }
    }
    seen.into_iter().collect()
}

/// Returns `true` if *any* event in the batch relates to a directory or a
/// rename/move — these force a full recursive rescan because the tree
/// structure may have changed in ways that per-path incremental updates
/// cannot correctly express.
fn affects_directories(events: &[Event]) -> bool {
    events.iter().any(|e| {
        matches!(
            e.kind,
            EventKind::Create(CreateKind::Folder)
                | EventKind::Remove(RemoveKind::Folder)
                | EventKind::Modify(ModifyKind::Name(_))
        )
    })
}

// ── Incremental update ────────────────────────────────────────────────────

/// Process a batch of unique changed paths by updating only the affected
/// index entries instead of performing a full recursive scan.
///
/// For each path:
///
/// *   If the path still exists on disk and passes safety checks (symlink,
///     containment, hidden file), the index entry for that path is updated
///     (or created) and an `Added` / `Modified` event is emitted.
/// *   If the path no longer exists, the old entry is removed and a
///     `Removed` event is emitted.
/// *   Temp files, hidden files, and unsafe paths are silently skipped.
fn process_event_batch(
    indexer: &FileIndexer,
    paths: &[PathBuf],
    event_tx: &mpsc::Sender<FileChangeEvent>,
) {
    let root = indexer.shared_folder();
    for p in paths {
        // Every path from notify should be inside the watched folder, but
        // defend against edge cases.
        if !p.starts_with(root) {
            continue;
        }

        match std::fs::metadata(p) {
            Ok(meta) if meta.is_file() => {
                // ── New or modified file ────────────────────────────────
                let name = match p.file_name().and_then(|n| n.to_str()) {
                    Some(name) if !name.starts_with('.') => name,
                    _ => continue,
                };

                // Safety: symlink must not escape the shared folder.
                if !crate::path_containment::symlink_is_safe(p, root) {
                    warn!("skipping changed path outside root: {}", p.display());
                    continue;
                }
                if !crate::path_containment::is_path_contained(p, root) {
                    warn!(
                        "skipping changed path that resolves outside shared folder: {}",
                        p.display()
                    );
                    continue;
                }

                let modified_time = meta.modified().unwrap_or(UNIX_EPOCH);
                let mut new_file = SharedFile::new(name, meta.len(), mime_type(p), modified_time);
                new_file.path = p.clone();

                let mut state = indexer.state.write().expect("file index lock poisoned");

                // Remove old entry for this path (the id will differ if
                // size or mtime changed — modify-in-place).
                let was_modified = state.files.iter().any(|(_, f)| f.path == *p);

                state.files.retain(|_, f| f.path != *p);
                state.files.insert(new_file.id.clone(), new_file.clone());

                let event = if was_modified {
                    FileChangeEvent::Modified(new_file)
                } else {
                    FileChangeEvent::Added(new_file)
                };
                let _ = event_tx.send(event);
            }
            Ok(_meta) => {
                // Directory — ignore; directory-level events trigger a full
                // rescan in the caller.
            }
            Err(_) => {
                // ── Removed file ────────────────────────────────────────
                let mut state = indexer.state.write().expect("file index lock poisoned");
                let removed = state
                    .files
                    .iter()
                    .find(|(_, f)| f.path == *p)
                    .map(|(id, f)| (id.clone(), f.path.clone()));

                if let Some((old_id, old_path)) = removed {
                    state.files.remove(&old_id);
                    let _ = event_tx.send(FileChangeEvent::Removed {
                        id: old_id,
                        path: old_path,
                    });
                }
            }
        }
    }
}

fn scan_folder(folder: &Path) -> Result<Vec<SharedFile>> {
    let mut files = Vec::new();
    scan_dir(folder, folder, &mut files)?;
    Ok(files)
}

fn scan_dir(root: &Path, directory: &Path, files: &mut Vec<SharedFile>) -> Result<()> {
    for entry in std::fs::read_dir(directory)
        .with_std_context(|_| format!("failed to read shared folder {}", directory.display()))?
    {
        let entry = match entry {
            Ok(entry) => entry,
            Err(error) => {
                warn!("failed to read shared-folder entry: {error}");
                continue;
            }
        };
        let path = entry.path();
        let name = match path.file_name().and_then(|value| value.to_str()) {
            Some(name) if !name.starts_with('.') => name,
            _ => continue,
        };
        if !crate::path_containment::symlink_is_safe(&path, root) {
            warn!(
                "skipping shared-folder path outside root: {}",
                path.display()
            );
            continue;
        }
        let metadata = match std::fs::metadata(&path) {
            Ok(metadata) => metadata,
            Err(error) => {
                warn!(
                    "failed to stat shared-folder path {}: {error}",
                    path.display()
                );
                continue;
            }
        };
        if metadata.is_dir() {
            scan_dir(root, &path, files)?;
        } else if metadata.is_file() {
            let modified_time = metadata.modified().unwrap_or(UNIX_EPOCH);
            let mut file = SharedFile::new(name, metadata.len(), mime_type(&path), modified_time);
            file.path = path;
            files.push(file);
        }
    }
    Ok(())
}

fn scan_folder_with_profile(
    folder: &Path,
    profile: &crate::user_profile::UserProfile,
) -> Result<Vec<SharedFile>> {
    let mut files = Vec::new();
    scan_dir_with_profile_checks(folder, folder, &mut files, profile)?;
    Ok(files)
}

fn scan_dir_with_profile_checks(
    root: &Path,
    directory: &Path,
    files: &mut Vec<SharedFile>,
    profile: &crate::user_profile::UserProfile,
) -> Result<()> {
    for entry in std::fs::read_dir(directory)
        .with_std_context(|_| format!("failed to read shared folder {}", directory.display()))?
    {
        let entry = match entry {
            Ok(entry) => entry,
            Err(error) => {
                warn!("failed to read shared-folder entry: {error}");
                continue;
            }
        };
        let path = entry.path();
        let name = match path.file_name().and_then(|value| value.to_str()) {
            Some(name) if !name.starts_with('.') => name,
            _ => continue,
        };
        // Security: symlink must not escape the shared folder.
        if !crate::path_containment::symlink_is_safe(&path, root) {
            warn!(
                "skipping shared-folder path outside root: {}",
                path.display()
            );
            continue;
        }
        // Security: path must resolve inside the shared folder.
        if !crate::path_containment::is_path_contained(&path, root) {
            warn!(
                "skipping path that resolves outside shared folder: {}",
                path.display()
            );
            continue;
        }
        let metadata = match std::fs::metadata(&path) {
            Ok(metadata) => metadata,
            Err(error) => {
                warn!(
                    "failed to stat shared-folder path {}: {error}",
                    path.display()
                );
                continue;
            }
        };
        if metadata.is_dir() {
            scan_dir_with_profile_checks(root, &path, files, profile)?;
        } else if metadata.is_file() {
            let modified_time = metadata.modified().unwrap_or(std::time::UNIX_EPOCH);
            let ext = path
                .extension()
                .and_then(|e| e.to_str())
                .unwrap_or_default()
                .to_lowercase();

            // Single canonical size + extension admission policy
            // (BORU-AUDIT-20).  The indexer used to re-implement this rule
            // inline; it now delegates to file_policy::admission.
            let admission = crate::file_policy::admission(
                metadata.len(),
                &ext,
                profile.max_file_size,
                &profile.allowed_extensions,
            );

            let mut file = SharedFile::new(name, metadata.len(), mime_type(&path), modified_time);
            file.path = path;
            file.over_limit = admission.over_limit;
            file.extension_blocked = admission.extension_blocked;
            files.push(file);
        }
    }
    Ok(())
}

/// Create a shared folder if it does not exist.
pub fn ensure_shared_folder(path: &Path) -> Result<()> {
    std::fs::create_dir_all(path)
        .with_std_context(|_| format!("failed to create shared folder {}", path.display()))?;
    Ok(())
}

/// Default local shared-folder location.
pub fn default_shared_folder_path() -> PathBuf {
    crate::data_dir::shared_folder_path(None)
}

fn mime_type(path: &Path) -> String {
    match path
        .extension()
        .and_then(|ext| ext.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase()
        .as_str()
    {
        "txt" => "text/plain",
        "md" => "text/markdown",
        "json" => "application/json",
        "pdf" => "application/pdf",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        _ => "application/octet-stream",
    }
    .to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{fs, time::Duration};
    use tempfile::TempDir;

    #[test]
    fn recursively_indexes_only_files_inside_shared_folder() {
        let dir = TempDir::new().unwrap();
        fs::create_dir(dir.path().join("nested")).unwrap();
        fs::write(dir.path().join("nested/file.txt"), b"hello").unwrap();
        fs::write(dir.path().join(".hidden"), b"no").unwrap();
        let indexer = FileIndexer::new(dir.path());
        let files = indexer.scan().unwrap();
        assert_eq!(files.len(), 1);
        assert_eq!(files[0].filename, "file.txt");
        assert_eq!(
            indexer.get_shared_file(&files[0].id).unwrap().path,
            files[0].path
        );
    }

    #[test]
    fn hash_is_lazy_and_queryable_after_explicit_hashing() {
        let dir = TempDir::new().unwrap();
        fs::write(dir.path().join("file.txt"), b"hello").unwrap();
        let indexer = FileIndexer::new(dir.path());
        let file = indexer.scan().unwrap().pop().unwrap();
        assert!(file.hash.is_none());
        let hash = indexer.hash_for_transfer(&file.id).unwrap();
        assert_eq!(
            indexer.get_shared_file(&hex::encode(hash)).unwrap().hash,
            Some(hash)
        );
    }

    #[test]
    fn watcher_updates_index_for_new_file() {
        let dir = TempDir::new().unwrap();
        let indexer = FileIndexer::new(dir.path());
        let rx = indexer.watch().unwrap();
        // Give the platform watcher thread time to install the inotify watch;
        // otherwise the test itself can race the watcher startup.
        std::thread::sleep(Duration::from_millis(200));
        fs::write(dir.path().join("new.txt"), b"new").unwrap();
        let event = rx.recv_timeout(Duration::from_secs(5)).unwrap();
        assert!(matches!(
            event,
            FileChangeEvent::Added(_) | FileChangeEvent::Modified(_)
        ));
        assert_eq!(indexer.list_shared_files().len(), 1);
    }

    #[test]
    fn debounce_merges_multiple_writes_into_single_update() {
        let dir = TempDir::new().unwrap();
        let indexer = FileIndexer::new(dir.path());
        let rx = indexer.watch().unwrap();
        std::thread::sleep(Duration::from_millis(200));

        // Write several files rapidly (within the debounce window).
        for i in 0..5 {
            fs::write(
                dir.path().join(format!("file_{i}.txt")),
                format!("content {i}"),
            )
            .unwrap();
        }

        // Wait for the debounced batch to arrive — we should get all
        // changes from a single round of processing.
        let mut received = Vec::new();
        while received.len() < 5 {
            match rx.recv_timeout(Duration::from_secs(4)) {
                Ok(event) => received.push(event),
                Err(_) => break,
            }
        }

        assert_eq!(
            indexer.list_shared_files().len(),
            5,
            "all 5 files should be indexed after debounced batch"
        );
        // All 5 Add events should have been emitted (may arrive in any order).
        let adds: Vec<_> = received
            .iter()
            .filter(|e| matches!(e, FileChangeEvent::Added(_)))
            .collect();
        assert_eq!(adds.len(), 5, "expected 5 Added events, got {}", adds.len());
    }

    #[test]
    fn incremental_update_modifies_existing_file() {
        let dir = TempDir::new().unwrap();
        let indexer = FileIndexer::new(dir.path());
        let rx = indexer.watch().unwrap();
        std::thread::sleep(Duration::from_millis(200));

        // Create a file and wait for it to be indexed.
        let path = dir.path().join("modify.txt");
        fs::write(&path, b"original").unwrap();
        let event1 = rx.recv_timeout(Duration::from_secs(5)).unwrap();
        assert!(matches!(event1, FileChangeEvent::Added(_)));

        let file_id = indexer.list_shared_files()[0].id.clone();

        // Modify the file content and wait for the incremental update.
        std::thread::sleep(Duration::from_millis(10)); // ensure distinct mtime
        fs::write(&path, b"modified content").unwrap();
        let event2 = rx.recv_timeout(Duration::from_secs(5)).unwrap();

        // The incremental path should detect the old entry existed at this
        // path and emit Modified, not Added.
        assert!(
            matches!(event2, FileChangeEvent::Modified(_)),
            "incremental update should produce Modified, got {event2:?}"
        );

        // The id should have changed because size/mtime changed.
        let files = indexer.list_shared_files();
        assert_eq!(files.len(), 1);
        assert_ne!(files[0].id, file_id, "id should change after modify");
    }

    #[test]
    fn temp_file_event_is_filtered_out() {
        let dir = TempDir::new().unwrap();
        let indexer = FileIndexer::new(dir.path());
        let rx = indexer.watch().unwrap();
        std::thread::sleep(Duration::from_millis(200));

        // Write a temp file — should be filtered by is_temp_file.
        fs::write(dir.path().join("output.tmp"), b"temp data").unwrap();
        // Write a .swp file — also filtered.
        fs::write(dir.path().join(".file.txt.swp"), b"vim swap").unwrap();

        // Wait briefly — no Added event should arrive for temp files.
        std::thread::sleep(Duration::from_millis(400));

        let files = indexer.list_shared_files();
        assert!(
            files.is_empty(),
            "temp files should not appear in the index: {files:?}"
        );

        // The receiver should not have any events (or at most an event
        // for a non-temp path if any passed through).  Drain what's
        // available.
        let drained: Vec<_> = rx.try_iter().collect();
        for e in &drained {
            assert!(
                !matches!(e, FileChangeEvent::Added(_)),
                "no Added event expected for temp files: {e:?}"
            );
        }
    }

    #[test]
    fn watcher_ignores_hidden_files() {
        let dir = TempDir::new().unwrap();
        let indexer = FileIndexer::new(dir.path());
        let rx = indexer.watch().unwrap();
        std::thread::sleep(Duration::from_millis(200));

        fs::write(dir.path().join(".hidden_config"), b"secret").unwrap();
        std::thread::sleep(Duration::from_millis(400));

        let files = indexer.list_shared_files();
        assert!(
            files.is_empty(),
            "hidden files should not appear in the index: {files:?}"
        );
        let drained: Vec<_> = rx.try_iter().collect();
        for e in &drained {
            assert!(
                !matches!(e, FileChangeEvent::Added(_)),
                "no Added event for hidden file: {e:?}"
            );
        }
    }

    #[test]
    fn full_rescan_on_directory_create_still_indexes_nested_files() {
        let dir = TempDir::new().unwrap();
        let indexer = FileIndexer::new(dir.path());
        let rx = indexer.watch().unwrap();
        std::thread::sleep(Duration::from_millis(200));

        // Create a nested directory structure.
        fs::create_dir_all(dir.path().join("sub/deep")).unwrap();
        fs::write(dir.path().join("sub/alpha.txt"), b"alpha").unwrap();
        fs::write(dir.path().join("sub/deep/beta.txt"), b"beta").unwrap();

        // Collect events — the directory Create should force a full rescan.
        let mut received = Vec::new();
        while received.len() < 2 {
            match rx.recv_timeout(Duration::from_secs(4)) {
                Ok(event) => {
                    received.push(event);
                }
                Err(_) => break,
            }
        }

        assert_eq!(
            indexer.list_shared_files().len(),
            2,
            "both nested files should be indexed after rescan"
        );
        // At least one Removed event may or may not appear depending on
        // timing; the key assertion is the index count above.
        let adds: Vec<_> = received
            .iter()
            .filter(|e| matches!(e, FileChangeEvent::Added(_)))
            .collect();
        assert_eq!(adds.len(), 2, "expected 2 Added events, got {}", adds.len());
    }

    // ── Unit tests for helper functions ────────────────────────────────────

    #[test]
    fn is_temp_file_detects_known_suffixes() {
        assert!(is_temp_file(Path::new("foo.tmp")));
        assert!(is_temp_file(Path::new("foo.temp")));
        assert!(is_temp_file(Path::new(".file.swp")));
        assert!(is_temp_file(Path::new("foo.swx")));
        assert!(is_temp_file(Path::new("foo~")));
        assert!(is_temp_file(Path::new("download.part")));
        assert!(is_temp_file(Path::new("dl.crdownload")));
        assert!(is_temp_file(Path::new("backup.bak")));

        assert!(!is_temp_file(Path::new("foo.txt")));
        assert!(!is_temp_file(Path::new("foo")));
        assert!(!is_temp_file(Path::new("")));
    }

    #[test]
    fn collect_unique_paths_deduplicates_and_skips_temp() {
        use notify::event::CreateKind;

        let make_event = |paths: Vec<&str>, kind: EventKind| Event {
            kind,
            paths: paths.into_iter().map(PathBuf::from).collect(),
            ..Event::default()
        };

        let events = vec![
            make_event(
                vec!["a.txt", "b.txt", "c.tmp"],
                EventKind::Create(CreateKind::File),
            ),
            make_event(vec!["a.txt", "b.txt"], EventKind::Create(CreateKind::File)),
        ];

        let result = collect_unique_paths(&events);
        assert_eq!(result.len(), 2, "a.txt + b.txt, c.tmp filtered");
        assert!(result.iter().any(|p| p.ends_with("a.txt")));
        assert!(result.iter().any(|p| p.ends_with("b.txt")));
    }

    #[test]
    fn affects_directories_detects_folder_events() {
        use notify::event::CreateKind;

        // A file create should NOT trigger full rescan.
        let file_event = Event {
            kind: EventKind::Create(CreateKind::File),
            paths: vec![PathBuf::from("f.txt")],
            ..Event::default()
        };
        assert!(!affects_directories(&[file_event]));

        // A folder create SHOULD trigger full rescan.
        let folder_event = Event {
            kind: EventKind::Create(CreateKind::Folder),
            paths: vec![PathBuf::from("sub")],
            ..Event::default()
        };
        assert!(affects_directories(&[folder_event]));

        // A rename SHOULD trigger full rescan.
        let rename_event = Event {
            kind: EventKind::Modify(ModifyKind::Name(notify::event::RenameMode::Any)),
            paths: vec![PathBuf::from("old"), PathBuf::from("new")],
            ..Event::default()
        };
        assert!(affects_directories(&[rename_event]));
    }

    /// The indexer's size/extension flags must come from the single canonical
    /// [`crate::file_policy::admission`] rule (BORU-AUDIT-20).  If someone
    /// re-introduces an inline copy that disagrees with the canonical rule,
    /// this test fails.
    #[test]
    fn profile_scan_flags_match_canonical_file_policy() {
        use crate::user_profile::UserProfile;

        let dir = TempDir::new().unwrap();
        fs::write(dir.path().join("ok.txt"), b"small").unwrap();
        fs::write(dir.path().join("big.bin"), vec![0u8; 4096]).unwrap();
        fs::write(dir.path().join("blocked.exe"), b"x").unwrap();
        fs::write(dir.path().join("mixed.JPG"), vec![0u8; 4096]).unwrap();

        let mut profile = UserProfile::new(
            iroh::PublicKey::from_bytes(&[1u8; 32])
                .expect("32 one-bytes is a valid ed25519 public key"),
        );
        profile.file_sharing_enabled = true;
        profile.max_file_size = 1024;
        profile.allowed_extensions = vec!["txt".into(), "jpg".into()];

        let files = scan_folder_with_profile(dir.path(), &profile).unwrap();
        assert_eq!(files.len(), 4);

        for file in &files {
            let ext = file
                .path
                .extension()
                .and_then(|e| e.to_str())
                .unwrap_or_default()
                .to_lowercase();
            let canonical = crate::file_policy::admission(
                file.size,
                &ext,
                profile.max_file_size,
                &profile.allowed_extensions,
            );
            assert_eq!(
                file.over_limit, canonical.over_limit,
                "over_limit mismatch for {}",
                file.filename
            );
            assert_eq!(
                file.extension_blocked, canonical.extension_blocked,
                "extension_blocked mismatch for {}",
                file.filename
            );
        }

        // Spot-check each file:
        // - ok.txt: in limits, allowed extension → announceable.
        // - big.bin: over limit → not announceable.
        // - blocked.exe: blocked extension → not announceable.
        // - mixed.JPG: over limit AND extension allowed case-insensitively.
        let by_name = |name: &str| files.iter().find(|f| f.filename == name).unwrap();
        assert!(by_name("ok.txt").is_announceable());
        assert!(!by_name("big.bin").is_announceable());
        assert!(!by_name("blocked.exe").is_announceable());
        assert!(!by_name("mixed.JPG").is_announceable());
        assert!(by_name("mixed.JPG").over_limit);
        assert!(!by_name("mixed.JPG").extension_blocked);
    }
}
