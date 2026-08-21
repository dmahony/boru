//! Privacy-preserving support bundle export.
#![allow(missing_docs)]
use std::path::Path;
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use crate::diagnostics::{DiagnosticEvent, DiagnosticEventKind, Diagnostics, IcedMessageJournal};

pub const SCHEMA_VERSION: u32 = 1;
const MAX_WARNINGS: usize = 100;

#[derive(Debug, Clone, Default)]
pub struct SupportBundleInput {
    pub build_sha: String,
    pub os: String,
    pub arch: String,
    pub enabled_features: Vec<String>,
    pub endpoint_id: String,
    pub relay_transport: String,
    pub dht_health: String,
    pub connection_paths: Vec<String>,
    pub schema_version: String,
    pub active_subscription_count: usize,
    pub active_task_count: usize,
    pub diagnostics: Option<Diagnostics>,
    pub journal: Option<IcedMessageJournal>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SupportBundle {
    pub schema_version: u32,
    pub boru_version: String,
    pub build_sha: String,
    pub platform: PlatformSummary,
    pub endpoint: EndpointSummary,
    pub network: NetworkSummary,
    pub storage: StorageSummary,
    pub warnings: Vec<RedactedWarning>,
    pub summary: String,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PlatformSummary { pub os: String, pub arch: String, pub enabled_features: Vec<String> }
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct EndpointSummary { pub short_id: String, pub relay_transport: String, pub connection_paths: Vec<String> }
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct NetworkSummary { pub dht_health: String, pub active_subscription_count: usize, pub active_task_count: usize }
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct StorageSummary { pub schema_version: String }
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RedactedWarning { pub sequence: Option<u64>, pub kind: String, pub message: String }

pub fn build(input: &SupportBundleInput) -> SupportBundle {
    let mut warnings = Vec::new();
    if let Some(diagnostics) = &input.diagnostics {
        for event in diagnostics.all_events().iter().rev().take(MAX_WARNINGS) {
            if let Some(warning) = redact_event(event) { warnings.push(warning); }
        }
    }
    if let Some(journal) = &input.journal {
        for entry in journal.all_entries().iter().rev().take(MAX_WARNINGS) {
            if !entry.success { warnings.push(RedactedWarning { sequence: Some(entry.sequence), kind: "gui_update_failed".into(), message: "GUI update failed (details redacted)".into() }); }
        }
    }
    warnings.truncate(MAX_WARNINGS);
    let mut bundle = SupportBundle {
        schema_version: SCHEMA_VERSION,
        boru_version: env!("CARGO_PKG_VERSION").into(),
        build_sha: safe_label(&input.build_sha),
        platform: PlatformSummary { os: safe_label(&input.os), arch: safe_label(&input.arch), enabled_features: input.enabled_features.iter().map(|s| safe_label(s)).collect() },
        endpoint: EndpointSummary { short_id: short_id(&input.endpoint_id), relay_transport: safe_label(&input.relay_transport), connection_paths: input.connection_paths.iter().map(|s| safe_label(s)).collect() },
        network: NetworkSummary { dht_health: safe_label(&input.dht_health), active_subscription_count: input.active_subscription_count, active_task_count: input.active_task_count },
        storage: StorageSummary { schema_version: safe_label(&input.schema_version) }, warnings, summary: String::new(),
    };
    bundle.summary = format!("Boru {} (schema {})\nPlatform: {} {}\nEndpoint: {}\nTransport: {}\nDHT: {}\nWarnings: {}", bundle.boru_version, bundle.schema_version, bundle.platform.os, bundle.platform.arch, bundle.endpoint.short_id, bundle.endpoint.relay_transport, bundle.network.dht_health, bundle.warnings.len());
    bundle
}

pub fn export_json(path: &Path, input: &SupportBundleInput) -> Result<()> {
    let bytes = serde_json::to_vec_pretty(&build(input)).context("serialize support bundle")?;
    std::fs::write(path, bytes).with_context(|| format!("write support bundle to {}", path.display()))
}
fn redact_event(event: &DiagnosticEvent) -> Option<RedactedWarning> {
    let (kind, message) = match &event.kind {
        DiagnosticEventKind::Error(_) => ("error", "Diagnostic error (details redacted)"),
        DiagnosticEventKind::AddressLookupFailed { .. } => ("address_lookup_failed", "Address lookup failed"),
        DiagnosticEventKind::ConnectionFailed { .. } => ("connection_failed", "Connection failed"),
        DiagnosticEventKind::RoomSubscriptionFailed { .. } => ("subscription_failed", "Room subscription failed"),
        DiagnosticEventKind::RoomJoinFailed => ("room_join_failed", "Room join failed"),
        DiagnosticEventKind::CatalogueFetchFailed { .. } => ("catalogue_fetch_failed", "Catalogue fetch failed"),
        DiagnosticEventKind::CatalogueSignatureRejected { .. } => ("catalogue_signature_rejected", "Catalogue signature rejected"),
        DiagnosticEventKind::ProbeTimedOut { .. } => ("probe_timed_out", "Diagnostic probe timed out"),
        DiagnosticEventKind::ActionTimedOut { .. } => ("action_timed_out", "GUI action timed out"), _ => return None,
    };
    Some(RedactedWarning { sequence: Some(event.sequence), kind: kind.into(), message: message.into() })
}
fn safe_label(value: &str) -> String { value.chars().take(128).map(|c| if c.is_control() || matches!(c, '/' | '\\' | '$' | '`') { '_' } else { c }).collect() }
fn short_id(value: &str) -> String { safe_label(value).chars().take(12).collect() }

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn redacts_secrets_messages_files_and_paths() {
        let diagnostics = Diagnostics::new();
        diagnostics.record(None, DiagnosticEventKind::Error("AUTH_TOKEN=secret message body /tmp/file".into()));
        let input = SupportBundleInput { build_sha: "abc".into(), os: "/host/private".into(), endpoint_id: "abcdef-secret".into(), diagnostics: Some(diagnostics), ..Default::default() };
        let json = serde_json::to_string(&build(&input)).unwrap();
        assert!(json.contains("schema_version")); assert!(!json.contains("secret")); assert!(!json.contains("message body")); assert!(!json.contains("/tmp/file")); assert!(json.contains("details redacted"));
    }
    #[test]
    fn hostile_labels_are_path_safe() {
        let bundle = build(&SupportBundleInput { endpoint_id: "../../../../secret".into(), os: "x\n/y".into(), ..Default::default() });
        assert_eq!(bundle.endpoint.short_id, ".._.._.._.._"); assert!(!bundle.platform.os.contains('\n'));
    }
}
