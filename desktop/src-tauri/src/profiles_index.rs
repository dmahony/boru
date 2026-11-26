//! Lazy cross-profile identity alias index. Contains identity metadata only.
use std::{path::{Path, PathBuf}, str::FromStr, time::{SystemTime, UNIX_EPOCH}};
use rusqlite::{params, Connection, OptionalExtension};

const SCHEMA: &str = "CREATE TABLE IF NOT EXISTS profile_index (endpoint_id TEXT PRIMARY KEY, display_name TEXT, alias_normalized TEXT NOT NULL UNIQUE, updated_at_ms INTEGER NOT NULL)";

pub fn normalize_alias(alias: &str) -> String {
    alias.split_whitespace().collect::<Vec<_>>().join(" ").trim().to_lowercase()
}

fn profile_dirs(root: &Path) -> Result<Vec<PathBuf>, String> {
    let mut dirs = Vec::new();
    if root.join("secret_key.txt").is_file() { dirs.push(root.to_path_buf()); }
    let entries = match std::fs::read_dir(root) { Ok(v) => v, Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(dirs), Err(e) => return Err(format!("profile discovery failed: {e}")) };
    for entry in entries {
        let entry = entry.map_err(|e| format!("profile discovery failed: {e}"))?;
        if entry.file_type().map_err(|e| format!("profile discovery failed: {e}"))?.is_dir() && entry.path().join("secret_key.txt").is_file() { dirs.push(entry.path()); }
    }
    dirs.sort();
    Ok(dirs)
}

fn profile_alias(dir: &Path) -> Result<(String, String, String), String> {
    let raw = std::fs::read_to_string(dir.join("secret_key.txt")).map_err(|e| format!("cannot read {}: {e}", dir.join("secret_key.txt").display()))?;
    let key = iroh::SecretKey::from_str(raw.trim()).map_err(|e| format!("invalid key in {}: {e}", dir.display()))?;
    let endpoint = key.public().to_string();
    let alias = Connection::open(dir.join(boru_core::storage::DB_FILE_NAME)).ok()
        .and_then(|c| c.query_row("SELECT value FROM kv_store WHERE key='identity_alias_v1'", [], |r| r.get::<_, String>(0)).ok());
    let fallback = std::fs::read_to_string(dir.join("profile.json")).ok()
        .and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok())
        .and_then(|v| v.get("profile").and_then(|p| p.get("display_name")).and_then(|n| n.as_str()).map(str::to_owned)).unwrap_or_default();
    let display = alias.unwrap_or(fallback);
    Ok((endpoint, display.clone(), normalize_alias(&display)))
}

/// Creates schema and backfills on first access; repeat calls are safe.
pub fn initialize(root: &Path) -> Result<(), String> {
    let profiles = profile_dirs(root)?;
    // Do not create the index at all until the first actual profile is discoverable.
    if profiles.is_empty() { return Ok(()); }
    let mut conn = Connection::open(root.join("profiles.db")).map_err(|e| format!("profiles index open failed: {e}"))?;
    conn.execute(SCHEMA, []).map_err(|e| format!("profiles index schema failed: {e}"))?;
    let tx = conn.transaction().map_err(|e| e.to_string())?;
    let mut candidates = Vec::new();
    for dir in profiles { candidates.push(profile_alias(&dir)?); }
    candidates.sort_by(|a,b| a.0.cmp(&b.0));
    let mut seen = std::collections::HashMap::<String, (String,String)>::new();
    for (endpoint, alias_display, normalized) in candidates {
        if normalized.is_empty() { tracing::warn!(endpoint_id=%endpoint, "profile alias is empty; omitted from alias index"); continue; }
        if let Some((other_id, other_alias)) = seen.get(&normalized) {
            tracing::warn!(alias_normalized=%normalized, endpoint_id=%endpoint, legacy_alias=%alias_display, conflicting_endpoint_id=%other_id, conflicting_legacy_alias=%other_alias, "duplicate legacy profile aliases; resolve with set_alias");
            continue;
        }
        seen.insert(normalized.clone(), (endpoint.clone(), alias_display.clone()));
        let now = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_millis() as i64;
        // Existing indexed values win; backfill never silently overwrites user changes.
        tx.execute("INSERT OR IGNORE INTO profile_index(endpoint_id,display_name,alias_normalized,updated_at_ms) VALUES (?1,?2,?3,?4)", params![endpoint, alias_display, normalized, now]).map_err(|e| format!("profiles index backfill failed: {e}"))?;
    }
    tx.commit().map_err(|e| e.to_string())
}

/// Claims an alias, writes the per-profile store, and rolls the index back on failure.
pub fn set_alias(root: &Path, endpoint: &str, alias: &str, write_profile: impl FnOnce() -> Result<(), String>) -> Result<(), String> {
    initialize(root)?;
    std::fs::create_dir_all(root).map_err(|_| "storage_error".to_owned())?;
    let normalized = normalize_alias(alias);
    let now = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_millis() as i64;
    let mut conn = Connection::open(root.join("profiles.db")).map_err(|_| "storage_error".to_owned())?;
    let previous: Option<(Option<String>, String, i64)> = conn.query_row(
        "SELECT display_name,alias_normalized,updated_at_ms FROM profile_index WHERE endpoint_id=?1",
        [endpoint], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
    ).optional().map_err(|_| "storage_error".to_owned())?;
    if let Err(error) = conn.execute("INSERT INTO profile_index(endpoint_id,display_name,alias_normalized,updated_at_ms) VALUES (?1,?2,?3,?4) ON CONFLICT(endpoint_id) DO UPDATE SET display_name=excluded.display_name,alias_normalized=excluded.alias_normalized,updated_at_ms=excluded.updated_at_ms", params![endpoint, alias, normalized, now]) {
        if error.sqlite_error_code() == Some(rusqlite::ErrorCode::ConstraintViolation) { return Err("alias_taken".to_owned()); }
        return Err("storage_error".to_owned());
    }
    if let Err(error) = write_profile() {
        match previous {
            Some((display, prior_alias, updated)) => { let _ = conn.execute("UPDATE profile_index SET display_name=?2,alias_normalized=?3,updated_at_ms=?4 WHERE endpoint_id=?1", params![endpoint, display, prior_alias, updated]); }
            None => { let _ = conn.execute("DELETE FROM profile_index WHERE endpoint_id=?1", [endpoint]); }
        }
        return Err(error);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    fn make_profile(dir: &Path, alias: &str) -> String {
        std::fs::create_dir_all(dir).unwrap();
        let key=iroh::SecretKey::generate();
        std::fs::write(dir.join("secret_key.txt"), key.to_bytes().iter().map(|b|format!("{b:02x}")).collect::<String>()).unwrap();
        std::fs::write(dir.join("profile.json"), serde_json::json!({"profile":{"display_name":alias}}).to_string()).unwrap();
        key.public().to_string()
    }
    #[test] fn normalization_collapses_whitespace_and_case(){ assert_eq!(normalize_alias("  Alice\n  BOB  "), "alice bob"); }
    #[test] fn lazy_schema_and_idempotent_backfill(){
        let d=tempfile::tempdir().unwrap();
        initialize(d.path()).unwrap(); assert!(!d.path().join("profiles.db").exists());
        let id=make_profile(d.path(), "  Alice  "); initialize(d.path()).unwrap();
        let c=Connection::open(d.path().join("profiles.db")).unwrap();
        let row:(String,String)=c.query_row("SELECT endpoint_id,alias_normalized FROM profile_index",[],|r|Ok((r.get(0)?,r.get(1)?))).unwrap();
        assert_eq!(row,(id,"alice".into())); initialize(d.path()).unwrap();
        assert_eq!(c.query_row("SELECT count(*) FROM profile_index",[],|r|r.get::<_,i64>(0)).unwrap(),1);
    }
    #[test] fn duplicate_aliases_keep_deterministic_row_and_warn_path(){
        let d=tempfile::tempdir().unwrap(); let a=make_profile(&d.path().join("a"),"Same"); let b=make_profile(&d.path().join("b")," same ");
        initialize(d.path()).unwrap(); let c=Connection::open(d.path().join("profiles.db")).unwrap();
        let ids:Vec<String>=c.prepare("SELECT endpoint_id FROM profile_index").unwrap().query_map([],|r|r.get(0)).unwrap().map(Result::unwrap).collect();
        assert_eq!(ids.len(),1); assert_eq!(ids[0], [a,b].into_iter().min().unwrap());
    }
    #[test] fn alias_claim_is_globally_unique_and_same_profile_update_is_allowed() {
        let d=tempfile::tempdir().unwrap();
        set_alias(d.path(), "one", "Alice", || Ok(())).unwrap();
        assert_eq!(set_alias(d.path(), "two", " alice ", || Ok(())).unwrap_err(), "alias_taken");
        set_alias(d.path(), "one", "Alicia", || Ok(())).unwrap();
        let c=Connection::open(d.path().join("profiles.db")).unwrap();
        assert_eq!(c.query_row("SELECT alias_normalized FROM profile_index WHERE endpoint_id='one'",[],|r|r.get::<_,String>(0)).unwrap(), "alicia");
    }
    #[test] fn failed_profile_write_restores_previous_index_row() {
        let d=tempfile::tempdir().unwrap();
        set_alias(d.path(), "one", "Alice", || Ok(())).unwrap();
        assert_eq!(set_alias(d.path(), "one", "Alicia", || Err("storage_error".into())).unwrap_err(), "storage_error");
        let c=Connection::open(d.path().join("profiles.db")).unwrap();
        assert_eq!(c.query_row("SELECT alias_normalized FROM profile_index WHERE endpoint_id='one'",[],|r|r.get::<_,String>(0)).unwrap(), "alice");
    }
}
