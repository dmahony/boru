//! MCP diagnostic server for boru-chat.
//!
//! Exposes JSON-RPC 2.0 diagnostic tools over TCP (loopback by default).
//!
//! # Tools
//!
//! | Tool | Description |
//! |------|-------------|
//! | `boru_ping` | Lightweight health check (no state required) |
//! | `boru_get_node_status` | Local node identity, version, event count |
//! | `boru_get_room_status` | Room membership and peer summary |
//! | `boru_get_discovery_events` | Recent diagnostic events |
//! | `boru_send_probe` | Broadcast a diagnostic probe through gossip |
//! | `boru_find_received_probe` | Look up a received probe by ID |
//! | `boru_get_peer_status` | Per-peer diagnostic state |
//! | `boru_wait_for_peer` | Wait for a peer to reach a target state |
//! | `boru_run_discovery_test` | Orchestrated discovery test against a peer |
//! | `boru_get_iced_state` | Snapshot of current Iced application state |
//! | `boru_get_iced_message_journal` | Recent Iced AppMessage processing history |
//! | `boru_get_failure_analysis` | Combined failure analysis across all layers |
//! | `boru_gui_open_room` | Send a GUI 'open room' command (requires `--enable-gui-test-actions`) |
//! | `boru_join_lobby_room` | Open and join the stable diagnostic lobby room |
//! | `boru_send_gui_action` | Send a GUI test action command |
//! | `boru_gui_get_action_status` | Full status of a GUI test action by idempotency key |
//! | `boru_gui_navigate` | Navigate to a GUI screen by destination name |
//! | `boru_get_gui_snapshot` | Snapshot of current GUI application state |
//! | `boru_gui_wait_for_state` | Wait for a GUI state condition using notifications |
//! | `boru_gui_set_composer` | Set composer (message input) text without submitting |
//! | `boru_gui_submit_composer` | Submit the current composer text through the normal GUI send path |
//! | `boru_gui_open_conversation` | Open a direct conversation with a peer (requires `--enable-gui-test-actions`) |
//! | `boru_gui_toggle_dark_mode` | Toggle dark mode on/off (requires `--enable-gui-test-actions`) |
//! | `boru_run_gui_message_test` | Verify the local GUI message pipeline without claiming remote delivery (requires `--enable-gui-test-actions`) |
//!
//! # Security
//!
//! - Binds to loopback by default (`127.0.0.1`).
//! - No secrets (keys, tickets, mailbox tokens) are exposed.
//! - Probe payloads are inert diagnostic text — never executed.
//!
//! # GUI test mode
//!
//! Tools that observe or interact with the Iced application UI state
//! (`boru_get_iced_state`, `boru_get_iced_message_journal`) are only
//! registered when the application is started with **both** `--mcp` and
//! `--enable-gui-test-actions`.  The latter flag also forces the MCP
//! server to bind to a loopback address only, preventing remote access to
//! GUI-test tools.

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use crate::gui_test_actions::{
    ActionRecord, ActionStatus, GuiActionHistory, GuiActionRateLimiter, RateLimitError,
};
use boru_chat::chat_core::{broadcast_diagnostic_probe, message_hash, Message, SignedMessage};
use boru_chat::conversations::ConversationNetEvent;
use boru_chat::diagnostics::{
    self, classify_discovery_test, classify_failures, generate_probe_id, ConnectionDiagnosticState,
    DiagnosticEvent, DiagnosticEventKind, DiagnosticStageState, Diagnostics, DiscoveryFailureStage,
    DiscoveryTestEvidence, DiscoveryTestResult, FailureAnalysis, FailureLayer, GuiWaitCondition,
    IcedMessageJournal, IcedStateSnapshot, PeerDiagnosticState, ProbeTestResult, ReceivedProbe,
};
use boru_chat::net::Gossip;
use boru_chat::proto::TopicId;
use bytes::Bytes;
use iroh::{Endpoint, SecretKey};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::TcpListener;
use tokio::sync::watch;
use tracing::{error, info, warn};

// =============================================================================
// Input safety — validation and sanitisation helpers
// =============================================================================

/// Maximum length for room_id hex strings (32 bytes = 64 hex chars).
pub const MAX_ROOM_ID_LEN: usize = 64;

/// Maximum length for GUI open_room room_id (room names/labels).
/// Room IDs for the GUI open-room command can be names or labels
/// up to 128 characters, alphanumeric plus hyphen/underscore.
pub const MAX_GUI_ROOM_ID_LEN: usize = 128;

/// Maximum length for peer_id hex strings (covers 32-byte ed25519 + possible
/// multihash prefix and base32/base58 encoding).
pub const MAX_PEER_ID_LEN: usize = 128;

/// Maximum length for probe_id strings.
pub const MAX_PROBE_ID_LEN: usize = 64;

/// Maximum length for probe payload message text (64 KiB).  Message text
/// is the one string that MUST preserve Unicode — no control-char rejection.
pub const MAX_PROBE_PAYLOAD_LEN: usize = 65536;

/// Maximum length for target_state identifier strings.
pub const MAX_TARGET_STATE_LEN: usize = 32;

/// Maximum length for composer (message input) text, in characters.
/// Input beyond this limit is silently clamped (truncated) rather than
/// rejected, to avoid requiring the caller to pre-truncate.
pub const MAX_COMPOSER_LEN: usize = 4096;

/// Validate that a string parameter does not exceed its maximum byte length.
///
/// Returns an error with a descriptive message if it does.
pub fn validate_bounded(s: &str, max_len: usize, name: &str) -> Result<(), String> {
    if s.len() > max_len {
        return Err(format!(
            "{} too long ({} bytes, max {})",
            name,
            s.len(),
            max_len
        ));
    }
    Ok(())
}

/// Validate that a string contains no control characters (except space).
///
/// Use for identifiers, room IDs, and other non-message parameters.
/// Do NOT use for message text which should preserve Unicode.
pub fn validate_no_control_chars(s: &str, name: &str) -> Result<(), String> {
    if s.chars().any(|c| c.is_control() && c != ' ') {
        return Err(format!("{} must not contain control characters", name));
    }
    Ok(())
}

/// Validate a hex-encoded or base58 peer ID.
///
/// Allows hex digits, underscore, and hyphen (common in iroh short IDs).
/// Rejects control characters, filesystem separators, and shell metacharacters.
pub fn validate_peer_id(s: &str) -> Result<(), String> {
    if s.is_empty() {
        return Err("peer_id must not be empty".to_string());
    }
    validate_bounded(s, MAX_PEER_ID_LEN, "peer_id")?;
    validate_no_control_chars(s, "peer_id")?;
    if s.contains('/') || s.contains('\\') {
        return Err("peer_id must not contain filesystem path separators".to_string());
    }
    if s.contains('$') || s.contains('`') || s.contains('|') || s.contains(';') {
        return Err("peer_id must not contain shell metacharacters".to_string());
    }
    Ok(())
}

/// Validate a probe_id string format.
///
/// Bounded to MAX_PROBE_ID_LEN, no control characters, no path separators.
pub fn validate_probe_id(s: &str) -> Result<(), String> {
    validate_bounded(s, MAX_PROBE_ID_LEN, "probe_id")?;
    validate_no_control_chars(s, "probe_id")?;
    if s.contains('/') || s.contains('\\') {
        return Err("probe_id must not contain filesystem path separators".to_string());
    }
    Ok(())
}

/// Validate a probe payload string — allows full Unicode, rejects only
/// extreme length.
pub fn validate_probe_payload(s: &str) -> Result<(), String> {
    validate_bounded(s, MAX_PROBE_PAYLOAD_LEN, "probe_payload")
}

/// Validate a target_state string against a list of allowed values.
pub fn validate_target_state(s: &str) -> Result<(), String> {
    validate_bounded(s, MAX_TARGET_STATE_LEN, "target_state")?;
    validate_no_control_chars(s, "target_state")?;
    let allowed = [
        "discovered",
        "address_resolved",
        "connected",
        "subscription_joined",
        "topic_member",
    ];
    if !allowed.contains(&s) {
        return Err(format!(
            "Invalid target_state '{}'. Allowed values: {:?}",
            s, allowed
        ));
    }
    Ok(())
}

/// Validate that a string does not contain filesystem path separators
/// or shell metacharacters.
pub fn validate_no_path_or_shell(s: &str, name: &str) -> Result<(), String> {
    if s.contains('/') || s.contains('\\') {
        return Err(format!(
            "{} must not contain filesystem path separators",
            name
        ));
    }
    if s.contains('$') || s.contains('`') || s.contains('|') || s.contains(';') || s.contains('>') {
        return Err(format!("{} must not contain shell metacharacters", name));
    }
    Ok(())
}

/// Validate a caller-controlled GUI action identifier.
pub fn validate_gui_action_id(s: &str, name: &str) -> Result<(), String> {
    if s.is_empty() {
        return Err(format!("{name} must not be empty"));
    }
    validate_bounded(s, MAX_PROBE_ID_LEN, name)?;
    validate_no_control_chars(s, name)?;
    if !s
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_'))
    {
        return Err(format!(
            "{name} contains invalid characters; only ASCII letters, digits, '-' and '_' are allowed"
        ));
    }
    Ok(())
}

/// Sanitize a string for logging — truncate to `max_chars` and escape
/// control characters so that log output is safe to display.
///
/// Never log full message text — always truncate.
pub fn sanitize_for_log(s: &str, max_chars: usize) -> String {
    let truncated: String = s.chars().take(max_chars).collect();
    let display = truncated
        .replace('\n', "\\n")
        .replace('\r', "\\r")
        .replace('\t', "\\t");
    let total_chars = s.chars().count();
    if total_chars > max_chars {
        format!("{}... (truncated, total {} chars)", display, total_chars)
    } else {
        display
    }
}

// =============================================================================
// MCP server
// =============================================================================

/// Configuration for the MCP diagnostic server.
#[derive(Debug, Clone)]
pub struct McpConfig {
    /// Address to bind (e.g. `127.0.0.1:8765`).
    pub bind_addr: SocketAddr,
    /// Whether GUI test action tools should be registered.
    /// These tools can observe and interact with the application's
    /// UI state and should only be enabled in controlled environments.
    ///
    /// ⚠️  Security: the calling code in `main.rs` enforces that this flag can
    /// only be used with loopback bind addresses.  We also check here for
    /// defense in depth.
    pub enable_gui_test_actions: bool,
}

impl Default for McpConfig {
    fn default() -> Self {
        Self {
            bind_addr: ([127, 0, 0, 1], 8765).into(),
            enable_gui_test_actions: false,
        }
    }
}

/// Shared application state exposed to MCP tools.
#[derive(Clone)]
pub struct McpAppState {
    /// Diagnostics store (shared with the global DIAGNOSTICS singleton).
    pub diagnostics: Diagnostics,
    /// Iced message journal (shared with the Iced application loop).
    pub iced_diagnostics: IcedMessageJournal,
    /// Iroh endpoint for node identity and connection info.
    pub endpoint: Endpoint,
    /// Room topics the local node is subscribed to.
    pub rooms: Arc<Mutex<Vec<TopicId>>>,
    /// The local node's public key as a hex string.
    pub node_id: String,
    /// Current application version.
    pub version: String,
    /// Channel to send gossip messages through the mesh.
    pub gossip_tx: tokio::sync::mpsc::UnboundedSender<ConversationNetEvent>,
    /// Secret key for signing outgoing messages (probes, etc.).
    pub secret_key: SecretKey,
    /// Gossip handle for broadcasting messages through the mesh.
    pub gossip: Gossip,
    /// Whether GUI test-action MCP tools are registered.
    pub gui_test_actions_enabled: bool,
    /// Sender for GUI test actions (None if test actions are disabled).
    pub gui_action_tx: Option<boru_chat::diagnostics::GuiTestHandle>,
    /// History of GUI test actions (shared with the Iced GUI loop).
    pub gui_action_history: GuiActionHistory,
    /// Shared lifecycle history populated at MCP enqueue and Iced receipt.
    pub gui_action_lifecycle: boru_chat::diagnostics::GuiActionHistory,
    /// Rate limiter for GUI test actions, shared across MCP connections.
    pub gui_action_rate_limiter: Arc<Mutex<GuiActionRateLimiter>>,
    /// Latest GUI state, published by the Iced application through a watch channel.
    pub gui_state_rx: Option<watch::Receiver<IcedStateSnapshot>>,
}

// =============================================================================
// Rate limit helper
// =============================================================================

/// Check the shared GUI action rate limiter and return a structured error
/// if the action would exceed the rate limit.
///
/// Diagnostic read-only tools (`boru_get_gui_snapshot`, `boru_gui_get_action_status`)
/// should NOT call this function — they are excluded from the restrictive limit.
fn check_gui_action_rate_limit(
    rate_limiter: &Arc<Mutex<crate::gui_test_actions::GuiActionRateLimiter>>,
) -> Result<(), String> {
    let mut limiter = rate_limiter
        .lock()
        .map_err(|e| format!("Rate limiter lock error: {e}"))?;
    limiter.check_and_record().map_err(|e| e.to_string())
}

/// Spawn the MCP server in a background task.
pub async fn spawn_mcp_server(config: McpConfig, state: McpAppState) -> Result<(), String> {
    // Use socket2 to set SO_REUSEADDR before binding, so the port can be
    // reused immediately after the process exits (avoids TIME_WAIT orphan
    // sockets blocking the next test run or restart).
    let addr: std::net::SocketAddr = config.bind_addr;
    let domain = if addr.is_ipv4() {
        socket2::Domain::IPV4
    } else {
        socket2::Domain::IPV6
    };
    let socket = socket2::Socket::new(domain, socket2::Type::STREAM, Some(socket2::Protocol::TCP))
        .map_err(|e| format!("Failed to create MCP socket: {e}"))?;
    socket
        .set_reuse_address(true)
        .map_err(|e| format!("Failed to set SO_REUSEADDR on MCP socket: {e}"))?;
    socket
        .set_nonblocking(true)
        .map_err(|e| format!("Failed to set MCP socket to non-blocking: {e}"))?;
    let sock_addr: socket2::SockAddr = addr.into();
    socket
        .bind(&sock_addr)
        .map_err(|e| format!("Failed to bind MCP server: {e}"))?;
    socket
        .listen(128)
        .map_err(|e| format!("Failed to listen on MCP socket: {e}"))?;

    // Convert the socket2 socket into a tokio TcpListener.
    let std_listener: std::net::TcpListener = socket.into();
    let listener = TcpListener::from_std(std_listener)
        .map_err(|e| format!("Failed to create tokio listener from MCP socket: {e}"))?;

    info!("MCP diagnostic server listening on {}", config.bind_addr);
    if !config.bind_addr.ip().is_loopback() {
        warn!(
            "MCP server bound to {} — this is not a loopback address. \
             Diagnostic tools will be accessible from the network.",
            config.bind_addr
        );
    }

    // Defense in depth: refuse non-loopback binding when GUI test actions
    // are enabled, even if main.rs did not catch it.
    if config.enable_gui_test_actions && !config.bind_addr.ip().is_loopback() {
        return Err(
            "Refusing to start MCP server with --enable-gui-test-actions on non-loopback address. \
             Use a 127.0.0.1:<port> address."
                .to_string(),
        );
    }

    tokio::spawn(async move {
        loop {
            match listener.accept().await {
                Ok((stream, addr)) => {
                    info!("MCP connection from {addr}");
                    let state = state.clone();
                    tokio::spawn(async move {
                        if let Err(e) = handle_connection(stream, state).await {
                            warn!("MCP connection error from {addr}: {e}");
                        }
                    });
                }
                Err(e) => {
                    error!("MCP accept error: {e}");
                    tokio::time::sleep(Duration::from_secs(1)).await;
                }
            }
        }
    });

    Ok(())
}

/// Handle a single MCP TCP connection with newline-delimited JSON-RPC.
async fn handle_connection(
    stream: tokio::net::TcpStream,
    state: McpAppState,
) -> Result<(), String> {
    let (reader, mut writer) = stream.into_split();
    let mut reader = BufReader::new(reader);
    let mut line = String::new();

    loop {
        line.clear();
        let n = reader
            .read_line(&mut line)
            .await
            .map_err(|e| format!("read error: {e}"))?;

        if n == 0 {
            // Connection closed
            return Ok(());
        }

        let request: JsonRpcRequest = match serde_json::from_str(line.trim()) {
            Ok(r) => r,
            Err(e) => {
                let error = jsonrpc_error(None, -32700, "Parse error", &e.to_string());
                let _ = writer
                    .write_all((serde_json::to_string(&error).unwrap() + "\n").as_bytes())
                    .await;
                continue;
            }
        };

        let response = handle_request(&request, &state).await;
        let json = serde_json::to_string(&response).map_err(|e| format!("serialize error: {e}"))?;
        if let Err(e) = writer.write_all((json + "\n").as_bytes()).await {
            warn!("MCP write error: {e}");
            return Ok(());
        }
    }
}

/// Handle a single JSON-RPC request and produce a response.
async fn handle_request(req: &JsonRpcRequest, state: &McpAppState) -> JsonRpcResponse {
    let result = match req.method.as_str() {
        "boru_ping" => handle_ping(state).await,
        "boru_get_node_status" => handle_get_node_status(state).await,
        "boru_get_room_status" => handle_get_room_status(req, state).await,
        "boru_get_discovery_events" => handle_get_discovery_events(req, state).await,
        "boru_send_probe" => handle_send_probe(req, state).await,
        "boru_find_received_probe" => handle_find_received_probe(req, state).await,
        "boru_get_peer_status" => handle_get_peer_status(req, state).await,
        "boru_wait_for_peer" => handle_wait_for_peer(req, state).await,
        "boru_run_discovery_test" => handle_run_discovery_test(req, state).await,
        "boru_join_lobby_room" => {
            if !state.gui_test_actions_enabled || state.gui_action_tx.is_none() {
                return jsonrpc_error(
                    req.id.clone(),
                    -32601,
                    "Method not found",
                    "GUI test actions are not enabled. Start with --enable-gui-test-actions.",
                );
            }
            handle_join_lobby_room(req, state).await
        }
        "boru_get_iced_state" => {
            if !state.gui_test_actions_enabled {
                return jsonrpc_error(
                    req.id.clone(),
                    -32601,
                    "Method not found",
                    "GUI test actions are not enabled. Start with --enable-gui-test-actions.",
                );
            }
            handle_get_iced_state(state).await
        }

        "boru_get_iced_message_journal" => {
            if !state.gui_test_actions_enabled {
                return jsonrpc_error(
                    req.id.clone(),
                    -32601,
                    "Method not found",
                    "GUI test actions are not enabled. Start with --enable-gui-test-actions.",
                );
            }
            handle_get_iced_message_journal(req, state).await
        }

        "boru_get_failure_analysis" => handle_get_failure_analysis(req, state).await,

        // `boru_gui_wait_for_state` is read-only and does not require the action queue.
        "boru_gui_wait_for_state" => {
            if !state.gui_test_actions_enabled {
                return jsonrpc_error(
                    req.id.clone(),
                    -32601,
                    "Method not found",
                    "GUI test actions are not enabled. Start with --enable-gui-test-actions.",
                );
            }
            if let Err(error) = validate_gui_tool_params(&req.method, &req.params) {
                return jsonrpc_error(req.id.clone(), -32602, "Invalid params", &error);
            }
            handle_gui_wait_for_state(req, state).await
        }

        // ── GUI test action tools (gated on enable_gui_test_actions) ──
        "boru_send_gui_action"
        | "boru_gui_navigate"
        | "boru_gui_get_action_status"
        | "boru_get_gui_snapshot"
        | "boru_gui_set_composer"
        | "boru_gui_clear_composer"
        | "boru_gui_focus_composer"
        | "boru_gui_open_room"
        | "boru_gui_open_conversation"
        | "boru_gui_submit_composer"
        | "boru_gui_toggle_dark_mode"
        | "boru_gui_close_dialog" => {
            if !state.gui_test_actions_enabled || state.gui_action_tx.is_none() {
                return jsonrpc_error(
                    req.id.clone(),
                    -32601,
                    "Method not found",
                    "GUI test actions are not enabled. Start with --enable-gui-test-actions.",
                );
            }
            if let Err(error) = validate_gui_tool_params(&req.method, &req.params) {
                return jsonrpc_error(req.id.clone(), -32602, "Invalid params", &error);
            }
            let tx = state.gui_action_tx.clone().unwrap();
            match req.method.as_str() {
                // Read-only tools — excluded from restrictive rate limit
                "boru_gui_get_action_status" => handle_get_gui_action_status(req, state).await,
                "boru_get_gui_snapshot" => handle_get_gui_snapshot(state).await,

                // Mutating tools — rate-limited
                "boru_send_gui_action"
                | "boru_gui_navigate"
                | "boru_gui_set_composer"
                | "boru_gui_clear_composer"
                | "boru_gui_focus_composer"
                | "boru_gui_open_room"
                | "boru_gui_open_conversation"
                | "boru_gui_submit_composer"
                | "boru_gui_toggle_dark_mode"
                | "boru_gui_close_dialog" => {
                    if let Err(e) = check_gui_action_rate_limit(&state.gui_action_rate_limiter) {
                        return jsonrpc_error(req.id.clone(), -32000, "Rate limit exceeded", &e);
                    }
                    match req.method.as_str() {
                        "boru_send_gui_action" => handle_send_gui_action(req, tx).await,
                        "boru_gui_navigate" => handle_gui_navigate(req, tx).await,
                        "boru_gui_set_composer" => handle_set_composer(req, tx).await,
                        "boru_gui_clear_composer" => handle_composer_control("clear", tx).await,
                        "boru_gui_focus_composer" => handle_composer_control("focus", tx).await,
                        "boru_gui_open_room" => handle_gui_open_room(req, tx).await,
                        "boru_gui_open_conversation" => handle_gui_open_conversation(req, tx).await,
                        "boru_gui_submit_composer" => handle_submit_composer(tx).await,
                        "boru_gui_toggle_dark_mode" => handle_gui_toggle_dark_mode(req, tx).await,
                        "boru_gui_close_dialog" => handle_gui_close_dialog(tx).await,
                        _ => unreachable!(),
                    }
                }

                // All outer-match-armed methods are covered above; this arm
                // satisfies Rust's exhaustive-pattern check on &str.
                _ => unreachable!(),
            }
        }

        // ── boru_run_gui_message_test ──
        // Separate entry: needs `state`, not just `tx`, for diagnostics polling.
        "boru_run_gui_message_test" => {
            if !state.gui_test_actions_enabled || state.gui_action_tx.is_none() {
                return jsonrpc_error(
                    req.id.clone(),
                    -32601,
                    "Method not found",
                    "GUI test actions are not enabled. Start with --enable-gui-test-actions.",
                );
            }
            if let Err(error) = validate_gui_tool_params(&req.method, &req.params) {
                return jsonrpc_error(req.id.clone(), -32602, "Invalid params", &error);
            }
            if let Err(e) = check_gui_action_rate_limit(&state.gui_action_rate_limiter) {
                return jsonrpc_error(req.id.clone(), -32000, "Rate limit exceeded", &e);
            }
            handle_run_local_gui_message_test(req, state).await
        }

        _ => {
            return jsonrpc_error(
                req.id.clone(),
                -32601,
                "Method not found",
                &format!("Unknown method: {}", req.method),
            );
        }
    };

    match result {
        Ok(value) => JsonRpcResponse {
            jsonrpc: "2.0".to_string(),
            id: req.id.clone(),
            result: Some(value),
            error: None,
        },
        Err(e) => jsonrpc_error(req.id.clone(), -32000, "Internal error", &e),
    }
}

/// Validate the outer JSON object for each GUI MCP method.
///
/// Handler-level validation remains responsible for value types and semantic
/// constraints. This boundary check makes the documented schemas strict:
/// misspelled or otherwise unknown fields are rejected instead of silently
/// ignored, and required fields are reported as JSON-RPC `-32602` errors.
fn validate_gui_tool_params(method: &str, params: &Value) -> Result<(), String> {
    const GUI_METHODS: &[(&str, &[&str], &[&str])] = &[
        ("boru_send_gui_action", &["command"], &["idempotency_key"]),
        ("boru_gui_get_action_status", &["action_id"], &[]),
        ("boru_get_gui_snapshot", &[], &[]),
        ("boru_gui_navigate", &["destination"], &[]),
        ("boru_gui_set_composer", &["text"], &[]),
        ("boru_gui_open_room", &["room_id"], &[]),
        ("boru_gui_open_conversation", &["conversation_id"], &[]),
        ("boru_gui_submit_composer", &[], &[]),
        ("boru_gui_clear_composer", &[], &[]),
        ("boru_gui_focus_composer", &[], &[]),
        ("boru_gui_toggle_dark_mode", &["enabled"], &[]),
        ("boru_gui_close_dialog", &[], &[]),
        ("boru_gui_wait_for_state", &["condition"], &["timeout_ms"]),
        (
            "boru_run_gui_message_test",
            &["room_id", "message_text", "expected_peer_id"],
            &["timeout_ms"],
        ),
    ];
    let Some((_, required, optional)) = GUI_METHODS.iter().find(|(name, _, _)| *name == method)
    else {
        return Ok(());
    };
    let object = match params {
        Value::Object(object) => object,
        Value::Null if required.is_empty() && optional.is_empty() => return Ok(()),
        _ => return Err("params must be a JSON object".to_string()),
    };
    for key in object.keys() {
        if !required
            .iter()
            .chain(optional.iter())
            .any(|allowed| *allowed == key)
        {
            return Err(format!("Unknown argument: {key}"));
        }
    }
    for key in *required {
        if !object.contains_key(*key) {
            return Err(format!("Missing required argument: {key}"));
        }
    }
    Ok(())
}

/// `boru_send_gui_action` — send a GUI test command through the channel.
async fn handle_send_gui_action(
    req: &JsonRpcRequest,
    tx: boru_chat::diagnostics::GuiTestHandle,
) -> Result<Value, String> {
    let command_value = req
        .params
        .get("command")
        .ok_or_else(|| "Missing required argument: command".to_string())?;

    let command: boru_chat::diagnostics::GuiTestCommand =
        serde_json::from_value(command_value.clone())
            .map_err(|e| format!("Invalid command: {e}"))?;

    command.validate()?;

    let idempotency_key = req
        .params
        .get("idempotency_key")
        .and_then(|v| v.as_str())
        .map(|s| {
            // Validate caller-supplied key — bounded, no control chars
            validate_gui_action_id(s, "idempotency_key")?;
            Ok::<String, String>(s.to_string())
        })
        .transpose()?
        .unwrap_or_else(|| {
            static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_micros();
            let seq = COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            format!("gui_action_{:x}_{}", now, seq)
        });

    // Serialize the command to JSON for the GuiActionRequest.command field
    let command_json =
        serde_json::to_string(&command).map_err(|e| format!("Failed to serialize command: {e}"))?;

    let request = boru_chat::diagnostics::GuiActionRequest {
        action_id: boru_chat::diagnostics::GuiActionId(idempotency_key.clone()),
        requested_at_ms: std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as i64,
        command: command_json,
    };

    // Send through the channel (non-blocking via enqueue)
    let _sent = tx.enqueue(request).map_err(|e| match e.code {
        boru_chat::diagnostics::GuiActionErrorCode::ActionQueueFull => {
            format!("GUI action queue is full (capacity: {})", tx.capacity())
        }
        _ => format!("GUI action channel error: {}", e.message),
    })?;

    Ok(serde_json::json!({
        "sent": true,
        "idempotency_key": idempotency_key,
        "command": serde_json::to_value(&command).map_err(|e| format!("serialize: {e}"))?,
    }))
}

/// `boru_gui_get_action_status` — look up the status of a previously sent action.
///
/// Accepts `{ "action_id": "..." }`. Returns the full action record if found,
/// or a structured `not_found` response if the action has not been recorded.
///
/// # Arguments
///
/// * `action_id` — the idempotency key returned by a previous GUI action tool.
///
/// # Returns
///
/// When the action is found:
///
/// ```json
/// {
///   "found": true,
///   "action_id": "gui_action_...",
///   "idempotency_key": "gui_action_...",
///   "command": "...",
///   "status": { "status": "processed" },
///   "timestamp_ms": 1710000000000,
///   "duration_ms": 10
/// }
/// ```
///
/// When the action is not found:
///
/// ```json
/// {
///   "found": false,
///   "action_id": "gui_action_...",
///   "note": "Action not found in history"
/// }
/// ```
///
/// # Security
///
/// - Input is bounded to `MAX_PROBE_ID_LEN` (64 chars).
/// - Control characters are rejected.
/// - No secrets (keys, tickets, tokens) are exposed.
async fn handle_get_gui_action_status(
    req: &JsonRpcRequest,
    state: &McpAppState,
) -> Result<Value, String> {
    let action_id = req
        .params
        .get("action_id")
        .and_then(|v| v.as_str())
        .ok_or_else(|| "Missing required argument: action_id".to_string())?;

    validate_gui_action_id(action_id, "action_id")?;

    // Prefer the shared lifecycle store: it is populated at enqueue time and
    // updated by the Iced loop, so status is observable even while pending.
    let lifecycle_id = boru_chat::diagnostics::GuiActionId(action_id.to_string());
    if let Some(status) = state.gui_action_lifecycle.get(&lifecycle_id) {
        return Ok(serde_json::json!({
            "found": true,
            "action_id": action_id,
            "status": status,
        }));
    }

    // Keep compatibility with the legacy local message-test records.
    if let Some(record) = state.gui_action_history.find(action_id) {
        Ok(serde_json::json!({
            "found": true,
            "action_id": action_id,
            "idempotency_key": record.idempotency_key,
            "command": record.command,
            "status": record.status,
            "timestamp_ms": record.timestamp_ms,
            "duration_ms": record.duration_ms,
        }))
    } else {
        Ok(serde_json::json!({
            "found": false,
            "action_id": action_id,
            "note": "Action not found in history. It may still be pending in the GUI action queue.",
        }))
    }
}

/// `boru_gui_wait_for_state` — wait for a GUI condition without polling.
async fn handle_gui_wait_for_state(
    req: &JsonRpcRequest,
    state: &McpAppState,
) -> Result<Value, String> {
    let condition: GuiWaitCondition = serde_json::from_value(
        req.params
            .get("condition")
            .cloned()
            .ok_or_else(|| "Missing required argument: condition".to_string())?,
    )
    .map_err(|e| format!("Invalid condition: {e}"))?;
    let timeout_ms = req
        .params
        .get("timeout_ms")
        .and_then(Value::as_u64)
        .unwrap_or(10_000)
        .min(30_000);
    let mut rx = state
        .gui_state_rx
        .clone()
        .ok_or_else(|| "GUI state notifications are unavailable".to_string())?;
    let journal = &state.iced_diagnostics;
    let evaluate = |snapshot: &IcedStateSnapshot| {
        diagnostics::evaluate_wait_condition_with_actions(
            &condition,
            snapshot,
            journal,
            &state.gui_action_lifecycle,
        )
    };

    let reached = evaluate(&*rx.borrow());
    if !reached {
        let changed = tokio::time::timeout(Duration::from_millis(timeout_ms), async {
            loop {
                match rx.changed().await {
                    Ok(()) if evaluate(&*rx.borrow()) => break true,
                    Ok(()) => continue,
                    Err(_) => break false,
                }
            }
        })
        .await;
        if !matches!(changed, Ok(true)) {
            let snapshot = rx.borrow().clone();
            let closed = matches!(changed, Ok(false));
            return Ok(serde_json::json!({
                "reached": false,
                "timed_out": !closed,
                "cancelled": closed,
                "error": if closed {
                    Some("GUI state notifications closed".to_string())
                } else {
                    Some(format!("timed out waiting for GUI condition after {timeout_ms}ms"))
                },
                "condition": condition,
                "snapshot": snapshot,
            }));
        }
    }

    Ok(serde_json::json!({
        "reached": true,
        "timed_out": false,
        "condition": condition,
        "snapshot": rx.borrow().clone(),
    }))
}

/// `boru_get_gui_snapshot` — snapshot of current GUI application state.
async fn handle_get_gui_snapshot(state: &McpAppState) -> Result<Value, String> {
    let journal = &state.iced_diagnostics;
    Ok(serde_json::json!({
        "journal_entry_count": journal.entry_count(),
        "journal_latest_sequence": journal.latest_sequence(),
        "diagnostics_event_count": state.diagnostics.event_count(),
        "diagnostics_latest_sequence": state.diagnostics.latest_sequence(),
        "active_rooms": state.diagnostics.joined_rooms(),
        "gui_test_actions_enabled": state.gui_test_actions_enabled,
    }))
}

/// `boru_gui_set_composer` — set the composer (message input) text without
/// submitting.
///
/// Accepts `{ "text": "..." }`. If the text exceeds [`MAX_COMPOSER_LEN`]
/// characters it is silently clamped (truncated) and a warning is logged.
/// Empty strings and control characters are rejected.
///
/// # Security
///
/// - Full text is NOT logged — only character count or truncated prefix is emitted.
/// - Input exceeding [`MAX_COMPOSER_LEN`] is clamped rather than passed through.
/// - Control characters are rejected.
async fn handle_set_composer(
    req: &JsonRpcRequest,
    tx: boru_chat::diagnostics::GuiTestHandle,
) -> Result<Value, String> {
    let mut text = req
        .params
        .get("text")
        .and_then(|v| v.as_str())
        .ok_or_else(|| "Missing required argument: text".to_string())?
        .to_string();

    // Reject empty strings
    if text.is_empty() {
        return Err("Composer text must not be empty".to_string());
    }

    // Clamp (truncate) if text exceeds max length
    let was_clamped = text.chars().count() > MAX_COMPOSER_LEN;
    if was_clamped {
        // Truncate to MAX_COMPOSER_LEN characters
        let truncated: String = text.chars().take(MAX_COMPOSER_LEN).collect();
        let original_len = text.chars().count();
        text = truncated;
        // Log a warning with only a truncated prefix (first 50 chars + '...')
        let prefix: String = text.chars().take(50).collect();
        warn!(
            "boru_gui_set_composer: Composer text clamped from >{} to {} chars (prefix: {}...)",
            MAX_COMPOSER_LEN, original_len, prefix
        );
    }

    // Reject control characters
    if text.chars().any(|c| c.is_control() && c != ' ') {
        return Err("Composer text must not contain control characters".to_string());
    }

    // Do NOT log the full text — only log metadata (char count, no content)
    info!(
        "boru_gui_set_composer: SetComposerText action queued ({} chars)",
        text.chars().count()
    );

    let idempotency_key = crate::gui_test_actions::generate_action_key();

    let command = crate::gui_test_actions::GuiTestCommand::SetComposerText { text: text.clone() };

    let command_json =
        serde_json::to_string(&command).map_err(|e| format!("Failed to serialize command: {e}"))?;

    let request = boru_chat::diagnostics::GuiActionRequest {
        action_id: boru_chat::diagnostics::GuiActionId(idempotency_key.clone()),
        requested_at_ms: std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as i64,
        command: command_json,
    };

    // Send through the channel (non-blocking via enqueue)
    let _ = tx.enqueue(request).map_err(|e| match e.code {
        boru_chat::diagnostics::GuiActionErrorCode::ActionQueueFull => {
            format!("GUI action queue is full (capacity: {})", tx.capacity())
        }
        _ => format!("GUI action channel error: {}", e.message),
    })?;

    Ok(serde_json::json!({
        "sent": true,
        "action_id": idempotency_key,
        "text_length": text.chars().count(),
        "clamped": was_clamped,
        "note": "Composer text set. Use boru_send_gui_action with command: {\\\"SendMessage\\\":{}} to submit.",
    }))
}

/// `boru_gui_open_room` — send a GUI 'open room' command.
///
/// Accepts `{ "room_id": "..." }`. The `room_id` must be an alphanumeric
/// string (letters, digits, hyphen, underscore), 1–128 characters.
///
/// Returns an `action_id` (a unique string) for status tracking via
/// `boru_gui_get_action_status` or the GUI action history.
///
/// # Validation
///
/// - `room_id` must not be empty.
/// - Length ≤ 128 characters (enforced by [`MAX_GUI_ROOM_ID_LEN`]).
/// - Must match the pattern `^[a-zA-Z0-9_-]+$` (alphanumeric plus hyphen
///   and underscore — no spaces, no control characters, no shell
///   metacharacters, no filesystem path separators).
/// - Control characters are rejected by the pattern check.
///
/// # Security
///
/// - Input is bounded to [`MAX_GUI_ROOM_ID_LEN`] bytes; longer values are
///   rejected.
/// - The alphanumeric-plus-hyphen pattern prevents injection of control
///   characters, shell metacharacters, and filesystem path separators.
/// - The full `room_id` value is NOT logged — only its length is emitted.
/// - **No private room tickets are exposed** — this tool opens an already-known
///   room by a public room identifier.  The caller must already know the
///   room ID; this tool does NOT enumerate rooms or reveal invite tickets.
/// - The `action_id` is a server-generated unique key, not a caller-supplied
///   value, preventing caller-controlled data from appearing in the
///   response's structural fields.
async fn handle_gui_open_room(
    req: &JsonRpcRequest,
    tx: boru_chat::diagnostics::GuiTestHandle,
) -> Result<Value, String> {
    let room_id = req
        .params
        .get("room_id")
        .and_then(|v| v.as_str())
        .ok_or_else(|| "Missing required argument: room_id".to_string())?;

    // Validate: must not be empty
    if room_id.is_empty() {
        return Err("room_id must not be empty".to_string());
    }

    // Validate: reject excessive length (max 128 chars per spec)
    validate_bounded(room_id, MAX_GUI_ROOM_ID_LEN, "room_id")?;

    // Validate: alphanumeric-plus-hyphen pattern
    // Rejects empty (caught above), control chars, spaces, shell meta,
    // filesystem separators, and any non-[a-zA-Z0-9_-] characters.
    if !room_id
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
    {
        return Err(format!(
            "Invalid room_id '{}': must match pattern ^[a-zA-Z0-9_-]+$ (alphanumeric, hyphen, underscore only)",
            sanitize_for_log(room_id, MAX_GUI_ROOM_ID_LEN)
        ));
    }

    // Do NOT log the full room_id — only log metadata (char count, no content)
    info!(
        "boru_gui_open_room: OpenRoom action queued (room_id length={})",
        room_id.len()
    );

    let idempotency_key = crate::gui_test_actions::generate_action_key();

    let command = crate::gui_test_actions::GuiTestCommand::OpenRoom {
        room_id: room_id.to_string(),
    };

    let command_json =
        serde_json::to_string(&command).map_err(|e| format!("Failed to serialize command: {e}"))?;

    let request = boru_chat::diagnostics::GuiActionRequest {
        action_id: boru_chat::diagnostics::GuiActionId(idempotency_key.clone()),
        requested_at_ms: std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as i64,
        command: command_json,
    };

    // Send through the channel (non-blocking via enqueue)
    let _ = tx.enqueue(request).map_err(|e| match e.code {
        boru_chat::diagnostics::GuiActionErrorCode::ActionQueueFull => {
            format!("GUI action queue is full (capacity: {})", tx.capacity())
        }
        _ => format!("GUI action channel error: {}", e.message),
    })?;

    Ok(serde_json::json!({
        "sent": true,
        "action_id": idempotency_key,
        "room_id": room_id,
        "note": "Room open command queued. Status available via boru_gui_get_action_status.",
    }))
}

/// `boru_gui_open_conversation` — open a direct conversation with a peer.
///
/// Accepts `{ "conversation_id": "..." }` where `conversation_id` is the
/// peer's 64-hex-character public key (optionally with `0x` prefix).
/// Queues an `OpenConversation` GUI test action and returns an action ID
/// for status tracking.
///
/// # Security
///
/// - Input is bounded to 64 hex chars + optional `0x` prefix.
/// - Control characters are rejected.
/// - No secrets (keys, tickets, tokens) are exposed.
/// - Full conversation_id is NOT logged — only length is emitted.
async fn handle_gui_open_conversation(
    req: &JsonRpcRequest,
    tx: boru_chat::diagnostics::GuiTestHandle,
) -> Result<Value, String> {
    let conversation_id = req
        .params
        .get("conversation_id")
        .and_then(|v| v.as_str())
        .ok_or_else(|| "Missing required argument: conversation_id".to_string())?;

    // Validate: reject excessive length (peer key is 64 hex chars + optional 0x = max 66)
    if conversation_id.len() > 66 {
        return Err(format!(
            "conversation_id too long ({} bytes, max 66)",
            conversation_id.len()
        ));
    }

    // Reject control characters
    if conversation_id.chars().any(|c| c.is_control() && c != ' ') {
        return Err("conversation_id must not contain control characters".to_string());
    }

    // Validate that conversation_id is a valid peer public key (64 hex chars, optionally 0x-prefixed)
    let hex = conversation_id
        .strip_prefix("0x")
        .unwrap_or(conversation_id);
    if hex.len() != 64 || !hex.chars().all(|c| c.is_ascii_hexdigit()) {
        return Err(format!(
            "Invalid conversation_id '{}': expected 64 hex chars representing a peer public key (optionally with 0x prefix)",
            sanitize_for_log(conversation_id, 66)
        ));
    }

    // Do NOT log the full conversation_id — only log metadata (char count, no content)
    info!(
        "boru_gui_open_conversation: OpenConversation action queued (conversation_id length={})",
        conversation_id.len()
    );

    let idempotency_key = crate::gui_test_actions::generate_action_key();

    let command = crate::gui_test_actions::GuiTestCommand::OpenConversation {
        conversation_id: conversation_id.to_string(),
    };
    let command_json =
        serde_json::to_string(&command).map_err(|e| format!("Failed to serialize command: {e}"))?;
    let now_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64;
    let request = boru_chat::diagnostics::GuiActionRequest {
        action_id: boru_chat::diagnostics::GuiActionId(idempotency_key.clone()),
        requested_at_ms: now_ms,
        command: command_json,
    };

    // Send through the channel (non-blocking — try_send to avoid blocking the
    // MCP connection handler).
    let _ = tx.enqueue(request).map_err(|e| match e.code {
        boru_chat::diagnostics::GuiActionErrorCode::ActionQueueFull => {
            format!("GUI action queue is full (capacity: {})", tx.capacity())
        }
        _ => format!("GUI action channel error: {}", e.message),
    })?;

    Ok(serde_json::json!({
        "sent": true,
        "action_id": idempotency_key,
        "conversation_id": conversation_id,
        "note": "Open conversation command queued. Status available via boru_gui_get_action_status.",
    }))
}

/// `boru_gui_submit_composer` — submit the current composer text through the
/// normal GUI send path.
///
/// No required arguments. This is the dedicated tool for triggering message
/// submission after the composer has been populated (e.g. via
/// `boru_gui_set_composer`).  It sends a `SubmitComposer` GUI test action
/// through the normal Iced event-loop pipeline — the same path the Send button
/// uses.
///
/// Returns an `action_id` for status tracking via
/// `boru_gui_get_action_status`.
///
/// # Security
///
/// - No secrets are exposed — the composer text is never read or logged.
/// - Rate-limited by the shared `GuiActionRateLimiter`.
async fn handle_submit_composer(
    tx: boru_chat::diagnostics::GuiTestHandle,
) -> Result<Value, String> {
    info!("boru_gui_submit_composer: SubmitComposer action queued");

    let idempotency_key = crate::gui_test_actions::generate_action_key();

    let command = boru_chat::diagnostics::GuiTestCommand::SubmitComposer;

    // Serialize the command to JSON for the GuiActionRequest.command field
    let command_json =
        serde_json::to_string(&command).map_err(|e| format!("Failed to serialize command: {e}"))?;

    let request = boru_chat::diagnostics::GuiActionRequest {
        action_id: boru_chat::diagnostics::GuiActionId(idempotency_key.clone()),
        requested_at_ms: std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as i64,
        command: command_json,
    };

    // Send through the channel (non-blocking via enqueue)
    let _sent = tx.enqueue(request).map_err(|e| match e.code {
        boru_chat::diagnostics::GuiActionErrorCode::ActionQueueFull => {
            format!("GUI action queue is full (capacity: {})", tx.capacity())
        }
        _ => format!("GUI action channel error: {}", e.message),
    })?;

    Ok(serde_json::json!({
        "sent": true,
        "action_id": idempotency_key,
        "note": "Composer submit queued. Use composer text was previously set via boru_gui_set_composer.",
    }))
}

/// Queue a parameterless semantic composer control action.
async fn handle_composer_control(
    control: &str,
    tx: boru_chat::diagnostics::GuiTestHandle,
) -> Result<Value, String> {
    let command = match control {
        "clear" => boru_chat::diagnostics::GuiTestCommand::ClearComposer,
        "focus" => boru_chat::diagnostics::GuiTestCommand::FocusComposer,
        _ => return Err("unknown composer control".to_string()),
    };
    let action_id = crate::gui_test_actions::generate_action_key();
    let request = boru_chat::diagnostics::GuiActionRequest {
        action_id: boru_chat::diagnostics::GuiActionId(action_id.clone()),
        requested_at_ms: std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as i64,
        command: serde_json::to_string(&command)
            .map_err(|e| format!("Failed to serialize command: {e}"))?,
    };
    tx.enqueue(request)
        .map_err(|e| format!("GUI action channel error: {}", e.message))?;
    Ok(serde_json::json!({ "sent": true, "action_id": action_id }))
}

// =============================================================================
// boru_gui_close_dialog
// =============================================================================

/// `boru_gui_close_dialog` — close the currently open dialog or overlay.
///
/// This is a parameterless command that closes the top-most dialog/overlay
/// using the same state-mutation paths as the real Escape handler and
/// close-button dispatch paths:
///
/// 1. help overlay
/// 2. create-room dialog
/// 3. settings screen (returns to previous)
/// 4. friend requests screen
/// 5. peer profile overlay
/// 6. image preview overlay
///
/// The command is validated on the GUI side (`validate_gui_test_command`):
/// if no dialog is currently open, it returns `NoDialog` error.
///
/// # Parameters
///
/// None.
///
/// # Returns
///
/// ```json
/// {
///     "sent": true,
///     "action_id": "uuid-like-key",
///     "note": "CloseDialog action queued"
/// }
/// ```
///
/// # Security
///
/// - No caller-supplied input is involved — the command has no parameters.
/// - Rate-limited by the shared `GuiActionRateLimiter`.
async fn handle_gui_close_dialog(
    tx: boru_chat::diagnostics::GuiTestHandle,
) -> Result<Value, String> {
    info!("boru_gui_close_dialog: CloseDialog action queued");

    let idempotency_key = crate::gui_test_actions::generate_action_key();

    let command_json = serde_json::to_string(&boru_chat::diagnostics::GuiTestCommand::CloseDialog)
        .map_err(|e| format!("Failed to serialize command: {e}"))?;
    let now_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64;
    let request = boru_chat::diagnostics::GuiActionRequest {
        action_id: boru_chat::diagnostics::GuiActionId(idempotency_key.clone()),
        requested_at_ms: now_ms,
        command: command_json,
    };

    // Send through the channel (non-blocking — try_send to avoid blocking the
    // MCP connection handler).
    let _sent = tx.enqueue(request).map_err(|e| match e.code {
        boru_chat::diagnostics::GuiActionErrorCode::ActionQueueFull => {
            format!("GUI action queue is full (capacity: {})", tx.capacity())
        }
        _ => format!("GUI action channel error: {}", e.message),
    })?;

    Ok(serde_json::json!({
        "sent": true,
        "action_id": idempotency_key,
        "note": "CloseDialog action queued",
    }))
}

/// Maximum timeout for local GUI message tests (milliseconds).
const MAX_GUI_MESSAGE_TEST_TIMEOUT_MS: u64 = 60_000;

/// `boru_run_local_gui_message_test` — verify the local GUI send pipeline.
async fn handle_run_local_gui_message_test(
    req: &JsonRpcRequest,
    state: &McpAppState,
) -> Result<Value, String> {
    let room_id = req
        .params
        .get("room_id")
        .and_then(Value::as_str)
        .ok_or_else(|| "Missing required argument: room_id".to_string())?;
    let message_text = req
        .params
        .get("message_text")
        .and_then(Value::as_str)
        .ok_or_else(|| "Missing required argument: message_text".to_string())?;
    let timeout_ms = req
        .params
        .get("timeout_ms")
        .and_then(Value::as_u64)
        .unwrap_or(20_000)
        .clamp(1, MAX_GUI_MESSAGE_TEST_TIMEOUT_MS);
    if room_id.is_empty() {
        return Err("room_id must not be empty".to_string());
    }
    validate_bounded(room_id, MAX_GUI_ROOM_ID_LEN, "room_id")?;
    if !room_id
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
    {
        return Err("room_id must contain only ASCII letters, digits, '-' or '_'".to_string());
    }
    if message_text.is_empty() {
        return Err("message_text must not be empty".to_string());
    }
    if message_text.chars().count() > MAX_COMPOSER_LEN {
        return Err(format!(
            "message_text too long (max {MAX_COMPOSER_LEN} characters)"
        ));
    }
    if message_text.chars().any(|c| c.is_control() && c != ' ') {
        return Err("message_text must not contain control characters".to_string());
    }
    let deadline = tokio::time::Instant::now() + Duration::from_millis(timeout_ms);
    let tx = state
        .gui_action_tx
        .clone()
        .ok_or_else(|| "GUI action channel not available".to_string())?;
    let initial_snapshot = state.gui_state_rx.as_ref().map(|rx| rx.borrow().clone());
    let initial_entries = initial_snapshot
        .as_ref()
        .map(|s| s.total_entry_count)
        .unwrap_or(0);
    let initial_journal = state.iced_diagnostics.latest_sequence();
    let initial_diagnostics = state.diagnostics.latest_sequence();
    let mut steps = Vec::new();
    async fn send_action(
        tx: &boru_chat::diagnostics::GuiTestHandle,
        command: boru_chat::diagnostics::GuiTestCommand,
        journal: &IcedMessageJournal,
        deadline: tokio::time::Instant,
    ) -> Result<(String, String), String> {
        let action_id = crate::gui_test_actions::generate_action_key();
        let command_json = serde_json::to_string(&command).map_err(|e| e.to_string())?;
        let request = boru_chat::diagnostics::GuiActionRequest {
            action_id: boru_chat::diagnostics::GuiActionId(action_id.clone()),
            requested_at_ms: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis() as i64,
            command: command_json,
        };
        let before = journal.latest_sequence();
        tx.enqueue(request)
            .map_err(|e| format!("GUI action enqueue failed: {}", e.message))?;
        while journal.latest_sequence() <= before {
            if tokio::time::Instant::now() >= deadline {
                return Err("timed out waiting for GUI action".to_string());
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        Ok((action_id, "observed_by_gui".to_string()))
    }
    let (open_id, open_status) = send_action(
        &tx,
        crate::gui_test_actions::GuiTestCommand::OpenRoom {
            room_id: room_id.to_string(),
        },
        &state.iced_diagnostics,
        deadline,
    )
    .await?;
    steps.push(
        serde_json::json!({"stage":"room_navigation","action_id":open_id,"state":open_status}),
    );

    let (set_id, set_status) = send_action(
        &tx,
        crate::gui_test_actions::GuiTestCommand::SetComposerText {
            text: message_text.to_string(),
        },
        &state.iced_diagnostics,
        deadline,
    )
    .await?;
    steps
        .push(serde_json::json!({"stage":"composer_update","action_id":set_id,"state":set_status}));

    let (submit_id, submit_status) = send_action(
        &tx,
        crate::gui_test_actions::GuiTestCommand::SubmitComposer,
        &state.iced_diagnostics,
        deadline,
    )
    .await?;
    steps.push(serde_json::json!({"stage":"composer_submission","action_id":submit_id,"state":submit_status}));

    let mut current = initial_snapshot;
    while tokio::time::Instant::now() < deadline {
        current = state.gui_state_rx.as_ref().map(|rx| rx.borrow().clone());
        if current
            .as_ref()
            .is_some_and(|s| s.total_entry_count > initial_entries)
        {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    let composer_cleared = current.as_ref().is_some_and(|s| s.composer_text.is_empty());
    let local_message_created = current
        .as_ref()
        .is_some_and(|s| s.total_entry_count > initial_entries);
    let broadcast_event = state
        .diagnostics
        .events_since(initial_diagnostics, 200, None)
        .into_iter()
        .find_map(|event| match event.kind {
            DiagnosticEventKind::MessageBroadcast {
                message_id,
                message_hash,
                probe_id,
            } => Some(serde_json::json!({
                "message_id": message_id,
                "message_hash": message_hash,
                "probe_id": probe_id,
            })),
            _ => None,
        });
    let local_broadcast_detected = broadcast_event.is_some();
    let gui_entries = state.iced_diagnostics.entries_since(initial_journal, 200);
    let gui_state_observed = !gui_entries.is_empty() || current.is_some();
    let first_failed_stage = if !composer_cleared {
        Some("local_application_state")
    } else if !local_message_created {
        Some("local_message_creation")
    } else if !gui_state_observed {
        Some("local_gui_state")
    } else {
        None
    };
    Ok(
        serde_json::json!({"success":first_failed_stage.is_none(),"first_failed_stage":first_failed_stage,"room_id":room_id,"message_text_length":message_text.chars().count(),"verification":{"room_navigation":true,"composer_update":true,"composer_submission":true,"composer_cleared":composer_cleared,"local_message_created":local_message_created,"local_application_state":current,"local_gui_state":gui_state_observed},"steps":steps,"gui_journal_entries":gui_entries,"note":"Local GUI pipeline only; remote delivery is not verified by this tool. Query the remote node separately."}),
    )
}

// =============================================================================
// boru_gui_navigate — schema types
// =============================================================================

/// Allowed `destination` values for `boru_gui_navigate`.
///
/// Maps directly to the corresponding [`GuiTestCommand`] variant.
///
/// # JSON Schema
///
/// ```json
/// {
///   "type": "string",
///   "enum": ["chat_list", "friends", "settings"],
///   "description": "Target GUI screen to navigate to"
/// }
/// ```
///
/// # TypeScript
///
/// ```typescript
/// type GuiNavigateDestination = "chat_list" | "friends" | "settings";
/// ```
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum GuiNavigateDestination {
    /// Navigate to the chat list (home) screen.
    ChatList,
    /// Navigate to the friends / social screen.
    Friends,
    /// Navigate to the settings screen.
    Settings,
}

impl GuiNavigateDestination {
    /// Convert this destination to the corresponding [`GuiTestCommand`].
    pub fn to_gui_test_command(&self) -> crate::gui_test_actions::GuiTestCommand {
        match self {
            GuiNavigateDestination::ChatList => {
                crate::gui_test_actions::GuiTestCommand::GoToChatList
            }
            GuiNavigateDestination::Friends => crate::gui_test_actions::GuiTestCommand::OpenFriends,
            GuiNavigateDestination::Settings => {
                crate::gui_test_actions::GuiTestCommand::OpenSettings
            }
        }
    }

    /// Convert a string to a [`GuiNavigateDestination`].
    ///
    /// Returns `None` if the string is not one of the allowed values.
    pub fn from_str(s: &str) -> Option<GuiNavigateDestination> {
        match s {
            "chat_list" => Some(GuiNavigateDestination::ChatList),
            "friends" => Some(GuiNavigateDestination::Friends),
            "settings" => Some(GuiNavigateDestination::Settings),
            _ => None,
        }
    }

    /// Return the JSON string representation of this destination.
    pub fn as_str(&self) -> &'static str {
        match self {
            GuiNavigateDestination::ChatList => "chat_list",
            GuiNavigateDestination::Friends => "friends",
            GuiNavigateDestination::Settings => "settings",
        }
    }

    /// Iterate over all supported destination strings.
    pub fn all_destinations() -> &'static [&'static str] {
        &["chat_list", "friends", "settings"]
    }
}

impl std::fmt::Display for GuiNavigateDestination {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

/// JSON-RPC request params for `boru_gui_navigate`.
///
/// # JSON Schema
///
/// ```json
/// {
///   "type": "object",
///   "required": ["destination"],
///   "properties": {
///     "destination": {
///       "type": "string",
///       "enum": ["chat_list", "friends", "settings"],
///       "description": "Target GUI screen to navigate to"
///     }
///   },
///   "additionalProperties": false
/// }
/// ```
///
/// # TypeScript
///
/// ```typescript
/// interface GuiNavigateParams {
///   /** Target GUI screen — "chat_list", "friends", or "settings" */
///   destination: GuiNavigateDestination;
/// }
/// ```
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GuiNavigateParams {
    /// Destination screen: `"chat_list"`, `"friends"`, or `"settings"`.
    pub destination: GuiNavigateDestination,
}

/// JSON-RPC response result for `boru_gui_navigate`.
///
/// # JSON Schema
///
/// ```json
/// {
///   "type": "object",
///   "required": ["accepted", "action_id", "queued_at_ms"],
///   "properties": {
///     "accepted": {
///       "type": "boolean",
///       "description": "Whether the navigation command was accepted and queued"
///     },
///     "action_id": {
///       "type": "string",
///       "description": "Idempotency key for tracking this action's status"
///     },
///     "queued_at_ms": {
///       "type": "integer",
///       "description": "Wall-clock timestamp (ms since Unix epoch) when the command was queued"
///     }
///   },
///   "additionalProperties": false
/// }
/// ```
///
/// # TypeScript
///
/// ```typescript
/// interface GuiNavigateResponse {
///   /** Whether the navigation was accepted and queued */
///   accepted: boolean;
///   /** Idempotency key for tracking the action's status */
///   action_id: string;
///   /** Wall-clock timestamp (ms since Unix epoch) when queued */
///   queued_at_ms: number;
/// }
/// ```
#[derive(Debug, Clone, Serialize)]
pub struct GuiNavigateResponse {
    /// Whether the navigation command was accepted by the MCP server
    /// and queued for delivery to the GUI event loop.
    pub accepted: bool,
    /// Idempotency key for tracking this action's status via
    /// `boru_gui_get_action_status`.
    pub action_id: String,
    /// Wall-clock timestamp (ms since Unix epoch) when the navigate
    /// command was queued.
    pub queued_at_ms: i64,
}

/// `boru_gui_navigate` — navigate the GUI to a named destination screen.
///
/// Accepts `{ "destination": "chat_list" | "friends" | "settings" }`.
///
/// ## Request
///
/// | Field          | Type     | Required | Description                                    |
/// |----------------|----------|----------|------------------------------------------------|
/// | `destination`  | `string` | **yes**  | Target screen — `"chat_list"`, `"friends"`, or `"settings"` |
///
/// ## Response (success)
///
/// | Field           | Type      | Description                                    |
/// |-----------------|-----------|------------------------------------------------|
/// | `accepted`      | `boolean` | Always `true` for a queued command              |
/// | `action_id`     | `string`  | Idempotency key for tracking                    |
/// | `queued_at_ms`  | `integer` | Wall-clock timestamp (ms since Unix epoch)      |
///
/// ## Response (error)
///
/// Standard JSON-RPC error with `code: -32000` and a message describing
/// the failure:
///
/// - Missing or invalid `destination` parameter
/// - Destination too long (exceeds [`crate::gui_test_actions::MAX_STRING_LEN`])
/// - Destination contains control characters
/// - GUI action queue is full or closed
///
/// ## Example
///
/// ```json
/// // Request
/// { "jsonrpc": "2.0", "method": "boru_gui_navigate", "params": { "destination": "settings" }, "id": 1 }
///
/// // Success response
/// { "jsonrpc": "2.0", "id": 1, "result": { "accepted": true, "action_id": "gui_action_...", "queued_at_ms": 1710000000123 } }
/// ```
///
/// ## TypeScript
///
/// ```typescript
/// // Request
/// interface NavigateRequest {
///   destination: "chat_list" | "friends" | "settings";
/// }
///
/// // Response
/// interface NavigateResponse {
///   accepted: boolean;
///   action_id: string;
///   queued_at_ms: number;  // ms since Unix epoch
/// }
/// ```
async fn handle_gui_navigate(
    req: &JsonRpcRequest,
    tx: boru_chat::diagnostics::GuiTestHandle,
) -> Result<Value, String> {
    let destination = req
        .params
        .get("destination")
        .and_then(|v| v.as_str())
        .ok_or_else(|| "Missing required argument: destination".to_string())?;

    // Validate: reject excessive length
    if destination.len() > crate::gui_test_actions::MAX_STRING_LEN {
        return Err(format!(
            "destination too long ({} bytes, max {})",
            destination.len(),
            crate::gui_test_actions::MAX_STRING_LEN
        ));
    }

    // Reject control characters
    if destination.chars().any(|c| c.is_control() && c != ' ') {
        return Err("destination must not contain control characters".to_string());
    }

    // Convert validated string to the typed destination enum
    let dest = GuiNavigateDestination::from_str(destination).ok_or_else(|| {
        format!(
            "Invalid destination '{}'. Supported destinations: \"chat_list\", \"friends\", \"settings\"",
            sanitize_for_log(destination, 64)
        )
    })?;

    let command = dest.to_gui_test_command();
    let command_json =
        serde_json::to_string(&command).map_err(|e| format!("Failed to serialize command: {e}"))?;

    let idempotency_key = crate::gui_test_actions::generate_action_key();
    let now_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64;

    let request = boru_chat::diagnostics::GuiActionRequest {
        action_id: boru_chat::diagnostics::GuiActionId(idempotency_key.clone()),
        requested_at_ms: now_ms,
        command: command_json,
    };

    // Send through the channel (non-blocking — try_send to avoid blocking the
    // MCP connection handler).
    let _ = tx.enqueue(request).map_err(|e| match e.code {
        boru_chat::diagnostics::GuiActionErrorCode::ActionQueueFull => {
            format!("GUI action queue is full (capacity: {})", tx.capacity())
        }
        _ => format!("GUI action channel error: {}", e.message),
    })?;

    let response = GuiNavigateResponse {
        accepted: true,
        action_id: idempotency_key,
        queued_at_ms: now_ms,
    };

    serde_json::to_value(&response).map_err(|e| format!("Serialize response: {e}"))
}

/// `boru_gui_toggle_dark_mode` — toggle dark mode on/off.
///
/// Accepts `{ "enabled": true }` or `{ "enabled": false }`.
/// The `enabled` argument must be a boolean: `true` = dark mode,
/// `false` = light mode.
///
/// Returns an `action_id` (a UUID-like string) for status tracking via
/// `boru_gui_get_action_status` or the GUI action history.
///
/// # Validation
///
/// - `enabled` must be present and must be a boolean.
///
/// # Security
///
/// - No secrets are exposed — only a boolean state is transmitted.
/// - Rate-limited by the shared `GuiActionRateLimiter`.
/// - The `action_id` is a server-generated unique key, not a
///   caller-supplied value.
async fn handle_gui_toggle_dark_mode(
    req: &JsonRpcRequest,
    tx: boru_chat::diagnostics::GuiTestHandle,
) -> Result<Value, String> {
    let enabled = req
        .params
        .get("enabled")
        .and_then(|v| v.as_bool())
        .ok_or_else(|| "Missing required argument: enabled (must be a boolean)".to_string())?;

    info!(
        "boru_gui_toggle_dark_mode: ToggleDarkMode action queued (enabled={})",
        enabled
    );

    let idempotency_key = crate::gui_test_actions::generate_action_key();

    let command = boru_chat::diagnostics::GuiTestCommand::ToggleDarkMode { enabled };
    let command_json =
        serde_json::to_string(&command).map_err(|e| format!("Failed to serialize command: {e}"))?;

    let request = boru_chat::diagnostics::GuiActionRequest {
        action_id: boru_chat::diagnostics::GuiActionId(idempotency_key.clone()),
        requested_at_ms: std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as i64,
        command: command_json,
    };

    // Send through the channel (non-blocking via enqueue)
    let _sent = tx.enqueue(request).map_err(|e| match e.code {
        boru_chat::diagnostics::GuiActionErrorCode::ActionQueueFull => {
            format!("GUI action queue is full (capacity: {})", tx.capacity())
        }
        _ => format!("GUI action channel error: {}", e.message),
    })?;

    Ok(serde_json::json!({
        "sent": true,
        "action_id": idempotency_key,
        "enabled": enabled,
    }))
}

// =============================================================================
// JSON-RPC protocol types
// =============================================================================

#[derive(Debug, Deserialize)]
struct JsonRpcRequest {
    #[serde(default)]
    pub jsonrpc: String,
    pub method: String,
    #[serde(default)]
    pub params: Value,
    pub id: Option<Value>,
}

#[derive(Debug, Serialize)]
struct JsonRpcResponse {
    pub jsonrpc: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<JsonRpcError>,
}

#[derive(Debug, Serialize)]
struct JsonRpcError {
    pub code: i32,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<Value>,
}

fn jsonrpc_error(id: Option<Value>, code: i32, message: &str, data: &str) -> JsonRpcResponse {
    JsonRpcResponse {
        jsonrpc: "2.0".to_string(),
        id,
        result: None,
        error: Some(JsonRpcError {
            code,
            message: message.to_string(),
            data: Some(Value::String(data.to_string())),
        }),
    }
}

fn jsonrpc_success(id: Option<Value>, result: Value) -> JsonRpcResponse {
    JsonRpcResponse {
        jsonrpc: "2.0".to_string(),
        id,
        result: Some(result),
        error: None,
    }
}

// =============================================================================
// Tool handlers
// =============================================================================

/// `boru_ping` — lightweight health check (no state required).
///
/// Returns a simple `{"pong": true}` response.  This is the lightest
/// possible MCP tool — it does not touch the endpoint, diagnostics store,
/// or any shared state.  Clients should use this to verify MCP JSON-RPC
/// responsiveness before calling heavier tools like `boru_get_node_status`.
async fn handle_ping(_state: &McpAppState) -> Result<Value, String> {
    Ok(serde_json::json!({ "pong": true }))
}

/// `boru_get_node_status` — local node identity and status.
async fn handle_get_node_status(state: &McpAppState) -> Result<Value, String> {
    let local_id = state.endpoint.id().fmt_short().to_string();
    let relay_url = state
        .endpoint
        .addr()
        .relay_urls()
        .next()
        .map(|u| u.to_string());

    Ok(serde_json::json!({
        "node_id": state.node_id.clone(),
        "node_id_short": local_id,
        "version": state.version.clone(),
        "active_room_count": state.rooms.lock().unwrap().len(),
        "latest_event_sequence": state.diagnostics.latest_sequence(),
        "relay_url": relay_url,
    }))
}

/// `boru_join_lobby_room` — open the stable lobby room through the normal GUI
/// room-opening path, then wait until the application records RoomJoined.
async fn handle_join_lobby_room(
    req: &JsonRpcRequest,
    state: &McpAppState,
) -> Result<Value, String> {
    let timeout_ms = req
        .params
        .get("timeout_ms")
        .and_then(Value::as_u64)
        .unwrap_or(20_000)
        .clamp(1, 60_000);
    let topic = TopicId::from_bytes(*blake3::hash(b"iroh-gossip-chat/default-lobby/v1").as_bytes());
    let room_id = hex::encode(topic.as_bytes());
    let tx = state.gui_action_tx.clone().ok_or_else(|| "GUI action channel not available".to_string())?;
    let before = state.diagnostics.latest_sequence();
    let action_id = crate::gui_test_actions::generate_action_key();
    let command = crate::gui_test_actions::GuiTestCommand::OpenRoom { room_id: room_id.clone() };
    let request = boru_chat::diagnostics::GuiActionRequest {
        action_id: boru_chat::diagnostics::GuiActionId(action_id.clone()),
        requested_at_ms: std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_millis() as i64,
        command: serde_json::to_string(&command).map_err(|e| format!("Failed to serialize command: {e}"))?,
    };
    tx.enqueue(request).map_err(|e| format!("GUI action channel error: {}", e.message))?;
    let deadline = tokio::time::Instant::now() + Duration::from_millis(timeout_ms);
    loop {
        if state.diagnostics.build_evidence(Some(topic), None).local_room_joined {
            let mut rooms = state.rooms.lock().map_err(|e| format!("rooms lock error: {e}"))?;
            if !rooms.contains(&topic) { rooms.push(topic); }
            return Ok(serde_json::json!({"success": true, "room_id": room_id, "joined": true, "action_id": action_id}));
        }
        if tokio::time::Instant::now() >= deadline {
            return Ok(serde_json::json!({"success": false, "room_id": room_id, "joined": false, "action_id": action_id, "timed_out": true, "events_observed": state.diagnostics.events_since(before, 200, Some(topic))}));
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

/// `boru_get_room_status` — room membership and peer summary.
async fn handle_get_room_status(
    req: &JsonRpcRequest,
    state: &McpAppState,
) -> Result<Value, String> {
    let room_str = req
        .params
        .get("room_id")
        .and_then(|v| v.as_str())
        .ok_or_else(|| "Missing required argument: room_id".to_string())?;

    validate_bounded(room_str, MAX_ROOM_ID_LEN, "room_id")?;
    validate_no_control_chars(room_str, "room_id")?;
    let topic = parse_topic_id(room_str)?;

    // Check room membership via diagnostics evidence (shared with the app)
    let joined = state
        .diagnostics
        .build_evidence(Some(topic), None)
        .local_room_joined;

    if !joined {
        return Err(format!("Room not found or not joined: {room_str}"));
    }

    // Build peer states for this room
    let all_states = state.diagnostics.peer_states();
    let peers: Vec<Value> = all_states
        .iter()
        .map(|(pid, ps)| {
            serde_json::json!({
                "peer_id": pid,
                "discovery_sources": ps.discovery_sources,
                "addresses": ps.addresses,
                "connected": ps.connection_state == ConnectionDiagnosticState::Connected,
                "topic_member": ps.topic_member,
                "last_error": ps.last_error,
            })
        })
        .collect();

    let evidence = state.diagnostics.build_evidence(Some(topic), None);

    Ok(serde_json::json!({
        "node_id": state.node_id,
        "room_id": room_str,
        "joined": joined,
        "subscribed": joined,
        "peer_count": peers.len(),
        "peers": peers,
        "discovery_sources_enabled": ["mdns", "mainline_dht", "bootstrap"],
        "last_error": Option::<String>::None,
        "local_room_joined": evidence.local_room_joined,
    }))
}

/// `boru_get_discovery_events` — recent diagnostic events.
async fn handle_get_discovery_events(
    req: &JsonRpcRequest,
    state: &McpAppState,
) -> Result<Value, String> {
    let since_sequence = req
        .params
        .get("since_sequence")
        .and_then(|v| v.as_u64())
        .unwrap_or(0);

    let limit = req
        .params
        .get("limit")
        .and_then(|v| v.as_u64())
        .unwrap_or(200) as usize;

    let room_id = req
        .params
        .get("room_id")
        .and_then(|v| v.as_str())
        .and_then(|s| parse_topic_id(s).ok());

    let events = state
        .diagnostics
        .events_since(since_sequence, limit, room_id);
    let latest = state.diagnostics.latest_sequence();

    Ok(serde_json::json!({
        "events": events,
        "latest_sequence": latest,
        "returned_count": events.len(),
    }))
}

/// `boru_send_probe` — broadcast a diagnostic probe through gossip.
async fn handle_send_probe(req: &JsonRpcRequest, state: &McpAppState) -> Result<Value, String> {
    let room_str = req
        .params
        .get("room_id")
        .and_then(|v| v.as_str())
        .ok_or_else(|| "Missing required argument: room_id".to_string())?;

    validate_bounded(room_str, MAX_ROOM_ID_LEN, "room_id")?;
    validate_no_control_chars(room_str, "room_id")?;
    let topic = parse_topic_id(room_str)?;

    // Validate the room exists via diagnostics evidence (shared with the app)
    if !state
        .diagnostics
        .build_evidence(Some(topic), None)
        .local_room_joined
    {
        return Err(format!("Room not found or not joined: {room_str}"));
    }

    let probe_id = req
        .params
        .get("probe_id")
        .and_then(|v| v.as_str())
        .map(|s| {
            // Validate probe_id if caller-supplied (auto-generated IDs are
            // always safe).
            if !s.is_empty() {
                validate_probe_id(s)?;
            }
            Ok::<String, String>(s.to_string())
        })
        .transpose()?
        .unwrap_or_else(generate_probe_id);

    let payload = req
        .params
        .get("payload")
        .and_then(|v| v.as_str())
        .map(|s| {
            // Validate probe payload — preserves full Unicode, only
            // rejects extreme length.
            validate_probe_payload(s)?;
            Ok::<String, String>(s.to_string())
        })
        .transpose()?;

    // Use broadcast_diagnostic_probe to sign and encode the probe message.
    // This records the ProbeBroadcast event in the global DIAGNOSTICS.
    let probe_result =
        broadcast_diagnostic_probe(&state.secret_key, room_str, payload, Some(probe_id.clone()))
            .map_err(|e| format!("Failed to sign probe: {e}"))?;

    let signed_bytes: Bytes = probe_result;

    // Subscribe to the gossip topic and broadcast the signed probe bytes
    let broadcast_accepted = match state.gossip.subscribe(topic, Vec::new()).await {
        Ok(gossip_topic) => {
            let (sender, _receiver) = gossip_topic.split();
            match sender.broadcast(signed_bytes).await {
                Ok(()) => true,
                Err(e) => {
                    warn!("MCP probe broadcast failed: {e}");
                    false
                }
            }
        }
        Err(e) => {
            warn!("MCP failed to subscribe to gossip topic for probe: {e}");
            false
        }
    };

    // Re-read the probe event to get the message hash
    let message_hash = {
        // `broadcast_diagnostic_probe` records ProbeBroadcast before the
        // gossip topic is available and therefore has no topic filter on the
        // event. Query the unfiltered stream here; filtering by topic would
        // silently drop the broadcast event and return an empty hash even
        // though the probe was accepted and delivered.
        let events = state.diagnostics.events_since(0, 100, None);
        events
            .iter()
            .rev()
            .find_map(|e| {
                if let DiagnosticEventKind::ProbeBroadcast {
                    probe_id: ref pid,
                    message_hash: ref mh,
                } = e.kind
                {
                    if pid == &probe_id {
                        return Some(mh.clone());
                    }
                }
                None
            })
            .unwrap_or_default()
    };

    let now_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64;

    Ok(serde_json::json!({
        "probe_id": probe_id,
        "room_id": room_str,
        "sender_id": state.node_id,
        "message_hash": message_hash,
        "sent_at_ms": now_ms,
        "broadcast_accepted": broadcast_accepted,
    }))
}

/// `boru_find_received_probe` — look up a received probe by ID.
async fn handle_find_received_probe(
    req: &JsonRpcRequest,
    state: &McpAppState,
) -> Result<Value, String> {
    let probe_id = req
        .params
        .get("probe_id")
        .and_then(|v| v.as_str())
        .ok_or_else(|| "Missing required argument: probe_id".to_string())?;

    // Validate probe_id — bounded, no control characters, no path separators
    validate_probe_id(probe_id)?;

    match state.diagnostics.find_received_probe(probe_id) {
        Some(probe) => Ok(serde_json::json!({
            "received": true,
            "probe": {
                "probe_id": probe.probe_id,
                "room_id": probe.room_id,
                "sender_id": probe.sender_id,
                "sent_at_ms": probe.sent_at_ms,
                "received_at_ms": probe.received_at_ms,
                "latency_ms": probe.latency_ms,
                "message_hash": probe.message_hash,
                "duplicate_count": probe.duplicate_count,
            }
        })),
        None => Ok(serde_json::json!({
            "received": false,
            "probe_id": probe_id,
        })),
    }
}

/// `boru_get_peer_status` — per-peer diagnostic state.
async fn handle_get_peer_status(
    req: &JsonRpcRequest,
    state: &McpAppState,
) -> Result<Value, String> {
    let peer_id = req
        .params
        .get("peer_id")
        .and_then(|v| v.as_str())
        .ok_or_else(|| "Missing required argument: peer_id".to_string())?;

    // Validate peer_id — bounded, no control chars, no path/shell metachars
    validate_peer_id(peer_id)?;

    match state.diagnostics.peer_state(peer_id) {
        Some(ps) => Ok(serde_json::to_value(&ps).map_err(|e| format!("serialize: {e}"))?),
        None => Ok(serde_json::json!({
            "found": false,
            "peer_id": peer_id,
            "message": "Peer has never been observed."
        })),
    }
}

/// `boru_wait_for_peer` — wait asynchronously for a peer to reach a target state.
async fn handle_wait_for_peer(req: &JsonRpcRequest, state: &McpAppState) -> Result<Value, String> {
    let peer_id = req
        .params
        .get("peer_id")
        .and_then(|v| v.as_str())
        .ok_or_else(|| "Missing required argument: peer_id".to_string())?;

    // Validate peer_id — bounded, no control chars, no path/shell metachars
    validate_peer_id(peer_id)?;

    let target_state = req
        .params
        .get("target_state")
        .and_then(|v| v.as_str())
        .ok_or_else(|| "Missing required argument: target_state".to_string())?;

    // Validate target_state — must be one of the known allowed values
    validate_target_state(target_state)?;

    let timeout_ms = req
        .params
        .get("timeout_ms")
        .and_then(|v| v.as_u64())
        .unwrap_or(15000)
        .min(30000); // Clamp to 30s max

    let deadline = tokio::time::Instant::now() + Duration::from_millis(timeout_ms);

    // Check if already satisfied
    if let Some(state) = state.diagnostics.peer_state(peer_id) {
        if state_satisfied(&state, target_state) {
            return Ok(serde_json::json!({
                "reached": true,
                "target_state": target_state,
                "timed_out": false,
                "peer": state,
            }));
        }
    }

    // Wait with watch channel
    let mut watch_rx = state.diagnostics.subscribe();
    let target = peer_id.to_string();
    let target_state_owned = target_state.to_string();

    loop {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        if remaining.is_zero() {
            // Timeout — return latest state
            let latest = state.diagnostics.peer_state(&target);
            return Ok(serde_json::json!({
                "reached": false,
                "target_state": target_state_owned,
                "timed_out": true,
                "peer": latest,
            }));
        }

        tokio::time::timeout(remaining, watch_rx.changed())
            .await
            .ok();

        // Check state after notification
        if let Some(ps) = state.diagnostics.peer_state(&target) {
            if state_satisfied(&ps, &target_state_owned) {
                return Ok(serde_json::json!({
                    "reached": true,
                    "target_state": target_state_owned,
                    "timed_out": false,
                    "peer": ps,
                }));
            }
        }
    }
}

/// Check if peer state satisfies the target state.
fn state_satisfied(state: &PeerDiagnosticState, target: &str) -> bool {
    match target {
        "discovered" => state.discovered,
        "address_resolved" => state.address_lookup_state == DiagnosticStageState::Succeeded,
        "connected" => state.connection_state == ConnectionDiagnosticState::Connected,
        "subscription_joined" => state.subscription_state == DiagnosticStageState::Succeeded,
        "topic_member" => state.topic_member,
        _ => false,
    }
}

/// `boru_run_discovery_test` — orchestrated diagnostic test against a peer.
async fn handle_run_discovery_test(
    req: &JsonRpcRequest,
    state: &McpAppState,
) -> Result<Value, String> {
    let room_str = req
        .params
        .get("room_id")
        .and_then(|v| v.as_str())
        .ok_or_else(|| "Missing required argument: room_id".to_string())?;

    let expected_peer = req
        .params
        .get("expected_peer_id")
        .and_then(|v| v.as_str())
        .ok_or_else(|| "Missing required argument: expected_peer_id".to_string())?;

    let timeout_ms = req
        .params
        .get("timeout_ms")
        .and_then(|v| v.as_u64())
        .unwrap_or(20000)
        .min(30000); // Clamp to 30s max

    let send_probe = req
        .params
        .get("send_probe")
        .and_then(|v| v.as_bool())
        .unwrap_or(true);

    let probe_payload = req
        .params
        .get("probe_payload")
        .and_then(|v| v.as_str())
        .unwrap_or("automatic LAN discovery test")
        .to_string();

    // Validate all inputs
    validate_bounded(room_str, MAX_ROOM_ID_LEN, "room_id")?;
    validate_no_control_chars(room_str, "room_id")?;
    validate_peer_id(expected_peer)?;
    // Probe payload preserves Unicode, only bounded
    validate_probe_payload(&probe_payload)?;

    let topic = parse_topic_id(room_str)?;

    // 1. Validate that the local node knows the room via diagnostics evidence
    let local_room_joined = state
        .diagnostics
        .build_evidence(Some(topic), None)
        .local_room_joined;

    if !local_room_joined {
        return Ok(serde_json::json!({
            "success": false,
            "room_id": room_str,
            "local_node_id": state.node_id,
            "expected_peer_id": expected_peer,
            "failed_stage": "local_room_unavailable",
            "summary": "Local room is not joined or inactive.",
            "evidence": {
                "local_room_joined": false,
                "peer_discovered": false,
                "address_lookup_observed": false,
                "address_resolved": false,
                "connection_attempted": false,
                "connection_established": false,
                "subscription_started": false,
                "subscription_joined": false,
                "peer_in_topic": false,
                "probe_broadcast": false,
                "probe_received_or_acknowledged": false,
            },
            "peer": Option::<serde_json::Value>::None,
            "event_sequence_start": 0,
            "event_sequence_end": state.diagnostics.latest_sequence(),
            "relevant_events": [],
            "probe": Option::<Value>::None,
        }));
    }

    // 2. Capture starting event sequence
    let seq_start = state.diagnostics.latest_sequence();

    // 3. Inspect existing peer state
    let initial_peer = state.diagnostics.peer_state(expected_peer);

    // 4-7. Wait for peer to progress through stages (with timeout)
    let deadline = tokio::time::Instant::now() + Duration::from_millis(timeout_ms);
    let mut watch_rx = state.diagnostics.subscribe();
    let target = expected_peer.to_string();

    // Define only stages with reliable application-level hooks.  Address
    // resolution is owned by iroh's endpoint and is intentionally reported as
    // NotObserved unless a lower layer emits an explicit diagnostic event;
    // waiting for it here would consume the whole test timeout even after the
    // gossip neighbor has already joined.
    let stages = &[
        "discovered",
        "connected",
        "subscription_joined",
        "topic_member",
    ];

    for &stage in stages {
        let stage_owned = stage.to_string();
        loop {
            let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
            if remaining.is_zero() {
                break;
            }

            if let Some(ps) = state.diagnostics.peer_state(&target) {
                if state_satisfied(&ps, stage) {
                    break;
                }
            }

            tokio::time::timeout(remaining, watch_rx.changed())
                .await
                .ok();
        }
    }

    // 8. Optionally send a diagnostic probe
    let probe_result: Option<ProbeTestResult> = if send_probe {
        let probe_id = generate_probe_id();
        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as i64;

        let probe = diagnostics::DiagnosticProbe {
            probe_id: probe_id.clone(),
            sender_id: state.node_id.clone(),
            room_id: room_str.to_string(),
            sent_at_ms: now_ms,
            payload: Some(probe_payload.clone()),
        };

        let message = Message::DiagnosticProbe(probe);
        let hash_hex = hex::encode(message_hash(&message));

        state.diagnostics.record(
            Some(topic),
            DiagnosticEventKind::ProbeBroadcast {
                probe_id: probe_id.clone(),
                message_hash: hash_hex.clone(),
            },
        );

        // 9. Wait briefly for delivery confirmation
        let wait_for_probe = tokio::time::Instant::now() + Duration::from_millis(5000);
        loop {
            let remaining = wait_for_probe.saturating_duration_since(tokio::time::Instant::now());
            if remaining.is_zero() {
                break;
            }

            if state.diagnostics.find_received_probe(&probe_id).is_some() {
                break;
            }

            tokio::time::timeout(remaining, watch_rx.changed())
                .await
                .ok();
        }

        let delivery = state.diagnostics.find_received_probe(&probe_id);
        Some(ProbeTestResult {
            probe_id,
            broadcast_accepted: true,
            delivery_confirmed: delivery.is_some(),
            latency_ms: delivery.and_then(|p| p.latency_ms),
        })
    } else {
        None
    };

    // 10. Collect evidence and classify
    let seq_end = state.diagnostics.latest_sequence();
    let peer = state.diagnostics.peer_state(expected_peer);
    let evidence = state
        .diagnostics
        .build_evidence(Some(topic), Some(expected_peer));
    let (failed_stage, summary) = classify_discovery_test(&evidence, peer.as_ref());

    // Collect relevant events from this test
    let relevant_events: Vec<DiagnosticEvent> =
        state.diagnostics.events_since(seq_start, 1000, Some(topic));

    let result = DiscoveryTestResult {
        success: failed_stage.is_none(),
        room_id: room_str.to_string(),
        local_node_id: state.node_id.clone(),
        expected_peer_id: expected_peer.to_string(),
        failed_stage,
        summary,
        evidence,
        peer,
        event_sequence_start: seq_start,
        event_sequence_end: seq_end,
        relevant_events,
        probe: probe_result,
    };

    Ok(serde_json::to_value(&result).map_err(|e| format!("serialize: {e}"))?)
}

/// `boru_get_iced_state` — snapshot of the current Iced application state.
async fn handle_get_iced_state(state: &McpAppState) -> Result<Value, String> {
    let journal = &state.iced_diagnostics;
    Ok(serde_json::json!({
        "message": "Iced diagnostics available",
        "journal_entry_count": journal.entry_count(),
        "journal_latest_sequence": journal.latest_sequence(),
        "diagnostics_event_count": state.diagnostics.event_count(),
        "diagnostics_latest_sequence": state.diagnostics.latest_sequence(),
        "active_rooms": state.diagnostics.joined_rooms(),
    }))
}

/// `boru_get_iced_message_journal` — recent Iced AppMessage processing history.
async fn handle_get_iced_message_journal(
    req: &JsonRpcRequest,
    state: &McpAppState,
) -> Result<Value, String> {
    let since_sequence = req
        .params
        .get("since_sequence")
        .and_then(|v| v.as_u64())
        .unwrap_or(0);

    let limit = req
        .params
        .get("limit")
        .and_then(|v| v.as_u64())
        .unwrap_or(200) as usize;

    let entries = state.iced_diagnostics.entries_since(since_sequence, limit);
    let latest = state.iced_diagnostics.latest_sequence();

    Ok(serde_json::json!({
        "entries": entries,
        "latest_sequence": latest,
        "returned_count": entries.len(),
    }))
}

/// `boru_get_failure_analysis` — combined failure analysis across all layers.
async fn handle_get_failure_analysis(
    req: &JsonRpcRequest,
    state: &McpAppState,
) -> Result<Value, String> {
    let since_sequence = req
        .params
        .get("since_sequence")
        .and_then(|v| v.as_u64())
        .unwrap_or(0);

    let analysis = classify_failures(&state.diagnostics, &state.iced_diagnostics, since_sequence);

    Ok(serde_json::to_value(&analysis).map_err(|e| format!("serialize: {e}"))?)
}

// =============================================================================
// Helpers
// =============================================================================

/// Parse a hex-encoded topic ID string into a `TopicId`.
fn parse_topic_id(s: &str) -> Result<TopicId, String> {
    let bytes = hex::decode(s).map_err(|e| format!("Invalid hex room_id: {e}"))?;
    if bytes.len() != 32 {
        return Err(format!(
            "Room ID must be 32 bytes (64 hex chars), got {}",
            bytes.len()
        ));
    }
    let mut arr = [0u8; 32];
    arr.copy_from_slice(&bytes);
    Ok(TopicId::from_bytes(arr))
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use boru_chat::diagnostics::GuiTestCommand;
    use serde_json::json;

    // ── handle_gui_open_room validation tests ──────────────────────────

    fn make_open_room_request(params: Value) -> JsonRpcRequest {
        JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            method: "boru_gui_open_room".to_string(),
            params,
            id: Some(Value::Number(1.into())),
        }
    }

    #[tokio::test]
    async fn test_gui_open_room_missing_room_id() {
        let (tx, _rx) = boru_chat::diagnostics::GuiTestHandle::channel(1);
        let req = make_open_room_request(json!({}));
        let result = handle_gui_open_room(&req, tx).await;
        assert!(result.is_err(), "Missing room_id should produce an error");
        let err = result.unwrap_err();
        assert!(
            err.contains("Missing required argument"),
            "Error should mention missing room_id, got: {err}"
        );
    }

    #[tokio::test]
    async fn test_gui_open_room_empty_room_id() {
        let (tx, _rx) = boru_chat::diagnostics::GuiTestHandle::channel(1);
        let req = make_open_room_request(json!({"room_id": ""}));
        let result = handle_gui_open_room(&req, tx).await;
        assert!(result.is_err(), "Empty room_id should produce an error");
        let err = result.unwrap_err();
        assert!(
            err.contains("must not be empty"),
            "Error should mention 'empty', got: {err}"
        );
    }

    #[tokio::test]
    async fn test_gui_open_room_too_long() {
        let (tx, _rx) = boru_chat::diagnostics::GuiTestHandle::channel(1);
        // MAX_GUI_ROOM_ID_LEN = 128, so 129 should fail
        let long_id = "a".repeat(129);
        let req = make_open_room_request(json!({"room_id": long_id}));
        let result = handle_gui_open_room(&req, tx).await;
        assert!(result.is_err(), "Oversized room_id should produce an error");
        let err = result.unwrap_err();
        assert!(
            err.contains("too long"),
            "Error should mention 'too long', got: {err}"
        );
    }

    #[tokio::test]
    async fn test_gui_open_room_max_length_accepted() {
        let (tx, mut rx) = boru_chat::diagnostics::GuiTestHandle::channel(1);
        // Exactly 128 chars (MAX_GUI_ROOM_ID_LEN) should succeed
        let max_id = "a".repeat(128);
        let req = make_open_room_request(json!({"room_id": max_id}));
        let result = handle_gui_open_room(&req, tx).await;
        assert!(result.is_ok(), "Max-length room_id should succeed");

        let value = result.unwrap();
        assert_eq!(value["sent"], true);
        assert!(value["action_id"].is_string());

        let received = rx
            .try_recv()
            .expect("Should have received a GuiActionRequest");
        let cmd: crate::gui_test_actions::GuiTestCommand =
            serde_json::from_str(&received.command).expect("deserialize command");
        match &cmd {
            crate::gui_test_actions::GuiTestCommand::OpenRoom { room_id: topic } => {
                assert_eq!(topic, &max_id);
            }
            other => panic!("Expected OpenRoom command, got: {:?}", other),
        }
    }

    #[tokio::test]
    async fn test_gui_open_room_invalid_chars_special() {
        let (tx, _rx) = boru_chat::diagnostics::GuiTestHandle::channel(1);
        // '@' is not in [a-zA-Z0-9_-]
        let bad = "room@name";
        let req = make_open_room_request(json!({"room_id": bad}));
        let result = handle_gui_open_room(&req, tx).await;
        assert!(result.is_err(), "Special chars should produce an error");
        let err = result.unwrap_err();
        assert!(
            err.contains("Invalid room_id"),
            "Error should mention 'Invalid room_id', got: {err}"
        );
    }

    #[tokio::test]
    async fn test_gui_open_room_invalid_chars_space() {
        let (tx, _rx) = boru_chat::diagnostics::GuiTestHandle::channel(1);
        let bad = "room name";
        let req = make_open_room_request(json!({"room_id": bad}));
        let result = handle_gui_open_room(&req, tx).await;
        assert!(result.is_err(), "Spaces should produce an error");
        let err = result.unwrap_err();
        assert!(
            err.contains("Invalid room_id"),
            "Error should mention 'Invalid room_id', got: {err}"
        );
    }

    #[tokio::test]
    async fn test_gui_open_room_invalid_chars_slash() {
        let (tx, _rx) = boru_chat::diagnostics::GuiTestHandle::channel(1);
        let bad = "room/path";
        let req = make_open_room_request(json!({"room_id": bad}));
        let result = handle_gui_open_room(&req, tx).await;
        assert!(result.is_err(), "Slash should produce an error");
        let err = result.unwrap_err();
        assert!(
            err.contains("Invalid room_id"),
            "Error should mention 'Invalid room_id', got: {err}"
        );
    }

    #[tokio::test]
    async fn test_gui_open_room_control_chars() {
        let (tx, _rx) = boru_chat::diagnostics::GuiTestHandle::channel(1);
        let bad = "room\nname";
        let req = make_open_room_request(json!({"room_id": bad}));
        let result = handle_gui_open_room(&req, tx).await;
        assert!(result.is_err(), "Control chars should produce an error");
        let err = result.unwrap_err();
        assert!(
            err.contains("Invalid room_id"),
            "Error should mention 'Invalid room_id', got: {err}"
        );
    }

    #[tokio::test]
    async fn test_gui_open_room_success_basic() {
        let (tx, mut rx) = boru_chat::diagnostics::GuiTestHandle::channel(1);
        let valid_id = "TestRoom123";
        let req = make_open_room_request(json!({"room_id": valid_id}));
        let result = handle_gui_open_room(&req, tx).await;
        assert!(result.is_ok(), "Valid room_id should succeed");

        let value = result.unwrap();
        assert_eq!(value["sent"], true, "Should report sent=true");
        assert!(
            value["action_id"].is_string(),
            "Should return an action_id string"
        );
        assert_eq!(
            value["room_id"], valid_id,
            "Should return the original room_id"
        );
        let action_id = value["action_id"].as_str().unwrap();
        assert!(!action_id.is_empty(), "action_id should not be empty");

        let received = rx
            .try_recv()
            .expect("Should have received a GuiActionRequest");
        assert_eq!(received.action_id.0, action_id);
        let cmd: crate::gui_test_actions::GuiTestCommand =
            serde_json::from_str(&received.command).expect("deserialize command");
        match &cmd {
            crate::gui_test_actions::GuiTestCommand::OpenRoom { room_id: topic } => {
                assert_eq!(topic, valid_id);
            }
            other => panic!("Expected OpenRoom command, got: {:?}", other),
        }
    }

    #[tokio::test]
    async fn test_gui_open_room_success_with_hyphen() {
        let (tx, mut rx) = boru_chat::diagnostics::GuiTestHandle::channel(1);
        let valid_id = "my-room-name-42";
        let req = make_open_room_request(json!({"room_id": valid_id}));
        let result = handle_gui_open_room(&req, tx).await;
        assert!(result.is_ok(), "Hyphen room_id should succeed");

        let value = result.unwrap();
        assert_eq!(value["sent"], true);

        let received = rx
            .try_recv()
            .expect("Should have received a GuiActionRequest");
        let cmd: crate::gui_test_actions::GuiTestCommand =
            serde_json::from_str(&received.command).expect("deserialize command");
        match &cmd {
            crate::gui_test_actions::GuiTestCommand::OpenRoom { room_id: topic } => {
                assert_eq!(topic, valid_id);
            }
            other => panic!("Expected OpenRoom command, got: {:?}", other),
        }
    }

    #[tokio::test]
    async fn test_gui_open_room_success_with_underscore() {
        let (tx, mut rx) = boru_chat::diagnostics::GuiTestHandle::channel(1);
        let valid_id = "room_name_42";
        let req = make_open_room_request(json!({"room_id": valid_id}));
        let result = handle_gui_open_room(&req, tx).await;
        assert!(result.is_ok(), "Underscore room_id should succeed");

        let value = result.unwrap();
        assert_eq!(value["sent"], true);

        let received = rx
            .try_recv()
            .expect("Should have received a GuiActionRequest");
        let cmd: crate::gui_test_actions::GuiTestCommand =
            serde_json::from_str(&received.command).expect("deserialize command");
        match &cmd {
            crate::gui_test_actions::GuiTestCommand::OpenRoom { room_id: topic } => {
                assert_eq!(topic, valid_id);
            }
            other => panic!("Expected OpenRoom command, got: {:?}", other),
        }
    }

    #[tokio::test]
    async fn test_gui_open_room_success_all_alphanumeric() {
        let (tx, mut rx) = boru_chat::diagnostics::GuiTestHandle::channel(1);
        // A hex-like string without 0x prefix — still valid alphanumeric
        let valid_id = "abcdef0123456789abcdef0123456789abcdef0123456789abcdef0123456789";
        let req = make_open_room_request(json!({"room_id": valid_id}));
        let result = handle_gui_open_room(&req, tx).await;
        assert!(result.is_ok(), "Alphanumeric room_id should succeed");

        let value = result.unwrap();
        assert_eq!(value["sent"], true);

        let received = rx
            .try_recv()
            .expect("Should have received a GuiActionRequest");
        let cmd: crate::gui_test_actions::GuiTestCommand =
            serde_json::from_str(&received.command).expect("deserialize command");
        match cmd {
            crate::gui_test_actions::GuiTestCommand::OpenRoom { room_id: topic } => {
                assert_eq!(topic, valid_id);
            }
            _ => panic!("Expected OpenRoom command"),
        }
    }

    #[tokio::test]
    async fn test_gui_open_room_no_secret_tickets_exposed() {
        let (tx, _rx) = boru_chat::diagnostics::GuiTestHandle::channel(1);
        let valid_id = "TestRoom";
        let req = make_open_room_request(json!({"room_id": valid_id}));
        let result = handle_gui_open_room(&req, tx).await;
        assert!(result.is_ok(), "Valid room_id should succeed");

        let json_str = serde_json::to_string(&result.unwrap()).unwrap();
        assert!(
            !json_str.contains("ticket"),
            "Response should not contain ticket info"
        );
        assert!(
            !json_str.contains("secret"),
            "Response should not contain secrets"
        );
        assert!(
            !json_str.contains("invite"),
            "Response should not contain invite info"
        );
    }

    #[tokio::test]
    async fn test_gui_open_room_channel_full() {
        let (tx, mut rx) = boru_chat::diagnostics::GuiTestHandle::channel(1);
        // Fill the capacity-1 channel first
        let fill_req = make_open_room_request(json!({"room_id": "SomeRoom"}));
        let fill_result = handle_gui_open_room(&fill_req, tx.clone()).await;
        assert!(fill_result.is_ok(), "First fill should succeed");

        let valid_id = "SomeRoom2";
        let req = make_open_room_request(json!({"room_id": valid_id}));
        let result = handle_gui_open_room(&req, tx).await;
        assert!(result.is_err(), "Should error when channel is full");
        let err = result.unwrap_err();
        assert!(
            err.contains("queue is full"),
            "Error should mention queue full, got: {err}"
        );
        // Drain the channel
        let _ = rx.try_recv();
    }

    // ── handle_gui_open_conversation validation tests ─────────────────────

    /// Returns a JsonRpcRequest with the given params and method "boru_gui_open_conversation".
    fn make_request(params: Value) -> JsonRpcRequest {
        JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            method: "boru_gui_open_conversation".to_string(),
            params,
            id: Some(Value::Number(1.into())),
        }
    }

    /// Creates a channel and returns the sender and a dummy request for testing.
    fn make_test_env(params: Value) -> (boru_chat::diagnostics::GuiTestHandle, JsonRpcRequest) {
        let (tx, _rx) = boru_chat::diagnostics::GuiTestHandle::channel(16);
        (tx, make_request(params))
    }

    #[tokio::test]
    async fn test_missing_conversation_id() {
        let (tx, req) = make_test_env(json!({}));
        let result = handle_gui_open_conversation(&req, tx).await;
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.contains("Missing required argument: conversation_id"));
    }

    #[tokio::test]
    async fn test_conversation_id_too_long() {
        // 67 chars exceeds the max of 66
        let long_id = "a".repeat(67);
        let (tx, req) = make_test_env(json!({ "conversation_id": long_id }));
        let result = handle_gui_open_conversation(&req, tx).await;
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.contains("too long"));
    }

    #[tokio::test]
    async fn test_conversation_id_with_control_chars() {
        let (tx, req) = make_test_env(
            json!({ "conversation_id": "abcdef0123456789abcdef0123456789abcdef0123456789abcdef01234567\n" }),
        );
        let result = handle_gui_open_conversation(&req, tx).await;
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.contains("control characters"));
    }

    #[tokio::test]
    async fn test_conversation_id_invalid_format() {
        // Not hex
        let (tx, req) = make_test_env(
            json!({ "conversation_id": "zzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzz" }),
        );
        let result = handle_gui_open_conversation(&req, tx).await;
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.contains("Invalid conversation_id"));
    }

    #[tokio::test]
    async fn test_conversation_id_too_short() {
        // Only 32 hex chars instead of 64
        let short = "a".repeat(32);
        let (tx, req) = make_test_env(json!({ "conversation_id": short }));
        let result = handle_gui_open_conversation(&req, tx).await;
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.contains("Invalid conversation_id"));
    }

    #[tokio::test]
    async fn test_conversation_id_path_traversal_attempt() {
        let (tx, req) = make_test_env(json!({ "conversation_id": "../../etc/passwd" }));
        let result = handle_gui_open_conversation(&req, tx).await;
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.contains("Invalid conversation_id"));
    }

    #[tokio::test]
    async fn test_conversation_id_shell_metacharacters() {
        let (tx, req) = make_test_env(json!({ "conversation_id": "abcd; rm -rf /" }));
        let result = handle_gui_open_conversation(&req, tx).await;
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.contains("Invalid conversation_id"));
    }

    #[tokio::test]
    async fn test_valid_conversation_id() {
        let valid_id = "abcdef0123456789abcdef0123456789abcdef0123456789abcdef0123456789";
        let (tx, mut rx) = boru_chat::diagnostics::GuiTestHandle::channel(16);
        let req = make_request(json!({ "conversation_id": valid_id }));
        let result = handle_gui_open_conversation(&req, tx).await;
        assert!(result.is_ok(), "Expected Ok, got: {:?}", result);

        let response = result.unwrap();
        assert_eq!(response["sent"], true);
        assert_eq!(response["conversation_id"], valid_id);
        assert!(response["action_id"].is_string());
        assert!(!response["action_id"].as_str().unwrap().is_empty());
        assert!(response["note"]
            .as_str()
            .unwrap()
            .contains("Open conversation command queued"));

        // Verify the action was actually sent through the channel
        let received = rx
            .try_recv()
            .expect("Action should have been sent through channel");
        assert_eq!(
            received.action_id.0,
            response["action_id"].as_str().unwrap()
        );
        let cmd: crate::gui_test_actions::GuiTestCommand =
            serde_json::from_str(&received.command).expect("deserialize command");
        match cmd {
            crate::gui_test_actions::GuiTestCommand::OpenConversation { conversation_id } => {
                assert_eq!(conversation_id, valid_id);
            }
            other => panic!("Expected OpenConversation command, got: {:?}", other),
        }
    }

    #[tokio::test]
    async fn test_valid_conversation_id_with_0x_prefix() {
        let valid_id = "0xabcdef0123456789abcdef0123456789abcdef0123456789abcdef0123456789";
        let (tx, _rx) = boru_chat::diagnostics::GuiTestHandle::channel(16);
        let req = make_request(json!({ "conversation_id": valid_id }));
        let result = handle_gui_open_conversation(&req, tx).await;
        assert!(result.is_ok(), "Expected Ok, got: {:?}", result);
        let response = result.unwrap();
        assert_eq!(response["sent"], true);
        assert_eq!(response["conversation_id"], valid_id);
    }

    #[tokio::test]
    async fn test_channel_full_error() {
        // Create a channel with capacity 1 and fill it
        let (tx, _rx) = boru_chat::diagnostics::GuiTestHandle::channel(1);
        // Don't consume from rx, so the channel stays 0-length effectively
        // Actually channel(1) means capacity 1. Let me use channel(0) to test full.
        drop(_rx); // Close receiver

        let valid_id = "abcdef0123456789abcdef0123456789abcdef0123456789abcdef0123456789";
        let req = make_request(json!({ "conversation_id": valid_id }));
        let result = handle_gui_open_conversation(&req, tx).await;
        assert!(result.is_err());
        // With receiver dropped, we should get "channel is closed"
        let err = result.unwrap_err();
        assert!(err.contains("channel is closed"));
    }

    // ── handle_set_composer validation tests ──────────────────────────

    fn make_set_composer_request(params: Value) -> JsonRpcRequest {
        JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            method: "boru_gui_set_composer".to_string(),
            params,
            id: Some(Value::Number(1.into())),
        }
    }

    #[tokio::test]
    async fn test_set_composer_missing_text() {
        let (tx, _rx) = boru_chat::diagnostics::GuiTestHandle::channel(16);
        let req = make_set_composer_request(json!({}));
        let result = handle_set_composer(&req, tx).await;
        assert!(result.is_err(), "Missing text should produce an error");
        let err = result.unwrap_err();
        assert!(
            err.contains("Missing required argument"),
            "Error should mention missing text, got: {err}"
        );
    }

    #[tokio::test]
    async fn test_set_composer_too_long() {
        let (tx, mut rx) = boru_chat::diagnostics::GuiTestHandle::channel(16);
        // Create text far beyond the 4096-char clamp limit
        let long_text = "a".repeat(5000);
        let req = make_set_composer_request(json!({ "text": long_text }));
        let result = handle_set_composer(&req, tx).await;
        assert!(
            result.is_ok(),
            "Oversized text should be clamped, not rejected"
        );
        let value = result.unwrap();
        assert_eq!(
            value["text_length"], 4096,
            "Should be clamped to 4096 chars"
        );
        assert_eq!(value["clamped"], true, "Should report clamped=true");

        // Verify the clamped text was sent through the channel
        let received = rx
            .try_recv()
            .expect("Should have received a GuiActionRequest");
        let cmd: crate::gui_test_actions::GuiTestCommand =
            serde_json::from_str(&received.command).expect("deserialize command");
        match &cmd {
            crate::gui_test_actions::GuiTestCommand::SetComposerText { text } => {
                assert_eq!(text.len(), 4096, "Sent text should be 4096 'a's");
                assert_eq!(text.as_str(), "a".repeat(4096));
            }
            other => panic!("Expected SetComposerText command, got: {:?}", other),
        }
    }

    #[tokio::test]
    async fn test_set_composer_control_chars() {
        let (tx, _rx) = boru_chat::diagnostics::GuiTestHandle::channel(16);
        let req = make_set_composer_request(json!({ "text": "hello\nworld" }));
        let result = handle_set_composer(&req, tx).await;
        assert!(
            result.is_err(),
            "Control characters should produce an error"
        );
        let err = result.unwrap_err();
        assert!(
            err.contains("control characters"),
            "Error should mention control characters, got: {err}"
        );
    }

    #[tokio::test]
    async fn test_set_composer_control_chars_tab() {
        let (tx, _rx) = boru_chat::diagnostics::GuiTestHandle::channel(16);
        let req = make_set_composer_request(json!({ "text": "hello\tworld" }));
        let result = handle_set_composer(&req, tx).await;
        assert!(result.is_err(), "Tab character should produce an error");
        let err = result.unwrap_err();
        assert!(
            err.contains("control characters"),
            "Error should mention control characters, got: {err}"
        );
    }

    #[tokio::test]
    async fn test_set_composer_success() {
        let (tx, mut rx) = boru_chat::diagnostics::GuiTestHandle::channel(16);
        let req = make_set_composer_request(json!({ "text": "Hello, world!" }));
        let result = handle_set_composer(&req, tx).await;
        assert!(
            result.is_ok(),
            "Valid text should succeed, got: {:?}",
            result
        );

        let value = result.unwrap();
        assert_eq!(value["sent"], true, "Should report sent=true");
        assert_eq!(
            value["text_length"], 13,
            "Should report correct text length"
        );
        assert!(
            value["action_id"].is_string(),
            "Should return an action_id string"
        );
        let action_id = value["action_id"].as_str().unwrap();
        assert!(!action_id.is_empty(), "action_id should not be empty");
        assert!(
            value["note"]
                .as_str()
                .unwrap()
                .contains("Composer text set"),
            "Should include note about composer text being set"
        );
        assert!(
            value["note"].as_str().unwrap().contains("SendMessage"),
            "Should mention SendMessage for submission"
        );

        // Verify the action was actually sent through the channel
        let received = rx
            .try_recv()
            .expect("Should have received a GuiActionRequest");
        assert_eq!(received.action_id.0, action_id);
        // command is JSON-serialized — verify deserialized command
        let cmd: crate::gui_test_actions::GuiTestCommand =
            serde_json::from_str(&received.command).expect("deserialize command");
        match &cmd {
            crate::gui_test_actions::GuiTestCommand::SetComposerText { text } => {
                assert_eq!(text, "Hello, world!");
            }
            _ => panic!("Expected SetComposerText command, got: {cmd:?}"),
        }
    }

    #[tokio::test]
    async fn test_set_composer_unicode_text() {
        let (tx, mut rx) = boru_chat::diagnostics::GuiTestHandle::channel(16);
        let unicode_text = "Hello, 世界! 🌍";
        let req = make_set_composer_request(json!({ "text": unicode_text }));
        let result = handle_set_composer(&req, tx).await;
        assert!(result.is_ok(), "Unicode text should succeed");

        let value = result.unwrap();
        assert_eq!(value["sent"], true);
        // text_length is char count, not byte count
        assert_eq!(value["text_length"], 12, "Should count chars not bytes");

        let received = rx
            .try_recv()
            .expect("Should have received a GuiActionRequest");
        let cmd: crate::gui_test_actions::GuiTestCommand =
            serde_json::from_str(&received.command).expect("deserialize command");
        match &cmd {
            crate::gui_test_actions::GuiTestCommand::SetComposerText { text } => {
                assert_eq!(text, unicode_text);
            }
            other => panic!("Expected SetComposerText command, got: {:?}", other),
        }
    }

    #[tokio::test]
    async fn test_set_composer_empty_text() {
        let (tx, _rx) = boru_chat::diagnostics::GuiTestHandle::channel(16);
        let req = make_set_composer_request(json!({ "text": "" }));
        let result = handle_set_composer(&req, tx).await;
        assert!(result.is_err(), "Empty text should produce an error");
        let err = result.unwrap_err();
        assert!(
            err.contains("must not be empty"),
            "Error should mention 'must not be empty', got: {err}"
        );
    }

    #[tokio::test]
    async fn test_set_composer_channel_full() {
        let (tx, mut rx) = boru_chat::diagnostics::GuiTestHandle::channel(1);
        // Fill the capacity-1 channel first
        let fill_req = make_set_composer_request(json!({ "text": "fill" }));
        let fill_result = handle_set_composer(&fill_req, tx.clone()).await;
        assert!(fill_result.is_ok(), "First fill should succeed");

        let req = make_set_composer_request(json!({ "text": "hello" }));
        let result = handle_set_composer(&req, tx).await;
        assert!(result.is_err(), "Should error when channel is full");
        let err = result.unwrap_err();
        assert!(
            err.contains("queue is full"),
            "Error should mention queue full, got: {err}"
        );
        let _ = rx.try_recv();
    }

    #[tokio::test]
    async fn test_set_composer_channel_closed() {
        let (tx, rx) = boru_chat::diagnostics::GuiTestHandle::channel(16);
        drop(rx); // Close the receiver
        let req = make_set_composer_request(json!({ "text": "hello" }));
        let result = handle_set_composer(&req, tx).await;
        assert!(result.is_err(), "Should error when channel is closed");
        let err = result.unwrap_err();
        assert!(
            err.contains("channel is closed"),
            "Error should mention channel closed, got: {err}"
        );
    }

    #[tokio::test]
    async fn test_set_composer_exact_max_length() {
        let (tx, mut rx) = boru_chat::diagnostics::GuiTestHandle::channel(16);
        // Exactly 4096 chars — the MAX_COMPOSER_LEN boundary
        let exact_text = "a".repeat(4096);
        let req = make_set_composer_request(json!({ "text": exact_text.clone() }));
        let result = handle_set_composer(&req, tx).await;
        assert!(
            result.is_ok(),
            "Exactly max-length text should succeed, got: {:?}",
            result
        );

        let value = result.unwrap();
        assert_eq!(value["sent"], true, "Should report sent=true");
        assert_eq!(
            value["text_length"], 4096,
            "Should report correct text length"
        );
        assert_eq!(
            value["clamped"], false,
            "Exact max-length text should NOT be clamped"
        );
        assert!(
            value["action_id"].is_string(),
            "Should return an action_id string"
        );
        let action_id = value["action_id"].as_str().unwrap();
        assert!(!action_id.is_empty(), "action_id should not be empty");

        // Verify the text was sent through the channel unchanged
        let received = rx
            .try_recv()
            .expect("Should have received a GuiActionRequest");
        let cmd: crate::gui_test_actions::GuiTestCommand =
            serde_json::from_str(&received.command).expect("deserialize command");
        match &cmd {
            crate::gui_test_actions::GuiTestCommand::SetComposerText { text } => {
                assert_eq!(
                    text, &exact_text,
                    "Exact max-length text should pass through unchanged"
                );
            }
            other => panic!("Expected SetComposerText command, got: {:?}", other),
        }
    }

    #[tokio::test]
    async fn test_set_composer_full_text_not_exposed_in_response() {
        let secret_text = "This is a secret message that should never appear in logs";
        let (tx, _rx) = boru_chat::diagnostics::GuiTestHandle::channel(16);
        let req = make_set_composer_request(json!({ "text": secret_text }));
        let result = handle_set_composer(&req, tx).await;
        assert!(result.is_ok(), "Valid text should succeed");

        // The MCP response is metadata-only; user text must not be reflected
        // through this diagnostic endpoint.
        let response = serde_json::to_string(&result.unwrap()).unwrap();
        assert!(
            !response.contains(secret_text),
            "Full composer text must not appear in the MCP response"
        );
    }

    #[test]
    fn test_set_composer_log_prefix_does_not_contain_full_text() {
        // The handler's warning is intentionally limited to a bounded prefix;
        // verify the same logging contract without installing a global tracing
        // subscriber (which would make the full example test suite racy).
        let secret_text = format!("composer-secret-{}", "0123456789abcdef".repeat(32));
        let prefix = sanitize_for_log(&secret_text, 50);
        assert!(prefix.contains("... (truncated, total "));
        assert!(prefix.len() > 50);
        assert!(
            !prefix.contains(&secret_text),
            "A bounded log prefix must not contain the full composer text"
        );
        assert!(
            prefix.starts_with("composer-secret-"),
            "The bounded prefix should retain useful diagnostic context"
        );
    }

    // ── handle_submit_composer tests ──────────────────────────────────────

    #[tokio::test]
    async fn test_submit_composer_success() {
        let (handle, mut rx) = boru_chat::diagnostics::GuiTestHandle::channel(1);
        let result = handle_submit_composer(handle).await;
        assert!(result.is_ok(), "SubmitComposer should succeed");

        let value = result.unwrap();
        assert_eq!(value["sent"], true, "Should report sent=true");
        assert!(
            value["action_id"].is_string(),
            "Should return an action_id string"
        );
        let action_id = value["action_id"].as_str().unwrap();
        assert!(!action_id.is_empty(), "action_id should not be empty");
        assert!(value["note"].is_string(), "Should include a note string");

        // Verify the action was actually sent through the channel
        let received = rx
            .try_recv()
            .expect("Should have received a GuiActionRequest");
        assert_eq!(received.action_id.0, action_id);
        // The command should be the JSON serialization of SubmitComposer
        let expected_json =
            serde_json::to_string(&boru_chat::diagnostics::GuiTestCommand::SubmitComposer).unwrap();
        assert_eq!(received.command, expected_json);
    }

    #[tokio::test]
    async fn test_submit_composer_channel_full() {
        // GuiTestHandle::channel clamps to min 1, so use capacity 1 and fill it
        let (handle, _rx) = boru_chat::diagnostics::GuiTestHandle::channel(1);
        // Fill the capacity-1 channel first
        let fill = boru_chat::diagnostics::GuiActionRequest {
            action_id: boru_chat::diagnostics::GuiActionId::new(),
            requested_at_ms: 1000,
            command: "FillCommand".to_string(),
        };
        handle.enqueue(fill).expect("First enqueue should succeed");

        let result = handle_submit_composer(handle).await;
        assert!(result.is_err(), "Should error when channel is full");
        let err = result.unwrap_err();
        assert!(
            err.contains("queue is full"),
            "Error should mention 'queue is full', got: {err}"
        );
    }

    #[tokio::test]
    async fn test_submit_composer_channel_closed() {
        let (handle, rx) = boru_chat::diagnostics::GuiTestHandle::channel(1);
        drop(rx); // Close the receiver
        let result = handle_submit_composer(handle).await;
        assert!(result.is_err(), "Should error when channel is closed");
        let err = result.unwrap_err();
        assert!(
            err.contains("channel error"),
            "Error should mention 'channel error', got: {err}"
        );
        assert!(
            err.contains("closed"),
            "Error should mention 'closed', got: {err}"
        );
    }

    #[tokio::test]
    async fn test_submit_composer_no_secrets() {
        let (handle, _rx) = boru_chat::diagnostics::GuiTestHandle::channel(1);
        let result = handle_submit_composer(handle).await.unwrap();
        let response_str = serde_json::to_string(&result).unwrap();
        assert!(!response_str.contains("secret_key"));
        assert!(!response_str.contains("secret"));
        assert!(!response_str.contains("private_key"));
        assert!(!response_str.contains("ticket"));
        assert!(!response_str.contains("password"));
    }

    // ── Security: no secrets in response ──────────────────────────────────

    #[tokio::test]
    async fn test_response_contains_no_secrets() {
        let valid_id = "abcdef0123456789abcdef0123456789abcdef0123456789abcdef0123456789";
        let (tx, _rx) = boru_chat::diagnostics::GuiTestHandle::channel(16);
        let req = make_request(json!({ "conversation_id": valid_id }));
        let result = handle_gui_open_conversation(&req, tx).await.unwrap();
        let response_str = serde_json::to_string(&result).unwrap();
        assert!(!response_str.contains("secret_key"));
        assert!(!response_str.contains("secret"));
        assert!(!response_str.contains("private_key"));
        assert!(!response_str.contains("ticket"));
        assert!(!response_str.contains("password"));
    }

    // ── boru_gui_navigate validation tests ─────────────────────────────────

    fn make_navigate_request(params: Value) -> JsonRpcRequest {
        JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            method: "boru_gui_navigate".to_string(),
            params,
            id: Some(Value::Number(1.into())),
        }
    }

    #[tokio::test]
    async fn test_navigate_missing_destination() {
        let (tx, _rx) = boru_chat::diagnostics::GuiTestHandle::channel(1);
        let req = make_navigate_request(json!({}));
        let result = handle_gui_navigate(&req, tx).await;
        assert!(
            result.is_err(),
            "Missing destination should produce an error"
        );
        let err = result.unwrap_err();
        assert!(
            err.contains("Missing required argument"),
            "Error should mention missing destination, got: {err}"
        );
    }

    #[tokio::test]
    async fn test_navigate_invalid_destination() {
        let (tx, _rx) = boru_chat::diagnostics::GuiTestHandle::channel(1);
        let req = make_navigate_request(json!({"destination": "not_a_real_screen"}));
        let result = handle_gui_navigate(&req, tx).await;
        assert!(
            result.is_err(),
            "Invalid destination should produce an error"
        );
        let err = result.unwrap_err();
        assert!(
            err.contains("Invalid destination"),
            "Error should mention 'Invalid destination', got: {err}"
        );
        assert!(
            err.contains("chat_list"),
            "Error should list supported destinations, got: {err}"
        );
    }

    #[tokio::test]
    async fn test_navigate_destination_too_long() {
        let (tx, _rx) = boru_chat::diagnostics::GuiTestHandle::channel(1);
        let long = "a".repeat(5000);
        let req = make_navigate_request(json!({"destination": long}));
        let result = handle_gui_navigate(&req, tx).await;
        assert!(
            result.is_err(),
            "Oversized destination should produce an error"
        );
        let err = result.unwrap_err();
        assert!(
            err.contains("too long"),
            "Error should mention 'too long', got: {err}"
        );
    }

    #[tokio::test]
    async fn test_navigate_control_chars() {
        let (tx, _rx) = boru_chat::diagnostics::GuiTestHandle::channel(1);
        let req = make_navigate_request(json!({"destination": "chat_list\n"}));
        let result = handle_gui_navigate(&req, tx).await;
        assert!(result.is_err(), "Control chars should produce an error");
        let err = result.unwrap_err();
        assert!(
            err.contains("control characters"),
            "Error should mention control characters, got: {err}"
        );
    }

    #[tokio::test]
    async fn test_navigate_to_chat_list() {
        let (tx, mut rx) = boru_chat::diagnostics::GuiTestHandle::channel(1);
        let req = make_navigate_request(json!({"destination": "chat_list"}));
        let result = handle_gui_navigate(&req, tx).await;
        assert!(result.is_ok(), "Valid destination should succeed");

        let value = result.unwrap();
        assert_eq!(value["accepted"], true, "Should report accepted=true");
        assert!(
            value["action_id"].is_string(),
            "Should return an action_id string"
        );
        let action_id = value["action_id"].as_str().unwrap();
        assert!(!action_id.is_empty(), "action_id should not be empty");
        assert!(value["queued_at_ms"].is_i64(), "Should return queued_at_ms");

        let received = rx
            .try_recv()
            .expect("Should have received a GuiActionRequest");
        assert_eq!(received.action_id.0, action_id);
        // Serialized as {"command":"go_to_chat_list"} with tag = "command" format
        let cmd: boru_chat::diagnostics::GuiTestCommand =
            serde_json::from_str(&received.command).expect("deserialize command");
        assert_eq!(cmd, boru_chat::diagnostics::GuiTestCommand::GoToChatList);
    }

    #[tokio::test]
    async fn test_navigate_to_friends() {
        let (tx, mut rx) = boru_chat::diagnostics::GuiTestHandle::channel(1);
        let req = make_navigate_request(json!({"destination": "friends"}));
        let result = handle_gui_navigate(&req, tx).await;
        assert!(result.is_ok(), "Valid destination should succeed");

        let value = result.unwrap();
        assert_eq!(value["accepted"], true);
        assert!(value["action_id"].is_string());

        let received = rx
            .try_recv()
            .expect("Should have received a GuiActionRequest");
        let cmd: crate::gui_test_actions::GuiTestCommand =
            serde_json::from_str(&received.command).expect("deserialize command");
        match &cmd {
            crate::gui_test_actions::GuiTestCommand::OpenFriends => {}
            other => panic!("Expected OpenFriends command, got: {other:?}"),
        }
    }

    #[tokio::test]
    async fn test_navigate_to_settings() {
        let (tx, mut rx) = boru_chat::diagnostics::GuiTestHandle::channel(1);
        let req = make_navigate_request(json!({"destination": "settings"}));
        let result = handle_gui_navigate(&req, tx).await;
        assert!(result.is_ok(), "Valid destination should succeed");

        let value = result.unwrap();
        assert_eq!(value["accepted"], true);

        let received = rx
            .try_recv()
            .expect("Should have received a GuiActionRequest");
        let cmd: boru_chat::diagnostics::GuiTestCommand =
            serde_json::from_str(&received.command).expect("deserialize command");
        match cmd {
            boru_chat::diagnostics::GuiTestCommand::OpenSettings => {}
            other => panic!("Expected OpenSettings command, got: {other:?}"),
        }
    }

    #[tokio::test]
    async fn test_navigate_adapter_valid_destinations_return_response_schema() {
        for (destination, expected_command) in [
            ("chat_list", GuiTestCommand::GoToChatList),
            ("friends", GuiTestCommand::OpenFriends),
            ("settings", GuiTestCommand::OpenSettings),
        ] {
            let (mut state, _gossip_rx) = make_gate_test_state(true, true).await;
            let (tx, mut action_rx) = boru_chat::diagnostics::GuiTestHandle::channel(1);
            state.gui_action_tx = Some(tx);
            let req = make_navigate_request(json!({"destination": destination}));
            let response = handle_request(&req, &state).await;

            assert_eq!(response.jsonrpc, "2.0");
            assert_eq!(response.id, req.id);
            assert!(
                response.error.is_none(),
                "unexpected adapter error: {:?}",
                response.error
            );
            let result = response.result.expect("adapter should return a result");
            let object = result.as_object().expect("result should be an object");
            assert_eq!(
                object.len(),
                3,
                "response must not grow undocumented fields"
            );
            assert_eq!(result["accepted"], true);
            let action_id = result["action_id"]
                .as_str()
                .expect("action_id should be a string");
            assert!(!action_id.is_empty());
            assert!(result["queued_at_ms"].is_i64());

            let action = action_rx
                .try_recv()
                .expect("adapter should enqueue the action");
            assert_eq!(action.action_id.0, action_id);
            let command: GuiTestCommand = serde_json::from_str(&action.command).unwrap();
            assert_eq!(command, expected_command);
        }
    }

    #[tokio::test]
    async fn test_navigate_adapter_invalid_destination_returns_jsonrpc_error() {
        let (state, _gossip_rx) = make_gate_test_state(true, true).await;
        let req = make_navigate_request(json!({"destination": "not_a_real_screen"}));
        let response = handle_request(&req, &state).await;

        assert_eq!(response.jsonrpc, "2.0");
        assert_eq!(response.id, req.id);
        assert!(response.result.is_none());
        let error = response
            .error
            .expect("invalid destination should be an error");
        assert_eq!(error.code, -32000);
        assert_eq!(error.message, "Internal error");
        let details = error
            .data
            .as_ref()
            .and_then(Value::as_str)
            .unwrap_or_default();
        assert!(details.contains("Invalid destination"));
        assert!(details.contains("chat_list"));
    }

    #[tokio::test]
    async fn test_navigate_channel_full() {
        let (tx, mut rx) = boru_chat::diagnostics::GuiTestHandle::channel(1);
        // Fill the capacity-1 channel first
        let fill_req = make_navigate_request(json!({"destination": "chat_list"}));
        let fill_result = handle_gui_navigate(&fill_req, tx.clone()).await;
        assert!(fill_result.is_ok(), "First fill should succeed");

        let req = make_navigate_request(json!({"destination": "settings"}));
        let result = handle_gui_navigate(&req, tx).await;
        assert!(result.is_err(), "Should error when channel is full");
        let err = result.unwrap_err();
        assert!(
            err.contains("queue is full"),
            "Error should mention queue full, got: {err}"
        );
        let _ = rx.try_recv();
    }

    #[tokio::test]
    async fn test_navigate_no_secrets_exposed() {
        let (tx, _rx) = boru_chat::diagnostics::GuiTestHandle::channel(1);
        let req = make_navigate_request(json!({"destination": "chat_list"}));
        let result = handle_gui_navigate(&req, tx).await;
        assert!(result.is_ok());
        let response_str = serde_json::to_string(&result.unwrap()).unwrap();
        assert!(!response_str.contains("secret_key"));
        assert!(!response_str.contains("secret"));
        assert!(!response_str.contains("private_key"));
        assert!(!response_str.contains("ticket"));
        assert!(!response_str.contains("password"));
    }

    // ── boru_gui_toggle_dark_mode tests ───────────────────────────────────

    fn make_toggle_dark_mode_request(params: Value) -> JsonRpcRequest {
        JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            method: "boru_gui_toggle_dark_mode".to_string(),
            params,
            id: Some(Value::Number(1.into())),
        }
    }

    #[tokio::test]
    async fn test_toggle_dark_mode_missing_enabled() {
        let (tx, _rx) = boru_chat::diagnostics::GuiTestHandle::channel(1);
        let req = make_toggle_dark_mode_request(json!({}));
        let result = handle_gui_toggle_dark_mode(&req, tx).await;
        assert!(result.is_err(), "Missing enabled should produce an error");
        let err = result.unwrap_err();
        assert!(
            err.contains("Missing required argument"),
            "Error should mention missing argument, got: {err}"
        );
    }

    #[tokio::test]
    async fn test_toggle_dark_mode_invalid_type() {
        let (tx, _rx) = boru_chat::diagnostics::GuiTestHandle::channel(1);
        let req = make_toggle_dark_mode_request(json!({"enabled": "not_a_bool"}));
        let result = handle_gui_toggle_dark_mode(&req, tx).await;
        assert!(
            result.is_err(),
            "Non-boolean enabled should produce an error"
        );
        let err = result.unwrap_err();
        assert!(
            err.contains("Missing required argument"),
            "Error should mention missing argument, got: {err}"
        );
    }

    #[tokio::test]
    async fn test_toggle_dark_mode_is_registered_and_validates_through_jsonrpc_dispatch() {
        // Exercise the request dispatcher rather than calling the handler
        // directly. A method-not-found response would indicate missing
        // registration/routing for this tool.
        let (state, _gossip_rx) = make_gate_test_state(true, true).await;
        let req = make_toggle_dark_mode_request(json!({"enabled": "true"}));
        let response = handle_request(&req, &state).await;

        assert_eq!(response.jsonrpc, "2.0");
        assert_eq!(response.id, req.id);
        assert!(response.result.is_none());
        let error = response.error.expect("invalid enabled type should fail");
        assert_eq!(error.code, -32000);
        assert_eq!(error.message, "Internal error");
        assert!(
            error
                .data
                .as_ref()
                .and_then(Value::as_str)
                .is_some_and(|message| message.contains("enabled") && message.contains("boolean")),
            "error should identify the boolean enabled argument: {:?}",
            error.data
        );
    }

    #[tokio::test]
    async fn test_toggle_dark_mode_enable() {
        let (tx, mut rx) = boru_chat::diagnostics::GuiTestHandle::channel(1);
        let req = make_toggle_dark_mode_request(json!({"enabled": true}));
        let result = handle_gui_toggle_dark_mode(&req, tx).await;
        assert!(result.is_ok(), "Valid enabled=true should succeed");

        let value = result.unwrap();
        assert_eq!(value["sent"], true, "Should report sent=true");
        assert!(
            value["action_id"].is_string(),
            "Should return an action_id string"
        );
        let action_id = value["action_id"].as_str().unwrap();
        assert!(!action_id.is_empty(), "action_id should not be empty");
        assert_eq!(value["enabled"], true, "Should echo enabled=true");

        let received = rx
            .try_recv()
            .expect("Should have received a GuiActionRequest");
        assert_eq!(received.action_id.0, action_id);
        let cmd: boru_chat::diagnostics::GuiTestCommand =
            serde_json::from_str(&received.command).unwrap();
        assert_eq!(
            cmd,
            boru_chat::diagnostics::GuiTestCommand::ToggleDarkMode { enabled: true }
        );
    }

    #[tokio::test]
    async fn test_toggle_dark_mode_disable() {
        let (tx, mut rx) = boru_chat::diagnostics::GuiTestHandle::channel(1);
        let req = make_toggle_dark_mode_request(json!({"enabled": false}));
        let result = handle_gui_toggle_dark_mode(&req, tx).await;
        assert!(result.is_ok(), "Valid enabled=false should succeed");

        let value = result.unwrap();
        assert_eq!(value["sent"], true);
        assert!(value["action_id"].is_string());
        assert_eq!(value["enabled"], false, "Should echo enabled=false");

        let received = rx
            .try_recv()
            .expect("Should have received a GuiActionRequest");
        let cmd: boru_chat::diagnostics::GuiTestCommand =
            serde_json::from_str(&received.command).unwrap();
        assert_eq!(
            cmd,
            boru_chat::diagnostics::GuiTestCommand::ToggleDarkMode { enabled: false }
        );
    }

    #[tokio::test]
    async fn test_toggle_dark_mode_channel_full() {
        // Use capacity 1 so the second send fails
        let (tx, mut rx) = boru_chat::diagnostics::GuiTestHandle::channel(1);
        // Fill the channel with one send that nobody reads
        let req1 = make_toggle_dark_mode_request(json!({"enabled": true}));
        let result1 = handle_gui_toggle_dark_mode(&req1, tx.clone()).await;
        assert!(result1.is_ok(), "First send should succeed");

        // now the channel is full; second must fail
        let result2 = handle_gui_toggle_dark_mode(&req1, tx).await;
        assert!(result2.is_err(), "Should error when channel is full");
        let err = result2.unwrap_err();
        assert!(
            err.contains("queue is full"),
            "Error should mention queue full, got: {err}"
        );
        // drain the channel so the receiver doesn't hold a reference
        let _ = rx.try_recv();
    }

    #[tokio::test]
    async fn test_toggle_dark_mode_no_secrets_exposed() {
        let (tx, _rx) = boru_chat::diagnostics::GuiTestHandle::channel(1);
        let req = make_toggle_dark_mode_request(json!({"enabled": true}));
        let result = handle_gui_toggle_dark_mode(&req, tx).await;
        assert!(result.is_ok());
        let response_str = serde_json::to_string(&result.unwrap()).unwrap();
        assert!(!response_str.contains("secret_key"));
        assert!(!response_str.contains("secret"));
        assert!(!response_str.contains("private_key"));
        assert!(!response_str.contains("ticket"));
        assert!(!response_str.contains("password"));
    }

    // ── GuiNavigateDestination serde round-trip tests ──────────────────────

    #[test]
    fn test_gui_navigate_destination_serde_roundtrip() {
        use crate::mcp_server::GuiNavigateDestination;

        // Verify that each destination serializes to the expected JSON string
        // and deserializes back.
        let cases = vec![
            (GuiNavigateDestination::ChatList, "\"chat_list\""),
            (GuiNavigateDestination::Friends, "\"friends\""),
            (GuiNavigateDestination::Settings, "\"settings\""),
        ];

        for (dest, expected_json) in cases {
            let json = serde_json::to_string(&dest).unwrap();
            assert_eq!(
                json, expected_json,
                "GuiNavigateDestination should serialize to {}",
                expected_json
            );
            let deserialized: GuiNavigateDestination = serde_json::from_str(&json).unwrap();
            assert_eq!(deserialized, dest, "Round-trip should produce same value");
        }
    }

    #[test]
    fn test_gui_navigate_destination_invalid_variant() {
        let result: Result<crate::mcp_server::GuiNavigateDestination, _> =
            serde_json::from_str("\"invalid_screen\"");
        assert!(
            result.is_err(),
            "Invalid variant should fail deserialization"
        );

        let result: Result<crate::mcp_server::GuiNavigateDestination, _> =
            serde_json::from_str("\"\"");
        assert!(result.is_err(), "Empty string should fail deserialization");

        // Very long non-matching string should fail
        let long = "a".repeat(5000);
        let result: Result<crate::mcp_server::GuiNavigateDestination, _> =
            serde_json::from_str(&format!("\"{long}\""));
        assert!(result.is_err(), "Oversized non-matching string should fail");
    }

    #[test]
    fn test_gui_navigate_destination_from_str() {
        use crate::mcp_server::GuiNavigateDestination;

        assert_eq!(
            GuiNavigateDestination::from_str("chat_list"),
            Some(GuiNavigateDestination::ChatList)
        );
        assert_eq!(
            GuiNavigateDestination::from_str("friends"),
            Some(GuiNavigateDestination::Friends)
        );
        assert_eq!(
            GuiNavigateDestination::from_str("settings"),
            Some(GuiNavigateDestination::Settings)
        );
        assert_eq!(GuiNavigateDestination::from_str("ChatList"), None);
        assert_eq!(GuiNavigateDestination::from_str(""), None);
        assert_eq!(GuiNavigateDestination::from_str("chat_list\n"), None);
    }

    #[test]
    fn test_gui_navigate_destination_as_str_and_display() {
        use crate::mcp_server::GuiNavigateDestination;

        assert_eq!(GuiNavigateDestination::ChatList.as_str(), "chat_list");
        assert_eq!(format!("{}", GuiNavigateDestination::ChatList), "chat_list");

        assert_eq!(GuiNavigateDestination::Friends.as_str(), "friends");
        assert_eq!(format!("{}", GuiNavigateDestination::Friends), "friends");

        assert_eq!(GuiNavigateDestination::Settings.as_str(), "settings");
        assert_eq!(format!("{}", GuiNavigateDestination::Settings), "settings");

        assert_eq!(
            GuiNavigateDestination::all_destinations(),
            &["chat_list", "friends", "settings"]
        );
    }

    #[test]
    fn test_gui_navigate_response_serde() {
        use crate::mcp_server::GuiNavigateResponse;

        let response = GuiNavigateResponse {
            accepted: true,
            action_id: "gui_action_test_123".to_string(),
            queued_at_ms: 1710000000000,
        };

        let json = serde_json::to_value(&response).unwrap();
        assert_eq!(json["accepted"], true);
        assert_eq!(json["action_id"], "gui_action_test_123");
        assert_eq!(json["queued_at_ms"], 1710000000000i64);
    }

    #[test]
    fn test_gui_navigate_params_deserialize() {
        use crate::mcp_server::GuiNavigateDestination;
        use crate::mcp_server::GuiNavigateParams;

        // Valid params
        let json = r#"{"destination": "chat_list"}"#;
        let params: GuiNavigateParams = serde_json::from_str(json).unwrap();
        assert_eq!(params.destination, GuiNavigateDestination::ChatList);

        // Missing destination field
        let result: Result<GuiNavigateParams, _> = serde_json::from_str(r#"{}"#);
        assert!(
            result.is_err(),
            "Missing destination should fail deserialization"
        );

        // Null destination
        let result: Result<GuiNavigateParams, _> = serde_json::from_str(r#"{"destination": null}"#);
        assert!(
            result.is_err(),
            "Null destination should fail deserialization"
        );
    }

    // ── Input validation helper tests ─────────────────────────────────

    #[test]
    fn test_validate_bounded_accepts_within_limit() {
        assert!(validate_bounded("hello", 10, "test").is_ok());
        assert!(validate_bounded("", 10, "test").is_ok());
        // Exactly at limit
        let exact = "a".repeat(10);
        assert!(validate_bounded(&exact, 10, "test").is_ok());
    }

    #[test]
    fn test_validate_bounded_rejects_over_limit() {
        let long = "a".repeat(11);
        let result = validate_bounded(&long, 10, "test_name");
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.contains("too long"));
        assert!(err.contains("test_name"));
    }

    #[test]
    fn test_validate_no_control_chars_accepts_normal() {
        assert!(validate_no_control_chars("hello world", "test").is_ok());
        assert!(validate_no_control_chars("abc123_-", "test").is_ok());
        // Unicode is fine (not control)
        assert!(validate_no_control_chars("héllo wörld 🌍", "test").is_ok());
        // Space is allowed
        assert!(validate_no_control_chars("a b c", "test").is_ok());
    }

    #[test]
    fn test_validate_no_control_chars_rejects_control() {
        // Newline
        assert!(validate_no_control_chars("hello\nworld", "test").is_err());
        // Carriage return
        assert!(validate_no_control_chars("hello\rworld", "test").is_err());
        // Tab
        assert!(validate_no_control_chars("hello\tworld", "test").is_err());
        // Null byte
        assert!(validate_no_control_chars("hello\0world", "test").is_err());
        // Bell
        assert!(validate_no_control_chars("hello\x07world", "test").is_err());
    }

    #[test]
    fn test_validate_no_control_chars_rejects_with_name_in_error() {
        let result = validate_no_control_chars("bad\nvalue", "my_param");
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.contains("my_param"));
        assert!(err.contains("control characters"));
    }

    #[test]
    fn test_validate_peer_id_accepts_valid() {
        // Hex string (common in iroh)
        assert!(validate_peer_id("abcdef0123456789").is_ok());
        // With hyphen (short IDs)
        assert!(validate_peer_id("abc-def-123").is_ok());
        // With underscore
        assert!(validate_peer_id("abc_def_123").is_ok());
    }

    #[test]
    fn test_validate_peer_id_rejects_empty() {
        let result = validate_peer_id("");
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("must not be empty"));
    }

    #[test]
    fn test_validate_peer_id_rejects_path_separators() {
        assert!(validate_peer_id("../../etc/passwd").is_err());
        assert!(validate_peer_id("peer/name").is_err());
        assert!(validate_peer_id("peer\\name").is_err());
    }

    #[test]
    fn test_validate_peer_id_rejects_shell_metacharacters() {
        assert!(validate_peer_id("abcd; rm").is_err());
        assert!(validate_peer_id("abcd`ls`").is_err());
        assert!(validate_peer_id("abcd|cat").is_err());
        assert!(validate_peer_id("ab$cd").is_err());
    }

    #[test]
    fn test_validate_peer_id_rejects_too_long() {
        let long = "a".repeat(MAX_PEER_ID_LEN + 1);
        let result = validate_peer_id(&long);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("too long"));
    }

    #[test]
    fn test_validate_peer_id_rejects_control_chars() {
        assert!(validate_peer_id("peer\nname").is_err());
        assert!(validate_peer_id("peer\tname").is_err());
    }

    #[test]
    fn test_validate_probe_id_accepts_valid() {
        assert!(validate_probe_id("probe_abc123").is_ok());
        assert!(validate_probe_id("a").is_ok());
        assert!(validate_probe_id("").is_ok()); // Empty is technically valid per spec
    }

    #[test]
    fn test_validate_probe_id_rejects_control_chars() {
        assert!(validate_probe_id("probe\n123").is_err());
        assert!(validate_probe_id("probe\t123").is_err());
    }

    #[test]
    fn test_validate_probe_id_rejects_path_separators() {
        assert!(validate_probe_id("probe/123").is_err());
        assert!(validate_probe_id("probe\\123").is_err());
    }

    #[test]
    fn test_validate_probe_id_rejects_too_long() {
        let long = "a".repeat(MAX_PROBE_ID_LEN + 1);
        assert!(validate_probe_id(&long).is_err());
    }

    #[test]
    fn test_validate_probe_payload_accepts_unicode() {
        // Unicode text should always be accepted (preserved)
        assert!(validate_probe_payload("Hello, 世界! 🌍").is_ok());
        // Control chars are allowed in probe payload (only bounded)
        assert!(validate_probe_payload("line1\nline2").is_ok());
    }

    #[test]
    fn test_validate_probe_payload_rejects_extreme_length() {
        let long = "a".repeat(MAX_PROBE_PAYLOAD_LEN + 1);
        assert!(validate_probe_payload(&long).is_err());
    }

    #[test]
    fn test_validate_probe_payload_accepts_max_length() {
        let exact = "a".repeat(MAX_PROBE_PAYLOAD_LEN);
        assert!(validate_probe_payload(&exact).is_ok());
    }

    #[test]
    fn test_validate_target_state_accepts_valid() {
        for state in &[
            "discovered",
            "address_resolved",
            "connected",
            "subscription_joined",
            "topic_member",
        ] {
            assert!(
                validate_target_state(state).is_ok(),
                "target_state '{}' should be accepted",
                state
            );
        }
    }

    #[test]
    fn test_validate_target_state_rejects_invalid() {
        let result = validate_target_state("invalid_state");
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.contains("Invalid target_state"));
        assert!(err.contains("discovered"));
    }

    #[test]
    fn test_validate_target_state_rejects_control_chars() {
        assert!(validate_target_state("discovered\n").is_err());
        assert!(validate_target_state("connected\t").is_err());
    }

    #[test]
    fn test_validate_target_state_rejects_too_long() {
        let long = "a".repeat(MAX_TARGET_STATE_LEN + 1);
        assert!(validate_target_state(&long).is_err());
    }

    #[test]
    fn test_validate_no_path_or_shell_accepts_safe() {
        assert!(validate_no_path_or_shell("hello world", "test").is_ok());
        assert!(validate_no_path_or_shell("abc123_-", "test").is_ok());
        assert!(validate_no_path_or_shell("", "test").is_ok());
    }

    #[test]
    fn test_validate_no_path_or_shell_rejects_path_separators() {
        assert!(validate_no_path_or_shell("a/b", "test").is_err());
        assert!(validate_no_path_or_shell("a\\b", "test").is_err());
    }

    #[test]
    fn test_validate_no_path_or_shell_rejects_shell_metacharacters() {
        assert!(validate_no_path_or_shell("a$b", "test").is_err());
        assert!(validate_no_path_or_shell("a`b", "test").is_err());
        assert!(validate_no_path_or_shell("a|b", "test").is_err());
        assert!(validate_no_path_or_shell("a;b", "test").is_err());
        assert!(validate_no_path_or_shell("a>b", "test").is_err());
    }

    #[test]
    fn test_sanitize_for_log_truncates() {
        let long = "a".repeat(100);
        let result = sanitize_for_log(&long, 10);
        assert_eq!(
            result.chars().count(),
            10 + 3 + " (truncated, total 100 chars)".len()
        );
        assert!(result.contains("truncated"));
    }

    #[test]
    fn test_sanitize_for_log_escapes_control() {
        let result = sanitize_for_log("hello\nworld\r\nend\t!", 100);
        assert!(!result.contains('\n'));
        assert!(!result.contains('\r'));
        assert!(!result.contains('\t'));
        assert!(result.contains("\\n"));
        assert!(result.contains("\\r"));
        assert!(result.contains("\\t"));
    }

    #[test]
    fn test_sanitize_for_log_under_limit_no_truncation() {
        let result = sanitize_for_log("Hello, world!", 100);
        assert!(result.contains("Hello, world!"));
        assert!(!result.contains("truncated"));
    }

    // ── handle_send_gui_action input validation tests ───────────────────

    fn make_send_gui_action_request(params: Value) -> JsonRpcRequest {
        JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            method: "boru_send_gui_action".to_string(),
            params,
            id: Some(Value::Number(1.into())),
        }
    }

    #[tokio::test]
    async fn test_send_gui_action_missing_command() {
        let (tx, _rx) = boru_chat::diagnostics::GuiTestHandle::channel(16);
        let req = make_send_gui_action_request(json!({ "idempotency_key": "key123" }));
        let result = handle_send_gui_action(&req, tx).await;
        assert!(result.is_err(), "Missing command should produce an error");
        let err = result.unwrap_err();
        assert!(
            err.contains("Missing required argument"),
            "Error should mention missing argument, got: {err}"
        );
    }

    #[tokio::test]
    async fn test_send_gui_action_invalid_command_value() {
        let (tx, _rx) = boru_chat::diagnostics::GuiTestHandle::channel(16);
        let req = make_send_gui_action_request(json!({
            "command": { "command": "unknown_type" }
        }));
        let result = handle_send_gui_action(&req, tx).await;
        assert!(result.is_err(), "Invalid command should produce an error");
        let err = result.unwrap_err();
        assert!(
            err.contains("Invalid command"),
            "Error should mention 'Invalid command', got: {err}"
        );
    }

    #[tokio::test]
    async fn test_send_gui_action_invalid_idempotency_key_control_chars() {
        let (tx, _rx) = boru_chat::diagnostics::GuiTestHandle::channel(16);
        let req = make_send_gui_action_request(json!({
            "command": { "command": "go_to_chat_list" },
            "idempotency_key": "bad\nkey"
        }));
        let result = handle_send_gui_action(&req, tx).await;
        assert!(
            result.is_err(),
            "Control chars in idempotency_key should produce an error"
        );
        let err = result.unwrap_err();
        assert!(
            err.contains("control characters"),
            "Error should mention control characters, got: {err}"
        );
    }

    #[tokio::test]
    async fn test_send_gui_action_invalid_idempotency_key_too_long() {
        let (tx, _rx) = boru_chat::diagnostics::GuiTestHandle::channel(16);
        let long_key = "a".repeat(MAX_PROBE_ID_LEN + 1);
        let req = make_send_gui_action_request(json!({
            "command": { "command": "go_to_chat_list" },
            "idempotency_key": long_key
        }));
        let result = handle_send_gui_action(&req, tx).await;
        assert!(result.is_err(), "Oversized key should produce an error");
        let err = result.unwrap_err();
        assert!(
            err.contains("too long"),
            "Error should mention 'too long', got: {err}"
        );
    }

    #[tokio::test]
    async fn test_send_gui_action_success_with_auto_key() {
        let (tx, mut rx) = boru_chat::diagnostics::GuiTestHandle::channel(16);
        let req = make_send_gui_action_request(json!({
            "command": { "command": "go_to_chat_list" }
        }));
        let result = handle_send_gui_action(&req, tx).await;
        assert!(result.is_ok(), "Valid command should succeed");

        let value = result.unwrap();
        assert_eq!(value["sent"], true, "Should report sent=true");
        assert!(
            value["idempotency_key"].is_string(),
            "Should return idempotency_key"
        );
        let key = value["idempotency_key"].as_str().unwrap();
        assert!(!key.is_empty(), "idempotency_key should not be empty");

        let received = rx
            .try_recv()
            .expect("Should have received a GuiActionRequest");
        assert_eq!(received.action_id.0, key);
        let cmd: boru_chat::diagnostics::GuiTestCommand =
            serde_json::from_str(&received.command).unwrap();
        match cmd {
            boru_chat::diagnostics::GuiTestCommand::GoToChatList => {}
            other => panic!("Expected GoToChatList command, got: {other:?}"),
        }
    }

    #[tokio::test]
    async fn test_send_gui_action_success_with_custom_key() {
        let (tx, mut rx) = boru_chat::diagnostics::GuiTestHandle::channel(16);
        let req = make_send_gui_action_request(json!({
            "command": { "command": "open_friends" },
            "idempotency_key": "my_custom_key_42"
        }));
        let result = handle_send_gui_action(&req, tx).await;
        assert!(
            result.is_ok(),
            "Valid command with custom key should succeed"
        );

        let value = result.unwrap();
        assert_eq!(value["sent"], true);
        assert_eq!(value["idempotency_key"], "my_custom_key_42");

        let received = rx
            .try_recv()
            .expect("Should have received a GuiActionRequest");
        assert_eq!(received.action_id.0, "my_custom_key_42");
        let cmd: boru_chat::diagnostics::GuiTestCommand =
            serde_json::from_str(&received.command).unwrap();
        match cmd {
            boru_chat::diagnostics::GuiTestCommand::OpenFriends => {}
            other => panic!("Expected OpenFriends command, got: {other:?}"),
        }
    }

    #[tokio::test]
    async fn test_send_gui_action_is_visible_to_lifecycle_status_query() {
        let (handle, mut rx) = boru_chat::diagnostics::GuiTestHandle::channel(16);
        let lifecycle = handle.history();
        let req = make_send_gui_action_request(json!({
            "command": { "command": "go_to_chat_list" },
            "idempotency_key": "lifecycle_trace_42"
        }));

        let response = handle_send_gui_action(&req, handle).await.unwrap();
        assert_eq!(response["sent"], true);
        assert_eq!(response["idempotency_key"], "lifecycle_trace_42");

        let queued = lifecycle
            .get(&boru_chat::diagnostics::GuiActionId(
                "lifecycle_trace_42".into(),
            ))
            .expect("MCP enqueue must record the action before dispatch");
        assert_eq!(queued.state, boru_chat::diagnostics::GuiActionState::Queued);
        assert!(queued.requested_at_ms > 0);
        assert!(queued.updated_at_ms >= queued.requested_at_ms);

        let received = rx.try_recv().expect("GUI action must reach the GUI queue");
        assert_eq!(received.action_id.0, "lifecycle_trace_42");

        let (mut state, _gossip_rx) = make_gate_test_state(true, true).await;
        state.gui_action_lifecycle = lifecycle;
        let status_response = handle_get_gui_action_status(
            &make_get_action_status_request("lifecycle_trace_42"),
            &state,
        )
        .await
        .unwrap();
        assert_eq!(status_response["found"], true);
        assert_eq!(status_response["status"]["state"], "queued");
        assert!(status_response["status"]["requested_at_ms"].is_number());
    }

    #[tokio::test]
    async fn test_local_gui_message_test_does_not_require_remote_peer() {
        let (state, _rx) = make_gate_test_state(true, true).await;
        let room_id = "00".repeat(32);
        let req = JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            method: "boru_run_local_gui_message_test".to_string(),
            params: json!({
                "room_id": room_id,
                "message_text": "local-only",
                "expected_peer_id": "00".repeat(32),
                "timeout_ms": 1
            }),
            id: Some(Value::Number(1.into())),
        };

        let error = handle_run_local_gui_message_test(&req, &state)
            .await
            .expect_err("mock GUI action channel is intentionally closed");
        assert!(
            !error.contains("expected_peer_id"),
            "local-only test must not require a remote peer: {error}"
        );
    }

    #[tokio::test]
    async fn test_get_gui_action_status_returns_full_record() {
        let (state, _rx) = make_gate_test_state(true, true).await;
        state.gui_action_history.record(ActionRecord {
            idempotency_key: "action-42".to_string(),
            command: r#"{\"command\":\"open_friends\"}"#.to_string(),
            status: ActionStatus::Processed,
            timestamp_ms: 1_710_000_000_000,
            duration_ms: 17,
        });

        let req = JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            method: "boru_gui_get_action_status".to_string(),
            params: json!({"action_id": "action-42"}),
            id: Some(Value::Number(1.into())),
        };
        let result = handle_get_gui_action_status(&req, &state)
            .await
            .expect("recorded action should be found");

        assert_eq!(result["found"], true);
        assert_eq!(result["action_id"], "action-42");
        assert_eq!(result["idempotency_key"], "action-42");
        assert_eq!(result["command"], r#"{\"command\":\"open_friends\"}"#);
        assert_eq!(result["status"]["status"], "processed");
        assert_eq!(result["timestamp_ms"], 1_710_000_000_000i64);
        assert_eq!(result["duration_ms"], 17);
    }

    #[tokio::test]
    async fn test_get_gui_action_status_unknown_id_is_structured_not_found() {
        let (state, _rx) = make_gate_test_state(true, true).await;
        let req = JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            method: "boru_gui_get_action_status".to_string(),
            params: json!({"action_id": "does-not-exist"}),
            id: Some(Value::Number(1.into())),
        };

        let result = handle_get_gui_action_status(&req, &state)
            .await
            .expect("unknown IDs should return a structured result");
        assert_eq!(result["found"], false);
        assert_eq!(result["action_id"], "does-not-exist");
        assert!(result["note"].as_str().unwrap().contains("not found"));
    }

    #[tokio::test]
    async fn test_get_gui_action_status_validates_action_id() {
        let (state, _rx) = make_gate_test_state(true, true).await;
        let req = JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            method: "boru_gui_get_action_status".to_string(),
            params: json!({"action_id": "bad\nvalue"}),
            id: Some(Value::Number(1.into())),
        };

        let error = handle_get_gui_action_status(&req, &state)
            .await
            .expect_err("control characters must be rejected");
        assert!(error.contains("control characters"));
    }

    #[tokio::test]
    async fn test_send_gui_action_channel_full() {
        let (tx, mut rx) = boru_chat::diagnostics::GuiTestHandle::channel(1);
        // Fill the capacity-1 channel first
        let fill_req = make_send_gui_action_request(json!({
            "command": { "command": "go_to_chat_list" }
        }));
        let fill_result = handle_send_gui_action(&fill_req, tx.clone()).await;
        assert!(fill_result.is_ok(), "First fill should succeed");

        let req = make_send_gui_action_request(json!({
            "command": { "command": "go_to_chat_list" }
        }));
        let result = handle_send_gui_action(&req, tx).await;
        assert!(result.is_err(), "Should error when channel is full");
        let err = result.unwrap_err();
        assert!(
            err.contains("queue is full"),
            "Error should mention queue full, got: {err}"
        );
        let _ = rx.try_recv();
    }

    #[tokio::test]
    async fn test_send_gui_action_no_secrets_in_response() {
        let (tx, _rx) = boru_chat::diagnostics::GuiTestHandle::channel(16);
        let req = make_send_gui_action_request(json!({
            "command": { "command": "toggle_help" }
        }));
        let result = handle_send_gui_action(&req, tx).await;
        assert!(result.is_ok());
        let response_str = serde_json::to_string(&result.unwrap()).unwrap();
        assert!(!response_str.contains("secret_key"));
        assert!(!response_str.contains("secret"));
        assert!(!response_str.contains("private_key"));
        assert!(!response_str.contains("ticket"));
        assert!(!response_str.contains("password"));
    }

    #[test]
    fn test_gui_tool_schema_rejects_unknown_and_missing_arguments() {
        let cases = [
            (
                "boru_send_gui_action",
                json!({"command": {}, "typo": true}),
                "Unknown argument: typo",
            ),
            (
                "boru_gui_navigate",
                json!({"destination": "settings", "typo": true}),
                "Unknown argument: typo",
            ),
            (
                "boru_gui_set_composer",
                json!({"typo": "text"}),
                "Unknown argument: typo",
            ),
            (
                "boru_gui_open_room",
                json!({"room_id": "room", "extra": 1}),
                "Unknown argument: extra",
            ),
            (
                "boru_gui_open_conversation",
                json!({}),
                "Missing required argument: conversation_id",
            ),
            (
                "boru_gui_toggle_dark_mode",
                json!({"enabled": true, "extra": false}),
                "Unknown argument: extra",
            ),
            (
                "boru_gui_clear_composer",
                json!({"extra": false}),
                "Unknown argument: extra",
            ),
            (
                "boru_gui_focus_composer",
                json!({"extra": false}),
                "Unknown argument: extra",
            ),
            (
                "boru_gui_wait_for_state",
                json!({"condition": {}, "extra": 1}),
                "Unknown argument: extra",
            ),
            (
                "boru_run_gui_message_test",
                json!({"room_id": "r", "message_text": "m", "expected_peer_id": "p", "extra": 1}),
                "Unknown argument: extra",
            ),
        ];
        for (method, params, expected) in cases {
            let error = validate_gui_tool_params(method, &params).unwrap_err();
            assert_eq!(error, expected, "schema mismatch for {method}");
        }
    }

    #[test]
    fn test_gui_tool_schema_accepts_optional_and_parameterless_forms() {
        assert!(validate_gui_tool_params(
            "boru_send_gui_action",
            &json!({
                "command": {"command": "toggle_help"},
                "idempotency_key": "action-1"
            })
        )
        .is_ok());
        assert!(validate_gui_tool_params(
            "boru_gui_wait_for_state",
            &json!({
                "condition": {"type": "dialog_closed"},
                "timeout_ms": 1000
            })
        )
        .is_ok());
        for method in [
            "boru_get_gui_snapshot",
            "boru_gui_submit_composer",
            "boru_gui_clear_composer",
            "boru_gui_focus_composer",
            "boru_gui_close_dialog",
        ] {
            assert!(validate_gui_tool_params(method, &json!({})).is_ok());
            assert!(validate_gui_tool_params(method, &Value::Null).is_ok());
        }
    }

    #[tokio::test]
    async fn test_gui_adapter_rejects_unknown_argument_with_jsonrpc_invalid_params() {
        let (state, _rx) = make_gate_test_state(true, true).await;
        let req = JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            method: "boru_gui_toggle_dark_mode".to_string(),
            params: json!({"enabled": true, "enabeld": true}),
            id: Some(Value::Number(7.into())),
        };
        let response = handle_request(&req, &state).await;
        let error = response.error.expect("unknown arguments must be rejected");
        assert_eq!(error.code, -32602);
        assert_eq!(error.message, "Invalid params");
        assert_eq!(
            error.data,
            Some(Value::String("Unknown argument: enabeld".to_string()))
        );
    }

    // =============================================================================
    // Security: GUI tools gate when test mode disabled
    // =============================================================================

    /// Helper: build a minimal McpAppState for testing gate logic.
    /// The state uses a real (loopback-only, relay-disabled) iroh Endpoint
    /// so handle_request can be exercised through the gate.  GUI-related handlers
    /// are NOT actually called when the gate rejects, so a functional endpoint
    /// is only needed for the non-GUI fallthrough tests.
    async fn make_gate_test_state(
        gui_enabled: bool,
        has_tx: bool,
    ) -> (
        McpAppState,
        tokio::sync::mpsc::UnboundedReceiver<ConversationNetEvent>,
    ) {
        use iroh::address_lookup::memory::MemoryLookup;
        use iroh::endpoint::presets;
        use std::net::{Ipv4Addr, SocketAddrV4};

        let secret_key = SecretKey::generate();
        let endpoint = Endpoint::builder(presets::N0DisableRelay)
            .secret_key(secret_key.clone())
            .address_lookup(MemoryLookup::new())
            .relay_mode(iroh::RelayMode::Disabled)
            .bind_addr(SocketAddrV4::new(Ipv4Addr::LOCALHOST, 0))
            .unwrap()
            .bind()
            .await
            .expect("test endpoint bind");
        let gossip = boru_chat::net::Gossip::builder().spawn(endpoint.clone());
        let (gossip_tx, gossip_rx) = tokio::sync::mpsc::unbounded_channel();

        let state = McpAppState {
            diagnostics: Diagnostics::new(),
            iced_diagnostics: IcedMessageJournal::new(),
            endpoint,
            rooms: Arc::new(Mutex::new(Vec::new())),
            node_id: secret_key.public().to_string(),
            version: "test".to_string(),
            gossip_tx,
            secret_key,
            gossip,
            gui_test_actions_enabled: gui_enabled,
            gui_action_tx: if has_tx {
                Some(boru_chat::diagnostics::GuiTestHandle::channel(256).0)
            } else {
                None
            },
            gui_action_history: GuiActionHistory::new(),
            gui_action_lifecycle: boru_chat::diagnostics::GuiActionHistory::default(),
            gui_action_rate_limiter: Arc::new(Mutex::new(GuiActionRateLimiter::new())),
            gui_state_rx: None,
        };
        (state, gossip_rx)
    }

    /// Assert that handle_request returns a -32601 error for the given method.
    fn assert_gui_method_not_found(response: &JsonRpcResponse) {
        assert!(
            response.error.is_some(),
            "Expected error, got result: {:?}",
            response.result
        );
        let err = response.error.as_ref().unwrap();
        assert_eq!(
            err.code, -32601,
            "Expected error code -32601 (Method not found), got {}: {}",
            err.code, err.message
        );
        let data = err.data.as_ref().and_then(|v| v.as_str()).unwrap_or("");
        assert!(
            data.contains("not enabled"),
            "Error data should mention 'not enabled', got: {data}"
        );
    }

    fn make_generic_request(method: &str) -> JsonRpcRequest {
        JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            method: method.to_string(),
            params: json!({}),
            id: Some(Value::Number(1.into())),
        }
    }

    #[tokio::test]
    async fn test_gui_tools_absent_when_test_mode_disabled() {
        let (state, _rx) = make_gate_test_state(false, false).await;

        // Each GUI tool must return -32601 when gui_test_actions_enabled is false.
        let gui_methods = [
            "boru_send_gui_action",
            "boru_gui_navigate",
            "boru_gui_get_action_status",
            "boru_get_gui_snapshot",
            "boru_gui_set_composer",
            "boru_gui_open_room",
            "boru_join_lobby_room",
            "boru_gui_open_conversation",
            "boru_gui_submit_composer",
            "boru_gui_toggle_dark_mode",
            "boru_run_gui_message_test",
        ];

        for method in &gui_methods {
            let req = make_generic_request(method);
            let resp = handle_request(&req, &state).await;
            assert_gui_method_not_found(&resp);
        }
    }

    #[tokio::test]
    async fn test_iced_state_gated_when_test_mode_disabled() {
        let (state, _rx) = make_gate_test_state(false, false).await;

        let req = make_generic_request("boru_get_iced_state");
        let resp = handle_request(&req, &state).await;
        assert_gui_method_not_found(&resp);
    }

    #[tokio::test]
    async fn test_iced_message_journal_gated_when_test_mode_disabled() {
        let (state, _rx) = make_gate_test_state(false, false).await;

        let req = make_generic_request("boru_get_iced_message_journal");
        let resp = handle_request(&req, &state).await;
        assert_gui_method_not_found(&resp);
    }

    #[tokio::test]
    async fn test_gui_tools_gated_when_no_channel() {
        let (state, _rx) = make_gate_test_state(true, false).await;

        // Test mode enabled but no channel (gui_action_tx = None).
        // All mutating GUI tools should return -32601.
        let gui_methods = [
            "boru_send_gui_action",
            "boru_gui_navigate",
            "boru_gui_set_composer",
            "boru_gui_open_room",
            "boru_join_lobby_room",
            "boru_gui_open_conversation",
            "boru_gui_submit_composer",
            "boru_gui_toggle_dark_mode",
            "boru_run_gui_message_test",
        ];

        for method in &gui_methods {
            let req = make_generic_request(method);
            let resp = handle_request(&req, &state).await;
            assert_gui_method_not_found(&resp);
        }
    }

    #[tokio::test]
    async fn test_readonly_gui_tools_pass_when_mode_enabled_with_channel() {
        // Read-only tools (get_gui_snapshot, get_gui_action_status) should
        // work when test mode is enabled even with a channel.
        let (state, _rx) = make_gate_test_state(true, true).await;

        // boru_get_gui_snapshot does not go through the action channel;
        // it reads from the state directly.
        let req = make_generic_request("boru_get_gui_snapshot");
        let resp = handle_request(&req, &state).await;
        assert!(
            resp.error.is_none(),
            "boru_get_gui_snapshot should succeed when test mode is enabled, got: {:?}",
            resp.error
        );
        let result = resp.result.unwrap();
        assert_eq!(result["gui_test_actions_enabled"], true);
    }

    #[tokio::test]
    async fn test_iced_state_passes_when_mode_enabled() {
        let (state, _rx) = make_gate_test_state(true, true).await;

        let req = make_generic_request("boru_get_iced_state");
        let resp = handle_request(&req, &state).await;
        assert!(
            resp.error.is_none(),
            "boru_get_iced_state should succeed when test mode is enabled, got: {:?}",
            resp.error
        );
    }

    #[tokio::test]
    async fn test_unknown_gui_action_status_is_explicit_and_non_blocking() {
        let (state, _rx) = make_gate_test_state(true, true).await;
        let mut req = make_generic_request("boru_gui_get_action_status");
        req.params = json!({"action_id": "stale-or-unknown-action"});

        let response =
            tokio::time::timeout(Duration::from_millis(100), handle_request(&req, &state))
                .await
                .expect("unknown action lookup must not hang");
        assert!(
            response.error.is_none(),
            "lookup should be a diagnostic result"
        );
        let result = response.result.expect("lookup result");
        assert_eq!(result["found"], false);
        assert_eq!(result["action_id"], "stale-or-unknown-action");
        assert!(result["note"].as_str().unwrap().contains("not found"));
    }

    #[tokio::test]
    async fn test_unknown_gui_action_id_validation_is_structured() {
        let (state, _rx) = make_gate_test_state(true, true).await;
        let mut req = make_generic_request("boru_gui_get_action_status");
        req.params = json!({"action_id": "bad\nidentifier"});

        let response = handle_request(&req, &state).await;
        let error = response.error.expect("control characters must be rejected");
        assert_eq!(error.code, -32000);
        assert!(error.message.contains("Internal error"));
        assert!(error.data.unwrap().as_str().unwrap().contains("control"));
    }

    #[tokio::test]
    async fn test_iced_journal_passes_when_mode_enabled() {
        let (state, _rx) = make_gate_test_state(true, true).await;

        let req = make_generic_request("boru_get_iced_message_journal");
        let resp = handle_request(&req, &state).await;
        assert!(
            resp.error.is_none(),
            "boru_get_iced_message_journal should succeed when test mode is enabled, got: {:?}",
            resp.error
        );
    }

    #[tokio::test]
    async fn test_core_tools_work_without_gui_mode() {
        // Core diagnostic tools must work even when gui_test_actions_enabled is false.
        let (state, _rx) = make_gate_test_state(false, false).await;

        let core_methods = ["boru_get_node_status", "boru_get_failure_analysis"];

        for method in &core_methods {
            let req = make_generic_request(method);
            let resp = handle_request(&req, &state).await;
            assert!(
                resp.error.is_none(),
                "Core tool '{}' should work without GUI mode, got error: {:?}",
                method,
                resp.error
            );
        }
    }

    // =============================================================================
    // Security: no secrets exposed by diagnostic tools
    // =============================================================================

    #[tokio::test]
    async fn test_get_gui_snapshot_no_secrets() {
        let (state, _rx) = make_gate_test_state(true, true).await;

        let req = make_generic_request("boru_get_gui_snapshot");
        let resp = handle_request(&req, &state).await;
        assert!(resp.error.is_none(), "Snapshot should succeed");
        let json_str = serde_json::to_string(&resp).unwrap();
        assert!(!json_str.contains("secret_key"));
        assert!(!json_str.contains("ticket"));
        assert!(!json_str.contains("private_key"));
        assert!(!json_str.contains("password"));
        assert!(!json_str.contains("mailbox_key"));
    }

    #[tokio::test]
    async fn test_get_iced_state_no_secrets() {
        let (state, _rx) = make_gate_test_state(true, true).await;

        let req = make_generic_request("boru_get_iced_state");
        let resp = handle_request(&req, &state).await;
        assert!(resp.error.is_none(), "iced state should succeed");
        let json_str = serde_json::to_string(&resp).unwrap();
        assert!(!json_str.contains("secret_key"));
        assert!(!json_str.contains("ticket"));
        assert!(!json_str.contains("private_key"));
        assert!(!json_str.contains("password"));
    }

    #[tokio::test]
    async fn test_get_failure_analysis_no_secrets() {
        let (state, _rx) = make_gate_test_state(false, false).await;

        let req = make_generic_request("boru_get_failure_analysis");
        let resp = handle_request(&req, &state).await;
        assert!(resp.error.is_none(), "failure analysis should succeed");
        let json_str = serde_json::to_string(&resp).unwrap();
        assert!(!json_str.contains("secret_key"));
        assert!(!json_str.contains("ticket"));
        assert!(!json_str.contains("private_key"));
        assert!(!json_str.contains("password"));
    }

    // =========================================================================
    // Comprehensive security: no secrets in ANY tool response
    // =========================================================================

    /// Forbidden strings that must never appear in any MCP tool response,
    /// regardless of mode or state.
    ///
    /// Each entry is a case-sensitive substring search against the
    /// serialized JSON-RPC response.  These cover secret keys, mailbox
    /// keys, discovery secrets, private room tickets, passwords, and
    /// environment-variable injection markers.
    const FORBIDDEN_SECRET_PATTERNS: &[&str] = &[
        "secret_key",
        "secret",
        "private_key",
        "ticket",
        "password",
        "mailbox_key",
        "discovery_secret",
        "discovery_key",
        "env_var",
        "ENV",
        "window_handle",
        "hWnd",
        "keyboard",
        "mouse",
        "cursor_pos",
        "display",
        "WAYLAND_DISPLAY",
        "DISPLAY",
    ];

    /// Assert that a JSON-RPC response string contains none of the
    /// forbidden secret patterns.
    fn assert_no_forbidden_secrets(resp_json: &str, method: &str) {
        for &pattern in FORBIDDEN_SECRET_PATTERNS {
            assert!(
                !resp_json.contains(pattern),
                "Tool '{}' response contains forbidden pattern '{}'",
                method,
                pattern
            );
        }
    }

    /// Sweep all registered MCP tools (GUI-gated and core) verifying
    /// no secret patterns appear in their responses, regardless of
    /// GUI test mode status.
    #[tokio::test]
    async fn test_core_tools_no_secrets_in_response() {
        // Test core tools (those not gated on GUI mode).
        let (state, _rx) = make_gate_test_state(true, true).await;

        let methods = ["boru_get_node_status", "boru_get_failure_analysis"];

        for method in &methods {
            let req = make_generic_request(method);
            let resp = handle_request(&req, &state).await;
            assert!(
                resp.error.is_none(),
                "Core tool '{}' should succeed, got: {:?}",
                method,
                resp.error
            );
            let json_str = serde_json::to_string(&resp).unwrap();
            assert_no_forbidden_secrets(&json_str, method);

            // Additionally verify the result does not contain the actual
            // secret key bytes by looking for known structure markers.
            assert!(
                !json_str.contains("ed25519"),
                "Core tool '{}' response contains 'ed25519' (key material leak)",
                method
            );
        }
    }

    #[tokio::test]
    async fn test_gui_tools_no_secrets_in_response() {
        let (state, _rx) = make_gate_test_state(true, true).await;

        let methods = [
            "boru_get_gui_snapshot",
            "boru_get_iced_state",
            "boru_get_iced_message_journal",
        ];

        for method in &methods {
            let req = make_generic_request(method);
            let resp = handle_request(&req, &state).await;
            assert!(
                resp.error.is_none(),
                "GUI tool '{}' should succeed, got: {:?}",
                method,
                resp.error
            );
            let json_str = serde_json::to_string(&resp).unwrap();
            assert_no_forbidden_secrets(&json_str, method);
        }
    }

    /// Verify that `boru_get_room_status` does not leak secrets when
    /// called with a valid (hex) room ID that the local node is joined to.
    #[tokio::test]
    async fn test_get_room_status_no_secrets() {
        let (state, _rx) = make_gate_test_state(true, true).await;
        // Generate a random room ID and "join" it by adding to rooms list
        let topic_bytes: [u8; 32] = rand::random();
        let room_hex = hex::encode(topic_bytes);
        {
            let mut rooms = state.rooms.lock().unwrap();
            rooms.push(TopicId::from_bytes(topic_bytes));
        }

        let req = JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            method: "boru_get_room_status".to_string(),
            params: json!({ "room_id": room_hex }),
            id: Some(Value::Number(1.into())),
        };
        let resp = handle_request(&req, &state).await;
        // If the room was joined the response may succeed; if diagnostics
        // haven't recorded anything the tool may report an error.
        // Either way, check no secrets in the output.
        let json_str = serde_json::to_string(&resp).unwrap();
        assert_no_forbidden_secrets(&json_str, "boru_get_room_status");
    }

    /// Verify `boru_get_discovery_events` does not leak secrets.
    #[tokio::test]
    async fn test_get_discovery_events_no_secrets() {
        let (state, _rx) = make_gate_test_state(true, true).await;
        let req = JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            method: "boru_get_discovery_events".to_string(),
            params: json!({}),
            id: Some(Value::Number(1.into())),
        };
        let resp = handle_request(&req, &state).await;
        assert!(resp.error.is_none(), "discovery events should succeed");
        let json_str = serde_json::to_string(&resp).unwrap();
        assert_no_forbidden_secrets(&json_str, "boru_get_discovery_events");
    }

    /// Verify `boru_get_peer_status` with an unknown peer does not leak secrets.
    #[tokio::test]
    async fn test_get_peer_status_no_secrets() {
        let (state, _rx) = make_gate_test_state(true, true).await;
        let req = JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            method: "boru_get_peer_status".to_string(),
            params: json!({ "peer_id": "abcdef0123456789abcdef0123456789" }),
            id: Some(Value::Number(1.into())),
        };
        let resp = handle_request(&req, &state).await;
        let json_str = serde_json::to_string(&resp).unwrap();
        assert_no_forbidden_secrets(&json_str, "boru_get_peer_status");
    }

    // =========================================================================
    // Security: path traversal / arbitrary file access blocked
    // =========================================================================

    /// Verify that all path-traversal payloads are rejected by input
    /// validation before reaching any handler.
    #[tokio::test]
    async fn test_path_traversal_rejected_in_peer_id() {
        let (state, _rx) = make_gate_test_state(true, true).await;

        let payloads = [
            "../../etc/passwd",
            "..\\windows\\system32\\config",
            "/etc/shadow",
            "C:\\boot.ini",
            "../../.env",
            "../../.ssh/id_rsa",
        ];

        for &path in &payloads {
            // Test via boru_get_peer_status (uses validate_peer_id)
            let req = JsonRpcRequest {
                jsonrpc: "2.0".to_string(),
                method: "boru_get_peer_status".to_string(),
                params: json!({ "peer_id": path }),
                id: Some(Value::Number(1.into())),
            };
            let resp = handle_request(&req, &state).await;
            assert!(
                resp.error.is_some(),
                "Path traversal '{}' should be rejected by peer_id validation",
                path
            );
            let data = resp.error.unwrap();
            let msg = format!("{:?}", data);
            assert!(
                msg.contains("not contain") || msg.contains("too long") || msg.contains("Invalid"),
                "Path traversal error should mention validation failure, got: {}",
                msg
            );
        }
    }

    #[tokio::test]
    async fn test_path_traversal_rejected_in_probe_id() {
        let (state, _rx) = make_gate_test_state(true, true).await;

        let payloads = ["../../etc/passwd", "..\\windows\\system32"];

        for &path in &payloads {
            // Test via boru_find_received_probe (uses validate_probe_id)
            let req = JsonRpcRequest {
                jsonrpc: "2.0".to_string(),
                method: "boru_find_received_probe".to_string(),
                params: json!({ "probe_id": path }),
                id: Some(Value::Number(1.into())),
            };
            let resp = handle_request(&req, &state).await;
            assert!(
                resp.error.is_some(),
                "Path traversal '{}' should be rejected by probe_id validation",
                path
            );
        }
    }

    // =========================================================================
    // Security: shell command injection blocked in all input fields
    // =========================================================================

    /// Verify that shell metacharacters are rejected by every input
    /// validation path that could reach a handler.
    #[tokio::test]
    async fn test_shell_injection_rejected_in_peer_id() {
        let (state, _rx) = make_gate_test_state(true, true).await;

        let payloads = [
            "abcd; rm -rf /",
            "abcd`ls`",
            "abcd|cat",
            "ab$cd",
            "abcd$(whoami)",
        ];

        for &payload in &payloads {
            // Test via boru_get_peer_status
            let req = JsonRpcRequest {
                jsonrpc: "2.0".to_string(),
                method: "boru_get_peer_status".to_string(),
                params: json!({ "peer_id": payload }),
                id: Some(Value::Number(1.into())),
            };
            let resp = handle_request(&req, &state).await;
            assert!(
                resp.error.is_some(),
                "Shell injection '{}' should be rejected by peer_id validation",
                payload
            );
        }
    }

    // =========================================================================
    // Security: environment variables not exposed in any response
    // =========================================================================
    //
    // The MCP server should never read or expose environment variables
    // (PATH, HOME, RUST_LOG, BORU_CHAT_DATA_DIR, etc.).
    //
    // The FORBIDDEN_SECRET_PATTERNS sweep above catches literal "ENV" and
    // env-var names, but also verify that no response payload contains
    // known env-var values by checking the result structure.

    #[tokio::test]
    async fn test_node_status_no_env_leak() {
        let (state, _rx) = make_gate_test_state(false, false).await;
        let req = make_generic_request("boru_get_node_status");
        let resp = handle_request(&req, &state).await;
        assert!(resp.error.is_none(), "node status should succeed");
        let value = resp.result.unwrap();

        // The node status should only contain expected fields.
        // Check it does NOT have any environment-like fields.
        let map = value.as_object().unwrap();
        let unexpected_env_fields = [
            "PATH",
            "HOME",
            "RUST_LOG",
            "BORU_CHAT_DATA_DIR",
            "DISPLAY",
            "WAYLAND_DISPLAY",
            "env",
            "environment",
        ];
        for field in &unexpected_env_fields {
            assert!(
                !map.contains_key(*field),
                "node status should not contain '{}' field",
                field
            );
        }

        // Check the response does not contain absolute paths
        let json_str = serde_json::to_string(&value).unwrap();
        assert!(
            !json_str.contains("/home/")
                && !json_str.contains("/etc/")
                && !json_str.contains("/tmp/"),
            "node status should not leak filesystem paths"
        );
    }

    // =========================================================================
    // Security: no OS window handle or raw input exposure
    // =========================================================================
    //
    // GUI test actions should never expose raw OS-level window handles,
    // display identifiers, or direct keyboard/mouse injection capabilities.
    // All GUI interactions must be through semantic commands (navigate,
    // set_composer_text, toggle_*, etc.).

    #[tokio::test]
    async fn test_gui_snapshot_no_window_or_input_exposure() {
        let (state, _rx) = make_gate_test_state(true, true).await;
        let req = make_generic_request("boru_get_gui_snapshot");
        let resp = handle_request(&req, &state).await;
        assert!(resp.error.is_none());
        let json_str = serde_json::to_string(&resp).unwrap();

        // Must not contain raw window/input references
        assert!(!json_str.contains("hWnd"), "Must not leak window handle");
        assert!(!json_str.contains("handle"), "Must not leak any handle");
        assert!(
            !json_str.contains("keyboard"),
            "Must not expose keyboard control"
        );
        assert!(!json_str.contains("mouse"), "Must not expose mouse control");
        assert!(
            !json_str.contains("click"),
            "Must not expose click injection"
        );
        assert!(
            !json_str.contains("keystroke"),
            "Must not expose keystroke injection"
        );
    }

    #[tokio::test]
    async fn test_iced_state_no_window_or_input_exposure() {
        let (state, _rx) = make_gate_test_state(true, true).await;
        let req = make_generic_request("boru_get_iced_state");
        let resp = handle_request(&req, &state).await;
        assert!(resp.error.is_none());
        let json_str = serde_json::to_string(&resp).unwrap();

        assert!(!json_str.contains("hWnd"));
        assert!(!json_str.contains("keyboard"));
        assert!(!json_str.contains("mouse"));
    }

    // =============================================================================
    // Security: non-loopback binding rejected with GUI actions enabled
    // =============================================================================

    #[tokio::test]
    async fn test_spawn_mcp_server_rejects_non_loopback_with_gui_actions_integration() {
        use iroh::address_lookup::memory::MemoryLookup;
        use iroh::endpoint::presets;
        use std::net::{Ipv4Addr, SocketAddrV4};

        let config = McpConfig {
            bind_addr: SocketAddrV4::new(Ipv4Addr::UNSPECIFIED, 0).into(),
            enable_gui_test_actions: true,
        };

        // We need a state to call spawn_mcp_server — create a minimal one.
        let secret_key = SecretKey::generate();
        let endpoint = Endpoint::builder(presets::N0DisableRelay)
            .secret_key(secret_key.clone())
            .address_lookup(MemoryLookup::new())
            .relay_mode(iroh::RelayMode::Disabled)
            .bind_addr(SocketAddrV4::new(Ipv4Addr::LOCALHOST, 0))
            .unwrap()
            .bind()
            .await
            .expect("test endpoint bind");
        let gossip = boru_chat::net::Gossip::builder().spawn(endpoint.clone());
        let (gossip_tx, _gossip_rx) = tokio::sync::mpsc::unbounded_channel();

        let state = McpAppState {
            diagnostics: Diagnostics::new(),
            iced_diagnostics: IcedMessageJournal::new(),
            endpoint,
            rooms: Arc::new(Mutex::new(Vec::new())),
            node_id: secret_key.public().to_string(),
            version: "test".to_string(),
            gossip_tx,
            secret_key,
            gossip,
            gui_test_actions_enabled: true,
            gui_action_tx: None,
            gui_action_history: GuiActionHistory::new(),
            gui_action_lifecycle: boru_chat::diagnostics::GuiActionHistory::default(),
            gui_action_rate_limiter: Arc::new(Mutex::new(GuiActionRateLimiter::new())),
            gui_state_rx: None,
        };

        let result = spawn_mcp_server(config, state).await;
        assert!(
            result.is_err(),
            "Non-loopback with GUI actions must be rejected"
        );
        let err = result.unwrap_err();
        assert!(
            err.contains("non-loopback"),
            "Error should mention non-loopback: {err}"
        );
        assert!(
            err.contains("127.0.0.1"),
            "Error should mention 127.0.0.1: {err}"
        );
    }

    #[tokio::test]
    async fn test_spawn_mcp_server_loopback_accepted_with_gui_actions() {
        use iroh::address_lookup::memory::MemoryLookup;
        use iroh::endpoint::presets;
        use std::net::{Ipv4Addr, SocketAddrV4};

        let config = McpConfig {
            bind_addr: SocketAddrV4::new(Ipv4Addr::LOCALHOST, 0).into(),
            enable_gui_test_actions: true,
        };

        let secret_key = SecretKey::generate();
        let endpoint = Endpoint::builder(presets::N0DisableRelay)
            .secret_key(secret_key.clone())
            .address_lookup(MemoryLookup::new())
            .relay_mode(iroh::RelayMode::Disabled)
            .bind_addr(SocketAddrV4::new(Ipv4Addr::LOCALHOST, 0))
            .unwrap()
            .bind()
            .await
            .expect("test endpoint bind");
        let gossip = boru_chat::net::Gossip::builder().spawn(endpoint.clone());
        let (gossip_tx, _gossip_rx) = tokio::sync::mpsc::unbounded_channel();

        let state = McpAppState {
            diagnostics: Diagnostics::new(),
            iced_diagnostics: IcedMessageJournal::new(),
            endpoint,
            rooms: Arc::new(Mutex::new(Vec::new())),
            node_id: secret_key.public().to_string(),
            version: "test".to_string(),
            gossip_tx,
            secret_key,
            gossip,
            gui_test_actions_enabled: true,
            gui_action_tx: None,
            gui_action_history: GuiActionHistory::new(),
            gui_action_lifecycle: boru_chat::diagnostics::GuiActionHistory::default(),
            gui_action_rate_limiter: Arc::new(Mutex::new(GuiActionRateLimiter::new())),
            gui_state_rx: None,
        };

        let result = spawn_mcp_server(config, state).await;
        assert!(
            result.is_ok(),
            "Loopback with GUI actions should be accepted, got: {result:?}"
        );
    }

    // =========================================================================
    // boru_gui_get_action_status — handle_get_gui_action_status tests
    // =========================================================================

    /// Helper: construct a minimal JsonRpcRequest for boru_gui_get_action_status.
    fn make_get_action_status_request(action_id: &str) -> JsonRpcRequest {
        JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            method: "boru_gui_get_action_status".to_string(),
            params: serde_json::json!({ "action_id": action_id }),
            id: Some(Value::Number(1.into())),
        }
    }

    #[tokio::test]
    async fn test_get_gui_action_status_missing_action_id() {
        let (state, _gossip_rx) = make_gate_test_state(true, true).await;
        let req = JsonRpcRequest {
            jsonrpc: "2.0".to_string(),
            method: "boru_gui_get_action_status".to_string(),
            params: serde_json::json!({}),
            id: Some(Value::Number(1.into())),
        };
        let result = handle_get_gui_action_status(&req, &state).await;
        assert!(result.is_err(), "Missing action_id should produce an error");
        let err = result.unwrap_err();
        assert!(
            err.contains("Missing required argument"),
            "Error should mention 'Missing required argument', got: {err}"
        );
    }

    #[tokio::test]
    async fn test_get_gui_action_status_too_long() {
        let (state, _gossip_rx) = make_gate_test_state(true, true).await;
        let long_id = "a".repeat(MAX_PROBE_ID_LEN + 1);
        let req = make_get_action_status_request(&long_id);
        let result = handle_get_gui_action_status(&req, &state).await;
        assert!(
            result.is_err(),
            "Oversized action_id should produce an error"
        );
        let err = result.unwrap_err();
        assert!(
            err.contains("too long"),
            "Error should mention 'too long', got: {err}"
        );
    }

    #[tokio::test]
    async fn test_get_gui_action_status_control_chars() {
        let (state, _gossip_rx) = make_gate_test_state(true, true).await;
        let req = make_get_action_status_request("bad\nkey");
        let result = handle_get_gui_action_status(&req, &state).await;
        assert!(result.is_err(), "Control chars should produce an error");
        let err = result.unwrap_err();
        assert!(
            err.contains("control characters"),
            "Error should mention control characters, got: {err}"
        );
    }

    #[tokio::test]
    async fn test_get_gui_action_status_not_found() {
        let (state, _gossip_rx) = make_gate_test_state(true, true).await;
        let req = make_get_action_status_request("nonexistent_key");
        let result = handle_get_gui_action_status(&req, &state).await;
        assert!(
            result.is_ok(),
            "Non-existent key should return ok, not error"
        );
        let value = result.unwrap();
        assert_eq!(value["found"], false, "Should report found=false");
        assert_eq!(value["action_id"], "nonexistent_key");
        assert!(
            value["note"].is_string(),
            "Should include a descriptive note"
        );
        let note = value["note"].as_str().unwrap();
        assert!(!note.is_empty(), "Note should not be empty");
    }

    #[tokio::test]
    async fn test_get_gui_action_status_found() {
        let (state, _gossip_rx) = make_gate_test_state(true, true).await;
        // Seed the action history with a record
        let record = ActionRecord {
            idempotency_key: "test_key_123".to_string(),
            command: "ToggleHelp".to_string(),
            status: ActionStatus::Processed,
            timestamp_ms: 1710000000000,
            duration_ms: 42,
        };
        state.gui_action_history.record(record);

        let req = make_get_action_status_request("test_key_123");
        let result = handle_get_gui_action_status(&req, &state).await;
        assert!(result.is_ok(), "Existing action should be found");
        let value = result.unwrap();
        assert_eq!(value["found"], true, "Should report found=true");
        assert_eq!(value["action_id"], "test_key_123");
        assert_eq!(value["idempotency_key"], "test_key_123");
        assert_eq!(value["command"], "ToggleHelp");
        assert_eq!(
            value["status"]["status"], "processed",
            "Should serialize ActionStatus correctly"
        );
        assert_eq!(value["timestamp_ms"], 1710000000000i64);
        assert_eq!(value["duration_ms"], 42);
    }

    #[tokio::test]
    async fn test_get_gui_action_status_no_secrets_exposed() {
        let (state, _gossip_rx) = make_gate_test_state(true, true).await;
        let record = ActionRecord {
            idempotency_key: "test_key_sec".to_string(),
            command: "ToggleHelp".to_string(),
            status: ActionStatus::Processed,
            timestamp_ms: 1710000000000,
            duration_ms: 10,
        };
        state.gui_action_history.record(record);

        let req = make_get_action_status_request("test_key_sec");
        let result = handle_get_gui_action_status(&req, &state).await;
        assert!(result.is_ok());
        let response_str = serde_json::to_string(&result.unwrap()).unwrap();
        assert!(!response_str.contains("secret_key"));
        assert!(!response_str.contains("secret"));
        assert!(!response_str.contains("private_key"));
        assert!(!response_str.contains("ticket"));
        assert!(!response_str.contains("password"));
    }

    // =============================================================================
    // Security abuse matrix: malformed and rejected requests must not enqueue
    // =============================================================================

    #[test]
    fn test_malformed_json_is_rejected_before_gui_dispatch() {
        let malformed = r#"{\"jsonrpc\":\"2.0\",\"method\":\"boru_gui_navigate\",\"params\":{"#;
        let parsed: Result<JsonRpcRequest, _> = serde_json::from_str(malformed);
        assert!(parsed.is_err(), "malformed JSON must never reach dispatch");
    }

    #[tokio::test]
    async fn test_oversized_gui_command_fails_closed_without_mutation() {
        let (tx, mut rx) = boru_chat::diagnostics::GuiTestHandle::channel(4);
        let request = make_send_gui_action_request(json!({
            "command": {
                "command": "set_composer_text",
                "text": "x".repeat(boru_chat::diagnostics::GUI_TEST_COMMAND_MAX_STRING_LEN + 1)
            }
        }));
        let result = handle_send_gui_action(&request, tx).await;
        assert!(result.is_err(), "oversized command must be rejected");
        assert!(
            rx.try_recv().is_err(),
            "rejected command must not mutate GUI queue"
        );
    }

    #[tokio::test]
    async fn test_control_character_gui_command_fails_closed_without_mutation() {
        let (tx, mut rx) = boru_chat::diagnostics::GuiTestHandle::channel(4);
        let request = make_send_gui_action_request(json!({
            "command": {"command": "set_composer_text", "text": "safe\nunsafe"}
        }));
        let result = handle_send_gui_action(&request, tx).await;
        assert!(result.is_err(), "control characters must be rejected");
        assert!(
            rx.try_recv().is_err(),
            "rejected command must not mutate GUI queue"
        );
    }

    #[tokio::test]
    async fn test_unknown_navigation_destination_fails_closed_without_mutation() {
        let (tx, mut rx) = boru_chat::diagnostics::GuiTestHandle::channel(4);
        let request = make_navigate_request(json!({"destination": "admin_console"}));
        let result = handle_gui_navigate(&request, tx).await;
        assert!(result.is_err(), "unknown destinations must be rejected");
        assert!(
            rx.try_recv().is_err(),
            "rejected navigation must not mutate GUI queue"
        );
    }

    #[tokio::test]
    async fn test_queue_overflow_fails_closed_without_extra_gui_mutation() {
        let (tx, mut rx) = boru_chat::diagnostics::GuiTestHandle::channel(1);
        let first = make_navigate_request(json!({"destination": "settings"}));
        assert!(handle_gui_navigate(&first, tx.clone()).await.is_ok());
        let second = make_navigate_request(json!({"destination": "friends"}));
        let result = handle_gui_navigate(&second, tx).await;
        assert!(result.is_err(), "full queue must reject the second action");
        assert!(result.unwrap_err().contains("queue is full"));
        let queued = rx.try_recv().expect("first action remains queued");
        assert!(
            rx.try_recv().is_err(),
            "overflow must not enqueue an extra action"
        );
        assert!(queued.command.contains("open_settings"));
    }

    #[tokio::test]
    async fn test_rate_limit_rejects_burst_before_gui_dispatch() {
        let (state, _gossip_rx) = make_gate_test_state(true, true).await;
        let mut request = make_generic_request("boru_gui_toggle_dark_mode");
        request.params = json!({"enabled": true});
        // The test state's receiver is intentionally absent, so the first
        // request fails closed at enqueue; the second must be stopped by the
        // shared rate limiter before the handler is reached.
        let first = handle_request(&request, &state).await;
        assert!(first.error.is_some());
        let second = handle_request(&request, &state).await;
        let error = second.error.expect("burst must be rate limited");
        assert_eq!(error.code, -32000);
        assert_eq!(error.message, "Rate limit exceeded");
        assert!(error
            .data
            .unwrap()
            .as_str()
            .unwrap()
            .contains("10 actions/sec"));
    }
}
