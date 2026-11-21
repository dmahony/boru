# AppSnapshot contract → runtime source map

Contract: `src/snapshot.rs` (`SNAPSHOT_SCHEMA_VERSION = 1`, commit `bbff29d2`, task 0). This is a read-only serialized projection; all collection fields default to empty when omitted and all optional fields default to `None`. Required scalar fields have no serde default. `Option<T>` serializes as value or JSON `null`; `Vec<T>` is `[]` when empty. No behavior changes are proposed here.

## Top-level (`AppSnapshot`)

| Field | Exact Rust type | Semantics | Authoritative source/query |
|---|---|---|---|
| `schema_version` | `u32` | Required; current contract constant `SNAPSHOT_SCHEMA_VERSION` (1), reject unsupported versions | Constant in `src/snapshot.rs`, not runtime state |
| `identity` | `Option<IdentitySnapshot>` | Absent/null if identity unavailable; default `None` | Runtime identity `IcedChat.local_public: PublicKey`; only public key representation, never secret key |
| `active_profile` | `Option<ProfileSnapshot>` | Absent/null if no selected profile | No distinct active-profile selection source found; `UserProfileStore` (`profile_store`) is scoped to `local_public`, and profile display data is loaded through the profile store. Do not infer a separate profile selector from UI state. |
| `rooms` | `Vec<RoomSnapshot>` | Empty if none | `ConversationStore` (`conversation_store`) entries filtered to room/group conversations (not DMs); persisted room metadata is `ConversationMeta` via `MessageStore::list_conversation_meta` / `ConversationApplicationService::list_conversations`. Use runtime room membership/status source only for the optional live participant count. |
| `direct_messages` | `Vec<DirectMessageSnapshot>` | Empty if none | `ConversationStore` entries for direct conversations plus `MessageStore::list_conversation_meta` for last activity/preview; peer label resolves through the runtime friends/name resolution path (friends store first, then announced/session names, then short key). |
| `unread` | `Vec<UnreadSnapshot>` | Empty if no unread conversation rows; count is required and may be zero | Durable authoritative query: `MessageStore::list_conversation_meta` exposes `unread_count`; per-conversation query `get_unread_count`, aggregate `total_unread_count`. Include only conversations present in the conversation query, or explicitly define whether zero rows are emitted. |
| `activity` | `Vec<ActivitySnapshot>` | Empty when no recent events | No canonical user-activity service/query identified in the current contract/runtime. Existing Iced activity/dashboard buffers are screen-specific presentation state, not an authoritative all-app activity source; source needs definition before projection. |
| `network_health` | `Option<NetworkHealthSnapshot>` | Null if health cannot be queried | Runtime endpoint/network manager plus peer/relay state (`IcedChat` endpoint, neighbors, direct/relayed peers, mesh health); status must be derived from endpoint readiness/health service, peer count from runtime connected-peer set, relay from active endpoint/connection details. No existing GUI diagnostic snapshot field is itself authoritative for app state. |
| `transfers` | `Vec<TransferSummary>` | Empty if no active/recent transfers | `TransferStore::snapshot()` is the transfer-state authority; the GUI `FilesState` active maps are resynchronized from it (`resync_outbound_panel`, `resync_inbound_panel`). Project transfer records, not rendered rows. |

## Nested fields

| Type.field | Exact Rust type | Semantics | Authoritative source/query |
|---|---|---|---|
| `IdentitySnapshot.id` | `String` | Required; stable public identifier | `IcedChat.local_public.to_string()`; never secret key material |
| `IdentitySnapshot.display_name` | `Option<String>` | Optional/null when not configured | Runtime local label/profile display name (`local_label`; profile data from `profile_store`), not a widget label |
| `ProfileSnapshot.id` | `String` | Required stable profile identifier | `UserProfile.id` associated with the active local profile; current app binds store to local public key; no independent active-profile selection identified |
| `ProfileSnapshot.display_name` | `String` | Required; empty string is representable (no serde default) | `UserProfile.display_name` |
| `ProfileSnapshot.description` | `Option<String>` | Optional/null when unset | Corresponding `UserProfile` description/bio field, if present; no UI-derived fallback |
| `RoomSnapshot.id` | `String` | Required stable conversation ID | `ConversationEntry.topic` encoded consistently (current core conversation ID is 32 bytes; use canonical hex/text encoding) |
| `RoomSnapshot.name` | `String` | Required; may be empty by type contract | `ConversationEntry.display_name()` / authoritative conversation metadata name |
| `RoomSnapshot.topic` | `Option<String>` | Optional/null when no topic/description | Persisted room/conversation metadata, when available; do not substitute a locally rendered subtitle |
| `RoomSnapshot.participant_count` | `Option<u32>` | Null if live count unknown | Runtime room membership/peer set for that topic; never infer from directory advertised counts or visible participant widgets |
| `RoomSnapshot.last_activity_at_ms` | `Option<u64>` | Null if no activity timestamp | `ConversationMeta.last_activity_at_ms` from `MessageStore`; convert only if nonnegative/valid |
| `DirectMessageSnapshot.id` | `String` | Required stable conversation ID | Canonical direct topic derived/stored for local identity and peer; conversation-store key |
| `DirectMessageSnapshot.peer_id` | `String` | Required stable peer ID | Direct-conversation peer key / decoded direct topic counterpart; canonical public-key representation |
| `DirectMessageSnapshot.peer_name` | `String` | Required display label; fallback is still a string | Central peer-name resolver backed by FriendsStore, last-announced/session name, then short public-key formatter |
| `DirectMessageSnapshot.last_message_preview` | `Option<String>` | Null if no preview; empty stored preview should be treated as absent | `ConversationMeta.last_message_preview` from message store (stored empty default means no preview) |
| `DirectMessageSnapshot.last_activity_at_ms` | `Option<u64>` | Null if no timestamp | `ConversationMeta.last_activity_at_ms` |
| `UnreadSnapshot.conversation_id` | `String` | Required stable conversation ID | Same canonical room/DM ID encoding as respective records |
| `UnreadSnapshot.count` | `u32` | Required; zero is valid | Stored `ConversationMeta.unread_count` / `MessageStore::get_unread_count` |
| `ActivitySnapshot.id` | `String` | Required stable item ID | No established canonical activity-event ID source; do not mint from widget row index |
| `ActivitySnapshot.kind` | `String` | Required machine-readable kind | No canonical activity taxonomy/source currently established |
| `ActivitySnapshot.summary` | `String` | Required user-visible summary | Must come from a defined safe activity projection, not view text |
| `ActivitySnapshot.occurred_at_ms` | `u64` | Required Unix milliseconds | No canonical all-app activity event timestamp source currently established |
| `ActivitySnapshot.conversation_id` | `Option<String>` | Null when activity is not conversation-related | Canonical conversation ID from source event, if any |
| `NetworkHealthSnapshot.ready` | `bool` | Required | Runtime endpoint readiness; not a UI loading flag |
| `NetworkHealthSnapshot.status` | `String` | Required status label; no prescribed enum/default | Derive from runtime endpoint/mesh health state with a stable documented vocabulary |
| `NetworkHealthSnapshot.connected_peer_count` | `Option<u32>` | Null when not knowable | Current runtime connected-neighbor/peer set; distinguish connected from merely discovered peers |
| `NetworkHealthSnapshot.relay_url` | `Option<String>` | Null when no active/known relay | Active endpoint/connection relay configuration/connection details; avoid exposing unrelated peer relay URLs |
| `TransferSummary.id` | `String` | Required stable transfer ID | `TransferRecord.transfer_id` from `TransferStore::snapshot()` |
| `TransferSummary.direction` | `String` | Required; contract values `upload` or `download` | Transfer record direction; normalize to these exact values |
| `TransferSummary.file_name` | `String` | Required display name | Transfer record's safe display filename (label only) |
| `TransferSummary.state` | `String` | Required state label | Transfer record state (`TransferState`); map to stable documented vocabulary, not UI card text |
| `TransferSummary.transferred_bytes` | `u64` | Required; zero valid | Transfer record progress bytes |
| `TransferSummary.total_bytes` | `Option<u64>` | Null when total unknown | Transfer record total size |

## Iced coupling to avoid

Current GUI diagnostic publishing is `IcedChat::publish_gui_state()` in `src/bin/boru/app.rs` (~9693): it reads `screen`, `composer_text`, dialogs, `conversations`, `neighbors`, friend counts and other IcedChat fields to construct `IcedStateSnapshot`. Its `build_dashboard_snapshot()` (~9768) explicitly traverses `files_state.shared_by_me_rows`, `inbound_active`, `downloaded_history`, `peer_catalogue_view`, and other screen-specific `FilesState` collections; it returns `None` unless the File Sharing screen is active. That is suitable as a GUI diagnostic/test snapshot, not as the AppSnapshot data layer: these are presentation caches, vary with active screen, can be stale/partial, and bind a frontend adapter to Iced types. The new projection must query/share GUI-independent runtime services/stores directly (ConversationStore/MessageStore, UserProfileStore, transfer store, endpoint/network state and a defined activity source) and must not call `view_*`, inspect widget trees, or derive authority from row structs.

## Gaps to resolve before implementing the projection

1. Define an authoritative active-profile selection (or explicitly specify that only the local profile is active).
2. Define a canonical all-app activity stream and stable IDs/kinds.
3. Expose GUI-independent read APIs for rooms/DM classification and current per-room participant counts, endpoint readiness/relay details, and transfer snapshots. `ConversationApplicationService` already exposes safe conversation reads/unread queries, but does not yet cover the full AppSnapshot contract.
