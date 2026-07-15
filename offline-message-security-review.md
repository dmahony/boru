# Offline-Message Security & Privacy Review

**Date:** 2026-07-07  
**Reviewer:** Hermes Agent (profile: linux)  
**Scope:** iroh-gossip crate + chat/setup examples  
**Rendered from:** commit `2be492a` on branch `wt/t_a3ed8918`

## 1. Identity Model

### Storage
- The `SecretKey` is persisted as hex-encoded bytes (64 hex chars + newline) at:
  `$XDG_DATA_HOME/iroh-gossip-chat/secret_key.txt` (or `$IROH_GOSSIP_CHAT_DATA_DIR` override, or `$HOME/.local/share/...`, or `$LOCALAPPDATA/...`, or `./.iroh-gossip-chat/...`).
- File permissions: `0o600` on Unix.
- Directory permissions: `0o700` on Unix.

### Assessment
| Aspect | Status | Notes |
|--------|--------|-------|
| Key persistence | ✅ | Stable identity across restarts |
| File permissions | ✅ | 0o600 is correct for secret material |
| Directory permissions | ✅ | 0o700 prevents directory listing |
| Key encryption at rest | ⚠️ **Gap** | The key is stored as raw hex with no passphrase/KEK. A file-system-level attacker (malware, backup exfiltration) gains the full identity. This is acknowledged as an example-level tradeoff, not a production-grade store. |
| Fallback data dir | ⚠️ **Minor** | The `CWD/.iroh-gossip-chat` fallback could write to /tmp, /var/tmp, or shared directories. No effort is made to lock or warn about this. |
| `--secret-key` CLI override | ⚠️ | Passed via string on the command line — visible in `/proc/self/cmdline` to anyone with the same UID. Not fixable without reading from a file or fd instead. |

### Recommendation
For the example, document that `--secret-key` exposes the key to process-table sniffing. For a production variant, encrypt the key file with age or a password-derived key.

---

## 2. Storage Permissions & Data at Rest

### What gets persisted
- **Secret key** only. No message content, no peer metadata, no timestamps are written to disk.
- **Tor storage dirs** (when `--tor` is used): state and cache dirs under `$TMPDIR/iroh-gossip-chat-tor-{pid}-{random}/`, each set to `0o700`.

### Assessment
- No message persistence on disk — good. The `AppState.entries: Vec<ChatEntry>` lives only in memory and is dropped on quit.
- No log aggregation or telemetry files written by the chat example.
- ✅ The example produces no durable message artifacts by default.

### Gap: No retention-configurable message log
There is no option to persist chat history to disk. If a user expects their chat log to survive a restart, they will find nothing. This is a feature gap, not a security gap, but it means users may independently add ad-hoc persistence (e.g. shell redirects, terminal scrollback saving) that bypasses security considerations.

### Recommendation
Document the in-memory-only nature of chat history prominently. If persistence is added later, it should use the same defensive permissions (0o600/0o700) and default to a dedicated subdirectory, not the secret key's directory.

---

## 3. Message Signing & Authentication

### Mechanism (`examples/chat.rs`)
- `SignedMessage` envelope:
  - `from: PublicKey`
  - `data: Bytes` (postcard-encoded inner `Message`)
  - `signature: Signature` (Ed25519 via `secret_key.sign()`)
- Verification: `key.verify(&data, &signature)` before decoding.
- The signature covers the inner data; the outer envelope includes `from` as plaintext.

### Assessment
| Aspect | Status | Notes |
|--------|--------|-------|
| Source authentication | ✅ | Ed25519 signatures tied to a stable public key |
| Integrity | ✅ | Any tampering invalidates the signature |
| Anti-spoofing at gossip layer | ✅ | PlumTree rejects messages whose `id` does not match `blake3(content)` (see `validate()`) |
| Replay protection | ❌ **Gap** | No message nonce, timestamp, or sequence number. An attacker who captures a signed `Message::AboutMe { name: "admin" }` can replay it in a different context. |
| Forward secrecy | ❌ Not expected | Key compromise reveals all past signatures. This is inherent to Ed25519 and is not claimed. |
| Data authentication on re-joins | ✅ | When a peer comes back online, any signed message they receive can be verified, so they don't accept forged messages from the past. But they don't receive messages they missed while offline. |

### Recommendation
Add a message sequence counter or timestamp to the `Message` enum to provide replay resistance. Optional: include the `TopicId` in the signed payload to bind messages to their topic context.

---

## 4. Encryption Boundaries

### Current state
- **Transport encryption**: iroh's QUIC connections are secured with TLS 1.3 (via `tls-ring` feature). Traffic between iroh endpoints is encrypted on the wire.
- **Payload encryption**: There is **no end-to-end encryption** in the gossip protocol. The inner `Message` payload is plaintext once decrypted at the transport level. Every iroh peer that joins the topic sees every message in cleartext.
- **Tor transport**: When `--tor` is enabled, traffic flows through Tor hidden services. However, the warning states: "Tor-backed custom transport is operational. Gossip messages are relayed over Tor hidden services." — the inner payload is still cleartext at the gossip layer.

### Assessment
| Layer | Encrypted? | Notes |
|-------|-----------|-------|
| Wire (QUIC/TLS) | ✅ | TLS 1.3 between iroh endpoints |
| Wire (Tor) | ✅ | Tor onion routing |
| Gossip payload | ❌ **Gap** | Messages visible to every peer in the topic |
| End-to-end | ❌ **Gap** | No sender-specific encryption per recipient |

### Risk
Anyone who joins the gossip topic (by obtaining the topic ID and a bootstrap ticket) sees every message in cleartext. Tickets contain the topic ID and peer addresses — if a ticket is leaked, all future messages on that topic are readable by the ticket holder until the ticket is re-issued with a new topic.

### Recommendation
If confidentiality is required, add an E2E encryption layer on top of `SignedMessage`. Each message would be encrypted with a group key or per-recipient keys before being signed and broadcast. Without this, the system provides authenticity but no secrecy. This was likely an intentional tradeoff in the example design.

---

## 5. Retention Policy & Message Lifespan

### In-memory caches (PlumTree gossip layer)
| Cache | Duration | Purpose |
|-------|----------|---------|
| `cache: TimeBoundCache<MessageId, Gossip>` | 30s (`message_cache_retention`) | Payload cache for Graft responses |
| `received_messages: TimeBoundCache<MessageId, ()>` | 90s (`message_id_retention`) | Dedup — prevents re-emitting received messages |
| `missing_messages: HashMap<MessageId, VecDeque<(PI, Round)>>` | Until Graft timeout + graft_timeout_2 | Messages we've heard about but haven't received |
| `lazy_push_queue` | Dispatched every 5ms | Batches IHave announcements |

### Application layer
- `AppState.entries`: in-memory for session lifetime. Cleared on quit.
- No persistent message store.
- No configurable retention at the application level.

### Assessment
| Find | Severity | Notes |
|------|----------|-------|
| No persistent message store | ✅ Good | Enables privacy-by-design: no disk artifacts |
| 30s payload cache is short | ✅ | Limits window for late-coming peers to retrieve messages via Graft |
| 90s ID dedup cache | ✅ | Prevents message amplification attacks without storing payloads |
| No offline message queue | ✅ | A peer who was offline during a broadcast never receives it |
| No message expiry/metadata in API | ⚠️ Info | The `Message` enum has no `created_at` or `ttl` field, so the protocol has no basis for time-based expiry. The 30s cache is a protocol implementation detail, not a message-level policy. |

### Recommendation
Document that the chat example has no offline message delivery. If offline delivery is desired in the future, it needs a store-and-forward mechanism with explicit retention limits and peer authentication.

---

## 6. Offline Leakage Risks

### Scenario analysis

#### Scenario A: Peer is offline during broadcast
- Messages are **lost** — no store-and-forward mechanism.
- When the peer reconnects, it receives only messages broadcast after reconnection.
- No replay of missed messages. ✅

#### Scenario B: Peer goes offline while in the active view
- The HyParView protocol detects the disconnect and fills the slot from the passive view (up to 30 peers).
- No queuing of undelivered messages. ✅
- The disconnected peer's address (the `EndpointAddr` shared in the ticket) remains in peers' `PeerState::Pending` with a send queue. Once the disconnect is detected, the peer is removed from the active view, and the pending queue is drained/discarded. ✅

#### Scenario C: An attacker obtains a ticket
- The ticket contains `topic: TopicId` and `peers: Vec<EndpointAddr>` (which includes the peer's public key and any transport addresses).
- The attacker can join the topic and receive all current and future messages (until the topic ID changes or the legitimate peers leave and re-establish with a new topic).
- The attacker can also broadcast signed messages as themselves (but cannot impersonate others without their secret key). ⚠️
- If the attacker was present, they can see all messages in cleartext — no E2E encryption. ⚠️

#### Scenario D: Passive view information leakage
- Each peer maintains a passive view of up to 30 peers per topic.
- This is exchanged during `Shuffle` operations — an attacker who binds to a peer learns about other peers in the topic.
- The passive view contains only the peer identity (`PublicKey`), not their transport addresses (the `PeerData` is opaque and only shared when actively connecting). This limits the privacy exposure to identity disclosure, not address disclosure.

### Assessment
| Attack surface | Risk | Mitigation |
|---------------|------|------------|
| Ticket leakage → full topic access | **High** | No mitigation in current design. Topic ID is the sole admission token. |
| No E2E encryption → plaintext payloads | **Medium** | By design of the example. Transport-level encryption only. |
| Passive view enumeration | **Low** | Reveals only PublicKey, not addresses. But a passive observer can map the social graph of a topic. |
| Missing offline messages | **Low** (privacy win) | No backlog to leak. |
| Late-arriving peer gets 30s window | **Low** | Messages older than 30s are evicted from the cache. Only messages still propagating during the 30s window are retrievable. |

### Recommendation
1. **Ticket rotation**: A peer should periodically re-issue tickets with a new topic ID (e.g., per-session or time-windowed) so that a leaked ticket grants finite access.
2. **E2E encryption**: For any deployment where confidentiality matters, add a group encryption layer over the signed message envelope. The current `SignedMessage` structure already separates "signer" from "data" — the `data` field is a natural place to put an encrypted payload.
3. **Topic-join ACL**: If implemented, an access-control layer should rate-limit or authenticate join requests. Currently any peer with the topic ID + one bootstrap address can join.

---

## 7. Summary of Findings

### Critical (blocking)
None.

### High
1. **No E2E encryption** — All topic participants see every message in cleartext. Transport-level TLS/Tor does not close this gap.
2. **Ticket is a bearer token** — Anyone with a ticket can join the topic and read messages indefinitely. No revocation mechanism.

### Medium
3. **Secret key unencrypted at rest** — The hex-encoded key file on disk has no passphrase protection. Mitigated by 0o600 permissions.
4. **No replay protection** — A captured signed message can be replayed.
5. **`--secret-key` CLI exposure** — Secret key appears in `/proc/self/cmdline` when passed via CLI flag.

### Low
6. **Passive view leaks topic membership graph** — PublicKey enumeration is possible.
7. **CWD fallback for data dir** — Could write to a shared directory without warning.

### Already compliant (no action needed)
- ✅ Signature verification on all messages (Ed25519)
- ✅ In-memory-only message history (no disk artifacts)
- ✅ 30-second message cache eviction (limits retrieval window)
- ✅ No offline message queueing (no backlog to compromise)
- ✅ 0o600/0o700 permissions on key file and Tor storage dirs
- ✅ Public key is the stable identity, not printed in raw hex
- ✅ No telemetry, no phone-home, no analytics

---

## 8. Cross-Reference: Architecture Document Assumptions

The parent task (t_f8079269) produced an architecture document that was expected at:
`/home/dan/.hermes/kanban/boards/iroh-gossip-chat/workspaces/t_f8079269/offline-message-architecture.md`

This file was not found on disk (empty/absent). The review above was conducted by
reading the actual source code. If assumption validation against a specific
architecture document is still needed, the document should be re-created and
cross-checked against this review.
