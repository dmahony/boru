//! Local index and filesystem monitor for the Boru Shared folder.
//!
//! The index contains metadata only. Files remain on disk and are never
//! uploaded by this module. Hashes are computed only when explicitly requested.

use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::{mpsc, Arc, RwLock},
    time::UNIX_EPOCH,
};

use blake3::Hasher;
use n0_error::{Result, StdResultExt};
use notify::{Config, Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use tracing::warn;

use crate::user_profile::SharedFile;

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
}

impl FileIndexer {
    /// Create an indexer. The shared folder is created on the first scan or watch.
    pub fn new(shared_folder: impl Into<PathBuf>) -> Self {
        Self {
            shared_folder: shared_folder.into(),
            state: Arc::new(RwLock::new(IndexState {
                files: HashMap::new(),
            })),
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

    /// Watch recursively and update this index as files change.
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
                for event in notify_rx {
                    if matches!(
                        event.kind,
                        EventKind::Create(_) | EventKind::Modify(_) | EventKind::Remove(_)
                    ) {
                        let previous = indexer.list_shared_files();
                        if indexer.scan().is_err() {
                            continue;
                        }
                        let current = indexer.list_shared_files();
                        emit_changes(&previous, &current, &event, &event_tx);
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
        if !crate::user_profile::UserProfile::symlink_is_safe(&path, root) {
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
        if !crate::user_profile::UserProfile::symlink_is_safe(&path, root) {
            warn!(
                "skipping shared-folder path outside root: {}",
                path.display()
            );
            continue;
        }
        // Security: path must resolve inside the shared folder.
        if !crate::user_profile::UserProfile::is_path_contained(&path, root) {
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

            let over_limit = metadata.len() > profile.max_file_size;
            let extension_blocked = if !profile.allowed_extensions.is_empty() {
                !profile
                    .allowed_extensions
                    .iter()
                    .any(|a| a.eq_ignore_ascii_case(&ext))
            } else {
                false
            };

            let mut file = SharedFile::new(name, metadata.len(), mime_type(&path), modified_time);
            file.path = path;
            file.over_limit = over_limit;
            file.extension_blocked = extension_blocked;
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
        std::thread::sleep(Duration::from_millis(100));
        fs::write(dir.path().join("new.txt"), b"new").unwrap();
        let event = rx.recv_timeout(Duration::from_secs(3)).unwrap();
        assert!(matches!(
            event,
            FileChangeEvent::Added(_) | FileChangeEvent::Modified(_)
        ));
        assert_eq!(indexer.list_shared_files().len(), 1);
    }
}
