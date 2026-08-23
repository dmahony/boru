//! Shared, transport-neutral E2E test-control contract.
//!
//! This module contains only bounded identifiers, compact snapshots, action
//! descriptors, and outcomes. Domain adapters and the existing GUI/MCP
//! dispatcher own execution; this module deliberately contains no application
//! or network behavior.
#![allow(missing_docs)]

use std::net::SocketAddr;

use serde::{Deserialize, Serialize};

pub const E2E_SCHEMA_NAME: &str = "boru-e2e";
pub const E2E_SCHEMA_VERSION: u16 = 1;
pub const DEFAULT_TEST_CONTROL_BIND: &str = "127.0.0.1:0";

macro_rules! opaque_id {
    ($name:ident) => {
        #[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
        #[serde(transparent)]
        pub struct $name(pub String);

        impl From<String> for $name {
            fn from(value: String) -> Self { Self(value) }
        }

        impl From<&str> for $name {
            fn from(value: &str) -> Self { Self(value.to_owned()) }
        }

        impl AsRef<str> for $name {
            fn as_ref(&self) -> &str { &self.0 }
        }
    };
}

opaque_id!(RunId);
opaque_id!(NodeAlias);
opaque_id!(WorkflowId);
opaque_id!(RoomMarker);
opaque_id!(MessageMarker);
opaque_id!(TransferMarker);
opaque_id!(FaultId);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SchemaVersion {
    pub schema: String,
    pub version: u16,
}

impl Default for SchemaVersion {
    fn default() -> Self {
        Self { schema: E2E_SCHEMA_NAME.to_owned(), version: E2E_SCHEMA_VERSION }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum E2eErrorCode {
    DisabledAction,
    InvalidState,
    Timeout,
    UnavailableCapability,
    UnsupportedPlatform,
    NotFound,
    InternalFailure,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct E2eError {
    pub code: E2eErrorCode,
    pub message: String,
}

impl E2eError {
    pub fn new(code: E2eErrorCode, message: impl Into<String>) -> Self {
        Self { code, message: message.into() }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RoomState { Created, Joining, Joined, Leaving, Left, Failed }

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MessageDeliveryState { Queued, Sent, Delivered, Seen, Failed }

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TransferState { Offered, Accepted, Active, Completed, Declined, Failed }

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NodeSnapshot {
    pub schema: SchemaVersion,
    pub node_alias: NodeAlias,
    pub online: bool,
    pub capability_names: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RoomSnapshot {
    pub schema: SchemaVersion,
    pub room_marker: RoomMarker,
    pub state: RoomState,
    pub local_member: bool,
    pub member_count: u32,
    pub safe_member_aliases: Vec<NodeAlias>,
    pub last_transition: Option<String>,
    pub last_error: Option<E2eErrorCode>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MessageSnapshot {
    pub schema: SchemaVersion,
    pub room_marker: RoomMarker,
    pub message_marker: MessageMarker,
    pub present: bool,
    pub delivery_state: Option<MessageDeliveryState>,
    pub duplicate_count: u32,
    pub sequence: Option<u64>,
    pub sender: Option<NodeAlias>,
    pub receiver: Option<NodeAlias>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TransferSnapshot {
    pub schema: SchemaVersion,
    pub transfer_marker: TransferMarker,
    pub state: TransferState,
    pub size_bytes: u64,
    pub bytes_transferred: u64,
    pub content_hash: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "snake_case")]
pub enum TestControlAction {
    CreateRoom { run_id: RunId, node_alias: NodeAlias, workflow_id: WorkflowId, room_marker: RoomMarker },
    JoinRoom { run_id: RunId, node_alias: NodeAlias, room_marker: RoomMarker },
    LeaveRoom { run_id: RunId, node_alias: NodeAlias, room_marker: RoomMarker },
    RejoinRoom { run_id: RunId, node_alias: NodeAlias, room_marker: RoomMarker },
    SendTestMessage { run_id: RunId, node_alias: NodeAlias, room_marker: RoomMarker, message_marker: MessageMarker },
    QueryMessage { run_id: RunId, node_alias: NodeAlias, room_marker: RoomMarker, message_marker: MessageMarker },
    ShareSyntheticFile { run_id: RunId, node_alias: NodeAlias, room_marker: RoomMarker, transfer_marker: TransferMarker, size_bytes: u64 },
    AcceptDownload { run_id: RunId, node_alias: NodeAlias, transfer_marker: TransferMarker },
    QueryTransfer { run_id: RunId, node_alias: NodeAlias, transfer_marker: TransferMarker },
    QueryNodeStatus { run_id: RunId, node_alias: NodeAlias },
    QueryRoomStatus { run_id: RunId, node_alias: NodeAlias, room_marker: RoomMarker },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TestActionKind { CreateRoom, JoinRoom, LeaveRoom, RejoinRoom, SendTestMessage, QueryMessage, ShareSyntheticFile, AcceptDownload, QueryTransfer, QueryNodeStatus, QueryRoomStatus }

impl TestControlAction {
    pub fn kind(&self) -> TestActionKind {
        match self {
            Self::CreateRoom { .. } => TestActionKind::CreateRoom,
            Self::JoinRoom { .. } => TestActionKind::JoinRoom,
            Self::LeaveRoom { .. } => TestActionKind::LeaveRoom,
            Self::RejoinRoom { .. } => TestActionKind::RejoinRoom,
            Self::SendTestMessage { .. } => TestActionKind::SendTestMessage,
            Self::QueryMessage { .. } => TestActionKind::QueryMessage,
            Self::ShareSyntheticFile { .. } => TestActionKind::ShareSyntheticFile,
            Self::AcceptDownload { .. } => TestActionKind::AcceptDownload,
            Self::QueryTransfer { .. } => TestActionKind::QueryTransfer,
            Self::QueryNodeStatus { .. } => TestActionKind::QueryNodeStatus,
            Self::QueryRoomStatus { .. } => TestActionKind::QueryRoomStatus,
        }
    }

    pub fn is_mutating(&self) -> bool {
        !matches!(self, Self::QueryMessage { .. } | Self::QueryTransfer { .. } | Self::QueryNodeStatus { .. } | Self::QueryRoomStatus { .. })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ActionOutcome<T> {
    pub schema: SchemaVersion,
    pub run_id: RunId,
    pub action_id: String,
    pub result: Result<T, E2eError>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TestControlConfig {
    pub enabled: bool,
    pub bind_addr: SocketAddr,
}

impl Default for TestControlConfig {
    fn default() -> Self { Self { enabled: false, bind_addr: DEFAULT_TEST_CONTROL_BIND.parse().expect("valid loopback default") } }
}

impl TestControlConfig {
    pub fn allows_mutation(&self) -> bool { self.enabled && self.bind_addr.ip().is_loopback() }
    pub fn accepts_bind(&self) -> bool { self.bind_addr.ip().is_loopback() }
    pub fn is_loopback(&self) -> bool { self.bind_addr.ip().is_loopback() }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn action_round_trip_and_mutation_gate_are_stable() {
        let action = TestControlAction::SendTestMessage {
            run_id: "run-1".into(), node_alias: "node-a".into(),
            room_marker: "room-1".into(), message_marker: "E2E:run-1:1".into(),
        };
        let encoded = serde_json::to_string(&action).unwrap();
        assert!(encoded.contains("send_test_message"));
        assert!(!encoded.contains("body"));
        assert!(action.is_mutating());
        assert_eq!(serde_json::from_str::<TestControlAction>(&encoded).unwrap(), action);
        assert!(!TestControlConfig::default().allows_mutation());
        assert!(TestControlConfig { enabled: true, ..Default::default() }.allows_mutation());
    }

    #[test]
    fn outcome_is_versioned_and_error_codes_are_machine_readable() {
        let outcome: ActionOutcome<()> = ActionOutcome {
            schema: Default::default(), run_id: "r".into(), action_id: "a".into(),
            result: Err(E2eError::new(E2eErrorCode::Timeout, "bounded operation expired")),
        };
        let json = serde_json::to_value(outcome).unwrap();
        assert_eq!(json["schema"]["version"], 1);
        assert_eq!(json["result"]["Err"]["code"], "timeout");
        assert!(!json.to_string().contains("private_key"));
    }
}
