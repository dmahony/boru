//! Explicit opt-in IPC surface for local automated desktop tests.
//! This module is absent unless `diagnostics-test` is enabled.
use tauri::State;

use crate::AppState;

/// Minimal authenticated smoke probe; later test actions must follow the contract in docs.
#[tauri::command]
pub(crate) fn test_diagnostics_ping(
    state: State<'_, AppState>,
    token: String,
) -> Result<&'static str, String> {
    let expected = std::env::var("BORU_TEST_DIAGNOSTICS_TOKEN")
        .map_err(|_| "test diagnostics token is not configured".to_owned())?;
    if token.is_empty() || !constant_time_eq(token.as_bytes(), expected.as_bytes()) {
        return Err("unauthorized".to_owned());
    }
    let guard = state.runtime.lock().map_err(|_| "runtime state lock poisoned".to_owned())?;
    if guard.is_none() {
        return Err("runtime unavailable".to_owned());
    }
    Ok("ok")
}

fn constant_time_eq(left: &[u8], right: &[u8]) -> bool {
    if left.len() != right.len() {
        return false;
    }
    left.iter().zip(right).fold(0u8, |diff, (a, b)| diff | (a ^ b)) == 0
}
