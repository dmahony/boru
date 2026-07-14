# MCP GUI Test Actions

This document describes the GUI test-action subsystem — an optional,
security-gated extension to the MCP diagnostic server that lets AI agents
programmatically observe and interact with the Iced desktop GUI.

It is separate from the core MCP diagnostics (documented in
[MCP_DIAGNOSTICS.md](MCP_DIAGNOSTICS.md)) and is designed solely for
automated testing, CI, and controlled local development.

## Purpose

The GUI test action system enables an AI agent (or any MCP client) to:

- Navigate to specific screens (`chat_list`, `settings`, `friend_requests`)
- Set composer text and send messages
- Toggle UI features (dark mode, help overlay, settings)
- Wait for UI conditions to be met (screen transition, dark-mode state, etc.)
- Read a snapshot of the current GUI state
- Inspect the Iced message journal for debugging the event-processing pipeline

This allows end-to-end verification of the GUI behaviour without requiring a
human to visually inspect the screen or manually interact with the application.

## Architecture

```
┌──────────┐   Newline-delimited JSON-RPC 2.0 over TCP
│  MCP     │ ──────────────────────────────────────────────┐
│  Client  │                                                │
│ (Agent)  │                                                ▼
└──────────┘                                      ┌──────────────────┐
                                                  │   MCP Server     │
                                                  │  (mcp_server.rs) │
                                                  │                  │
                                                  │  ┌────────────┐  │
                                                  │  │ gui_action_ │──┼──→ tokio::mpsc::Sender<GuiActionRequest>
                                                  │  │    tx      │  │         (bounded, 256 max)
                                                  │  └────────────┘  │
                                                  │  ┌────────────┐  │
                                                  │  │  IcedMessage│  │
                                                  │  │  Journal    │  │    Shared with IcedChat event loop
                                                  │  └────────────┘  │
                                                  └──────────────────┘
                                                            │
                                              tokio::mpsc::Sender
                                                    (bounded)
                                                            │
                                                            ▼
                                              ┌──────────────────────┐
                                              │   Iced Subscription  │
                                              │  (not yet wired —   │
                                              │   see Limitations)  │
                                              └──────────────────────┘
                                                            │
                                              AppMessage::GuiTestActionReceived
                                                            │
                                                            ▼
                                              ┌──────────────────────┐
                                              │   IcedChat::update() │
                                              │  (handler stubbed —  │
                                              │   see Limitations)   │
                                              └──────────────────────┘
```

The key components are:

1. **`gui_test_actions.rs`** — Defines command types, the action channel
   (`GuiActionSender` / `GuiActionReceiver`), action history, rate limiter,
   snapshot types, and idempotency-key generation.

2. **`mcp_server.rs`** — The MCP JSON-RPC server. When
   `enable_gui_test_actions: true` is set, it registers three additional tools
   (`boru_send_gui_action`, `boru_get_gui_action_status`,
   `boru_get_gui_snapshot`) and gates the observation tools
   (`boru_get_iced_state`, `boru_get_iced_message_journal`).

3. **`main.rs`** — Creates the GUI action channel and passes the sender half
   (`gui_action_tx`) into the MCP server state when
   `--enable-gui-test-actions` is active.

4. **`app.rs`** — Defines `AppMessage::GuiTestActionReceived` and related
   variants (`GuiTestWaitSatisfied`, `GuiTestWaitTimedOut`).  Handlers are
   currently stubbed (see [Known Limitations](#known-limitations)).

## Explicit opt-in mode

GUI test actions are **never enabled by default**.  They require two
conditions:

1. **`--mcp`** flag — the MCP diagnostic server must be running.
2. **`--enable-gui-test-actions`** flag — explicitly opts in to GUI test
   action tools.

Without `--enable-gui-test-actions`, the MCP server returns
`-32601 (Method not found)` for `boru_send_gui_action`,
`boru_get_gui_action_status`, `boru_get_gui_snapshot`,
`boru_get_iced_state`, and `boru_get_iced_message_journal`.

### Example startup

```sh
cargo run --features gui --example iced_chat -- \
  --mcp \
  --mcp-bind 127.0.0.1:8765 \
  --enable-gui-test-actions
```

## Security boundaries

### Loopback enforcement

When `--enable-gui-test-actions` is set, the MCP listener **must** be bound to a
loopback address.  `main.rs` rejects a non-loopback `--mcp-bind` before starting
the application, and `mcp_server::spawn_mcp_server` repeats the check as
defence in depth.  The check uses `SocketAddr::ip().is_loopback()`, so both
IPv4 loopback (`127.0.0.0/8`) and IPv6 loopback (`::1`) are accepted; wildcard,
LAN, and public addresses are rejected.

This is a security invariant, not merely a recommended deployment setting:

| GUI actions | MCP bind | Required result |
|-------------|----------|-----------------|
| disabled | any address | Server may start; non-loopback exposure emits a warning |
| enabled | loopback | Server may start |
| enabled | non-loopback | Startup fails; no GUI-action listener is made available |

The normal diagnostic tools remain independently available on non-loopback
binds when GUI actions are disabled.  Operators must still treat that mode as
network-exposed and use their normal network controls; enabling GUI actions is
never a way to make a remotely reachable MCP server safe.

### Security policy (normative)

The following rules define the supported security boundary.  A change that
weakens any rule is a security-sensitive change and must add or update a
negative test.

1. **Opt-in only.** GUI actions are available only when both `--mcp` and
   `--enable-gui-test-actions` are present.  The default is disabled.  When
   disabled, action and GUI-observation methods return JSON-RPC `-32601`.
2. **Local-only control.** With GUI actions enabled, the listener is loopback
   only as specified above.  There is no authentication layer, so loopback
   access must be treated as equivalent to local process access.
3. **Semantic commands only.** The command enum is a closed allowlist of
   application operations (`GoToChatList`, `OpenRoom`, `OpenConversation`,
   `OpenFriends`, `OpenSettings`, `CloseDialog`, `SetComposerText`,
   `SubmitComposer`, `ClearComposer`, `FocusComposer`, `SelectPeer`,
   `ToggleDarkMode`, `ToggleHelp`, and `Wait`).
   It does not provide pixel coordinates, keyboard/mouse injection, arbitrary
   widget selectors, shell commands, filesystem paths, process control, or
   unrestricted desktop control.
4. **Bounded and validated input.** Identifiers and text are limited to
   `GUI_TEST_COMMAND_MAX_STRING_LEN` (currently 4096 characters), identifiers
   use only ASCII letters, digits, `-`, and `_`, control characters are rejected
   from text, and wait timeouts are capped at 30,000 ms.  Queue admission is
   non-blocking and bounded at 256 pending actions; rate limiting also enforces
   a 100 ms minimum interval and 100 actions per rolling minute.
5. **No secrets in data or logs.** MCP responses and diagnostic snapshots must
   not contain private keys, tickets, mailbox/discovery secrets, friend
   addresses, or chat history.  Action logs may include only the action kind,
   bounded lengths, status, IDs, and timing; never raw composer text or secret
   material.  IDs and room/topic identifiers remain potentially correlatable
   metadata and must not be treated as anonymous.
6. **Fail closed.** Invalid commands, disabled actions, queue overflow, and a
   closed action channel are rejected with a structured error.  An accepted
   action means only that it was queued; completion must be verified through
   action status or GUI state and must not be inferred from queue admission.

### Testable security checklist

The policy is testable without a desktop-control harness:

| Invariant | Required negative/positive check |
|-----------|-----------------------------------|
| Loopback-only binding | `test_spawn_mcp_server_rejects_non_loopback_with_gui_actions`; loopback acceptance test |
| Opt-in gating | `enable_gui_test_actions_defaults_to_false`; disabled-method `-32601` checks |
| No desktop escape hatch | `test_gui_test_command_rejects_dangerous_variants` and JSON unknown-variant rejection |
| Input bounds | control-character, identifier, string-overflow, and timeout validation tests |
| Queue/rate bounds | queue-full and rate-limit tests in the MCP adapter/action-channel suite |
| Response secrecy | `test_gui_test_command_no_secrets_in_json`, `test_gui_action_error_no_secrets_in_serialized_output`, and diagnostic snapshot secrecy tests |

Run the focused checks with:

```sh
cargo test --features gui --lib gui_test_command
cargo test --features gui --example iced_chat mcp_server::tests
```

A policy check is not complete if it only verifies that a request was enqueued;
the test must also assert the rejection path or the documented post-condition.

### No secrets exposed

The GUI action tools expose only semantic UI state:

- Screen name (`chat_list`, `settings`, `friend_requests`)
- Boolean toggles (dark mode, help visibility, settings visibility)
- Composer text (truncated to first 200 characters in snapshots)
- Entry count in the active room
- Active room topic (hex)
- A `notice` string (connection status)

Private keys, cryptographic material, message contents beyond the composer
text, friend addresses, and chat history are never returned.

### Input validation

Every `GuiTestCommand` variant runs through `command.validate()` before
it is accepted:

- String fields are bounded at 4096 characters maximum (`MAX_STRING_LEN`).
- Control characters are rejected (except space).
- Screen names are restricted to an allowlist (`chat_list`, `settings`,
  `friend_requests`).
- Wait conditions are restricted to an allowlist (`screen`, `dark_mode`,
  `composer_text`, `entries_count`).
- Wait timeouts are capped at 30 000 ms.

### Rate limiting

A `GuiActionRateLimiter` enforces:

| Limit | Value | Enforced by |
|-------|-------|-------------|
| Minimum interval | 100 ms between actions | `MIN_ACTION_INTERVAL_NS` |
| Per-minute cap | 100 actions per rolling 60 s window | `MAX_ACTIONS_PER_MINUTE` |
| Queue depth | 256 pending actions maximum | `MAX_PENDING` (mpsc channel cap) |

The rate limiter tracks `Instant` timestamps in a `VecDeque`.  Actions older
than 60 seconds are pruned before each check.  If a limit is exceeded, a
descriptive error message is returned with the suggested retry delay.

### Action history

A bounded ring buffer (`GuiActionHistory`) retains the last 1000 completed
actions (`MAX_HISTORY`).  Each `ActionRecord` contains:

- `idempotency_key` — caller-supplied or auto-generated
- `command` — serialised command description
- `status` — one of `Queued`, `Processed`, `Failed { error }`,
  `TimedOut { elapsed_ms }`
- `timestamp_ms` — wall-clock timestamp when recorded
- `duration_ms` — processing duration

## Available action tools

The following MCP tools are available when `--enable-gui-test-actions` is
active.  Tools are listed with their JSON-RPC method name, parameters, and
response shape.

### `boru_send_gui_action`

Send a GUI test command through the action channel.  The command is queued
in the bounded mpsc channel and processed by the Iced event loop on the next
subscription tick.

**Parameters:**

| Param | Type | Required | Default | Description |
|-------|------|----------|---------|-------------|
| `command` | object | **yes** | — | A `GuiTestCommand` object (see below) |
| `idempotency_key` | string | no | auto-generated | Unique key for status tracking |

**`command` variants:**

| Tag (`command` field) | Parameters | Description |
|-----------------------|------------|-------------|
| `navigate_to` | `{ "screen": "chat_list" \| "settings" \| "friend_requests" }` | Navigate to a specific screen |
| `set_composer_text` | `{ "text": "<string>" }` | Set the message input text (max 4096 chars, no control chars) |
| `send_message` | `{}` | Press Send — submits the current composer content |
| `toggle_dark_mode` | `{ "enabled": true \| false }` | Toggle dark mode on/off |
| `toggle_help` | `{}` | Toggle the help overlay |
| `open_settings` | `{}` | Open the settings screen |
| `close_settings` | `{}` | Close the settings screen |
| `wait` | `{ "condition": "...", "expected": "...", "timeout_ms": <num> }` | Wait for a UI condition (see [Wait conditions](#wait-conditions)) |

**Response:**

```json
{
  "sent": true,
  "idempotency_key": "gui_action_...",
  "command": { "command": "navigate_to", "screen": "settings" }
}
```

**Errors:**

- `-32000`: "GUI action queue is full (max 256 pending)"
- `-32000`: "GUI action channel is closed"
- `-32000`: "Invalid command: ..." (validation failure)

---

### `boru_gui_navigate`

Navigate the GUI to a named destination screen. A dedicated convenience tool that
wraps the corresponding `GuiTestCommand` variant for common screens.

**TypeScript:**

```typescript
type GuiNavigateDestination = "chat_list" | "friends" | "settings";

interface GuiNavigateParams {
  /** Target GUI screen -- "chat_list", "friends", or "settings" */
  destination: GuiNavigateDestination;
}

interface GuiNavigateResponse {
  /** Whether the navigation was accepted and queued */
  accepted: boolean;
  /** Idempotency key for tracking the action's status */
  action_id: string;
  /** Wall-clock timestamp (ms since Unix epoch) when queued */
  queued_at_ms: number;
}
```

**JSON Schema:**

```json
{
  "request": {
    "type": "object",
    "required": ["destination"],
    "properties": {
      "destination": {
        "type": "string",
        "enum": ["chat_list", "friends", "settings"],
        "description": "Target GUI screen to navigate to"
      }
    },
    "additionalProperties": false
  },
  "response": {
    "type": "object",
    "required": ["accepted", "action_id", "queued_at_ms"],
    "properties": {
      "accepted": { "type": "boolean" },
      "action_id": { "type": "string" },
      "queued_at_ms": { "type": "integer" }
    },
    "additionalProperties": false
  }
}
```

**Parameters:**

| Param | Type | Required | Description |
|-------|------|----------|-------------|
| `destination` | string | **yes** | One of `"chat_list"`, `"friends"`, `"settings"` |

**Response (success):**

```json
{
  "accepted": true,
  "action_id": "gui_action_...",
  "queued_at_ms": 1710000000123
}
```

**Destination to command mapping:**

| Destination | `GuiTestCommand` | Iced `AppMessage` |
|-------------|------------------|-------------------|
| `chat_list` | `GoToChatList` | `AppMessage::GoToChatList` |
| `friends`   | `OpenFriends`   | (via Iced update) |
| `settings`  | `OpenSettings`  | `AppMessage::OpenSettings` |

**Errors:**

- `-32000`: "Missing required argument: destination"
- `-32000`: "Invalid destination '...' ..."
- `-32000`: "destination too long (N bytes, max 4096)"
- `-32000`: "destination must not contain control characters"
- `-32000`: "GUI action queue is full (max 256 pending)"
- `-32000`: "GUI action channel is closed"

---

### `boru_get_gui_action_status`

Look up the status of a previously sent action by its idempotency key.

**Parameters:**

| Param | Type | Required | Description |
|-------|------|----------|-------------|
| `idempotency_key` | string | **yes** | The key returned by `boru_send_gui_action` |

**Response:**

```json
{
  "idempotency_key": "gui_action_...",
  "status": "sent",
  "note": "Full status tracking requires querying the Iced GUI state."
}
```

> ⚠️  The current implementation returns a static `"sent"` status because
> the action history lives inside the IcedChat state, which is not directly
> accessible from the MCP server.  See [Known limitations](#known-limitations).

---

### `boru_get_gui_snapshot`

Returns a metadata snapshot of the GUI diagnostics subsystem, including the
journal entry count and whether GUI test actions are enabled.

**Parameters:** none

**Response:**

```json
{
  "journal_entry_count": 42,
  "journal_latest_sequence": 42,
  "diagnostics_event_count": 128,
  "diagnostics_latest_sequence": 128,
  "active_rooms": ["abcd..."],
  "gui_test_actions_enabled": true
}
```

---

### `boru_get_iced_state`

Snapshot of the Iced application's diagnostics metadata.

**Parameters:** none

**Response:**

```json
{
  "message": "Iced diagnostics available",
  "journal_entry_count": 42,
  "journal_latest_sequence": 42,
  "diagnostics_event_count": 128,
  "diagnostics_latest_sequence": 128,
  "active_rooms": ["abcd..."]
}
```

---

### `boru_get_iced_message_journal`

Recent Iced `AppMessage` processing history — each entry records a message
that was dispatched through the Iced `update()` function.

**Parameters:**

| Param | Type | Required | Default | Description |
|-------|------|----------|---------|-------------|
| `since_sequence` | number | no | `0` | Only return entries with sequence > this value |
| `limit` | number | no | `200` | Maximum entries to return |

**Response:**

```json
{
  "entries": [
    {
      "sequence": 1,
      "label": "OpenRoom",
      "timestamp": "2026-07-14T19:00:00Z",
      "duration_us": 1234
    }
  ],
  "latest_sequence": 42,
  "returned_count": 1
```

---

### `boru_gui_set_composer`

Set the composer (message input) text without submitting.  This is the
dedicated tool for populating the message input field before triggering
sending via `boru_gui_submit_composer`.

**Parameters:**

| Param | Type | Required | Description |
|-------|------|----------|-------------|
| `text` | string | **yes** | The message text to insert (max 4096 bytes, no control characters) |

**Response:**

```json
{
  "sent": true,
  "action_id": "gui_action_...",
  "text_length": 17,
  "note": "Composer text set. Use boru_gui_submit_composer to submit."
}
```

**Errors:**

- `-32000`: "Missing required argument: text"
- `-32000`: "Composer text too long (N bytes, max 4096)"
- `-32000`: "Composer text must not contain control characters"
- `-32000`: "GUI action queue is full (max 256 pending)"
- `-32000`: "GUI action channel is closed"

---

### `boru_gui_submit_composer`

Submit the current composer text through the normal GUI send path — the same
path the Send button uses.  Call this **after** `boru_gui_set_composer` has
populated the composer.

**Parameters:** none

**Response:**

```json
{
  "sent": true,
  "action_id": "gui_action_...",
  "note": "Composer submit queued."
}
```

**Errors:**

- `-32000`: "GUI action queue is full (max 256 pending)"
- `-32000`: "GUI action channel is closed"

---

### `boru_gui_open_room`

Send a GUI 'open room' command.  The `room_id` must be an alphanumeric string
(letters, digits, hyphen, underscore), 1–128 characters.  This queues an
`OpenRoom` GUI test action.

**Parameters:**

| Param | Type | Required | Description |
|-------|------|----------|-------------|
| `room_id` | string | **yes** | Room identifier (alphanumeric + `-`/`_`, 1–128 chars) |

**Response:**

```json
{
  "sent": true,
  "action_id": "gui_action_...",
  "room_id": "test-room",
  "note": "Room open command queued."
}
```

**Errors:**

- `-32000`: "Missing required argument: room_id"
- `-32000`: "room_id must not be empty"
- `-32000`: "room_id too long (N bytes, max 128)"
- `-32000`: "Invalid room_id '...': must match pattern ^[a-zA-Z0-9_-]+$"
- `-32000`: "GUI action queue is full (max 256 pending)"
- `-32000`: "GUI action channel is closed"

---

### `boru_gui_open_conversation`

Open a direct conversation with a peer by their public key.  The
`conversation_id` must be a 64-hex-character peer public key (optionally
with a `0x` prefix).

**Parameters:**

| Param | Type | Required | Description |
|-------|------|----------|-------------|
| `conversation_id` | string | **yes** | Peer public key (64 hex chars, optional `0x` prefix) |

**Response:**

```json
{
  "sent": true,
  "action_id": "gui_action_...",
  "conversation_id": "abcd...",
  "note": "Open conversation command queued."
}
```

**Errors:**

- `-32000`: "Missing required argument: conversation_id"
- `-32000`: "conversation_id too long (N bytes, max 66)"
- `-32000`: "conversation_id must not contain control characters"
- `-32000`: "Invalid conversation_id '...': expected 64 hex chars representing a peer public key"
- `-32000`: "GUI action queue is full (max 256 pending)"
- `-32000`: "GUI action channel is closed"

---

### `boru_gui_toggle_dark_mode`

Toggle the dark mode UI setting on or off.

**Parameters:**

| Param | Type | Required | Description |
|-------|------|----------|-------------|
| `enabled` | bool | **yes** | `true` to enable, `false` to disable |

**Response:**

```json
{
  "sent": true,
  "action_id": "gui_action_...",
  "enabled": true,
  "note": "Dark mode toggle queued."
}
```

**Errors:**

- `-32000`: "Missing required argument: enabled"
- `-32000`: "GUI action queue is full (max 256 pending)"
- `-32000`: "GUI action channel is closed"

---

### `boru_run_gui_message_test`

**Orchestrated end-to-end GUI message sending test.**  This single-call tool
automates the full local message pipeline: opens a room, sets composer text,
submits the message, then polls for both a `MessageBroadcast` diagnostic event
and Iced journal revision increases.

Use this on **one node** (the sender).  Then verify remote delivery on the
other node using `boru_get_discovery_events` and correlated `message_hash`.

**Parameters:**

| Param | Type | Required | Default | Description |
|-------|------|----------|---------|-------------|
| `room_id` | string | **yes** | — | Room identifier to open (alphanumeric + `-`/`_`) |
| `message_text` | string | **yes** | — | Message text to send (max 4096 bytes) |
| `expected_peer_id` | string | **yes** | — | Peer that should receive the message (64-hex public key) |
| `timeout_ms` | number | no | `20000` | Maximum wait (clamped to 30s) |

**Response (success):**

```json
{
  "success": true,
  "room_id": "test-room",
  "message_text_length": 17,
  "expected_peer_id": "B_def...",
  "timed_out": false,
  "verification": {
    "navigation_queued": true,
    "composer_set": true,
    "submission_queued": true,
    "message_broadcast_detected": true,
    "gui_revision_increased": true
  },
  "broadcast_event": {
    "message_id": "abc123...",
    "message_hash": "e3b0c442...",
    "probe_id": null
  },
  "steps": [
    { "step": "open_room", "action_id": "gui_action_...", "status": "queued" },
    { "step": "set_composer", "action_id": "gui_action_...", "status": "queued", "text_length": 17 },
    { "step": "submit_composer", "action_id": "gui_action_...", "status": "queued" }
  ],
  "event_sequence_start": 10,
  "event_sequence_end": 25,
  "gui_revision_start": 5,
  "gui_revision_end": 15,
  "relevant_event_count": 15,
  "gui_revision_entry_count": 10,
  "note": "Local message pipeline verified. Use boru_get_discovery_events on the remote node to confirm delivery."
}
```

**Response (timeout):**

```json
{
  "success": false,
  "timed_out": true,
  "verification": {
    "navigation_queued": true,
    "composer_set": true,
    "submission_queued": true,
    "message_broadcast_detected": false,
    "gui_revision_increased": false
  },
  "note": "Message pipeline timed out before broadcast was detected."
}
```

**Cross-node correlation fields in the response:**

| Field | Source | Purpose |
|-------|--------|---------|
| `broadcast_event.message_id` | Local `DiagnosticEventKind::MessageBroadcast` | Unique message identifier for cross-node lookup |
| `broadcast_event.message_hash` | Local broadcast event | Content hash used to match sender → receiver |
| `room_id` | Input parameter | Room context for the delivery |
| `expected_peer_id` | Input parameter | Expected receiving peer |
| `event_sequence_start` | Local diagnostics | Sequence anchor for subsequent event queries |

**Errors:**

- `-32000`: "Missing required argument: room_id"
- `-32000`: "Missing required argument: message_text"
- `-32000`: "Missing required argument: expected_peer_id"
- `-32000`: "Invalid room_id '...': must match pattern ^[a-zA-Z0-9_-]+$"
- `-32000`: "message_text too long (N bytes, max 4096)"
- `-32000`: "message_text must not contain control characters"
- `-32000`: "GUI action queue is full (max 256 pending)"
- `-32000`: "GUI action channel is closed"

## Action lifecycle

```
┌──────────┐    ┌──────────────┐    ┌───────────┐    ┌───────────────┐
│  MCP     │───→│  validate()  │───→│  try_send │───→│  Iced update  │
│  Client  │    │  (command)   │    │  (channel)│    │  (handler)    │
└──────────┘    └──────────────┘    └───────────┘    └───────────────┘
                      │                                     │
                      │ (rejected:                          │ (on completion:
                      │  "Invalid command")                 │  action_history
                      ▼                                     ▼
                  ┌─────────┐                        ┌──────────────┐
                  │  Error  │                        │  ActionRecord │
                  │  return │                        │  (in history) │
                  └─────────┘                        └──────────────┘
```

The full lifecycle is:

1. **Validation** — `GuiTestCommand::validate()` checks parameters
   (string length, allowed screens/conditions, timeout bounds).
2. **Rate-limit check** — `GuiActionRateLimiter::check_and_record()` enforces
   10 req/s and 100 req/min.
3. **Queue** — The request is pushed into the bounded tokio mpsc channel
   (capacity 256).
4. **Processing** — The Iced subscription reads from the receiver and
   dispatches `AppMessage::GuiTestActionReceived` into the `update()` loop.
5. **Completion** — The handler processes the command and records the outcome
   in `GuiActionHistory` as an `ActionRecord`.
6. **Status query** — The MCP client polls `boru_get_gui_action_status`
   with the idempotency key to learn the final status.

## Action IDs

Every `boru_send_gui_action` call produces an idempotency key:

- **Provided by caller** — pass `idempotency_key` in the parameters.
- **Auto-generated** — if omitted, `generate_action_key()` creates a key in
  the format `gui_action_<hex timestamp>_<sequence>`.

The sequence counter is a global `AtomicU64` starting at `1`, ensuring
uniqueness even across rapid calls within the same microsecond.

The MCP client should save the returned `idempotency_key` and use it in
subsequent `boru_get_gui_action_status` calls to check the action's outcome.

## Status querying

Currently, `boru_get_gui_action_status` returns a static `"sent"` status
because the `GuiActionHistory` lives inside the IcedChat application state,
which is not exposed via the MCP server state.  The recommended workaround
is to:

1. Use `boru_get_iced_message_journal` to inspect the sequence of
   `AppMessage` entries processed by the Iced event loop.
2. Look for entries with a matching label (e.g. `"GuiTestActionReceived"`)
   and correlate them with the idempotency key from the action history.

Once the GUI action receiver is wired into the Iced subscription (see
[Known limitations](#known-limitations)), the action history will be
accessible from the MCP server and `boru_get_gui_action_status` will return
definitive status.

## Wait conditions

The `GuiTestCommand::Wait` variant allows the agent to pause until a UI
condition is satisfied, with an optional timeout.

| Condition | Expected type | Description |
|-----------|---------------|-------------|
| `screen` | string | The active screen name (`chat_list`, `settings`, `friend_requests`) |
| `dark_mode` | string | `"true"` or `"false"` |
| `composer_text` | string | The exact composer text content |
| `entries_count` | string | A decimal number (e.g. `"5"`) — chat entries in the active room |

The timeout defaults to 5000 ms and is capped at 30 000 ms.

The wait is implemented as an Iced stream that polls the condition on each
frame and emits `AppMessage::GuiTestWaitSatisfied` or
`AppMessage::GuiTestWaitTimedOut` accordingly.  Wait conditions are only
available when `--enable-gui-test-actions` is active and are handled by the
Iced `update()` function.

## Rate limits

| Limit | Value | Implementation |
|-------|-------|----------------|
| **Per-second** | 10 actions/s | Minimum 100 ms between consecutive actions |
| **Per-minute** | 100 actions/min | Rolling 60-second window |
| **Queue depth** | 256 actions | Bounded mpsc channel capacity |
| **History retention** | 1000 records | Bounded `VecDeque` ring buffer |

All limits are hard-coded compile-time constants in `gui_test_actions.rs`:

```rust
pub const MAX_STRING_LEN: usize = 4096;
pub const MAX_PENDING: usize = 256;
pub const MAX_HISTORY: usize = 1000;
pub const MIN_ACTION_INTERVAL_NS: u64 = 100_000_000;  // 100 ms
pub const MAX_ACTIONS_PER_MINUTE: usize = 100;
```

## Local GUI message testing

You can test GUI actions from the command line without an AI agent using raw
TCP connections to the MCP server.

### Prerequisites

Start the iced_chat application with GUI test actions enabled:

```sh
cargo run --features gui --example iced_chat -- \
  --mcp \
  --mcp-bind 127.0.0.1:8765 \
  --enable-gui-test-actions
```

### Send a navigate_to action

```sh
echo '{"jsonrpc":"2.0","method":"boru_send_gui_action","params":{"command":{"command":"navigate_to","screen":"settings"}},"id":1}' \
  | nc -w 3 127.0.0.1 8765
```

### Send a set_composer_text action

```sh
echo '{"jsonrpc":"2.0","method":"boru_send_gui_action","params":{"command":{"command":"set_composer_text","text":"Hello from MCP!"}},"id":2}' \
  | nc -w 3 127.0.0.1 8765
```

### Send a send_message action

```sh
echo '{"jsonrpc":"2.0","method":"boru_send_gui_action","params":{"command":{"command":"send_message"}},"id":3}' \
  | nc -w 3 127.0.0.1 8765
```

### Check GUI snapshot

```sh
echo '{"jsonrpc":"2.0","method":"boru_get_gui_snapshot","params":{},"id":4}' \
  | nc -w 3 127.0.0.1 8765
```

### Read the Iced message journal

```sh
echo '{"jsonrpc":"2.0","method":"boru_get_iced_message_journal","params":{"limit":10},"id":5}' \
  | nc -w 3 127.0.0.1 8765
```

## Two-node GUI message test workflow

This section describes how an AI agent can perform an end-to-end GUI message
test across two LAN nodes, each running the iced_chat GUI with MCP enabled.

The workflow covers the full lifecycle: opening a shared room, sending a
message via GUI actions on Node A, verifying local broadcast, confirming
network receipt on Node B, and repeating the test in reverse.

### Prerequisites

- Two machines on the same LAN (Node A and Node B).
- Both have the iced_chat binary built with the `gui` feature.
- Both share a valid room ticket for the same topic (or a known room
  identifier accessible to both nodes).
- Both start with `--mcp --enable-gui-test-actions`.

### Step 1: Start nodes

**Node A:**

```sh
cargo run --features gui --example iced_chat -- \
  --mcp --mcp-bind 127.0.0.1:8765 --enable-gui-test-actions
```

**Node B:**

```sh
cargo run --features gui --example iced_chat -- \
  --mcp --mcp-bind 127.0.0.1:8766 --enable-gui-test-actions
```

### Step 2: Configure Hermes MCP connections

In the Hermes profile (e.g. `config.yaml` or via `hermes mcp`), add two MCP
servers:

```yaml
mcp_servers:
  boru_a:
    transport: tcp
    url: "127.0.0.1:8765"
  boru_b:
    transport: tcp
    url: "127.0.0.1:8766"
```

When the two nodes are on different machines, bind each MCP server to
`0.0.0.0:<port>` and point the Hermes configuration to the LAN IP
addresses.  The **remote MCP addresses are configured in the Hermes
profile only** — the boru-chat application never hard-codes remote
addresses.

### Step 3: Agent workflow

The following numbered steps define the full test protocol.  Each step
lists the MCP tool calls and the expected response fields used for
cross-node correlation.

---

#### 3.1 Query node status on both hosts

Retrieve each node's identity and capture the starting event sequence
numbers for later comparison.

```json
→ boru_a: boru_get_node_status({})
← { "node_id": "A_abc...", "version": "0.101.0",
    "latest_event_sequence": 10 }

→ boru_b: boru_get_node_status({})
← { "node_id": "B_def...", "version": "0.101.0",
    "latest_event_sequence": 8 }
```

**Recorded for correlation:**

| Node | Field | Value |
|------|-------|-------|
| A | `node_id` | `A_abc...` (sender in A→B test) |
| B | `node_id` | `B_def...` (receiver in A→B test) |

---

#### 3.2 Open the same room on both nodes via GUI actions

Use the `boru_gui_open_room` convenience tool to navigate each node to
the shared room.  The `room_id` must match an identifier known to both
nodes — it is used as the chat-room label, not the hex topic ID.

```json
→ boru_a: boru_gui_open_room({ "room_id": "test-room" })
← { "sent": true, "action_id": "gui_action_...",
    "room_id": "test-room",
    "note": "Room open command queued." }

→ boru_b: boru_gui_open_room({ "room_id": "test-room" })
← { "sent": true, "action_id": "gui_action_...",
    "room_id": "test-room",
    "note": "Room open command queued." }
```

**Correlation fields:** `room_id` — must be identical on both nodes.

---

#### 3.3 Wait until both GUIs show the room as selected

Poll the Iced message journal on each node until an `OpenRoom` entry
appears, indicating the GUI has processed the open action.

```json
→ boru_a: boru_get_iced_message_journal({
    "since_sequence": 0, "limit": 20 })
← { "entries": [ ...,
    { "sequence": 12, "message_variant": "OpenRoom",
      "success": true, "timestamp": "2026-07-14T19:01:00Z" },
    ... ],
  "latest_sequence": 15 }

→ boru_b: boru_get_iced_message_journal({
    "since_sequence": 0, "limit": 20 })
← { "entries": [ ...,
    { "sequence": 10, "message_variant": "OpenRoom",
      "success": true, "timestamp": "2026-07-14T19:01:01Z" },
    ... ],
  "latest_sequence": 13 }
```

**Wait strategy:** Retry every 500 ms until an `OpenRoom` entry with
`"success": true` appears on each node, or timeout after 15 seconds.

---

#### 3.4 Set composer text on Node A

Populate the message input field on Node A with the test message.

```json
→ boru_a: boru_gui_set_composer({
    "text": "Hello from Node A -- cross-node test" })
← { "sent": true,
    "action_id": "gui_action_...",
    "text_length": 36,
    "note": "Composer text set. Use boru_gui_submit_composer to submit." }
```

---

#### 3.5 Submit the composer text on Node A

Trigger the Send action through Node A's GUI.  This submits the composer
content through the normal send path.

```json
→ boru_a: boru_gui_submit_composer({})
← { "sent": true,
    "action_id": "gui_action_...",
    "note": "Composer submit queued." }
```

---

#### 3.6 Verify local GUI state on Node A

Check that Node A processed the submission and broadcast the message.

**A) Iced message journal — confirm `SendPressed` was processed:**

```json
→ boru_a: boru_get_iced_message_journal({
    "since_sequence": 15, "limit": 20 })
← { "entries": [ ...,
    { "sequence": 16, "message_variant": "SendPressed",
      "success": true, "timestamp": "2026-07-14T19:01:05Z",
      "duration_ms": 120 },
    ... ],
  "latest_sequence": 18 }
```

**B) Diagnostic events — extract the `MessageBroadcast` event for
cross-node correlation:**

```json
→ boru_a: boru_get_discovery_events({
    "since_sequence": 10, "limit": 20 })
← { "events": [ ...,
    { "sequence": 14, "timestamp": "2026-07-14T19:01:05Z",
      "room_id": "<topic-hex>",
      "peer_id": null,
      "kind": {
        "type": "message_broadcast",
        "message_id": "msg_abc123def456",
        "message_hash": "e3b0c44298fc1c149afbf4c8996fb924..."
      }
    },
    ... ],
  "latest_sequence": 18 }
```

**Cross-node correlation fields captured here:**

| Field | Value | Purpose |
|-------|-------|---------|
| `message_id` | `msg_abc123def456` | Unique message identifier for cross-node lookup |
| `message_hash` | `e3b0c442...` | Content hash — must match the receiver's `MessageReceived` event |
| `sequence` | `14` | Anchor for delta queries on subsequent polls |
| `timestamp` | `2026-07-14T19:01:05Z` | Wall-clock send time |

---

#### 3.7 Verify network receipt on Node B

Poll Node B's diagnostic events for a `MessageReceived` entry whose
`message_hash` matches the one captured in Step 3.6.

```json
→ boru_b: boru_get_discovery_events({
    "since_sequence": 8, "limit": 20 })
← { "events": [ ...,
    { "sequence": 10, "timestamp": "2026-07-14T19:01:05Z",
      "room_id": "<topic-hex>",
      "peer_id": "A_abc...",
      "kind": {
        "type": "message_received",
        "message_id": "msg_abc123def456",
        "message_hash": "e3b0c44298fc1c149afbf4c8996fb924...",
        "sender_id": "A_abc..."
      }
    },
    ... ],
  "latest_sequence": 12 }
```

**Verification — compare correlation fields:**

| Field | Sender (Step 3.6) | Receiver (Step 3.7) | Match? |
|-------|-------------------|---------------------|--------|
| `message_hash` | `e3b0c442...` | `e3b0c442...` | ✓ Must match |
| `message_id` | `msg_abc123def456` | `msg_abc123def456` | ✓ Must match |
| `sender_id` | — | `A_abc...` | ✓ Must match Node A's `node_id` |
| `room_id` | `<topic-hex>` | `<topic-hex>` | ✓ Must be the same room |

**Wait strategy:** Retry `boru_get_discovery_events` on Node B every
500–1000 ms for up to 15 seconds until a `MessageReceived` event with
the matching `message_hash` is found.

---

#### 3.8 Verify application handling on Node B

Confirm that Node B's Iced event loop processed the incoming message by
checking the message journal for a `NetEvent` entry (or similar
application-layer handling).

```json
→ boru_b: boru_get_iced_message_journal({
    "since_sequence": 13, "limit": 20 })
← { "entries": [ ...,
    { "sequence": 14, "message_variant": "NetEvent",
      "success": true, "timestamp": "2026-07-14T19:01:05Z",
      "duration_ms": 85 },
    ... ],
  "latest_sequence": 15 }
```

---

#### 3.9 Verify GUI state on Node B

Take a GUI snapshot on Node B and confirm the diagnostic subsystem is
active.

```json
→ boru_b: boru_get_gui_snapshot({})
← { "journal_entry_count": 15,
    "journal_latest_sequence": 15,
    "diagnostics_event_count": 20,
    "diagnostics_latest_sequence": 12,
    "active_rooms": ["<topic-hex>"],
    "gui_test_actions_enabled": true }
```

---

#### 3.10 Repeat B → A (bidirectional verification)

Repeat steps 3.4–3.9 with roles swapped:

1. **Set composer** on Node B via `boru_gui_set_composer`.
2. **Submit** on Node B via `boru_gui_submit_composer`.
3. **Verify local broadcast** on Node B (capture `message_hash`, `message_id`).
4. **Verify receipt** on Node A via `boru_get_discovery_events`
   (match `message_hash` and `sender_id`).
5. **Verify application handling** on Node A via
   `boru_get_iced_message_journal`.
6. **Verify GUI state** on Node A via `boru_get_gui_snapshot`.

---

### Cross-node test correlation record

The GUI action responses identify queued actions, but they do not by themselves
prove remote delivery. The test runner MUST create one opaque `test_id` for each
direction and carry it through the verification record. Do not put a remote MCP
address in this record (or in the application configuration).

Use the following record shape for both A→B and B→A:

```json
{
  "test_id": "gui-msg-20260715T190105Z-a-to-b-01",
  "direction": "A_to_B",
  "sender_node_id": "A_abc...",
  "receiver_node_id": "B_def...",
  "room_id": "<topic-hex>",
  "message_hash": "e3b0c44298fc1c149afbf4c8996fb924...",
  "probe_id": "probe_abc123",
  "sent_at_ms": 1740949200000,
  "received_at_ms": 1740949200500,
  "latency_ms": 500,
  "sender_action_ids": {
    "open_room": "gui_action_...",
    "set_composer": "gui_action_...",
    "submit_composer": "gui_action_..."
  },
  "sender_gui_verified": true,
  "receiver_network_verified": true,
  "receiver_application_verified": true,
  "receiver_gui_verified": true
}
```

Field sources and rules:

| Field | Source / rule |
|-------|---------------|
| `test_id` | Generated by the test runner; unique per direction and run. |
| `sender_node_id`, `receiver_node_id` | `boru_get_node_status`; never infer from an address. |
| `room_id` | The same room/topic supplied to both nodes; record the exact value. |
| `message_hash` | Sender's `MessageBroadcast` event or probe broadcast response; match it on the receiver. |
| `probe_id` | `boru_send_probe` response when using the diagnostic probe path; use `boru_find_received_probe` with this value. |
| `sent_at_ms` | Sender broadcast/probe response. |
| `received_at_ms`, `latency_ms` | Receiver's `boru_find_received_probe` response. |
| `sender_action_ids` | `action_id` returned by each GUI action call. |

For a GUI chat-message test, `boru_run_gui_message_test` proves only the local
pipeline and returns `broadcast_event.message_hash` plus per-step `action_id`
values. It does not return a `test_id`, receiver timestamp, or remote delivery
result. The runner must add `test_id` and `sender_node_id` from node status, then
query the other MCP server and populate the receiver fields. For a diagnostic
probe, `boru_send_probe` and `boru_find_received_probe` provide the strongest
cross-node correlation because both sides expose `probe_id`, `message_hash`,
`sender_id`, and timestamps.

Do not mark a direction successful solely because a GUI action returned
`sent: true` or because the sender observed `MessageBroadcast`. A direction is
successful only when the record contains matching `room_id` and `message_hash`,
the receiver identifies the expected sender, and the receiver application/GUI
checks have also passed. Report missing fields as `Unknown`/`Not Observed` rather
than filling them with inferred values.

### Deterministic two-node run checklist

Run these calls in order, retaining each response in the correlation record:

1. `boru_a.boru_get_node_status({})` and `boru_b.boru_get_node_status({})`.
2. `boru_a.boru_gui_open_room({"room_id": R})` and
   `boru_b.boru_gui_open_room({"room_id": R})`; poll each journal until the
   room-open action is processed.
3. On A, call `boru_gui_set_composer`, then
   `boru_gui_submit_composer`; verify A's journal and local broadcast.
4. On B, call `boru_find_received_probe` when the test uses a diagnostic
   probe, or poll B's discovery/application diagnostics for the correlated
   `message_hash` when the running build exposes message-receipt events.
5. Verify B's journal/snapshot and mark the A→B record only with observed
   values.
6. Repeat steps 3–5 with B as sender and A as receiver, producing a separate
   `test_id` and correlation record.

The two records must be evaluated independently: successful A→B delivery does
not establish symmetric B→A delivery.

---

### Automated test with `boru_run_gui_message_test`

The single-call orchestrated test automates Steps 3.2–3.6 on **one node**
(the sender).  It opens the room, sets composer text, submits, and polls
until a `MessageBroadcast` event and an Iced revision increase are
detected.  The response includes the `broadcast_event` with `message_id`
and `message_hash` for cross-node correlation.

Call this on Node A, then verify delivery on Node B using
`boru_get_discovery_events` as described in Step 3.7.

```json
→ boru_a: boru_run_gui_message_test({
    "room_id": "test-room",
    "message_text": "Hello from Node A -- auto test",
    "expected_peer_id": "B_def...",
    "timeout_ms": 20000
  })
← {
    "success": true,
    "room_id": "test-room",
    "message_text_length": 32,
    "expected_peer_id": "B_def...",
    "timed_out": false,
    "verification": {
      "navigation_queued": true,
      "composer_set": true,
      "submission_queued": true,
      "message_broadcast_detected": true,
      "gui_revision_increased": true
    },
    "broadcast_event": {
      "message_id": "msg_abc123def456",
      "message_hash": "e3b0c44298fc1c149afbf4c8996fb924...",
      "probe_id": null
    },
    "steps": [
      { "step": "open_room", "action_id": "gui_action_...", "status": "queued" },
      { "step": "set_composer", "action_id": "gui_action_...",
        "status": "queued", "text_length": 32 },
      { "step": "submit_composer", "action_id": "gui_action_...",
        "status": "queued" }
    ],
    "event_sequence_start": 10,
    "event_sequence_end": 18,
    "gui_revision_start": 5,
    "gui_revision_end": 12
  }

→ boru_b: boru_get_discovery_events({
    "since_sequence": 8, "limit": 20 })
← { "events": [ ...,
    { "sequence": 10, "room_id": "<topic-hex>",
      "kind": {
        "type": "message_received",
        "message_id": "msg_abc123def456",
        "message_hash": "e3b0c44298fc1c149afbf4c8996fb924...",
        "sender_id": "A_abc..."
      }
    },
    ... ]
  }
```

---

### Cross-node correlation fields reference

The following fields are the keys to correlating a message across two nodes:

| Field | Emitted by | Appears in | Purpose |
|-------|-----------|------------|---------|
| `message_hash` | Sender's `MessageBroadcast` event | `boru_get_discovery_events` | Content hash — must match sender↔receiver |
| `message_id` | Sender's `MessageBroadcast` event | `boru_get_discovery_events` | Unique message identifier for cross-node lookup |
| `sender_id` | Receiver's `MessageReceived` event | `boru_get_discovery_events` | Public key of the originator |
| `room_id` | Both sides | `boru_run_gui_message_test` input / event payload | Room context for the delivery |
| `action_id` | GUI action tools | `boru_gui_set_composer`, `boru_gui_submit_composer`, etc. | Per-node action tracking |
| `sequence` | Both `discovery_events` and `iced_journal` | `boru_get_discovery_events`, `boru_get_iced_message_journal` | Anchor for delta queries |
| `timestamp` | Both sides | `boru_get_discovery_events`, `boru_get_iced_message_journal` | Wall-clock timestamp for latency estimation |

---

### Verification checklist

| # | Check | Tools | ✓ |
|---|-------|-------|---|
| 1 | Node A and B are running and reachable | `boru_get_node_status` | ☐ |
| 2 | Both nodes opened the same room | `boru_gui_open_room`, `boru_get_iced_message_journal` | ☐ |
| 3 | Composer was populated on sender | `boru_gui_set_composer` | ☐ |
| 4 | Submission was queued on sender | `boru_gui_submit_composer` | ☐ |
| 5 | Sender's Iced journal shows `SendPressed` | `boru_get_iced_message_journal` | ☐ |
| 6 | Sender's diagnostics show `MessageBroadcast` | `boru_get_discovery_events` | ☐ |
| 7 | Receiver's diagnostics show `MessageReceived` with matching `message_hash` | `boru_get_discovery_events` | ☐ |
| 8 | Receiver's Iced journal shows `NetEvent` | `boru_get_iced_message_journal` | ☐ |
| 9 | Receiver's GUI snapshot confirms activity | `boru_get_gui_snapshot` | ☐ |
| 10 | Bidirectional test (B → A) passes all checks | Same tools, roles swapped | ☐ |

### Security note

The MCP bind addresses for Node A and Node B are **never hard-coded in
the boru-chat application**.  They are:

1. Specified at startup via `--mcp-bind` (CLI flag).
2. Configured in the Hermes profile (`config.yaml`) under
   `mcp_servers.*.url`.

This guarantees that the application binary does not contain any baked-in
remote addresses.  Each deployment can use its own addressing scheme
(loopback for co-located nodes, LAN IPs for remote machines, etc.).

## Known limitations

### GUI action receiver is not wired into Iced subscription

The `gui_action_rx` receiver is created in `main.rs` but is **not passed to
`IcedChat::new()` or added to the Iced subscription list**.  This means:

- `boru_send_gui_action` accepts commands and returns `"sent": true`, but
  the action never reaches the Iced event loop.
- `boru_get_gui_action_status` always returns `"status": "sent"` because
  the action history is never written.
- The `AppMessage::GuiTestActionReceived`, `GuiTestWaitSatisfied`, and
  `GuiTestWaitTimedOut` variants are defined but their `update()` handlers
  are stubs.

**Status:** Planned.  The infrastructure (types, channel, rate limiter,
history, validation, MCP tools) is fully implemented.  Wiring the receiver
into the Iced subscription to complete the loop is a remaining integration
step.

### No action history access from MCP

The `GuiActionHistory` lives inside the `IcedChat` application state, which
is not passed to the MCP server.  Even once the receiver is wired, the MCP
server will need access to the history (or the history needs to be shared
via the `McpAppState`) for `boru_get_gui_action_status` to return
definitive status.

### Snapshot is metadata only

`boru_get_gui_snapshot` and `boru_get_iced_state` return diagnostic metadata
(journal lengths, sequence numbers, enabled flags) — they do **not** return
the live GUI state (current screen, composer text, etc.).  A true
`GuiSnapshot` struct is defined in `gui_test_actions.rs` but is not yet
populated or exposed.

### No chat message injection

The MCP server can send diagnostic probes through the gossip mesh, but GUI
actions can only interact with the local Iced UI.  There is no mechanism to
inject chat messages into a remote room or impersonate a user.

### Non-persistent state

All action history, rate-limiter state, and event journals exist only in
memory.  They are lost on process restart.

### Single-node scope

GUI test actions operate on the local node's UI only.  They cannot observe
or control the GUI of a remote peer.  Cross-node verification (e.g. "Node A
sends a message, Node B sees it") must use the core diagnostic probes and
peer-discovery tools described in [MCP_DIAGNOSTICS.md](MCP_DIAGNOSTICS.md).
