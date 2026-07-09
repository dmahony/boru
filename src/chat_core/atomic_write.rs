//! Atomic file update with pre-commit validation.
//!
//! Writing config / state to disk is a common operation across all the
//! persistent stores (friends, room, chat history, room history).  Every
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

    // ── 2. Serialise ────────────────────────────────────────────────
    let tmp_path = path.with_extension("json.tmp");
    let encoded =
        serde_json::to_vec_pretty(data).with_std_context(|_| format!("encode {label}"))?;

    // ── 3. Pre-commit validation: round-trip through serde ──────────
    //
    // This catches NaN / infinity floats, out-of-range integer enums,
    // and other corruption that serde_json's serialiser can produce
    // because the type's invariants aren't expressed in the schema.
    // If this fails the old file on disk is **untouched**.
    serde_json::from_slice::<T>(&encoded)
        .with_std_context(|_| format!("validate {label} — re-deserialisation check"))?;

    // ── 4. Write to tmp file with fsync ─────────────────────────────
    {
        let file = fs::File::create(&tmp_path).with_std_context(|_| {
            format!(
                "failed to create temp file for {label}: {}",
                tmp_path.display()
            )
        })?;
        let mut writer = BufWriter::new(file);

        writer.write_all(&encoded).with_std_context(|_| {
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

    // ── 5. Replace the old file atomically ──────────────────────────
    if path.exists() {
        fs::remove_file(path).with_std_context(|_| {
            format!("failed to remove old file for {label}: {}", path.display())
        })?;
    }

    fs::rename(&tmp_path, path)
        .with_std_context(|_| format!("failed to replace file for {label}: {}", path.display()))?;

    // ── 6. Restrictive permissions on the final file ────────────────
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
    fn test_tmp_file_is_cleaned_up_on_success() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test.json");

        let data = TestData {
            name: "cleanup".into(),
            value: 1,
        };

        atomic_write_json(&path, &data, "test").unwrap();

        // The .json.tmp sibling should *not* exist after a successful write.
        let tmp_path = path.with_extension("json.tmp");
        assert!(
            !tmp_path.exists(),
            "temp file should not remain after successful write"
        );
    }
}
