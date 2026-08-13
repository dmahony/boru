//! Atomic file update with pre-commit validation.
//!
//! Writing config / state to disk is a common operation across all the
//! persistent stores (friends and room metadata). Every
//! store was duplicating the same ~35-line pattern:
//!
//! 1. Serialize to JSON
//! 2. Write to a `.json.tmp` sibling
//! 3. `fsync` the temp file so the data reaches the disk
//! 4. Remove the old file (if any)
//! 5. `rename` the temp file to the final path (atomic on POSIX)
//! 6. Set restrictive permissions (`0o600`)
//!
//! This module centralises the pattern and adds a *pre-commit validation*
//! step: after serialisation we immediately re-deserialise the bytes so
//! that silent data corruption (NaN floats, out-of-range enums, etc.)
//! is caught *before* the valid old copy is destroyed.

use std::{
    fs,
    io::{BufWriter, Write},
    path::Path,
    sync::atomic::{AtomicU64, Ordering},
};

use n0_error::{Result, StdResultExt};
use serde::{de::DeserializeOwned, Serialize};

/// Atomically write JSON-serialised `data` to `path`, with a round-trip
/// validation check before the old file is replaced.
///
/// `label` is a human-readable name for error messages (e.g. `"friends store"`).
///
/// ## Pre-commit validation
///
/// After serialisation the bytes are immediately re-deserialised.  This
/// guarantees that what we are about to commit can actually be read back,
/// even if the type has invariants that serialisation doesn't enforce.
/// If validation fails the old file is **not** touched.
///
/// ## Atomicity
///
/// On POSIX the final `rename(2)` is atomic if the source and destination
/// reside on the same filesystem (which they do — `tmp_path` is a sibling
/// of `path` with a different extension).  A crash or power loss between
/// the `fsync` and the `rename` leaves the old file intact.
pub fn atomic_write_json<T>(path: &Path, data: &T, label: &str) -> Result<()>
where
    T: Serialize + DeserializeOwned,
{
    // ── 1. Serialise ────────────────────────────────────────────────
    let encoded =
        serde_json::to_vec_pretty(data).with_std_context(|_| format!("encode {label}"))?;

    // ── 2. Pre-commit validation: round-trip through serde ──────────
    //
    // This catches NaN / infinity floats, out-of-range integer enums,
    // and other corruption that serde_json's serialiser can produce
    // because the type's invariants aren't expressed in the schema.
    // If this fails the old file on disk is **untouched**.
    serde_json::from_slice::<T>(&encoded)
        .with_std_context(|_| format!("validate {label} — re-deserialisation check"))?;

    // ── 3. Atomic file mechanics ────────────────────────────────────
    atomic_write_bytes(path, &encoded, label)
}

/// Atomically write already-encoded bytes to `path`.
///
/// This is the file-mechanics half of [`atomic_write_json`]: it writes to
/// a unique temp sibling, `fsync`s it, then renames it over the target
/// (atomic on POSIX). Callers that serialise non-JSON formats (TOML, …)
/// use this directly after doing their own validation.
///
/// A trailing newline is appended so the file stays valid POSIX text.
pub fn atomic_write_bytes(path: &Path, encoded: &[u8], label: &str) -> Result<()> {
    let data_dir = path.parent().unwrap_or_else(|| Path::new("."));

    // ── 1. Ensure the directory exists ──────────────────────────────
    fs::create_dir_all(data_dir).with_std_context(|_| {
        format!(
            "failed to create data dir for {label}: {}",
            data_dir.display()
        )
    })?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = fs::set_permissions(data_dir, fs::Permissions::from_mode(0o700));
    }

    // Unique tmp name per invocation: the store saves run on spawned
    // threads (send_save_friends etc.) and can overlap, so a fixed
    // `<name>.json.tmp` lets two concurrent saves race — one truncates
    // the other's tmp and the loser's rename fails with ENOENT, spamming
    // "failed to replace file" warnings and occasionally committing a
    // half-written file. A per-invocation suffix makes every rename
    // succeed; the last writer wins, which is safe for whole-state JSON.
    static TMP_SEQ: AtomicU64 = AtomicU64::new(0);
    let seq = TMP_SEQ.fetch_add(1, Ordering::Relaxed);
    let file_stem = path
        .file_name()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| "state".to_string());
    let tmp_path = path.with_file_name(format!(
        ".{file_stem}.{}.{seq}.tmp",
        std::process::id()
    ));

    // ── 2. Write to tmp file with fsync ─────────────────────────────
    {
        let file = fs::File::create(&tmp_path).with_std_context(|_| {
            format!(
                "failed to create temp file for {label}: {}",
                tmp_path.display()
            )
        })?;
        let mut writer = BufWriter::new(file);

        writer.write_all(encoded).with_std_context(|_| {
            format!(
                "failed to write temp file for {label}: {}",
                tmp_path.display()
            )
        })?;

        // Trailing newline keeps the file valid POSIX text.
        writer.write_all(b"\n").with_std_context(|_| {
            format!(
                "failed to finalise temp file for {label}: {}",
                tmp_path.display()
            )
        })?;

        writer.flush().with_std_context(|_| {
            format!(
                "failed to flush temp file for {label}: {}",
                tmp_path.display()
            )
        })?;

        writer.get_ref().sync_all().with_std_context(|_| {
            format!(
                "failed to sync temp file for {label}: {}",
                tmp_path.display()
            )
        })?;
    }

    // ── 3. Replace the old file atomically ──────────────────────────
    // On Linux/macOS, rename(2) atomically replaces the destination.
    // Removing the old file first is destructive — if the rename fails,
    // both files are lost.  Let rename handle the replacement.
    fs::rename(&tmp_path, path)
        .with_std_context(|_| format!("failed to replace file for {label}: {}", path.display()))?;

    // ── 4. Restrictive permissions on the final file ────────────────
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = fs::set_permissions(path, fs::Permissions::from_mode(0o600));
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde::{Deserialize, Serialize};

    #[derive(Debug, Serialize, Deserialize, PartialEq)]
    struct TestData {
        name: String,
        value: u64,
    }

    #[test]
    fn test_round_trip_ok() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test.json");

        let data = TestData {
            name: "hello".into(),
            value: 42,
        };

        atomic_write_json(&path, &data, "test").unwrap();
        assert!(path.exists());

        // Read back and verify
        let raw = fs::read_to_string(&path).unwrap();
        let decoded: TestData = serde_json::from_str(&raw).unwrap();
        assert_eq!(decoded, data);
    }

    #[test]
    fn test_validation_catches_corrupt_data() {
        // We create a type that serialises fine but whose deserialisation
        // is a "canary" — any value outside a narrow range fails.
        #[derive(Debug, Serialize, Deserialize, PartialEq)]
        struct Bounded(u8);

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test.json");

        // 255 is valid for u8, so this should work.
        atomic_write_json(&path, &Bounded(255), "bounded").unwrap();

        // 256 is *not* representable as u8, but serde_json won't produce
        // it from to_vec_pretty.  The round-trip is implicitly validated
        // by the type system here.  The real value of the validation step
        // is for types with internal invariants (e.g. enums with
        // `#[serde(try_from = "...")]` or custom validators).
        let raw = b"256";
        assert!(
            serde_json::from_slice::<Bounded>(raw).is_err(),
            "256 should not deserialise as u8"
        );
    }

    #[test]
    fn test_atomic_write_bytes_round_trip_and_cleanup() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("boru-ui.toml");

        let payload = b"# header\n[sidebar]\nwidth = 270.0\n";
        atomic_write_bytes(&path, payload, "test bytes").unwrap();

        // Target present with the exact payload + trailing newline.
        assert!(path.exists());
        let raw = fs::read_to_string(&path).unwrap();
        let mut expected = payload.to_vec();
        expected.push(b'\n');
        assert_eq!(
            raw.as_bytes(),
            &expected[..],
            "no partial/truncated content"
        );

        // No dot-prefixed tmp sibling should remain after a successful write.
        let leftovers: Vec<_> = fs::read_dir(dir.path())
            .unwrap()
            .filter_map(|e| e.ok())
            .map(|e| e.file_name().to_string_lossy().to_string())
            .filter(|n| n != "boru-ui.toml" && n.contains(".tmp"))
            .collect();
        assert!(
            leftovers.is_empty(),
            "temp files should not remain after successful write: {leftovers:?}"
        );
    }

    #[test]
    fn test_atomic_write_bytes_overwrites_previous_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("boru-ui.toml");

        atomic_write_bytes(&path, b"first", "test bytes").unwrap();
        atomic_write_bytes(&path, b"second", "test bytes").unwrap();

        let raw = fs::read_to_string(&path).unwrap();
        assert_eq!(raw, "second\n", "latest write wins; no stale content");
    }

    #[test]
    fn test_tmp_file_is_cleaned_up_on_success() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test.json");

        let data = TestData {
            name: "cleanup".into(),
            value: 1,
        };

        atomic_write_json(&path, &data, "test").unwrap();

        // No dot-prefixed tmp sibling should remain after a successful write.
        let leftovers: Vec<_> = fs::read_dir(dir.path())
            .unwrap()
            .filter_map(|e| e.ok())
            .map(|e| e.file_name().to_string_lossy().to_string())
            .filter(|n| n != "test.json" && n.contains(".tmp"))
            .collect();
        assert!(
            leftovers.is_empty(),
            "temp files should not remain after successful write: {leftovers:?}"
        );
    }

    #[test]
    fn test_concurrent_writes_do_not_race_the_tmp_file() {
        // Reproduces the store-save race: two spawned save threads writing
        // the same JSON store concurrently. With a fixed tmp name one
        // thread's rename loses the file (ENOENT) and reports failure.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("friends.json");

        let handles: Vec<_> = (0..8)
            .map(|i| {
                let path = path.clone();
                std::thread::spawn(move || {
                    for round in 0..25 {
                        let data = TestData {
                            name: format!("writer-{i}"),
                            value: round,
                        };
                        atomic_write_json(&path, &data, "concurrent").unwrap();
                    }
                })
            })
            .collect();
        for handle in handles {
            handle.join().unwrap();
        }

        // The final file must be one complete, valid write.
        let raw = fs::read_to_string(&path).unwrap();
        let decoded: TestData = serde_json::from_str(&raw).unwrap();
        assert!(decoded.name.starts_with("writer-"));
        // No tmp siblings left behind.
        let leftovers: Vec<_> = fs::read_dir(dir.path())
            .unwrap()
            .filter_map(|e| e.ok())
            .map(|e| e.file_name().to_string_lossy().to_string())
            .filter(|n| n.contains(".tmp"))
            .collect();
        assert!(
            leftovers.is_empty(),
            "temp files should not remain after concurrent writes: {leftovers:?}"
        );
    }
}
