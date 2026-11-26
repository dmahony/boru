//! Per-identity persistence for desktop-only presentation state.
use crate::AppState;
use serde::{Deserialize, Serialize};
use std::{
    collections::HashMap,
    fs,
    io::Write,
    path::{Path, PathBuf},
};
use tauri::State;

const FILE_NAME: &str = "ui-preferences.json";
const MIN_WIDTH: u32 = 640;
const MIN_HEIGHT: u32 = 420;
const MAX_WIDTH: u32 = 16_384;
const MAX_HEIGHT: u32 = 16_384;
const FALLBACK_PROFILE: &str = "unselected";

#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
#[serde(default, rename_all = "camelCase")]
pub struct UiPreferences {
    pub drafts: HashMap<String, String>,
    pub active_conversation_id: Option<String>,
    pub active_navigation: Option<String>,
    pub sidebar_width: Option<u32>,
    pub window: Option<WindowGeometry>,
    #[serde(default = "default_true")]
    pub remember_window_geometry: bool,
    #[serde(default = "default_true")]
    pub reopen_last_conversation: bool,
}
fn default_true() -> bool { true }
#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Serialize)]
#[serde(default)]
pub struct WindowGeometry {
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
}
impl Default for WindowGeometry {
    fn default() -> Self {
        Self {
            x: 0,
            y: 0,
            width: 1100,
            height: 760,
        }
    }
}

pub(crate) fn load(dir: &Path) -> Result<UiPreferences, String> {
    match fs::read(dir.join(FILE_NAME)) {
        Ok(bytes) => {
            serde_json::from_slice(&bytes).map_err(|e| format!("parse UI preferences: {e}"))
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(UiPreferences::default()),
        Err(e) => Err(format!("read UI preferences: {e}")),
    }
}
pub(crate) fn save(dir: &Path, prefs: &UiPreferences) -> Result<(), String> {
    fs::create_dir_all(dir).map_err(|e| format!("create preferences directory: {e}"))?;
    let path = dir.join(FILE_NAME);
    let mut tmp = PathBuf::from(&path);
    tmp.set_extension(format!("json.{}.tmp", std::process::id()));
    let result = (|| {
        let bytes =
            serde_json::to_vec(prefs).map_err(|e| format!("serialize UI preferences: {e}"))?;
        let mut file =
            fs::File::create(&tmp).map_err(|e| format!("create temporary preferences: {e}"))?;
        file.write_all(&bytes)
            .map_err(|e| format!("write temporary preferences: {e}"))?;
        file.sync_all()
            .map_err(|e| format!("sync temporary preferences: {e}"))?;
        fs::rename(&tmp, &path).map_err(|e| format!("replace UI preferences: {e}"))?;
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(tmp);
    }
    result
}
pub(crate) fn profile_dir(state: &AppState) -> Result<PathBuf, String> {
    let root = state
        .data_dir
        .lock()
        .map_err(|_| "UI preferences path lock poisoned")?
        .clone()
        .ok_or_else(|| "UI preferences unavailable".to_owned())?;
    let id = state
        .runtime
        .lock()
        .map_err(|_| "runtime state lock poisoned")?
        .as_ref()
        .map(|r| r.endpoint().id().to_string())
        .unwrap_or_else(|| FALLBACK_PROFILE.to_owned());
    Ok(root.join("ui-preferences").join(id))
}
/// Window geometry is owned by the native window lifecycle. A frontend save
/// (drafts, active conversation, toggles) must not be able to revert it.
pub(crate) fn preserve_geometry(existing: &UiPreferences, next: &mut UiPreferences) {
    if existing.window.is_some() {
        next.window = existing.window;
    }
}

/// Persist only the window geometry, leaving every other field untouched.
pub(crate) fn persist_window_geometry(
    state: &AppState,
    geometry: WindowGeometry,
) -> Result<(), String> {
    let dir = profile_dir(state)?;
    let mut prefs = load(&dir)?;
    if !prefs.remember_window_geometry || prefs.window == Some(geometry) {
        return Ok(());
    }
    prefs.window = Some(geometry);
    save(&dir, &prefs)
}

/// Load the preferences for the active profile, or defaults when unavailable.
pub(crate) fn load_current(state: &AppState) -> UiPreferences {
    profile_dir(state)
        .and_then(|dir| load(&dir))
        .unwrap_or_default()
}

#[tauri::command]
pub async fn load_ui_preferences(state: State<'_, AppState>) -> Result<UiPreferences, String> {
    let dir = profile_dir(&state)?;
    tauri::async_runtime::spawn_blocking(move || load(&dir))
        .await
        .map_err(|e| e.to_string())?
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SaveUiPreferencesRequest {
    pub preferences: UiPreferences,
}
#[tauri::command]
pub async fn save_ui_preferences(
    state: State<'_, AppState>,
    request: SaveUiPreferencesRequest,
) -> Result<(), String> {
    let dir = profile_dir(&state)?;
    tauri::async_runtime::spawn_blocking(move || {
        let existing = load(&dir).unwrap_or_default();
        let mut next = request.preferences;
        preserve_geometry(&existing, &mut next);
        save(&dir, &next)
    })
    .await
    .map_err(|e| e.to_string())?
}
/// Keep a saved window visible and large enough to use after display changes.
pub fn restore_geometry(
    saved: WindowGeometry,
    displays: &[(i32, i32, u32, u32)],
) -> WindowGeometry {
    let width = saved.width.clamp(MIN_WIDTH, MAX_WIDTH);
    let height = saved.height.clamp(MIN_HEIGHT, MAX_HEIGHT);
    if displays.is_empty() {
        return WindowGeometry {
            x: 0,
            y: 0,
            width,
            height,
        };
    }
    let visible = displays.iter().any(|&(x, y, w, h)| {
        saved.x < x.saturating_add(w.min(i32::MAX as u32) as i32)
            && saved.x.saturating_add(width.min(i32::MAX as u32) as i32) > x
            && saved.y < y.saturating_add(h.min(i32::MAX as u32) as i32)
            && saved.y.saturating_add(height.min(i32::MAX as u32) as i32) > y
    });
    let (x, y) = if visible {
        (saved.x, saved.y)
    } else {
        (displays[0].0, displays[0].1)
    };
    WindowGeometry {
        x,
        y,
        width,
        height,
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn per_profile_directories_are_independent_and_roundtrip() {
        let d = tempfile::tempdir().unwrap();
        let a = d.path().join("profile-a");
        let b = d.path().join("profile-b");
        let mut p = UiPreferences::default();
        p.drafts.insert("room".into(), "draft".into());
        save(&a, &p).unwrap();
        assert_eq!(load(&a).unwrap(), p);
        assert_eq!(load(&b).unwrap(), UiPreferences::default());
    }
    #[test]
    fn frontend_saves_cannot_revert_stored_window_geometry() {
        let d = tempfile::tempdir().unwrap();
        let stored = UiPreferences {
            window: Some(WindowGeometry {
                x: 40,
                y: 60,
                width: 1200,
                height: 800,
            }),
            ..UiPreferences::default()
        };
        save(d.path(), &stored).unwrap();
        let mut incoming = UiPreferences {
            drafts: HashMap::from([("room-a".to_owned(), "half typed".to_owned())]),
            active_conversation_id: Some("room-a".to_owned()),
            sidebar_width: Some(280),
            ..UiPreferences::default()
        };
        preserve_geometry(&load(d.path()).unwrap(), &mut incoming);
        save(d.path(), &incoming).unwrap();
        let saved = load(d.path()).unwrap();
        assert_eq!(saved.window, stored.window);
        assert_eq!(saved.drafts.get("room-a").map(String::as_str), Some("half typed"));
        assert_eq!(saved.sidebar_width, Some(280));
    }
    #[test]
    fn legacy_defaults_and_geometry_clamping() {
        let d = tempfile::tempdir().unwrap();
        fs::write(d.path().join(FILE_NAME), r#"{"activeNavigation":"chat"}"#).unwrap();
        assert_eq!(
            load(d.path()).unwrap().active_navigation.as_deref(),
            Some("chat")
        );
        let g = restore_geometry(
            WindowGeometry {
                x: -9000,
                y: 8000,
                width: 1,
                height: u32::MAX,
            },
            &[(0, 0, 1024, 768)],
        );
        assert_eq!((g.x, g.y, g.width, g.height), (0, 0, 640, 16384));
    }
}
