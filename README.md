# Boru

**A peer-to-peer chat application with no central server.**

Boru lets people communicate directly — group chat, direct messages, and file
sharing — over an encrypted peer-to-peer network. No company in the middle. No
server storing your data. Your conversations and files stay on your own devices.

## What it does

- **Group chat** — Join rooms and broadcast messages via a gossip protocol over QUIC.
- **Direct messaging** — Send private messages with encrypted inbox delivery for offline contacts.
- **File sharing** — Share files by content address with explicit permission grants.
- **Secure tunnels** — Expose a local TCP service to one trusted friend through an encrypted Iroh/QUIC tunnel.
- **Discovery** — Find peers via mDNS (LAN), Mainline DHT (WAN), tickets, or relay servers.

## Features

| Feature | Description |
|---|---|
| Gossip protocol | Room-based message broadcast over QUIC |
| Direct messaging | Inbox protocol for offline delivery + whisper for private 1:1 channels |
| Backfill | Late-joining peers request missed messages from existing peers |
| Friend management | Signed contact and friend-request negotiation |
| File sharing | Content-addressed attachments with signed, requester-filtered catalogues |
| Secure tunnels | Encrypted TCP forwarding, recipient-bound and revocable |
| Relational storage | SQLite persistence with managed forward-only migrations |
| Cross-platform GUI | Iced desktop app (Linux, macOS, Windows) |
| MCP integration | Model Context Protocol server for diagnostic tooling |

## Why "Boru"?

The project is named after **Brian Boru** (Brian Bóruma mac Cennétig, c.
941–1014), the High King of Ireland. He is remembered for bringing many Irish
kingdoms together under a shared cause. Boru applies that idea to communication:
people meet and exchange messages as peers, without relying on one central
authority to connect everyone.

## Running

```sh
# GUI
cargo run -- --name <nickname>

# With a custom data directory
BORU_DATA_DIR=~/.boru cargo run -- --name <nickname>

# All CLI options
cargo run -- --help
```

See [`docs/`](docs/) for architecture, storage, discovery, security model, and
networking details. The File Sharing dashboard is documented in
[`docs/file-sharing-guide.md`](docs/file-sharing-guide.md) (user guide) and
[`docs/fs-06-persistence-projections.md`](docs/fs-06-persistence-projections.md)
(architecture), with the release note and rollback guidance in
[`docs/fs-25-release-note.md`](docs/fs-25-release-note.md). External GIF search
(KLIPY provider, configuration, privacy, adding another provider) is documented
in [`docs/gif-search.md`](docs/gif-search.md).

## Third-party assets and licensing

Boru's own source is dual-licensed **MIT OR Apache-2.0** (see
[`LICENSE-MIT`](LICENSE-MIT) and [`LICENSE-APACHE`](LICENSE-APACHE)). The full
inventory of third-party components Boru builds on or bundles — including the
patched upstream crates, bundled fonts, Papirus icons, the GStreamer runtime
for Windows packaging, and the Twemoji emoji graphics — is recorded in
[`THIRD_PARTY_NOTICES.md`](THIRD_PARTY_NOTICES.md).

**Emoji artwork:** Boru renders emoji using the **Twemoji** asset set, which is
vendored (not downloaded at runtime) under `assets/emoji/twemoji/`. The
Twemoji graphics are licensed under **CC-BY 4.0** (the upstream code under
MIT); they are the work of the Twemoji project and are **not** owned by Boru.
See [`assets/emoji/twemoji/ATTRIBUTION.md`](assets/emoji/twemoji/ATTRIBUTION.md)
for the pinned upstream revision and the verbatim licence texts.
