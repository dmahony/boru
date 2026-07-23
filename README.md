# Boru

Gossip messages over broadcast trees — a peer-to-peer chat application built on
[iroh](https://github.com/n0-computer/iroh).

## Architecture

Boru is a Rust library (`boru_core`) and example GUI application
(`examples/iced_chat`) that provides:

- **Gossip protocol** — room-based message broadcasting over QUIC
- **Direct messaging** — inbox protocol for offline delivery, whisper protocol
  for private 1:1 channels
- **Backfill** — late-joining peers can request missed messages from existing
  peers
- **Friend management** — signed contact and friend-request negotiation
- **File sharing** — content-addressed file attachments, profile-offered files
  with signed, requester-filtered catalogues and per-peer permissions
- **Relational storage** — SQLite-based persistence with managed migrations

## Storage

All persistent data lives under a single data directory, resolved in this order:

1. `--data-dir` CLI flag
2. `BORU_DATA_DIR` environment variable (also checks legacy `BORU_CHAT_DATA_DIR` for backward compatibility)
3. `$XDG_DATA_HOME/boru` (typically `~/.local/share/boru/`)
4. `$PWD/.boru`
###
File Layout

```text
<data_dir>/
├── boru.db                # SQLite: inbox, outbox, file objects, attachments
├── chat_history.json       # Per-room chat message history
├── outbox.json             # Outgoing message delivery state
├── conversations.json      # Conversation metadata
├── rooms.json              # Room topic registry
├── friends.json            # Friend contact list
├── friend_requests.json    # Friend request state
├── mailbox.json            # Encrypted offline message delivery
├── settings.json           # UI / app preferences
├── user_profile.json       # Profile settings + shared file metadata
├── secret_key.txt          # Node identity secret key
├── message_store.db        # Legacy SQLite (migration source, read-only)
└── files/                  # Per-user image store
    └── <user-hash>/<content-hash>.<ext>
```

### Storage Layers

| Layer | Store | Backend | Purpose |
|---|---|---|---|
| **Primary relational** | `Storage` (SQLite) | `boru.db` | Inbox/outbox, contacts, file objects, attachments, shared files, permissions, downloads |
| **Chat history** | `ChatHistoryStore` | `chat_history.json` | Per-room message history (JSON, still active in GUI) |
| **Outgoing queue** | `OutboxStore` | `outbox.json` | Delivery state tracking (JSON, still active in GUI) |
| **Conversations** | `ConversationStore` | `conversations.json` | Conversation metadata (JSON) |
| **Friends** | `FriendsStore` | `friends.json` | Friend list (JSON) |
| **Friend requests** | `FriendRequestStore` | `friend_requests.json` | Pending/accepted/declined requests (JSON) |
| **Mailbox** | `MailboxStore` | `mailbox.json` | Encrypted offline-message envelopes (JSON) |
| **Room history** | `RoomHistoryStore` | `rooms.json` | Topic registry (JSON) |
| **User profile** | `UserProfile` | `user_profile.json` | Display name, sharing settings (JSON) |
| **Images** | `ImageStore` | `files/` | Content-addressed user-uploaded images |

### Key Design Properties

- **Exactly-once local persistence** — `INSERT … ON CONFLICT DO NOTHING`
  prevents duplicate message storage at the SQLite level.
- **At-least-once transport** — outbox rows survive crashes (Sent→Pending
  recovery), retry with configurable backoff, and ACK-based dedup at the
  recipient.
- **WAL mode + integrity checks** — crash-safe writes, automatic corruption
  detection on open.
- **Forward-only migrations** — schema is tracked; opening a newer DB on an
  older binary is safely rejected.
- **Content-addressed attachments** — file objects keyed by blake3 hash for
  deduplication and integrity.
- **Plaintext at rest** — ciphertext blobs are stored unencrypted in SQLite;
  transport-layer encryption (QUIC/TLS 1.3) protects messages in flight.
- **Restrictive permissions** — data directory and database are `0o700`/`0o600`
  on Unix.

### Schema Versions

| Version | What's added |
|---|---|
| 1 | `inbox`, `outbox`, `contacts`, `sync_cursor` (message delivery) |
| 2 | `file_objects`, `message_attachments`, `shared_files`, `file_collections`, `file_collection_items`, `shared_file_permissions`, `downloads`, `profile_manifest_state` |

See [`docs/message-storage-design.md`](docs/message-storage-design.md) for
the full storage architecture.

## Remote file sharing

Profiles advertise shared-file metadata through signed, requester-specific
catalogue snapshots. A catalogue contains safe display metadata and a
monotonic revision; it never contains local filesystem paths, permission rows,
or a download capability. The client verifies the owner's signature and the
owner identity before caching the projection. `known_revision` can produce a
`NotModified` response, while a revision change during pagination requires a
restart. There is no continuous catalogue-polling worker.

Clicking download performs a fresh authorization request over
`/boru-file-access/1`. The owner re-checks the live relationship, grants,
offer, availability, expected hash, size, and version, then issues a
requester-bound signed descriptor that expires after 60 seconds. Cached
catalogue visibility does not authorize access.

Iroh-blobs transfers the bytes. The receiver writes temporary output and
verifies the exact size and BLAKE3 content hash before atomically installing
the file and recording completion. Pause/resume re-resolves the peer and
re-authorizes; it is not byte-range resume of the destination file. Queue,
concurrency, size, timeout, and hash-verification limits bound resource use.

See [`docs/remote-file-sharing.md`](docs/remote-file-sharing.md),
[`docs/security-model.md`](docs/security-model.md), and
[`docs/privacy-model.md`](docs/privacy-model.md) for the protocol workflow,
security properties, privacy guarantees, storage behavior, and manual tests.

## Discovery

Peers find each other through multiple layered discovery mechanisms.
The system separates **address resolution** (finding transport addresses for
a known peer) from **member discovery** (finding which peers are in a room).

### Address Resolution (How to dial a known peer)

| Source | Technology | Scope |
|--------|-----------|-------|
| Current | In-memory active connection | Node-local |
| Persisted | `FriendsStore.known_addrs` | Node-local |
| mDNS | LAN multicast | Local network |
| Configured | Bootstrap addresses | Node-local |
| Relay | iroh relay server | WAN |
| **DHT** | Mainline DHT / Pkarr | Global |
| TrustedPeer | Config file | Node-local |

Resolution priority: `Current → Persisted → Mdns → Configured → Relay → Dht → TrustedPeer`

- **mDNS** discovers peers on the local network automatically (always active).
- **DhtAddressLookup** resolves `EndpointId` to transport addresses on the
  global Mainline DHT using Pkarr-signed records. Gated by `--no-dht`.
- By default, only relay URLs are published (`--publish-direct-addresses`
  exposes direct IPs — use with caution for privacy).

### Member Discovery (Finding room peers)

- **Public rooms**: Deterministic identity derived from (network, room name,
  protocol version). Peers use `distributed-topic-tracker` to publish and
  discover each other on the DHT. Continuous background loops re-publish
  presence every 5 minutes and discover new peers every 30 seconds.
- **Private rooms**: Same DHT mechanism but with namespace isolation via a
  32-byte `DiscoverySecret`. Records are HPKE-encrypted so only members with
  the secret can read them. Discovery is gated by `--no-dht`.
- **Tickets**: Both room types support out-of-band invitation tickets that
  encode the room identity (topic + optional secret + bootstrap relay),
  bypassing DHT entirely.

### Wire Format

Discovery records are ~171-byte Ed25519-signed envelopes carrying a 33-byte
payload: version byte + 32-byte `EndpointId`. Private-room records are
HPKE-encrypted per-minute. The validation pipeline checks size, timestamp,
decoding, identity match, and signature — in that order, cheapest first.

### Privacy

| Setting | Implication |
|---------|-------------|
| Default (relay-only) | IP addresses never published to DHT |
| `--publish-direct-addresses` | Public IP published on Mainline DHT (faster P2P) |
| Private rooms (with secret) | DHT namespace is undetectable without the secret; records encrypted |

### DHT Outage Behaviour

- Existing connections and known addresses continue working.
- Exponential backoff on publish/discover failures (1s → 2s → 4s → 60s cap).
- mDNS and ticket-based joins unaffected.
- Once DHT recovers, normal operation resumes automatically.

See [`docs/discovery-architecture.md`](docs/discovery-architecture.md) for
the full architecture, namespace derivation, validation pipeline, DHT outage
fallback, and operator guidance.

## Running

```sh
# GUI (with auto-discovery)
cargo run --example iced_chat --features gui -- --name <nickname>

# With a specific data directory
BORU_DATA_DIR=~/.boru cargo run --example iced_chat --features gui -- --name <nickname>

# GUI with advanced connection details dialog on start
cargo run --example iced_chat --features gui -- --name <nickname> --show-connection-details

# All CLI options
cargo run --example iced_chat --features gui -- --help
```

## GUI User Guide

The iced_chat GUI is a desktop chat application built with the [Iced](https://iced.rs/) toolkit
(v0.14). It follows a conversation-first design — like Telegram or Signal — with a sidebar for
navigation and a main panel for content.

> **Screenshots note:** The GUI uses Iced v0.14 with hardware-accelerated rendering (wgpu/tiny-skia).
> A representative mockup of the dark-theme dashboard layout is available at
> [`docs/screenshots/dashboard-mockup.html`](docs/screenshots/dashboard-mockup.html). Open it in a
> browser to see the interface structure. The mockup faithfully represents the layout, colour scheme,
> and component placement of the actual application.

### Interface Overview

The window is split into two areas:

```
┌────────────────┬────────────────────────────────────┐
│  Sidebar       │  Main Panel                        │
│  (280px fixed) │  (fills remaining width)           │
│                │                                    │
│  Boru   ＋ ⚙   │  BORU — landing / chat /           │
│  Label/Relay   │  settings / profile / etc.         │
│  ───────────── │                                    │
│  CHATS (N)     │                                    │
│  ● Peer 1  1m  │                                    │
│  ○ Peer 2  5h  │                                    │
│  ───────────── │                                    │
│  FRIENDS (N)   │                                    │
│  [input]       │                                    │
│  ● Alice  …  │                                    │
│  ───────────── │                                    │
│  DISCOVER (N)  │                                    │
│  ● Peer  Chat  │                                    │
│  ───────────── │                                    │
│  REQUESTS (N)  │                                    │
│  Manage Reqs   │                                    │
│  Alice ✓ ✗    │                                    │
└────────────────┴────────────────────────────────────┘
```

### Sidebar Sections

The sidebar has four collapsible sections:

**CHATS** — Lists all active conversations (direct-message threads and rooms).
Each row shows an avatar (initials circle or profile image), an online indicator
(● green / ○ grey), the conversation name, a one-line message preview, a timestamp,
and an unread count. Conversations are sorted: online first, then by most recent activity.
Click a row to open that chat. The "Remove" button on hover deletes the conversation
from the sidebar.

**FRIENDS** — Lists all confirmed friends with avatar, online status, and a "…" menu
button that opens the friend profile. At the top, a text input labelled "Add friend
by key…" accepts a peer public key to send a friend request. Friends are sorted
alphabetically.

**DISCOVER** — Shows peers discovered via mDNS (LAN) or DHT (WAN). Each row has an
avatar, a generated friendly name, a green online dot, and two action buttons:
"Chat" (opens a direct-message conversation) and "Browse Files" (opens the peer's
shared file catalogue). Discovered peers are not automatically friends — the Chat
button provides a quick way to start messaging them directly.

**REQUESTS** — Shows incoming pending friend requests with ✓ accept and ✗ decline
buttons. A "Manage Requests" button opens the full friend-request management screen
(which also shows outgoing requests and an expanded add-friend form).

### Screens

The main panel changes based on which screen is active:

- **Landing screen** — Branding ("BORU"), tagline, status card (Online/Mesh/Relay/ Friends
  Online), four quick-action buttons (Start Chat, Add Friend, Join Ticket, Browse Files),
  and a recent-activity feed. Shown when no conversation is selected.
- **Chat panel** — Message header, scrollable message log (date separators, chat bubbles
  with labels and timestamps), and a composer with attach/send/help buttons.
- **Settings** — Identity card (display name, public key with copy button, profile image),
  appearance (dark/light mode, chat text size), network (peer counts, mesh health, relay
  mode, DHT toggle), actions (clear history with confirmation), and shared files management.
- **Friend Profile** — Name (with inline rename), status, actions (Chat, Browse Files,
  Remove/Block), peer key copy, recent messages preview.
- **Peer Profile / Catalogue** — A remote peer's shared files catalogue with download buttons.
- **Friend Requests** — Full-screen view with incoming, outgoing, and canned-key sections.
- **Image Preview** — Full-size image view from within a chat.

### Adding a Friend

1. **By public key** — Type the peer's 52-character hex public key into the "Add friend
   by key…" input in the FRIENDS sidebar section, then press Enter. A friend request is
   sent. You can also use the "＋" button in the sidebar header → "Add Friend", which
   opens the full friend-request management screen with the same input.
2. **From Discover** — Click "Chat" on a discovered peer to start a conversation, then
   send a friend request from within the chat or from their profile.
3. **Import** — Use "＋" → "Import Friend" to import a friend from a file (the file format
   is internal — export via the friend profile's "…" menu when available).

Once your request is accepted, the friend appears in the FRIENDS section and their
online status updates in real time.

### Joining a Ticket

A ticket is an out-of-band invitation that encodes a room's identity (topic + optional
enrolment secret + bootstrap relay address). To join:

1. Click "Join Ticket" on the landing screen or from the "＋" sidebar menu.
2. Paste the ticket string into the input field.
3. Click "Join" (or press Enter).

The app will dial the bootstrap relay, resolve the room topic, and subscribe to the
room. The room then appears in the CHATS sidebar section.

Tickets bypass DHT discovery entirely — they are the primary way to invite someone into
a private room or a public room that hasn't been discovered yet.

### Advanced Networking Details

The Settings screen (⚙ in the sidebar header) shows detailed networking information:

- **Peer ID** — Your full Ed25519 public key (52-character hex), with a "Copy" button
- **Connection counts** — Direct connections, relayed connections, and gossip neighbours
- **Mesh health** — "Good", "Degraded (reason)", or "Offline (reason)"
- **Relay mode** — Current relay connectivity state
- **DHT discovery toggle** — Enable/disable Mainline DHT publishing and lookup
- **Direct address sharing** — Publish direct IP addresses to DHT (off by default)

For even deeper diagnostics, the Connection Details dialog (accessible via the settings
or with `--show-connection-details`) shows your node ID, relay URLs, direct addresses,
and full peer list.

See [`docs/networking-audit.md`](docs/networking-audit.md) and
[`docs/discovery-architecture.md`](docs/discovery-architecture.md) for the complete
networking architecture and protocol details.

### Peer Names

Every peer is identified by a 32-byte Ed25519 public key. Because raw hex keys are
unfriendly, Boru generates deterministic human-readable names:

- **Generated friendly names** — Each public key maps to an "Adjective Noun" combination
  (e.g. "Blue Falcon", "Quiet Harbour", "Crimson Fox"). The mapping is deterministic:
  the same peer always gets the same name across restarts and across machines. The
  algorithm uses a simple hash of the key bytes to select from curated word lists
  (110+ adjectives, 140+ nouns — all non-offensive, non-alarming terms).

- **Priority order** — The display name shown in the UI follows this chain:
  1. User-assigned nickname (friend label)
  2. Remote profile display name (from profile-update gossip)
  3. Last announced name (from friend-record metadata)
  4. Session/device name
  5. Generated friendly name ("Blue Falcon")
  6. Truncated peer ID ("dfab…961f") — used only as secondary identifying text

- **Truncated IDs** — Full public keys are shortened to `"dfab…961f"` format (first 4
  hex chars, ellipsis, last 4 hex chars) for compact display.

### Renaming a Friend

Open a friend's profile by clicking the "…" button next to their name in the FRIENDS
sidebar section. On the friend profile page:

1. Click the "⋮" (vertical three-dot) menu button in the header.
2. Select "Rename" from the dropdown.
3. Type the new name in the inline text input.
4. Press Enter or click the ✓ button to confirm.
5. Click ✕ to cancel.

The nickname is stored locally and takes precedence over all other name sources in the
priority chain. The friend never sees your chosen nickname.

### Drag-and-Drop

The current Iced v0.14 framework does not support native OS-level drag-and-drop of
files from the desktop onto application windows. There is no file-drop area on the
landing screen or anywhere in the GUI. Files must be shared via the explicit "Browse
Files" / "Add File" buttons or the `/send` command in the chat composer.

### Iced Framework Limitations

The Iced GUI toolkit (v0.14) has several inherent limitations that affect the UI:

- **No native drag-and-drop** — Iced does not support operating-system drag-and-drop
  events. File sharing requires explicit button clicks or slash commands.
- **No context menus** — All interactions use explicit on-screen buttons. Right-click
  context menus are not supported. The "⋮" (three-dot) menu on friend profiles is a
  manually positioned overlay, not a native context menu.
- **No rich text rendering** — All text is rendered in a single size/colour per widget.
  There is no inline markup (bold, italic, links) within message bodies.
- **No system tray** — The application does not minimise to a system tray icon.
- **No tab navigation out of the box** — Keyboard navigation (Tab/Shift+Tab) works
  but must be wired manually for each interactive element.
- **No vector icons** — All icons are rendered as Unicode/emoji characters (●, ○, ⚙,
  ＋, ⋮) rather than as vector graphics. Rendering quality depends on the system font.
- **Fixed sidebar width** — The sidebar is a fixed 280px. There is no user-resizable
  splitter.
- **No overlay composition** — Iced stacks elements using `stack![]`, which means
  overlays (dialogs, menus) cover the entire app and cannot be limited to a panel area.

### Remaining Planned UI Work

Several features are planned but not yet implemented:

- **Context menus** — Right-click menus on chat items, friend rows, and file entries
- **Dedicated unread badge pills** — Replace inline `" [N]"` text with styled pill
  containers (`accent_primary` background, white text, rounded rectangle)
- **Toast notifications** — Transient slide-in notifications for friend request
  received, message delivered, download complete
- **Search/filter in sidebar** — Text input to filter CHATS and FRIENDS sections
  for users with many conversations
- **Onboarding overlay** — First-launch 3-step tutorial explaining the P2P model,
  how to add friends, and where to find keys
- **Room-level settings** — Per-room mute, ticket sharing, leave room, export history
  (currently the chat header "Settings" button opens global settings)
- **Proper status dot widgets** — Replace Unicode ●/○ characters with rendered
  vector circles (8×8px, coloured) for platform-consistent appearance
- **"Voice" button** — The "Voice" button on the friend profile page currently has
  no action handler; it needs either voice-call implementation or a disabled state
  with a "coming soon" explanation
- **Export Friend** — Export a friend's contact info to a file (matching the existing
  "Import Friend" functionality)
- **Delivery status indicators** — Surface the existing `delivery_state` data in the
  chat log with ✓/✓✓/clock icons

See [`UX_AUDIT.md`](UX_AUDIT.md) for a full UX assessment and
[`docs/gui-architecture.md`](docs/gui-architecture.md) for the GUI component architecture.

## Features

| Feature | Description |
|---|---|
| `net` | Networking stack (gossip, inbox, backfill, whisper, discovery) — enabled by default |
| `gui` | Iced GUI example with image optimization |
| `sim` | Deterministic simulation test framework |
