# Onboarding & Pairing Design

> **Author:** Architecture Analysis (Phase 1)  
> **Date:** 2026-07-22  
> **Scope:** `examples/iced_chat/` GUI and `boru_chat` library  
> **Status:** Draft — based on codebase inspection

---

## Table of Contents

1. [Current Friend-Add Flow](#1-current-friend-add-flow)
2. [Current First-Launch Experience](#2-current-first-launch-experience)
3. [Proposed Onboarding State Machine](#3-proposed-onboarding-state-machine)
4. [Invitation Payload Format](#4-invitation-payload-format)
5. [QR Code Pairing](#5-qr-code-pairing)
6. [Backward Compatibility](#6-backward-compatibility)
7. [Security and Privacy Considerations](#7-security-and-privacy-considerations)
8. [Reused Components](#8-reused-components)
9. [Implementation Plan (Phase 2)](#9-implementation-plan-phase-2)

---

## 1. Current Friend-Add Flow

### 1.1 User adds a friend by public key

```
User clicks "Add Friend" (empty state button or sidebar "+" menu)
  └─> Screen switches to FriendRequests
        └─> Input field labelled "Peer public key…"
              └─> User types 52-char hex public key
                    └─> "Send Request" button
                          └─> FriendRequestStore::send_request()
                                └─> SignedContactMessage::sign() with ContactAction::FriendRequest
                                      └─> Sent over whisper protocol to the target peer
                                            └─> Recipient receives via WhisperEvent
                                                  └─> Stored in friend_requests.json
```

**What works well:**
- Signed contact messages (`SignedContactMessage`) provide authentication, replay protection (24h clock skew window), and transport independence.
- `FriendRequestStore` enforces a strict state machine (Pending → Accepted/Declined/Cancelled) with validation rules (no self-requests, duplicate detection, authorization checks).
- The request lifecycle is separated from the `FriendsStore` (v4 migration removed pending relationship variants from friends.json).

**What doesn't work well:**
- The input field requires a raw 52-character hex public key — a hard ask for any user.
- No QR code scanning (the menu item exists but is `disabled: true` with no action handler).
- No real-time key format validation or visual feedback.
- No success toast after sending; user must navigate to "Outgoing Requests" section.
- No way to export a friend request as a shareable payload.
- No invitation/pairing mechanism that works without knowing the other peer's key in advance.

### 1.2 Friend request lifecycle

```
┌──────────┐
│  Pending  │
└────┬─────┘
  ┌───┼─────────┐
  ▼   ▼         ▼
┌────────┐ ┌───────┐ ┌─────────┐
│Accepted│ │Declined│ │Cancelled│
└────────┘ └───────┘ └─────────┘
```

- **Pending** — requester has sent; recipient hasn't responded.
- **Accepted** — both sides are now friends; `FriendRelationship::Friends` set in friends.json. A direct conversation topic is derived via `direct_topic(a, b)` (deterministic BLAKE3).
- **Declined** — terminal; recipient rejected.
- **Cancelled** — terminal; requester withdrew before response.

After acceptance, a `ConversationInvite` action is sent (containing the derived `TopicId` and bootstrap addresses) to establish the gossip mesh for the direct conversation.

### 1.3 Current friend-add screen

The `view_friend_requests()` screen shows:
- **Incoming requests** section — list of pending requests with Accept/Decline buttons.
- **Outgoing requests** section — list of sent requests with Pending/Accepted/Declined/Failed status labels.
- **Add by key** input — text field for entering a 52-char hex public key + Send Request button.
- **Error display** — red text below the input on validation/network errors.

---

## 2. Current First-Launch Experience

### 2.1 What happens on first launch

1. Secret key is generated silently (`SecretKey::generate()`) and written to `secret_key.txt` (0o600 permissions on Unix).
2. Data directory is created (0o700).
3. All store files are loaded — they don't exist yet, so empty stores are created.
4. `first_run` is set to `room_history.is_empty() && friends.is_empty() == true`.
5. The empty state screen is shown:
   - "BORU CHAT" heading + "Private. Peer-to-peer. No central servers." tagline.
   - Status card: Online status, mesh health, relay mode, friend count.
   - Four action buttons: **Start Chat**, **Add Friend**, **Join Ticket**, **Browse Files**.
   - Recent Activity feed (empty).
6. `first_run` is set to `false` on the first user action.

### 2.2 UX problems identified

| Problem | Severity | Evidence |
|---------|----------|----------|
| No guidance on what to do first | High | UX_AUDIT.md: "No onboarding flow" |
| "Start Chat" creates a random room | Medium | User has no context for rooms |
| "Add Friend" expects hex public key | High | 52-char hex string is a hard barrier |
| QR code scanning is disabled | Medium | UX_AUDIT.md: dead placeholder menu item |
| "Join Ticket" has no explanation | Medium | "Enter ticket ID" — what's a ticket? |
| Networking jargon on empty state | Low | "Mesh: healthy", "Relay: custom" displayed |
| No way to discover peers without mDNS/DHT | Medium | No QR-based pairing mechanism |

---

## 3. Proposed Onboarding State Machine

### 3.1 Design goals

1. **30-second to first message**: A new user should be able to exchange a message with someone within 30 seconds.
2. **No prior knowledge required**: The user should not need to understand public keys, DHT, relays, or gossip.
3. **Paired devices**: Support device-to-device pairing via QR codes (showing and scanning).
4. **Progressive disclosure**: Hide networking jargon; show it only in Settings/Advanced.
5. **Backward compatible**: Existing friend-add-by-key flow and ticket joining must continue working.

### 3.2 State machine

```
                  ┌──────────────┐
                  │  First Launch │
                  │  (first_run)  │
                  └──────┬───────┘
                         │
                         ▼
              ┌─────────────────────┐
              │  Welcome Overlay    │
              │  (one-time, 3-step) │
              │  1. What this app is│
              │  2. Show your code  │
              │  3. Scan a code     │
              └─────────┬───────────┘
                        │ "Got it"
                        ▼
              ┌─────────────────────┐
         ┌───▶│  Enhanced Empty     │───────── Optional: re-show via
         │    │  State              │          Settings > "Show Welcome"
         │    └──┬──────┬──────┬────┘
         │       │      │      │
         │       │      │      ▼
         │       │      │  [Show My Code]
         │       │      │  └─ QR code with invitation
         │       │      │     or text-based share
         │       │      ▼
         │       │  [Scan Code]
         │       │  └─ Camera/File import
         │       │     to decode invitation
         │       ▼
         │  [Friend Requests]
         │  └─ Existing flow (by-key)
         │     + new "Scan QR" from here too
         │
         └── User dismisses / takes action
                ──▶ onboarding_complete = true
```

### 3.3 Welcome overlay content

**Step 1 — "Welcome to Boru Chat"**
> "This app connects you directly to other people — no central servers involved. Messages are private and peer-to-peer."

**Step 2 — "Your Code"**
> "This is your unique friend code. Share it with someone to connect."
> [Large QR code] [Copy text button]
> "They can scan it from their Boru Chat app."

**Step 3 — "Connect with Someone"**
> "To connect with someone, scan their code or enter their friend key."
> [Scan QR Code button] [Enter key... button]
> "Once connected, you can chat, share files, and more."

**Dismiss:** "Got it" button at the bottom. Dismissible via Esc key.

### 3.4 Enhanced empty state changes

| Element | Current | Proposed |
|---------|---------|----------|
| Status card | Mesh health, relay, online, friends | Online status, friend count only |
| Action buttons | Start Chat, Add Friend, Join Ticket, Browse Files | **Show My Code**, **Scan Code**, Add Friend, Join Ticket, Start Chat |
| Suggested actions | None | "New here? Show your code to a friend, or scan theirs." |
| Recent activity | Show activity feed | Unchanged |
| Mesh/Relay | Visible | Moved to Settings > Network section |

### 3.5 Pairing flow (new)

#### A. Show my code (outgoing pairing)

```
User taps "Show My Code"
  └─> Modal displays:
        └─> Large QR code encoding the user's invitation
        └─> Text field with the encoded invitation string (copyable)
        └─> "Copy" button (copies invitation string to clipboard)
        └─> "Save as image" button (optional)
        └─> "Share" button (platform share sheet, where available)
        └─> "Close" button
```

#### B. Scan a code (incoming pairing)

```
User taps "Scan Code"
  └─> Opens camera viewfinder (or file import fallback)
        └─> Decodes QR → extracts invitation string
        └─> Parses RoomInvitation
              ├─ If it's a friend invitation → auto-send friend request
              │   (with optional confirmation dialog)
              └─ If it's a room ticket → auto-join room
                    └─> Join confirmation dialog
```

#### C. Friend invitation from QR code

The QR code encodes a new invitation type that includes:
- The peer's public key (identity)
- An optional display name
- A timestamp (for replay protection)

This is distinct from room tickets — it's a **peer introduction** payload, not a room invitation. The flow:

```
1. Alice shows her QR code → Bob scans it
2. Bob's app decodes: FriendInvitation { peer_id: Alice_pk, name: "Alice", timestamp }
3. Bob's app sends a friend request to Alice (ContactAction::FriendRequest)
4. Alice receives the request from Bob
5. Alice accepts → friendship established, direct conversation starts
```

---

## 4. Invitation Payload Format

### 4.1 Existing invitation types

| Type | Prefix | Payload | Use case |
|------|--------|---------|----------|
| Legacy Ticket | `bis1` | TopicId + peers + relay + optional DiscoverySecret | Room joining (old format) |
| RoomInviteV2 | `boru1:` | version(1) + TopicId(32) + DiscoverySecret(32) = 65 bytes → ~105 chars base32 | Room joining (current) |

### 4.2 Proposed FriendInvitation format

A new payload type for peer introductions:

```
PREFIX: "boru1f:"  (boru 1 friend)
PAYLOAD: version(1) + public_key(32) + display_name_len(1) + display_name(variable) + timestamp(8)
```

- **version**: 1 byte (currently `1`)
- **public_key**: 32 bytes (Ed25519 public key, the `PublicKey` from iroh)
- **display_name_len**: 1 byte (0-64, max display name length)
- **display_name**: UTF-8 bytes (variable, up to 64 bytes)
- **timestamp**: 8 bytes (unix milliseconds, LE u64, for replay window)

Total: 42 + variable bytes (~42-106 bytes). Encoded in base32-nopad-lowercase:
~67-170 chars + `boru1f:` prefix.

The timestamp enables a 5-minute replay window (matching the existing `MAX_CONTROL_CLOCK_SKEW_SECS` pattern).

### 4.3 RoomInviteV3 (future refinement)

RoomInviteV2 already exists and works well. For the onboarding feature, no changes are needed to room invitations unless we want to embed a sender identity. If needed, a V3 could add:

```
PREFIX: "boru1r:"  (boru 1 room — distinct from friend)
PAYLOAD: version(1) + topic(32) + discovery_secret(32) + sender_pk(32) + sender_name_len(1) + sender_name(variable)

Total: 98+ bytes → ~157+ chars base32 + prefix
```

This would allow the QR to show who created the room, enabling the recipient to pre-populate a display name. **Not required for Phase 1.**

### 4.4 Encoding summary

```
FriendInvitation:
  boru1f:<base32-nopad-lowercase>
  Payload: [version:1][public_key:32][name_len:1][name:0..64][timestamp:8]

RoomInviteV2 (existing):
  boru1:<base32-nopad-lowercase>
  Payload: [version:1][topic:32][discovery_secret:32]

Future RoomInviteV3 (optional):
  boru1r:<base32-nopad-lowercase>
  Payload: [version:1][topic:32][secret:32][sender_pk:32][name_len:1][name:0..64]
```

### 4.5 QR code data model

```rust
/// An invitation that can be encoded in a QR code.
pub enum QrInvitation {
    /// Friend introduction — encodes the local peer's identity for pairing.
    Friend(Box<FriendInvitation>),
    /// Room invitation — reuses the existing RoomInviteV2 format.
    Room(RoomInviteV2),
}
```

The QR code data is simply the encoded invitation string. No binary QR content — the QR encodes the text string `boru1f:...` or `boru1:...`. This keeps QR decoding simple (text extraction → string parsing).

---

## 5. QR Code Pairing

### 5.1 Dependencies required

| Dependency | Purpose | Notes |
|-----------|---------|-------|
| **QR generation** | Generate QR codes for display | Candidates: `qrcode` crate (pure Rust), `rqrr` for decoding. The `qrcode` crate has 0 dependencies and generates QR codes as in-memory bitmaps. |
| **QR decoding / camera** | Scan QR codes from camera or file | Options: (a) Use `rqrr` for decoding from image files; (b) Use a system camera library (harder on Linux); (c) Accept image file import as fallback. |

**Recommendation for Phase 2:**
- **Generation**: `qrcode` crate (Apache 2.0 / MIT, pure Rust, no native deps). Render as an Iced `Canvas` widget from the bitmap data.
- **Decoding**: Start with image-file import only (`rqrr` or `quirc` crate). Camera integration can be deferred to a later phase — the file-import path already exists ("Import Friend" menu item) and can be extended to decode QR images.

### 5.2 QR generation in Iced

The `qrcode` crate produces a `QrCode` struct from which we get a 2D matrix of `bool` (dark/light modules). This can be rendered as an Iced `Canvas` widget:

```rust
// Conceptual approach
let code = QrCode::new(invitation_string)?;
let canvas = move |frame: &mut Frame, _bounds: Size| {
    let module_count = code.len();
    let module_size = bounds.width.min(bounds.height) / module_count as f32;
    for y in 0..module_count {
        for x in 0..module_count {
            if code[(x, y)] {
                frame.fill_rectangle(
                    Point::new(x as f32 * module_size, y as f32 * module_size),
                    Size::new(module_size, module_size),
                    Color::BLACK,
                );
            }
        }
    }
};
```

### 5.3 UI integration points

| UI location | Change | Priority |
|-------------|--------|----------|
| Welcome overlay (Step 2) | Show QR code + "Copy" button | High |
| Empty state | Replace two action buttons: "Show My Code", "Scan Code" | High |
| "+" menu | Enable "Scan QR Code" item (was disabled) | High |
| Friend request screen | Add "Scan QR" button next to "Peer public key" input | Medium |
| Settings > Identity section | Show QR code for friend key | Low |
| Friend profile | "Share my code" button | Low |

---

## 6. Backward Compatibility

### 6.1 What must keep working

| Feature | Risk if broken | Mitigation |
|---------|---------------|------------|
| Add friend by public key | Existing users' primary flow | Unchanged — text input remains |
| Legacy Ticket (`bis1`) joining | Existing room invites | `RoomInvitation::parse()` already handles both formats |
| RoomInviteV2 (`boru1:`) | Existing room invites | Unchanged |
| Old `friends.json` v3→v4 | On-disk friend data | Migration already in place |
| Friend request state machine | Existing pending requests | The `FriendRequestStore` API is unchanged |
| Direct conversation negotiation | Post-acceptance flow | `ContactAction::ConversationInvite` unchanged |

### 6.2 New invitation format backward compat

The `boru1f:` prefix is distinct from `boru1:` (room invites) and `bis1` (legacy tickets). The existing `RoomInvitation::parse()` already uses prefix detection:

```rust
// Already implemented in RoomInvitation::parse()
if trimmed.starts_with(RoomInviteV2::PREFIX) { // "boru1:"
    return Ok(Self::Stable(RoomInviteV2::parse(trimmed)?));
}
// Falls through to legacy Ticket parse
```

A new `FriendInvitation` type would go through a separate parse path:

```rust
pub enum ParsedInvitation {
    Friend(FriendInvitation),
    Room(RoomInvitation),
}

pub fn parse_invitation(input: &str) -> Result<ParsedInvitation> {
    let trimmed = input.trim();
    if trimmed.starts_with("boru1f:") {
        return Ok(ParsedInvitation::Friend(FriendInvitation::parse(trimmed)?));
    }
    Ok(ParsedInvitation::Room(RoomInvitation::parse(trimmed)?))
}
```

Old clients that receive a `boru1f:` string in a "Join Ticket" field will see a parse error (no matching prefix). This is acceptable — friend invitations are not supposed to be pasted into the room-join field.

### 6.3 Wire-level compatibility

No existing wire protocols are modified:
- **Whisper protocol** — `ContactAction::FriendRequest` already carries an optional `name` field. No change needed for the friend request transport.
- **Friend invitation to friend request** — Scanning a QR code triggers the same `ContactAction::FriendRequest` whisper message that the text-input path uses. The transport and storage are identical.
- **Room invitations** — QR codes for room joining encode the same `RoomInviteV2` string format. The join path is identical.

---

## 7. Security and Privacy Considerations

### 7.1 Threat model

| Threat | Severity | Mitigation |
|--------|----------|------------|
| QR code displayed publicly allows anyone to send friend requests | Low-Medium | The QR encodes only the public key — no secret material. Anyone can already discover your public key via DHT or mDNS. Friend requests require user acceptance to become friends. |
| QR code screenshot/replay | Low | FriendInvitation includes a timestamp. The invitation is valid for the replay window (matching existing `MAX_CONTROL_CLOCK_SKEW_SECS`). After that, a scanned code triggers a normal friend request; no special privilege. |
| Malicious QR code directs to wrong peer | Medium | The parsed `PublicKey` is verified during request via `SignedContactMessage` — the whisper transport confirms the sender's identity. A spoofed QR pointing to a different peer would result in a friend request being sent to that peer (who would receive it as a normal incoming request). The user accepting is then friends with the wrong person — comparable to dialing the wrong number. |
| QR code in screenshot = permanent invitation | Low | The invitation is just a public key + name + timestamp. It doesn't grant any persistent access. It merely triggers a friend request, which the recipient must accept. |

### 7.2 Privacy properties preserved

- **No additional PII exposed**: The QR encodes only what's already public: the `PublicKey` (visible in Settings, discoverable via DHT) and an optional display name (already announced in ProfileUpdate broadcasts).
- **No location data**: The QR does not encode IP addresses, relay URLs, or network coordinates.
- **No secret key exposure**: Only the public half of the identity key pair is encoded.
- **Auditable**: All friend request activity continues to be recorded in `friend_requests.json` and signed contact messages.

### 7.3 UI-level safety

- **Scan confirmation**: Before sending a friend request from a scanned QR, show a confirmation dialog with the peer's display name and a truncated public key (e.g., `alice...3f8a`). User must explicitly confirm.
- **Rate limiting**: Limit friend request sends to 10 per minute (matching `FriendRequestStore`'s existing duplicate-pending protection).
- **Camera permission**: Standard OS permission prompt for camera access (if camera scanning is added).
- **No auto-accept**: Scanning someone else's code only sends a *request* — it does not auto-accept. Friendship requires both sides.

---

## 8. Reused Components

### 8.1 Existing components that need no changes

| Component | File | Why it works |
|-----------|------|-------------|
| `FriendRequestStore` | `src/friend_request.rs` | State machine, persistence, validation — all unchanged. QR scanning just calls `send_request()`. |
| `SignedContactMessage` | `src/contact.rs` | Signed control message envelope — unchanged. QR-triggered requests use the same `ContactAction::FriendRequest`. |
| `ContactAction::FriendRequest` | `src/contact.rs` | Already has an optional `name` field — sufficient for FriendInvitation display name. |
| `RoomInviteV2` | `src/chat_core.rs` | Existing format for room QR codes. No changes needed. |
| `RoomInvitation::parse()` | `src/chat_core.rs` | Already supports prefix-based routing. New prefix slots in naturally. |
| `FriendsStore` | `src/friends.rs` | Friend record persistence — unchanged. |
| `UserProfile` | `src/user_profile.rs` | Display name, avatar — read from here for the QR code content. |
| `direct_topic()` | `src/contact.rs` | Deterministic direct conversation derivation — unchanged. |
| `WhisperHandle` | `src/whisper/` | Transport for friend requests — unchanged. |
| `main.rs` identity setup | `examples/iced_chat/main.rs` | Secret key generation, `local_public` — unchanged. |

### 8.2 Existing components that need minor changes

| Component | Change needed | Effort |
|-----------|---------------|--------|
| Sidebar "+" menu | Enable "Scan QR Code" item, wire to scan action | Small |
| `view_main_empty_state()` | Add "Show My Code" and "Scan Code" buttons, hide mesh/relay | Medium |
| `view_friend_requests()` | Add "Scan QR" button, improve key input UX | Small |
| `AppMessage` enum | Add `ShowMyCode`, `ScanQRCode`, `QrCodeScanned(String)` messages | Small |
| `IcedChat` state | Add `show_my_code_modal: bool`, `scan_qr_active: bool` fields | Small |
| Empty state status card | Remove mesh/relay lines, keep online status and friend count | Small |

### 8.3 New components needed

| Component | Description | File location | Effort |
|-----------|-------------|---------------|--------|
| `FriendInvitation` struct | New payload type for peer introductions | `src/contact.rs` or new `src/invitation.rs` | Small |
| `QrCodeDisplay` widget | Iced Canvas widget rendering a QR code bitmap | `examples/iced_chat/qr_widget.rs` | Medium |
| `QrCodeScanner` | QR decoding from image file (and optionally camera) | `examples/iced_chat/qr_scanner.rs` | Medium |
| Welcome overlay view | 3-step one-time onboarding overlay | `examples/iced_chat/app.rs` | Medium |
| `show_my_code_view()` | Modal with QR code + copy button | `examples/iced_chat/app.rs` | Small |
| `parse_invitation()` | Top-level parser routing `boru1f:` and `boru1:` | `src/chat_core.rs` | Small |

### 8.4 Dependencies to add

| Crate | Version | Purpose | License | Notes |
|-------|---------|---------|---------|-------|
| `qrcode` | 0.14 | QR code generation (pure Rust) | MIT/Apache 2.0 | 0 deps, well-maintained |
| `rqrr` | 0.4 | QR decoding from images | Apache 2.0 / MIT | Pure Rust, reads from bitmap |

Camera integration (`nokhwa` or `camino`) is deferred — use file import + `rqrr` for decoding in Phase 2.

---

## 9. Implementation Plan (Phase 2)

### Task breakdown

**Task 1: `FriendInvitation` payload type**

- Create `src/invitation.rs` (or extend `src/contact.rs`) with:
  - `FriendInvitation` struct: peer_id, display_name, timestamp
  - `encode()` / `decode()` methods with `boru1f:` prefix + base32-nopad
  - `parse_invitation()` that routes between `boru1f:`, `boru1:`, and legacy formats
  - Unit tests for round-trip and boundary cases

**Task 2: Add QR generation dependency and widget**

- Add `qrcode = "0.14"` to `Cargo.toml` (gui feature only)
- Create `examples/iced_chat/qr_widget.rs`:
  - `qr_code_view(invitation_string, size) -> Element` helper
  - Render QR module matrix as Iced `Canvas`
  - Light/dark themed colors

**Task 3: Wire "Show My Code" in empty state**

- Add `AppMessage::ShowMyCode`, `HideMyCode`
- Add `show_my_code_modal: bool` to `IcedChat` state
- Create `view_my_code()` modal with QR code + copy button + display name
- Replace empty state buttons (add "Show My Code" as first action)

**Task 4: Wire "Scan Code" for friend requests**

- Add `AppMessage::ScanQrCode` and `QrCodeScanned(String)`
- Implement file-import-based QR scanning (reusing existing file picker)
- On decode: parse invitation, show confirmation dialog, send friend request or join room
- Enable the disabled "Scan QR Code" in the "+" menu

**Task 5: Welcome overlay**

- Create 3-step welcome overlay (`view_welcome_overlay`)
- Show only on `first_run == true`
- Steps: Welcome → Your Code (QR) → Connect (scan/enter)
- "Got it" dismisses; sets `first_run = false`
- Re-showable from Settings > "Show Welcome"

**Task 6: Clean up empty state**

- Remove mesh health line from status card
- Remove relay mode line from status card
- Move both to Settings > Network section
- Keep online status and friend count only

---

## Appendix A: File Map

| Path | Relevance |
|------|-----------|
| `examples/iced_chat/app.rs` | Core GUI application (~18.6K lines). Screen enum, IcedChat fields, view functions, message handling, friend request UI |
| `examples/iced_chat/main.rs` | CLI entry point (~1.5K lines). Endpoint setup, identity loading, protocol handlers |
| `src/contact.rs` | `SignedContactMessage`, `ContactAction` enum, `direct_topic()` |
| `src/friend_request.rs` | `FriendRequestStore`, `FriendRequest`, state machine |
| `src/friends.rs` | `FriendsStore`, `FriendRecord`, `FriendRelationship` |
| `src/user_profile.rs` | `UserProfile`, `SharedFile`, display name, avatar |
| `src/chat_core.rs` | `RoomInvitation`, `RoomInviteV2`, `Ticket`, `Message` |
| `src/storage.rs` | SQLite `Storage` with migrations |
| `src/discovery_*.rs` | Discovery validation, records, secrets, backends |
| `docs/discovery-architecture.md` | Full discovery documentation |
| `docs/gui-architecture.md` | GUI architecture overview |
| `UX_AUDIT.md` | UX findings (onboarding gap documented) |
| `DESIGN_SYSTEM.md` | Visual tokens and component library |

## Appendix B: Key Types Reference

```rust
// Identity
SecretKey, PublicKey   // iroh types — 32-byte Ed25519 keypair
EndpointId             // = PublicKey (same 32-byte identity)

// Friend management
FriendId(String)                            // friends.rs — wraps PublicKey::to_string()
FriendRecord { label, known_addrs, ... }    // friends.rs — per-friend metadata
FriendRequest { requester, recipient, status }  // friend_request.rs
FriendRequestStatus::Pending | Accepted | Declined | Cancelled
FriendRelationship::NotFriend | Friends | Blocked

// Contact protocol (whisper-transported)
ContactAction::FriendRequest { name }
ContactAction::FriendRequestAccepted
ContactAction::FriendRequestRejected
ContactAction::ConversationInvite { topic, addrs }
SignedContactMessage { from, sent_at_unix_secs, data, signature }

// Invitations
RoomInviteV2 { topic, discovery_secret }    // "boru1:" prefix
Ticket { topic, peers, ... }                // "bis1" prefix (legacy)
RoomInvitation::Stable(RoomInviteV2) | Legacy(Ticket)

// Profiles
UserProfile { user_id, display_name, bio, avatar_identifier, ... }
SharedFileMeta { id, filename, size, mime_type, ... }
PeerProfileData { display_name, avatar_ticket, shared_files, ... }

// Screen
enum Screen {
    ChatList, Chat(TopicId), FriendRequests, Settings,
    PeerProfile(PublicKey), PeerCatalogue(PublicKey),
    ImagePreview { topic, entry_index },
    FriendProfile(PublicKey),
}

// First run detection
first_run = room_history.is_empty() && friends.is_empty()
```
