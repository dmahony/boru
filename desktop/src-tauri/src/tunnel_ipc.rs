use serde::Serialize;
use tauri::State;

use crate::AppState;

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct TunnelDto {
    id: String,
    target: String,
    status: String,
    allowed_peer: String,
    expires_at_ms: u64,
    active_connections: usize,
}

#[tauri::command]
pub(crate) async fn tunnel_list(state: State<'_, AppState>) -> Result<Vec<TunnelDto>, String> {
    let service = state.tunnel_service.lock().map_err(|_| "runtime_unavailable".to_owned())?.clone().ok_or_else(|| "no_identity".to_owned())?;
    Ok(service.list_tunnels().into_iter().map(|item| {
        let target = match item.target { boru_core::tunnel::service::TunnelTarget::Tcp { host, port } => format!("{host}:{port}") };
        let id = item.id.0.iter().map(|byte| format!("{byte:02x}")).collect::<String>();
        TunnelDto { id, target, status: item.status.label().into(), allowed_peer: item.allowed_peer.fmt_short().to_string(), expires_at_ms: item.expires_at_ms, active_connections: item.active_connections }
    }).collect())
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct LocalServiceDto { port: u16, label: String, is_http: bool }

#[tauri::command]
pub(crate) async fn scan_local_services(state: State<'_, AppState>) -> Result<Vec<LocalServiceDto>, String> {
    if state.runtime.lock().map_err(|_| "runtime_unavailable".to_owned())?.is_none() { return Err("no_identity".to_owned()); }
    let excluded = state.tunnel_service.lock().map_err(|_| "runtime_unavailable".to_owned())?.as_ref().map(|service| service.list_tunnels().iter().filter_map(|t| match t.target { boru_core::tunnel::service::TunnelTarget::Tcp { port, .. } => Some(port) }).collect()).unwrap_or_default();
    let suggestions = boru_core::local_service_scan::scan_local_services(Some(std::process::id()), excluded).await;
    Ok(suggestions.into_iter().map(|s| LocalServiceDto { port: s.port, label: s.label, is_http: s.is_http }).collect())
}

#[tauri::command]
pub(crate) async fn revoke_tunnel(state: State<'_, AppState>, tunnel_id: String) -> Result<(), String> {
    let bytes = tunnel_id.as_bytes();
    if bytes.len() != 64 { return Err("invalid_tunnel_id".to_owned()); }
    let mut id_bytes = [0u8; 32];
    for (index, pair) in bytes.chunks_exact(2).enumerate() {
        let digit = |b: u8| match b { b'0'..=b'9' => Some(b - b'0'), b'a'..=b'f' => Some(b - b'a' + 10), b'A'..=b'F' => Some(b - b'A' + 10), _ => None };
        id_bytes[index] = digit(pair[0]).and_then(|a| digit(pair[1]).map(|b| (a << 4) | b)).ok_or_else(|| "invalid_tunnel_id".to_owned())?;
    }
    let id = boru_core::tunnel::TunnelId(id_bytes);
    let service = state.tunnel_service.lock().map_err(|_| "runtime_unavailable".to_owned())?.clone().ok_or_else(|| "no_identity".to_owned())?;
    service.revoke_tunnel_with_termination(id, true).map(|_| ()).map_err(|_| "tunnel_unavailable".to_owned())
}
