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
cargo run --example boru --features gui -- --name <nickname>

# With a custom data directory
BORU_DATA_DIR=~/.boru cargo run --example boru --features gui -- --name <nickname>

# All CLI options
cargo run --example boru --features gui -- --help
```

See [`docs/`](docs/) for architecture, storage, discovery, security model, and networking details.
