# Configuration

Boru is configured through CLI flags, environment variables, and JSON
settings files. This document covers all available options.

## CLI Flags

### boru (GUI)

| Flag | Type | Default | Description |
|------|------|---------|-------------|
| `--name` | String | Short public key | Display name in chat |
| `--secret-key` | String | Auto-generated | Hex-encoded ed25519 secret key |
| `--data-dir` | Path | See below | Persistent data directory |
| `--relay` | URL | `https://boru.chat:8443` | iroh relay server URL |
| `--no-relay` | Flag | Off | Disable relay mode entirely |
| `--no-dht` | Flag | Off | Disable member-discovery DHT (private-room and user-created public-room trackers) |
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
| `BORU_DATA_DIR` | Path | — | Override the persistent data directory (also checks legacy `BORU_CHAT_DATA_DIR` for backward compatibility) |
| `BORU_FILES_DIR` | Path | `<data_dir>/files/` | Override the image/files storage root |
| `BORU_PERF` | `0`/`1` | `0` | Enable performance instrumentation |
| `BORU_PERF_PRINT` | `0`/`1` | `1` | Print performance summary at exit |
| `BORU_PERF_SLOW_MS` | Integer | `100` | Slow-operation threshold in milliseconds |
| `BORU_DEBUG` | `0`/`1` | `0` | Enable gossip debug event log |
| `BORU_DEBUG_PATH` | Path | `~/.local/share/boru/gossip-debug.log` | Gossip debug log path |
| `KLIPY_API_KEY` | String | — | Optional API key for external GIF search (KLIPY provider). When unset, external GIF search is disabled gracefully and the picker shows a "KLIPY not configured" state. See [External GIF Search](#external-gif-search-klipy). |
| `RUST_LOG` | EnvFilter | `info` | Tracing filter (overrides file log filter) |
| `XDG_DATA_HOME` | Path | `~/.local/share` | Base for default data directory |

### Data Directory Resolution

The data directory is resolved in this order:

1. `--data-dir` CLI flag
2. `BORU_DATA_DIR` environment variable
3. `$XDG_DATA_HOME/boru` (typically `~/.local/share/boru/`)
4. `$HOME/.local/share/boru/`
5. `$LOCALAPPDATA/boru` (Windows only)
6. `$PWD/.boru` (fallback)

Boru also honours the legacy `BORU_CHAT_DATA_DIR` variable and legacy paths
(`$XDG_DATA_HOME/boru-chat`, `$PWD/.boru-chat`) for backward compatibility.

## Settings File (`settings.json`)

The settings file is stored in the data directory and persists UI preferences.
Currently limited — see `src/bin/boru/app.rs` for the authoritative list.

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `theme` | String | `"light"` | UI theme (`light`/`dark`) |
| (future) | | | More settings will be added as the UI matures |

## Catalogue Limits File (`catalogue_limits.json`)

Catalogue request limits can be tuned without rebuilding the application by
placing `catalogue_limits.json` in the data directory and loading it with
`CatalogueLimitsConfig::load_from_path`. The loader applies defaults for omitted
fields, rejects zero values and invalid relationships, and reports malformed
JSON or unreadable files as an error. The example schema is in
`docs/catalogue_limits.json`; all fields are positive integers.

The default file location is `<data_dir>/catalogue_limits.json`. Deployments may
also load another path explicitly at startup. An absent file is not an error;
callers can use `CatalogueLimitsConfig::default()` when no override is needed.

## External GIF Search (KLIPY)

External GIF search (the GIF picker in the chat composer) uses the KLIPY
provider. It is **optional** and disabled by default. For developer-facing
details (obtaining a key, the `GifProvider` abstraction, adding another
provider, attribution and caching limits), see
[`docs/gif-search.md`](gif-search.md).

- The API key is read at runtime from the `KLIPY_API_KEY` environment
  variable. It is **never hardcoded or committed**, and it is **not stored in
  `settings.json`** or any other plaintext config file. Keys are obtained
  from the KLIPY Partner Panel (`https://partner.klipy.com/api-keys`).
- When `KLIPY_API_KEY` is unset, the GIF picker shows a "KLIPY is not
  configured" state; the app continues to work normally (text chat,
  attachments, and user-uploaded GIFs are unaffected).
- The key is never logged: `Debug` output for the config module redacts it
  (`<redacted>`), and request URLs that embed the key are never written to
  logs or error messages.
- The key is never sent to peers and never included in chat messages.
- Example configuration file with a documented placeholder:
  `docs/klipy.env.example` (copy it to `klipy.env` or export the variable).

**Privacy.** Search terms are sent to the KLIPY service when you use the GIF
picker. Boru does not send usernames, peer IDs, room IDs, message contents,
contact details, or attachment metadata to KLIPY, and does not add behavioural
analytics. Full search queries are not logged at normal log levels (request
URLs in logs have the API key and query string redacted).

Additional privacy guarantees:

- **External search is opt-in per use.** The GIF picker only contacts KLIPY
  when you open it and type a search (or load trending suggestions). Merely
  opening a conversation never loads external media.
- **Remote peers cannot trigger searches on your device.** A received GIF
  message carries direct media URLs; the receiving client fetches the
  rendition (bounded to 15 MiB) and never calls the provider search endpoint.
- **No full-resolution auto-load.** Previews download the smallest rendition;
  playback uses a mid-tier rendition, not the original. Full-size originals
  are only ever fetched if a playback rendition is missing or expired, and
  never merely because a conversation is opened.
- **No tracking in stored payloads.** `SharedGif` messages carry only the
  provider, provider ID, rendition URLs, format, dimensions, and alt text —
  no tracking parameters, search queries, or identity.
- **No proxying.** Only GIF search/trending/media requests go to KLIPY; all
  Boru P2P chat and file-transfer traffic stays on iroh's own transport.

**Attribution.** KLIPY's integration requirements (verified 2026-08-08)
require "Search KLIPY" as the default placeholder text in the search input
(watermark / "Powered by KLIPY" marks are optional). The Boru picker's search
input placeholder is "Search KLIPY", meeting this REQUIRED attribution — see
[`docs/gif-search.md`](gif-search.md) §7.


**Desktop-build risk.** Boru's desktop builds do **not** embed a shared API
key in the binary; the key is supplied per-user through the environment. If a
distribution ever embeds a shared key instead, that key is recoverable by
anyone who downloads the binary, so embedding is strongly discouraged. Prefer
per-user `KLIPY_API_KEY` configuration.

**Authentication seam.** All key access goes through `KlipyConfig`
(`boru_core::klipy_config`), so the authentication mechanism can be replaced
(e.g. secure store, OAuth) without changing the UI or the domain model.

## Download Limits File (`download_limits.json`)

Download admission can be tuned without rebuilding by loading
`DownloadLimitsConfig::load_from_path` from `download_limits.json`. The global
concurrency cap defaults to `5`; downloads restored during startup are capped at
`3` concurrent transfers; the global queue cap defaults to `32` pending
downloads.  All limit values must be positive integers.  The example schema is
in `docs/download_limits.json`.

| Field | Default | Description |
|-------|---------|-------------|
| `max_concurrent_downloads` | `5` | Maximum active transfers at once |
| `max_startup_downloads` | `3` | Burst cap for downloads restored on restart |
| `max_downloads_per_peer` | `2` | Maximum queued+active downloads from one peer |
| `max_active_hash_verifications` | `2` | CPU-bound hash verification slots |
| `max_queued_downloads` | `32` | Maximum queued downloads before rejection |
| `progress_update_interval` | `250ms` | Minimum interval between DB progress writes |

Environment overrides are available through the following variables via
`DownloadLimitsConfig::from_env()`:

| Variable | Overrides |
|----------|-----------|
| `BORU_MAX_CONCURRENT_DOWNLOADS` | `max_concurrent_downloads` |
| `BORU_MAX_STARTUP_DOWNLOADS` | `max_startup_downloads` |
| `BORU_MAX_DOWNLOADS_PER_PEER` | `max_downloads_per_peer` |
| `BORU_MAX_QUEUED_DOWNLOADS` | `max_queued_downloads` |
| `BORU_PROGRESS_DB_UPDATE_INTERVAL_MS` | `progress_update_interval` |
| A startup caller can use `DownloadLimiter::try_enqueue_startup` so restored
downloads observe the burst cap, while ordinary calls to `try_enqueue` use the
global concurrency cap.

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
│   └── boru.log
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
