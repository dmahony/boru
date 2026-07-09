# Browser-WASM Evaluation: browser-chat as a reference for iroh-gossip-chat

Evaluated: 2026-07-09
Reference: https://github.com/n0-computer/iroh-examples/tree/main/browser-chat

## 1. browser-chat architecture (the reference)

browser-chat is a **workspace** with 4 members:

| Crate | Purpose |
|-------|---------|
| `shared` | Core `ChatNode` + `ChatSender` + signed message types + presence heartbeat |
| `browser-wasm` | Thin `wasm-bindgen` wrapper — exposes `ChatNode`, `Channel`, `ChannelSender` to JS |
| `cli` | Simple stdin/stdout CLI using `shared` |
| `frontend/` | Vite + React 18 + TypeScript + Tailwind/shadcn web app |

**Key design decisions in browser-chat:**
- Ephemeral identity (random `SecretKey` per session, or `IROH_SECRET` env var)
- 5-second presence interval with `SignedMessage::sign_and_encode`
- `ChatTicket` using `iroh_tickets::Ticket` trait + postcard serialization
- Signed messages with `PublicKey::verify()` — first hop, always verified
- Simple `serde_wasm_bindgen` serialization of `Event` enum for JS bridge
- `wasm-bindgen` + `wasm-streams` for async event streaming to JS
- `vite-plugin-wasm` + `vite-plugin-top-level-await` for bundling

## 2. iroh-gossip-chat architecture (our project)

Our project is the **`iroh-gossip` library crate** itself + frontend examples:

| Module | Purpose | WASM compatible? |
|--------|---------|-----------------|
| `net.rs` | Gossip protocol networking (iroh Endpoint, Router, Gossip) | **Maybe** — uses tokio net/fs |
| `chat_core.rs` | Shared state machine, protocol types, event handling | **Mostly** — uses std-only types |
| `chat_callbacks.rs` | Frontend callback trait | **Yes** — pure types |
| `friends.rs` | Durable friends list (file I/O) | **No** — filesystem |
| `room.rs` | Room metadata (file I/O) | **No** — filesystem |
| `room_history.rs` | Recent room list (file I/O) | **No** — filesystem |
| `chat_history.rs` | Message history (file I/O) | **No** — filesystem |
| `small_room.rs` | Direct QUIC connections for small groups | **Maybe** — direct connections |
| `room_docs.rs` | iroh docs protocol for metadata/roster sync | **Maybe** — depends on iroh docs |
| `api.rs` | Gossip API types | **Yes** — pure types |
| `proto/` | Plumbtree/Hyparview gossip algorithms | **Yes** — `no_std` compatible |
| `backfill.rs` | Late-join history sync (custom ALPN) | **Maybe** — depends on QUIC |

**Frontends:**
- `examples/chat.rs` — TUI (ratatui + crossterm) → **NOT wasm**
- `examples/iced_chat/` — native GUI (iced 0.14) → **NOT wasm**
- No browser/JS frontend

**Feature flags:**
- `crate-type = ["cdylib", "rlib"]` — already set for wasm!
- `gui` feature gates iced/tokio
- `examples` feature gates ratatui/crossterm
- `net` feature gates iroh/tokio networking

## 3. Reusable pieces from browser-chat

### High-value reuse (directly adaptable)

1. **wasm-bindgen wrapper pattern** — `browser-wasm/src/lib.rs` is ~200 lines of clean wrapper code. Pattern: wrap Rust struct behind `#[wasm_bindgen]`, expose async methods via `wasm-bindgen-futures`, stream events via `wasm-streams`. Adaptable to wrap `chat_core`'s event model.

2. **Vite + wasm-pack build setup** — `vite.config.ts` with `vite-plugin-wasm` + `vite-plugin-top-level-await`, `package.json` scripts for `build:wasm`. Directly copyable.

3. **React component architecture** — `Homescreen` (create/join), `ChatView` (message list + input), `Sidebar` (channel list), `Meta` (peer list + neighbors + settings). Good template even if we use a different UI framework.

4. **API abstraction layer** — `frontend/src/lib/api.ts` defines a clean `API` interface with `createChannel`, `joinChannel`, `sendMessage`, `getMessages`, `getPeers`, subscription methods. This decouples UI from the wasm bridge.

### Medium-value reuse (needs adaptation)

5. **ChatTicket pattern** — browser-chat uses `iroh_tickets::Ticket` trait + postcard. Our project already does this differently (iroh-tickets-based tickets), but the serialization pattern validates.

6. **Presence heartbeat** — 5s interval with signed Presence messages. We have `FriendPingManager` (30s QUIC-based ping) — different approach. The gossip-based presence (broadcast to topic) is more suitable for browser.

7. **Event subscription via JS callbacks** — browser-chat's `subscribeToMessages`/`subscribeToPeers`/`subscribeToNeighbors` pattern. Needs adaptation to our `ChatCallbacks` trait.

8. **serde-wasm-bindgen Event serialization** — browser-chat serializes `Event` enum to JS objects. Works for simple types; our richer event model (images, friend events) would need extension.

### Low-value / not reusable

9. **CLI main.rs** — Our TUI frontend is far more sophisticated.
10. **shared crate's ChatNode** — Too simple; we need our full chat_core state machine.

## 4. Blockers for a browser port

### Hard blockers (require design changes)

| Blocker | Details |
|---------|---------|
| **Filesystem persistence** | `friends.rs`, `room.rs`, `room_history.rs`, `chat_history.rs` all use file I/O (`std::fs`, `tokio::fs`). WASM has no filesystem — needs IndexedDB backend via `wasm-bindgen` or a local-storage abstraction layer. |
| **Tokio `fs` and `net` features** | Our `net` feature pulls in `tokio` with `io-util`, `sync`, `rt`, `macros` (fine) but also `fs` (blocked on wasm). `small_room.rs` uses direct QUIC connections. |
| **iroh-blobs** | `examples/iced_chat/main.rs` imports `iroh_blobs::store::mem::MemStore` — this may or may not compile to wasm. |
| **Gossip net module complexity** | `net.rs` is 2000+ lines with tokio tasks, timers, join sets — a wasm build would need careful cfg-gating. |
| **rfd (file dialog)** | In `dev-dependencies` — `rfd = "0.15"` for file dialogs. Won't compile to wasm without `wasm` feature. |

### Soft blockers (solvable, effort)

| Blocker | Mitigation |
|---------|------------|
| **Persistent identity** | SecretKey currently stored at `~/.local/share/iroh-gossip-chat/secret_key.txt`. Browser needs localStorage/IndexedDB. Easy fix with `wasm-bindgen` storage. |
| **tokio::main** | All frontends use `#[tokio::main]`. WASM needs a different executor. Use `wasm-bindgen-futures` + `wasm-bindgen(start)`. |
| **ratatui/crossterm/iced** | Already feature-gated behind `examples` and `gui` features — won't be pulled into a wasm build. |
| **Image sharing** | The `pending_image` system in chat_core assumes local file access. Browser needs `<input type="file">` + drag-drop. |
| **Multiple frontend maintenance** | Adding a 4th frontend (browser) to a project that already maintains TUI + iced monolithic + iced modular. |

## 5. Effort estimate

| Phase | What | Estimated effort | Value |
|-------|------|------------------|-------|
| 0 | Add `wasm32-unknown-unknown` target check; cfg-gate fs-dependent modules in wasm build | 1-2 days | Enables build validation |
| 1 | Create `browser-wasm` crate with thin wrapper around `chat_core` join/send/recv | 3-5 days | MVP browser experience |
| 2 | Vite + React frontend (copy browser-chat pattern, adapt to our API) | 3-5 days | Usable web UI |
| 3 | Storage abstraction layer (IndexedDB for identity, friends, chat history) | 5-7 days | Feature parity |
| 4 | Image sharing in browser (`<input type="file">` + blob transfer) | 2-3 days | Image support |
| 5 | Multi-room with browser tabs (service worker for shared state) | 3-5 days | Full functionality |
| **Total** | | **16-25 days** | |

## 6. Recommendation

### VERDICT: CONDITIONAL GO — but only Phase 0 + Phase 1 right now

**Go with a minimal browser MVP first** — don't try to port the full feature set. Here's the reasoning:

**Why not "full no-go":**
1. The crate already has `crate-type = ["cdylib", "rlib"]` — wasm is clearly part of the design intent.
2. The `proto/` module is already `no_std` compatible — the core gossip algorithms don't need the OS.
3. browser-chat proves the wasm pipeline works end-to-end with iroh + iroh-gossip.
4. A browser frontend would be the strongest demo of the project — currently only TUI/GUI exist.
5. The `net` feature's tokio dependency is the same one browser-chat uses — if it works there, it should work here after proper cfg-gating.

**Why not "full go right now":**
1. 16-25 days of work is a major commitment for one optional frontend.
2. The filesystem abstractions (friends, chat_history, room) would need a significant refactor to add a pluggable storage backend.
3. The project already maintains 3 frontends — adding a 4th is a maintenance burden.
4. browser-chat itself already exists as a reference — users who want a browser-based gossip chat can use it directly.

**The right path:**

Phase 0 (immediate — 1-2 days):
- Add `wasm32-unknown-unknown` target build validation to CI
- Create a `browser` feature that pulls only wasm-compatible deps
- cfg-gate `std::fs` usage in `friends.rs`, `room.rs`, `room_history.rs`, `chat_history.rs` behind `cfg(not(target_arch = "wasm32"))`

Phase 1 (next — 3-5 days):
- Create `examples/browser-wasm/` — a new crate wrapping `chat_core`'s join/send/event-stream API
- Copy browser-chat's `browser-wasm/src/lib.rs` pattern, but bridge to `ChatCallbacks` instead of building a new ChatNode
- Create `examples/browser-frontend/` with Vite + React + Tailwind, using browser-chat's frontend as template
- Minimal feature set: create room, join room, send text messages, receive text messages

Phases 2-5 are future work; don't commit to them now.

### Implementation notes for Phase 1

Key things to get right:
1. **Don't duplicate ChatNode** — browser-chat's shared crate has a `ChatNode` that doesn't exist in our project. Instead, wrap our existing `chat_core` module: the wasm bridge calls `chat_core::handle_net_event` and `AppState` methods.
2. **Event streaming** — our `GossipReceiver` is a `tokio::sync::watch` or similar. Convert to `wasm-streams::ReadableStream` for JS consumption.
3. **SecretKey persistence** — on wasm, use `wasm-bindgen` to read/write localStorage.
4. **No filesystem** — Phase 1 makes no persistence guarantees. No friends, no history, no rooms list.
5. **Use `wasm-bindgen-futures`** for bridging async Rust -> JS promises.

## 7. Summary

```
Reusable pieces:  wasm-bindgen wrapper pattern, Vite+wasm-pack build,
                  React component architecture, API abstraction layer,
                  presence heartbeat pattern, ticket serialization

Hard blockers:    Filesystem persistence (friends/history/rooms),
                  tokio::fs dependency, iroh-blobs wasm compat,
                  rfd dev-dependency

Soft blockers:    Identity persistence, image sharing, 4th frontend burden

Effort:           16-25 days for full parity, 4-10 days for MVP

Recommendation:   CONDITIONAL GO — Phase 0 (cfg-gating, CI) + Phase 1
                  (minimal browser wasm wrapper + Vite/React frontend)
                  now; defer Phases 2-5 to future based on demand.
```
