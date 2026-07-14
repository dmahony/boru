# GUI action MCP JSON-RPC schemas

This is the authoritative wire contract for GUI action methods. Requests use JSON-RPC 2.0 over the MCP TCP transport. GUI methods are available only when `--enable-gui-test-actions` is enabled.

All request `params` values are JSON objects. Unknown properties are rejected with JSON-RPC error code `-32602` (`Invalid params`). Missing required properties are also `-32602`. Value validation failures, queue failures, and application failures are returned as `-32000` with a descriptive `data` string. Disabled tools return `-32601`.

## Shared types

`ActionQueued` is the common asynchronous acknowledgement:

```json
{"sent": true, "action_id": "gui_action_..."}
```

`GuiTestCommand` is a tagged object with `command` as its discriminator. The accepted tags are:

```text
go_to_chat_list
open_room {room_id}
open_conversation {conversation_id}
open_friends
open_settings
close_dialog
set_composer_text {text}
submit_composer
select_peer {peer_id}
toggle_dark_mode {enabled}
toggle_help
wait {condition, timeout_ms}
```

Unknown command tags and unknown fields inside a command or wait condition are rejected during deserialization. Strings are bounded and control characters are rejected by command validation. Wait timeout is bounded to 30,000 ms.

## Methods

### boru_send_gui_action

Request: `{command: GuiTestCommand, idempotency_key?: string}`. Response: `{sent: boolean, idempotency_key: string, command: GuiTestCommand}`. The server generates `idempotency_key` when omitted. Errors include missing/invalid command, invalid idempotency key, queue full, and channel closed.

### boru_gui_get_action_status

Request: `{action_id: string}`. Response when retained:

```json
{"found":true,"action_id":"...","idempotency_key":"...","command":"...","status":{},"timestamp_ms":0,"duration_ms":0}
```

Response when absent: `{"found":false,"action_id":"...","note":"..."}`. `action_id` is required, bounded, and must not contain control characters.

### boru_get_gui_snapshot

Request: `{}`. Response:

```json
{"journal_entry_count":0,"journal_latest_sequence":0,"diagnostics_event_count":0,"diagnostics_latest_sequence":0,"active_rooms":[],"gui_test_actions_enabled":true}
```

### boru_gui_navigate

Request: `{destination: "chat_list" | "friends" | "settings"}`. Response:

```json
{"accepted":true,"action_id":"gui_action_...","queued_at_ms":0}
```

### boru_gui_set_composer

Request: `{text: string}`. Empty text is rejected; text longer than the composer limit is clamped. Response: `{sent: true, action_id: string, text_length: integer, clamped: boolean, note: string}`.

### boru_gui_open_room

Request: `{room_id: string}`. `room_id` is non-empty, at most 128 bytes, and ASCII alphanumeric plus `-` or `_`. Response: `{sent: true, action_id: string, room_id: string, note: string}`.

### boru_gui_open_conversation

Request: `{conversation_id: string}`. The value is 64 hexadecimal characters, optionally prefixed with `0x`. Response: `{sent: true, action_id: string, conversation_id: string, note: string}`.

### boru_gui_submit_composer

Request: `{}`. Response: `{sent: true, action_id: string, note: string}`.

### boru_gui_toggle_dark_mode

Request: `{enabled: boolean}`. Response: `{sent: true, action_id: string, enabled: boolean}`.

### boru_gui_close_dialog

Request: `{}`. Response: `{sent: true, action_id: string, note: string}`.

### boru_gui_wait_for_state

Request: `{condition: GuiWaitCondition, timeout_ms?: integer}`. `timeout_ms` defaults to 10,000 and is clamped to 30,000. Response: `{reached: boolean, timed_out: boolean, condition: GuiWaitCondition, snapshot: IcedStateSnapshot}`.

`GuiWaitCondition` is tagged by `type` and accepts `screen_is`, `room_selected`, `peer_visible`, `message_visible`, `gui_revision_at_least`, `conversation_selected`, `composer_text_is`, `dialog_open`, `dialog_closed`, and `unread_count_at_least` with the fields defined by the corresponding Rust enum. Unknown condition fields are rejected.

### boru_run_gui_message_test

Request: `{room_id: string, message_text: string, expected_peer_id: string, timeout_ms?: integer}`. `timeout_ms` defaults to 20,000 and is clamped to 60,000 by the local pipeline handler. Response contains `success`, `first_failed_stage`, `room_id`, `message_text_length`, `verification`, `steps`, and `note`. This tool verifies only the local GUI pipeline; it does not prove remote delivery.

## JSON-RPC error envelope

```json
{
  "jsonrpc":"2.0",
  "id":1,
  "error":{"code":-32602,"message":"Invalid params","data":"Unknown argument: typo"}
}
```

The adapter tests exercise the strict outer schemas through `handle_request`, and direct handler tests cover semantic type, length, character, queue, and channel errors.
