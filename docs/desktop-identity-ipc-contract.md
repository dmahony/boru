# Desktop identity / onboarding IPC audit (React port)

Status: audit of the current implementation; no identity, network, crypto, or storage code changed.

## Implemented identity startup contract

The Tauri bridge now implements `get_startup_state`, `list_profiles`, `select_identity`, `create_identity`, and `update_alias` in `desktop/src-tauri/src/identity.rs`. Requests/results are camelCase, endpoints are public IDs, and secret keys never appear in IPC responses. `get_startup_state` returns `{state: 'no_identity'|'existing', endpointId?, displayName?}`; the React router shows onboarding for `no_identity` and the workspace for `existing`. The app's startup probe only checks and validates `secret_key.txt`; it does not create directories, keys, or other files. Existing identity startup opens that key; identity creation is explicit and refuses to overwrite an existing identity.

The Iced binary deliberately retains its existing auto-create behavior. This lifecycle change is Tauri-only.

## Previous audit findings

- The Iced binary resolves one data directory, loads `secret_key.txt` if present, otherwise generates and persists a secret key (`src/bin/boru/main.rs::load_or_generate_secret_key_at`, `run_app` around line 805). The welcome/empty-state flag is `first_run = room_history.is_empty() && friends.is_empty()` (`src/bin/boru/app.rs` around 6004); this is not identity detection and is not a profile picker. The first action clears it.
- Tauri registers only snapshot, rooms/messages/contacts/settings, UI-event replay, send/read, room-open, and media commands (`desktop/src-tauri/src/lib.rs::command_handler` and `run`). There are currently **no** IPC commands for profile listing/selection, identity creation, alias update, ticket join, privacy copy, or startup identity detection.
- Tauri startup unconditionally calls `DesktopRuntime::start_from_config` with `secret_key: None`. That runtime reads `secret_key.txt` or generates and writes a new key if absent (`src/desktop_runtime.rs::start_from_config`, approx. 476–515). Thus today it cannot meet “no identity => onboarding; never auto-create” without changing startup orchestration. A UI-only wrapper would be misleading and unsafe; no such wrapper is added in this audit.
- Iced ticket join is the `AppMessage::JoinFromTicket` flow (`app.rs` approx. 11189 onward). It trims and rejects empty input, parses managed invitation v2 / owner-bound invitation / standard `RoomInvitation` (stable `boru1:` or legacy ticket), rejects discovery topics, checks managed-room authorization in SQLite, persists room history and joins via gossip. It reports parse, missing-storage/policy, authorization, discovery-topic, timeout/subscription errors in the UI. This is not exposed through Tauri.
- Alias/display name is a local label from CLI `--name`, persisted app settings, or short endpoint ID (`app.rs` approx. 6013–18); settings update sites persist `display_name`. There is no Tauri setter. Iced’s initial empty state copy is “BORU CHAT” / “Private. Peer-to-peer. No central servers.” (see `view_main_empty_state`); there is no separate privacy-acknowledgement IPC/API.

## React contract required (proposed wire surface; currently unimplemented)

All command args/results use camelCase JSON. Secret key bytes/encoding, key file paths, and filesystem paths must never be returned, logged, or placed in JS/DOM. Endpoint IDs are public identifiers.

| Command | Request | Success | Expected errors |
|---|---|---|---|
| `get_startup_identity_state` | `{}` | `{state: 'existing'|'none', endpointId?: string, displayName?: string}`; never key material | `storage_error`, `invalid_identity`, `runtime_error` |
| `list_identity_profiles` | `{}` | `{profiles: [{endpointId, displayName, hasExistingIdentity}]}` | `storage_error`, `invalid_identity` |
| `select_identity_profile` | `{endpointId: string}` | `{endpointId: string}` | `invalid_param`, `not_found`, `identity_load_error`, `runtime_error` |
| `create_identity_profile` | `{displayName?: string}` (only after explicit user confirmation) | `{endpointId: string}` | `invalid_param`, `storage_error`, `identity_create_error` |
| `join_room_ticket` | `{ticket: string}` | `{topicId: string}` | `invalid_param` (empty/malformed/discovery topic), `storage_unavailable`, `authorization_error`, `join_timeout`, `join_error` |
| `set_identity_alias` | `{displayName: string}` | `{displayName: string}` | `invalid_param`, `storage_error`, `runtime_error` |
| `get_onboarding_privacy_info` | `{}` | `{title, body}` (static informational copy only) | none expected |

These names/payloads are a proposed contract for downstream React work, not existing registered commands. Existing Tauri errors elsewhere are Rust `Result<_, bridge::AppError>`; the table’s stable error codes need to be formalized before frontend integration. Startup enum: `loading | existing | none | error` in UI; backend result states `existing | none`. Routing: `existing -> workspace` (same endpoint ID); `none -> onboarding`; error -> explicit retry/error screen. Never generate or select an identity automatically. Creation occurs only after explicit confirmation. A ticket join requires the active identity and must not create one.

## Readiness constraint

Profile listing/selection/creation requires a defined profile-root/selection model; the current Tauri runtime has one fixed app-data directory and auto-generates a key there. Implementing the proposed commands as wrappers alone cannot preserve semantics while satisfying the no-auto-create rule. The startup lifecycle must first be explicitly redesigned to inspect existing identity state before runtime construction and to construct the runtime only after selection/creation; that is a state-machine change, not a thin wrapper. No Rust workflow change was made here.