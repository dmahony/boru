# Configuration

boru-chat is configured through CLI flags, environment variables, and JSON
settings files. This document covers all available options.

## CLI Flags

### iced_chat (GUI)

| Flag | Type | Default | Description |
|------|------|---------|-------------|
| `--name` | String | Short public key | Display name in chat |
| `--secret-key` | String | Auto-generated | Hex-encoded ed25519 secret key |
| `--data-dir` | Path | See below | Persistent data directory |
| `--relay` | URL | `https://boru.chat:8443` | iroh relay server URL |
| `--no-relay` | Flag | Off | Disable relay mode entirely |
| `--no-dht` | Flag | Off | Disable private-room DHT discovery (public lobby unaffected) |
| `--bind-port` | u16 | `0` (OS-assigned) | Local QUIC bind port |
| `--perf` | Flag | Off | Enable performance instrumentation |
| `--mcp` | Flag | Off | Enable MCP diagnostic server |
| `--enable-gui-test-actions` | Flag | Off | Enable GUI test actions via MCP (requires `--mcp`) |
| `--mcp-bind` | String | `127.0.0.1:8765` | MCP server bind address |

### Subcommands

| Command | Description |
|---------|-------------|
| `open [topic]` | Open a new or saved chat room (without topic: saved/reuse; with topic: specific) |
| `join <ticket>` | Join an existing chat room via ticket |
| `logs` | Open the standalone log viewer |

### `doctor` example

| Flag | Description |
|------|-------------|
| No specific flags beyond normal net features | |

### `setup` example

| Flag | Description |
|------|-------------|
| No specific flags (uses default net features) | |

## Environment Variables

| Variable | Type | Default | Description |
|----------|------|---------|-------------|
| `BORU_CHAT_DATA_DIR` | Path | — | Override the persistent data directory |
| `BORU_CHAT_FILES_DIR` | Path | `<data_dir>/files/` | Override the image/files storage root |
| `BORU_PERF` | `0`/`1` | `0` | Enable performance instrumentation |
| `BORU_PERF_PRINT` | `0`/`1` | `1` | Print performance summary at exit |
| `BORU_PERF_SLOW_MS` | Integer | `100` | Slow-operation threshold in milliseconds |
| `BORU_DEBUG` | `0`/`1` | `0` | Enable gossip debug event log |
| `BORU_DEBUG_PATH` | Path | `~/.local/share/boru-chat/gossip-debug.log` | Gossip debug log path |
| `RUST_LOG` | EnvFilter | `info` | Tracing filter (overrides file log filter) |
| `XDG_DATA_HOME` | Path | `~/.local/share` | Base for default data directory |

### Data Directory Resolution

The data directory is resolved in this order:

1. `--data-dir` CLI flag
2. `BORU_CHAT_DATA_DIR` environment variable
3. `$XDG_DATA_HOME/boru-chat` (typically `~/.local/share/boru-chat/`)
4. `$HOME/.local/share/boru-chat/`
5. `$LOCALAPPDATA/boru-chat` (Windows only)
6. `$PWD/.boru-chat` (fallback)

## Settings File (`settings.json`)

The settings file is stored in the data directory and persists UI preferences.
Currently limited — see `examples/iced_chat/app.rs` for the authoritative list.

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `theme` | String | `"light"` | UI theme (`light`/`dark`) |
| (future) | | | More settings will be added as the UI matures |

## Profile File (`profile.json`)

Stored beside `secret_key.txt` in the data directory.

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `display_name` | String | Short public key | User-visible display name (max 64 chars) |
| `bio` | String Empty | User biography (max 140 chars) |
| `share_files` | Bool | `false` | Enable file sharing with peers |
| `max_file_size` | u64 | 100 MB | Maximum allowed file size for sharing |
| `shared_files` | Array | `[]` | Metadata about files offered to peers |

## Data Directory Layout

```
<data_dir>/
├── boru.db                    # SQLite relational storage (V4, current)
├── chat_history.json          # Per-room chat message history (JSON)
├── outbox.json                # Outgoing message delivery state (JSON)
├── conversations.json         # Conversation metadata (JSON)
├── rooms.json                 # Room topic registry (JSON)
├── friends.json               # Friend contact list (JSON)
├── friend_requests.json       # Friend request state (JSON)
├── mailbox.json               # Encrypted offline envelopes (JSON)
├── settings.json              # UI preferences (JSON)
├── profile.json               # User profile + shared file metadata (JSON)
├── secret_key.txt             # Node identity key (hex-encoded ed25519)
├── message_store.db           # Legacy SQLite store (migration source, read-only)
│
├── logs/                      # Persistent trace logs
│   └── iced_chat.log
│
├── gossip-debug.log           # Gossip debug trace (BORU_DEBUG=1)
│
├── files/                     # Per-user image store
│   └── <user-hash>/
│       └── <content-hash>.<ext>
│
└── library/                   # File library managed storage
    ├── <prefix>/
    │   └── <content-hash>     # Imported files (content-addressed)
    └── .refs/
        └── <content-hash>     # Referenced file source paths
```

## Build Features

See `docs/build-release.md` for feature flags and build configuration.
