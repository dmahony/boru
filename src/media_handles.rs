//! Process-local opaque media capabilities for desktop IPC.
//!
//! Public operations accept only handles and byte chunks; filesystem paths are
//! confined to a private temporary directory and never cross the API boundary.
use std::{collections::HashMap, fs::File, io::{Read, Write}, path::{Path, PathBuf}, sync::{Arc, Mutex, atomic::{AtomicBool, Ordering}}, time::{Duration, Instant}};

use rand::{distr::Alphanumeric, RngExt};
use std::fmt;

/// Upper bound for one media object (512 MiB).
pub const MAX_MEDIA_BYTES: u64 = 512 * 1024 * 1024;
/// Maximum bytes accepted in a single IPC chunk.
pub const MAX_CHUNK_BYTES: usize = 1024 * 1024;

/// Capability-scoped media operation failure.
#[derive(Debug, PartialEq, Eq)]
pub enum MediaHandleError {
    /// Identifier is unknown or has been removed.
    InvalidHandle,
    /// Capability exceeded its lifetime.
    Expired,
    /// Capability belongs to a different window/session.
    Unauthorized,
    /// Operation does not match upload/download direction.
    WrongType,
    /// MIME type is unsupported.
    InvalidContentType,
    /// Object exceeds the configured byte limit.
    TooLarge,
    /// Chunk offset or length is invalid.
    InvalidChunk,
    /// Transfer has been cancelled.
    Cancelled,
    /// Private temporary storage operation failed.
    Io,
}

impl fmt::Display for MediaHandleError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::InvalidHandle => "invalid media handle",
            Self::Expired => "media handle expired",
            Self::Unauthorized => "media handle is not authorized for this session",
            Self::WrongType => "wrong media handle type",
            Self::InvalidContentType => "invalid media content type",
            Self::TooLarge => "media size limit exceeded",
            Self::InvalidChunk => "invalid media chunk",
            Self::Cancelled => "media transfer cancelled",
            Self::Io => "media storage failed",
        })
    }
}
impl std::error::Error for MediaHandleError {}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Direction { Upload, Download }
#[derive(Debug)]
struct Entry {
    session: String,
    content_type: String,
    direction: Direction,
    path: PathBuf,
    size: u64,
    expires: Instant,
    cancelled: Arc<AtomicBool>,
}

/// Registry-owned temporary directory, removed when the registry is dropped.
#[derive(Debug)]
struct TempRoot {
    path: PathBuf,
}

impl TempRoot {
    fn create() -> Result<Self, MediaHandleError> {
        let mut rng = rand::rng();
        let suffix: String = (&mut rng).sample_iter(&Alphanumeric).take(16).map(char::from).collect();
        let path = std::env::temp_dir().join(format!("boru-media-{suffix}"));
        std::fs::create_dir(&path).map_err(|_| MediaHandleError::Io)?;
        Ok(Self { path })
    }

    fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for TempRoot {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.path);
    }
}

/// Thread-safe registry of per-session, expiring upload/download capabilities.
/// The directory is owned by the registry and removed when it is dropped.
#[derive(Debug)]
pub struct MediaHandleRegistry {
    root: TempRoot,
    entries: Mutex<HashMap<String, Entry>>,
    ttl: Duration,
    max_bytes: u64,
}

impl MediaHandleRegistry {
    /// Create an isolated registry with a bounded lifetime and object size.
    pub fn new(ttl: Duration, max_bytes: u64) -> Result<Self, MediaHandleError> {
        let max_bytes = max_bytes.min(MAX_MEDIA_BYTES);
        let root = TempRoot::create()?;
        Ok(Self { root, entries: Mutex::new(HashMap::new()), ttl, max_bytes })
    }

    /// Create a write-only upload capability scoped to `session`.
    pub fn begin_upload(&self, session: &str, content_type: &str) -> Result<String, MediaHandleError> {
        validate_session(session)?;
        validate_content_type(content_type)?;
        self.insert(session, content_type, Direction::Upload)
    }

    /// Create a read-only capability for the supplied bytes (e.g. a selected asset).
    pub fn register_download(&self, session: &str, content_type: &str, bytes: &[u8]) -> Result<String, MediaHandleError> {
        validate_session(session)?;
        validate_content_type(content_type)?;
        if bytes.len() as u64 > self.max_bytes { return Err(MediaHandleError::TooLarge); }
        let id = self.insert(session, content_type, Direction::Download)?;
        let mut entries = self.entries.lock().map_err(|_| MediaHandleError::Io)?;
        let entry = entries.get_mut(&id).ok_or(MediaHandleError::InvalidHandle)?;
        let mut file = File::create(&entry.path).map_err(|_| MediaHandleError::Io)?;
        file.write_all(bytes).map_err(|_| MediaHandleError::Io)?;
        entry.size = bytes.len() as u64;
        Ok(id)
    }

    /// Append one ordered chunk to an upload without buffering the full object.
    pub fn write_chunk(&self, session: &str, id: &str, offset: u64, chunk: &[u8]) -> Result<u64, MediaHandleError> {
        if chunk.len() > MAX_CHUNK_BYTES { return Err(MediaHandleError::InvalidChunk); }
        let mut entries = self.entries.lock().map_err(|_| MediaHandleError::Io)?;
        let e = authorized(&mut entries, session, id, Direction::Upload)?;
        if e.cancelled.load(Ordering::Acquire) { return Err(MediaHandleError::Cancelled); }
        if offset != e.size { return Err(MediaHandleError::InvalidChunk); }
        let new_size = e.size.checked_add(chunk.len() as u64).ok_or(MediaHandleError::TooLarge)?;
        if new_size > self.max_bytes { return Err(MediaHandleError::TooLarge); }
        let mut file = std::fs::OpenOptions::new().append(true).open(&e.path).map_err(|_| MediaHandleError::Io)?;
        file.write_all(chunk).map_err(|_| MediaHandleError::Io)?;
        e.size = new_size;
        Ok(new_size)
    }

    /// Read at most one chunk from an authorized download capability.
    pub fn read_chunk(&self, session: &str, id: &str, offset: u64, limit: usize) -> Result<Vec<u8>, MediaHandleError> {
        if limit == 0 || limit > MAX_CHUNK_BYTES { return Err(MediaHandleError::InvalidChunk); }
        let entries = self.entries.lock().map_err(|_| MediaHandleError::Io)?;
        let e = authorized_ref(&entries, session, id, Direction::Download)?;
        if e.cancelled.load(Ordering::Acquire) { return Err(MediaHandleError::Cancelled); }
        if offset > e.size { return Err(MediaHandleError::InvalidChunk); }
        let len = (e.size - offset).min(limit as u64) as usize;
        let mut file = File::open(&e.path).map_err(|_| MediaHandleError::Io)?;
        use std::io::{Seek, SeekFrom};
        file.seek(SeekFrom::Start(offset)).map_err(|_| MediaHandleError::Io)?;
        let mut data = vec![0; len];
        file.read_exact(&mut data).map_err(|_| MediaHandleError::Io)?;
        Ok(data)
    }

    /// Return the validated MIME type attached to an authorized capability.
    pub fn content_type(&self, session: &str, id: &str) -> Result<String, MediaHandleError> {
        let entries = self.entries.lock().map_err(|_| MediaHandleError::Io)?;
        let e = entries.get(id).ok_or(MediaHandleError::InvalidHandle)?;
        if e.expires <= Instant::now() { return Err(MediaHandleError::Expired); }
        if e.session != session { return Err(MediaHandleError::Unauthorized); }
        Ok(e.content_type.clone())
    }

    /// Close an upload capability and turn it into a read-only download capability.
    /// The same opaque handle is retained so callers can immediately decode the
    /// completed bytes without transferring a filesystem path through IPC.
    pub fn finish_upload(&self, session: &str, id: &str) -> Result<u64, MediaHandleError> {
        let mut entries = self.entries.lock().map_err(|_| MediaHandleError::Io)?;
        let entry = authorized(&mut entries, session, id, Direction::Upload)?;
        if entry.cancelled.load(Ordering::Acquire) { return Err(MediaHandleError::Cancelled); }
        entry.direction = Direction::Download;
        Ok(entry.size)
    }

    /// Return total object size after validating the capability and direction.
    pub fn size(&self, session: &str, id: &str, upload: bool) -> Result<u64, MediaHandleError> {
        let entries = self.entries.lock().map_err(|_| MediaHandleError::Io)?;
        Ok(authorized_ref(&entries, session, id, if upload { Direction::Upload } else { Direction::Download })?.size)
    }

    /// Cancel and remove a capability's private asset.
    pub fn cancel(&self, session: &str, id: &str) -> Result<(), MediaHandleError> {
        let mut entries = self.entries.lock().map_err(|_| MediaHandleError::Io)?;
        let e = authorized_ref(&entries, session, id, entries.get(id).map(|e| e.direction).ok_or(MediaHandleError::InvalidHandle)?)?;
        e.cancelled.store(true, Ordering::Release);
        let entry = entries.remove(id).ok_or(MediaHandleError::InvalidHandle)?;
        std::fs::remove_file(entry.path).map_err(|_| MediaHandleError::Io)
    }

    /// Remove expired capabilities and their private assets.
    pub fn cleanup_expired(&self) -> usize {
        let Ok(mut entries) = self.entries.lock() else { return 0 };
        let expired: Vec<_> = entries.iter().filter(|(_, e)| e.expires <= Instant::now()).map(|(id, _)| id.clone()).collect();
        for id in &expired { if let Some(e) = entries.remove(id) { let _ = std::fs::remove_file(e.path); } }
        expired.len()
    }

    fn insert(&self, session: &str, content_type: &str, direction: Direction) -> Result<String, MediaHandleError> {
        let mut rng = rand::rng();
        let id: String = (&mut rng).sample_iter(&Alphanumeric).take(48).map(char::from).collect();
        let path = self.root.path().join(&id);
        File::create(&path).map_err(|_| MediaHandleError::Io)?;
        self.entries.lock().map_err(|_| MediaHandleError::Io)?.insert(id.clone(), Entry { session: session.to_owned(), content_type: content_type.to_owned(), direction, path, size: 0, expires: Instant::now() + self.ttl, cancelled: Arc::new(AtomicBool::new(false)) });
        Ok(id)
    }
}

fn validate_session(session: &str) -> Result<(), MediaHandleError> {
    if session.is_empty() || session.len() > 128 || !session.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_') { return Err(MediaHandleError::Unauthorized); }
    Ok(())
}
fn validate_content_type(value: &str) -> Result<(), MediaHandleError> {
    let allowed = ["image/png", "image/jpeg", "image/gif", "image/webp", "video/mp4", "video/webm", "application/octet-stream"];
    if allowed.contains(&value) { Ok(()) } else { Err(MediaHandleError::InvalidContentType) }
}
fn authorized<'a>(entries: &'a mut HashMap<String, Entry>, session: &str, id: &str, direction: Direction) -> Result<&'a mut Entry, MediaHandleError> {
    if let Some(e) = entries.get(id) { if e.expires <= Instant::now() { return Err(MediaHandleError::Expired); } }
    let e = entries.get_mut(id).ok_or(MediaHandleError::InvalidHandle)?;
    if e.session != session { return Err(MediaHandleError::Unauthorized); }
    if e.direction != direction { return Err(MediaHandleError::WrongType); }
    Ok(e)
}
fn authorized_ref<'a>(entries: &'a HashMap<String, Entry>, session: &str, id: &str, direction: Direction) -> Result<&'a Entry, MediaHandleError> {
    let e = entries.get(id).ok_or(MediaHandleError::InvalidHandle)?;
    if e.expires <= Instant::now() { return Err(MediaHandleError::Expired); }
    if e.session != session { return Err(MediaHandleError::Unauthorized); }
    if e.direction != direction { return Err(MediaHandleError::WrongType); }
    Ok(e)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn scopes_types_caps_and_streams_chunks() {
        let r = MediaHandleRegistry::new(Duration::from_secs(60), 2_000_000).unwrap();
        let id = r.begin_upload("window_1", "image/png").unwrap();
        assert_eq!(r.write_chunk("window_1", &id, 0, b"abc").unwrap(), 3);
        assert_eq!(r.read_chunk("window_1", &id, 0, 1), Err(MediaHandleError::WrongType));
        assert_eq!(r.size("window_2", &id, true), Err(MediaHandleError::Unauthorized));
        assert_eq!(r.write_chunk("window_1", &id, 0, b"x"), Err(MediaHandleError::InvalidChunk));
        let down = r.register_download("window_1", "image/png", b"abcdef").unwrap();
        assert_eq!(r.read_chunk("window_1", &down, 2, 2).unwrap(), b"cd");
        assert_eq!(r.begin_upload("window_1", "text/html"), Err(MediaHandleError::InvalidContentType));
        assert_eq!(r.register_download("window_1", "image/png", &vec![0; 2_000_001]), Err(MediaHandleError::TooLarge));
        assert_eq!(r.finish_upload("window_1", &id).unwrap(), 3);
        assert_eq!(r.read_chunk("window_1", &id, 0, 3).unwrap(), b"abc");
        assert_eq!(r.write_chunk("window_1", &id, 3, b"d"), Err(MediaHandleError::WrongType));
        assert_eq!(r.finish_upload("window_1", &id), Err(MediaHandleError::WrongType));
    }
    #[test]
    fn streams_a_hundred_megabyte_object_in_bounded_chunks() {
        const TOTAL: u64 = 100 * 1024 * 1024 + 7;
        let registry = MediaHandleRegistry::new(Duration::from_secs(600), MAX_MEDIA_BYTES).unwrap();
        let id = registry.begin_upload("window_1", "video/mp4").unwrap();
        let chunk = vec![0xABu8; MAX_CHUNK_BYTES];

        let started = Instant::now();
        let mut written = 0u64;
        while written < TOTAL {
            let len = ((TOTAL - written).min(MAX_CHUNK_BYTES as u64)) as usize;
            written = registry
                .write_chunk("window_1", &id, written, &chunk[..len])
                .unwrap();
        }
        let elapsed = started.elapsed();
        assert_eq!(registry.size("window_1", &id, true).unwrap(), TOTAL);
        assert!(
            elapsed < Duration::from_secs(120),
            "100 MiB through bounded chunks took {elapsed:?}"
        );

        // The whole object round-trips through the private asset one bounded
        // chunk at a time, so peak memory stays constant as the object grows.
        let path = registry.entries.lock().unwrap().get(&id).unwrap().path.clone();
        let mut file = File::open(path).unwrap();
        let mut buf = vec![0u8; MAX_CHUNK_BYTES];
        let mut read_total = 0u64;
        loop {
            let read = file.read(&mut buf).unwrap();
            if read == 0 {
                break;
            }
            assert!(buf[..read].iter().all(|byte| *byte == 0xAB));
            read_total += read as u64;
        }
        assert_eq!(read_total, TOTAL);
    }

    #[test]
    fn expires_cancels_and_confines_files_to_private_temp_root() {
        let r = MediaHandleRegistry::new(Duration::from_millis(1), 1024).unwrap();
        let id = r.begin_upload("session", "application/octet-stream").unwrap();
        let path = r.entries.lock().unwrap().get(&id).unwrap().path.clone();
        assert!(path.starts_with(r.root.path()));
        std::thread::sleep(Duration::from_millis(3));
        assert_eq!(r.write_chunk("session", &id, 0, b"x"), Err(MediaHandleError::Expired));
        assert_eq!(r.cleanup_expired(), 1);
        let r = MediaHandleRegistry::new(Duration::from_secs(1), 1024).unwrap();
        let id = r.begin_upload("session", "application/octet-stream").unwrap();
        r.cancel("session", &id).unwrap();
        assert_eq!(r.write_chunk("session", &id, 0, b"x"), Err(MediaHandleError::InvalidHandle));
    }
}
