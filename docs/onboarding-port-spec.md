# Onboarding port spec: Iced evidence and React boundary

**Status:** source audit, not an implementation contract. The requested three dedicated Iced onboarding screens and their Tauri commands do not exist in this checkout. Do not implement against invented APIs: the command names/payloads in `docs/desktop-identity-ipc-contract.md` are proposals, not registered commands. React work must treat the distinctions below as blockers/requirements for the Rust IPC implementation.

## Source-of-truth findings

- Iced starts with a generated/persisted identity when `secret_key.txt` is absent (`src/bin/boru/main.rs::load_or_generate_secret_key_at`). `first_run` is `room_history.is_empty() && friends.is_empty()` (`src/bin/boru/app.rs`), not identity onboarding; first user action clears it. Current empty Home copy is localized: greeting `Good {time}` and “Your Boru node is online and ready.” (`home.greeting`, `home.welcome`). There is no first-run privacy-information acknowledgement screen or alias-setup wizard.
- There are no dedicated onboarding alias validation rules, reserved words, charset restrictions, or trimming operation. Display name is a local label, selected from CLI/settings/short endpoint ID; profile validation allows any Unicode scalar string up to 64 characters (`String::chars().count()`), including empty. No reserved-word rejection is present. Do not invent stricter rules for a port claiming behavioral parity. A future UI may trim its submitted alias only if Rust explicitly adopts that contract; Iced's label display trims surrounding whitespace when rendering the Home greeting, not necessarily when storing the value.
- Tauri command registration (`desktop/src-tauri/src/lib.rs`) has no commands for ticket join, alias update, privacy information, identity detection or identity creation. Thus **there are no audited request/return/error shapes for these screens**. Proposed contracts are recorded in `desktop-identity-ipc-contract.md`; they require Rust implementation and registration before frontend use.

## Screen-by-screen requirements

### 1. Join by ticket (existing Iced behavior; no Tauri command)

Iced entry points include the Home/sidebar ticket input and Discover ticket input, all converging on `AppMessage::JoinFromTicket` (`src/bin/boru/app.rs`).

- Before parsing, trim leading/trailing whitespace; if empty, do not start work and show exactly: **“Paste a ticket before joining a room.”**
- Parse supported forms: managed recipient-bound invitation v2, managed owner-bound invitation, stable `boru1:` invitation, or legacy `RoomInvitation`. Do not impose a prefix-only regex or normalize/alter ticket bytes beyond outer whitespace trimming; Rust parsing is authoritative.
- Reject discovery-service topics: **“This ticket is for the discovery service, not a chat room.”**
- Preserve managed invitation authorization checks and SQLite policy persistence. Missing storage, acceptance failure, missing/invalid policy are failures (current raw English messages include “SQLite storage unavailable; managed room cannot be joined”, “Could not accept recipient-bound invitation: {error}”, “Could not pin managed-room owner root: {error}”, “Managed-room policy is missing; access remains denied”, and “Managed-room policy failed validation: {error}”).
- Standard parse error copy is **“Invalid ticket: {error}”**; malformed v2/owner-bound errors are **“Invalid recipient-bound invitation: {error}”** / **“Invalid owner-bound invitation: {error}”**. These currently include Rust parser error details; do not leak secrets in any surfaced detail.
- On start show **“Joining room…”**. Existing async join, persistence, authorization, topic checks, timeout/subscription handling, and gossip/network behavior are Rust-owned. Only this actual join path needs connectivity; local empty-input/format validation is offline-capable. Do not add directory lookup or preflight networking.
- Acceptance: whitespace-only input yields the empty error with no network side effect; outer whitespace around a valid ticket is accepted; each supported ticket family reaches Rust parser; discovery topic fails locally; authorization failures remain fail-closed; pending/success/failure states are visible and ticket text remains available for retry as current flow dictates.

### 2. Alias setup (no dedicated Iced onboarding screen or Tauri command)

Current Iced only has display-name settings/local-label behavior. The profile model's relevant persisted field constraint is at most **64 Unicode scalar values**, not bytes or graphemes. There is no alias charset, non-empty, or reserved-word rule. Avoid claiming the wizard's alias rules are audited when none exist.

- Existing display-name strings: `profile.display_name` (“Display name”), `profile.display_name_placeholder` (“Your display name”); Home reflects `Good {time}, {trimmed local label}` when nonblank.
- Rust-side persistence/validation must remain authoritative. If introducing a `set_identity_alias` IPC command, its currently proposed `{displayName: string} -> {displayName: string}` shape in `desktop-identity-ipc-contract.md` is **not implemented**. Define whether blank/whitespace is allowed and whether trimming occurs before shipping it; do not silently diverge.
- Acceptance: test 64 Unicode scalar values accepted and 65 rejected by Rust profile validation; include multibyte characters in both. Since no onboarding-specific client validator exists, React must not present a guessed charset/reserved list as parity.

### 3. Initial privacy information (no dedicated Iced screen or Tauri command)

No privacy acknowledgement screen or `get_onboarding_privacy_info` command exists. Current home statement is “Your Boru node is online and ready.” Existing design/audit text describes BORU as private, peer-to-peer, with no central servers, but that is not a screen-specific, audited copy contract. The proposed privacy-info command `{}` -> `{title, body}` is unimplemented; Rust must supply the exact static copy before React integration.

- Showing static copy is offline-capable and must not initialize discovery or change app networking.
- Acceptance: no consent toggle/acknowledgement may silently change network settings; do not claim a privacy promise beyond approved Rust copy.

## Command contract and network invariants

| Screen/action | Registered Tauri command today | Proposed (unimplemented) shape | Offline? |
|---|---|---|---|
| Join ticket | None | `join_room_ticket({ticket: string}) -> {topicId: string}` (proposed only) | Validation yes; actual room join no |
| Set alias | None | `set_identity_alias({displayName: string}) -> {displayName: string}` (proposed only) | Yes, local persistence |
| Privacy text | None | `get_onboarding_privacy_info({}) -> {title, body}` (proposed only) | Yes, static |

All args/results would be camelCase JSON if the proposed bridge is adopted. Existing Tauri errors use `Result<_, bridge::AppError>`; proposed stable codes have not been formalized. Preserve current join protocol, ticket parsing, authorization, persistence, gossip subscription, discovery, and relay behavior exactly. Do not add network calls to alias/privacy flows; do not change relay/DHT/mDNS defaults or ticket semantics as a UI-port side effect. Ticket join must use the already active identity and must not create or replace one.

## Security constraints (mandatory for React, Rust bridge and tests)

- Secret key bytes/encoding, key-file path, filesystem paths, and sensitive identity material stay in Rust. Never return these through IPC or write them to JS console/logs, DOM/HTML, error text, analytics, crash reports, or test fixtures/snapshots. Do not stringify arbitrary Rust errors if they could contain sensitive material; map to sanitized user-facing codes/messages.
- Endpoint IDs are public identifiers per the audited IPC note, but expose only when needed. Do not treat them as secret key material or derive/display additional identity data without need.
- Ticket text can contain invitation/bootstrap secrets: do not log it, include it in error messages, telemetry, or fixtures. Keep only transient UI state necessary for user retry; avoid persistence unless Rust's existing flow requires it.
- UI validation is usability only; repeat all security-sensitive validation and authorization in Rust. Fail closed on unknown/malformed tickets or managed-room policy failures.
- Tests use synthetic, non-secret values. Assert absence of sensitive fields in serialized command results and UI-visible errors; never put real keys/tickets in snapshots.

## Porting gate

Before frontend implementation is called complete, Rust must define/register the missing commands and stable sanitized errors, decide alias trimming/blank semantics and approved privacy copy, and test command serialization. Until then the exact Iced evidence above is authoritative, while the proposed IPC table is only a draft target.
