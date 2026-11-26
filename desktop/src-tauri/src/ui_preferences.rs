//! Atomic persistence for desktop-only presentation state. This file is separate
//! from the protocol/profile database and intentionally tolerates older files.
use serde::{Deserialize, Serialize};
use std::{fs, io::Write, path::{Path, PathBuf}};

const FILE_NAME: &str = "ui-preferences.json";
const MIN_WIDTH: u32 = 640;
const MIN_HEIGHT: u32 = 420;
const MAX_WIDTH: u32 = 16_384;
const MAX_HEIGHT: u32 = 16_384;

#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
#[serde(default)]
pub struct UiPreferences {
    pub drafts: std::collections::HashMap<String, String>,
    pub active_conversation_id: Option<String>,
    pub active_navigation: Option<String>,
    pub sidebar_width: Option<u32>,
    pub window: Option<WindowGeometry>,
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Serialize)]
#[serde(default)]
pub struct WindowGeometry {
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
}
impl Default for WindowGeometry {
    fn default() -> Self { Self { x: 0, y: 0, width: 1100, height: 760 } }
}

pub fn load(dir: &Path) -> Result<UiPreferences, String> {
    let path = dir.join(FILE_NAME);
    match fs::read(&path) {
        Ok(bytes) => serde_json::from_slice(&bytes).map_err(|e| format!("parse UI preferences: {e}")),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(UiPreferences::default()),
        Err(e) => Err(format!("read UI preferences: {e}")),
    }
}

pub fn save(dir: &Path, prefs: &UiPreferences) -> Result<(), String> {
    fs::create_dir_all(dir).map_err(|e| format!("create preferences directory: {e}"))?;
    let path = dir.join(FILE_NAME);
    let mut tmp = PathBuf::from(&path);
    tmp.set_extension(format!("json.{}.tmp", std::process::id()));
    let result = (|| {
        let bytes = serde_json::to_vec(prefs).map_err(|e| format!("serialize UI preferences: {e}"))?;
        let mut file = fs::File::create(&tmp).map_err(|e| format!("create temporary preferences: {e}"))?;
        file.write_all(&bytes).map_err(|e| format!("write temporary preferences: {e}"))?;
        file.sync_all().map_err(|e| format!("sync temporary preferences: {e}"))?;
        fs::rename(&tmp, &path).map_err(|e| format!("replace UI preferences: {e}"))?;
        Ok(())
    })();
    if result.is_err() { let _ = fs::remove_file(&tmp); }
    result
}

/// Keep a saved window visible and large enough to use after display changes.
pub fn restore_geometry(saved: WindowGeometry, displays: &[(i32, i32, u32, u32)]) -> WindowGeometry {
    let width = saved.width.clamp(MIN_WIDTH, MAX_WIDTH);
    let height = saved.height.clamp(MIN_HEIGHT, MAX_HEIGHT);
    if displays.is_empty() { return WindowGeometry { x: 0, y: 0, width, height }; }
    let visible = displays.iter().any(|&(x, y, w, h)| {
        saved.x < x.saturating_add(w.min(i32::MAX as u32) as i32)
            && saved.x.saturating_add(width.min(i32::MAX as u32) as i32) > x
            && saved.y < y.saturating_add(h.min(i32::MAX as u32) as i32)
            && saved.y.saturating_add(height.min(i32::MAX as u32) as i32) > y
    });
    let (x, y) = if visible { (saved.x, saved.y) } else { (displays[0].0, displays[0].1) };
    WindowGeometry { x, y, width, height }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn missing_or_legacy_preferences_load_with_defaults() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(load(dir.path()).unwrap(), UiPreferences::default());
        fs::write(dir.path().join(FILE_NAME), r#"{"active_navigation":"chat"}"#).unwrap();
        let old = load(dir.path()).unwrap();
        assert_eq!(old.active_navigation.as_deref(), Some("chat"));
        assert!(old.drafts.is_empty());
        assert_eq!(old.window, None);
    }
    #[test]
    fn preferences_roundtrip_atomically_and_restore_after_restart() {
        let dir = tempfile::tempdir().unwrap();
        let mut prefs = UiPreferences::default();
        prefs.active_conversation_id = Some("room-a".into());
        prefs.drafts.insert("room-a".into(), "unsent text".into());
        prefs.window = Some(WindowGeometry { x: 20, y: 30, width: 1400, height: 900 });
        save(dir.path(), &prefs).unwrap();
        assert_eq!(load(dir.path()).unwrap(), prefs);
    }
    #[test]
    fn removed_monitor_does_not_leave_window_off_screen() {
        let current = [(0, 0, 1920, 1080), (1920, 0, 1280, 1024)];
        let saved = WindowGeometry { x: 2500, y: 100, width: 1100, height: 800 };
        assert_eq!(restore_geometry(saved, &current).x, 2500);
        let after_removal = [(0, 0, 1920, 1080)];
        let restored = restore_geometry(saved, &after_removal);
        assert_eq!((restored.x, restored.y), (0, 0));
        assert!(restored.width <= 1920 && restored.height <= 1080);
    }
    #[test]
    fn geometry_is_clamped_to_usable_minimum_and_maximum() {
        let g = restore_geometry(WindowGeometry { x: -9000, y: 8000, width: 1, height: u32::MAX }, &[(0, 0, 1024, 768)]);
        assert_eq!((g.x, g.y, g.width, g.height), (0, 0, 640, 16_384));
    }
}
