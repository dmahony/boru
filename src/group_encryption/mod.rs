//! Group encryption — end-to-end encrypted group messaging with p2panda.
//!
//! This module provides the building blocks for encrypting and decrypting
//! messages within a group of peers using the p2panda encryption framework:
//!
//! - [`types`](crate::group_encryption::types) — type-conversion bridge between iroh and p2panda types,
//!   plus newtype wrappers (`PeerId`, `OpId`).
//! - [`registry`](crate::group_encryption::registry) — encryption key and session registry for active groups.
//! - [`manager`](crate::group_encryption::manager) — high-level encrypt/decrypt orchestration.
//! - [`message`](crate::group_encryption::message) — wire-format encrypted message types.
//! - [`ordering`](crate::group_encryption::ordering) — causal ordering of encrypted operations (Merkle CRDT).
//! - [`membership`](crate::group_encryption::membership) — dynamic group-membership management and handshake.

#![allow(
    clippy::large_enum_variant,
    clippy::result_large_err,
    clippy::type_complexity,
    clippy::let_unit_value
)]

pub mod types;

// ── Stub modules (to be implemented in follow-up phases) ─────────────────────

/// Per-group encryption state and high-level API.
pub mod encryption_state;
/// High-level encrypt/decrypt orchestration.
pub mod manager;
/// Dynamic group-membership management and handshake.
pub mod membership;
/// Wire-format encrypted message types.
pub mod message;
/// Causal ordering of encrypted operations (Merkle CRDT).
pub mod ordering;
/// Persistence for per-group encryption state (save/load to SQLite).
pub mod persistence;
/// Encryption key and session registry.
pub mod registry;
/// End-to-end integration tests for encrypted group messaging.
#[cfg(test)]
pub mod tests;
