# Onboarding and Pairing Design

This document describes the first-run landing screen, invitation pairing paths,
privacy constraints, and recovery behaviour for Boru's desktop GUI.

## Goals

- Give a first-time user an obvious starting point.
- Make invitation sharing safe to copy, paste, and QR-encode without leaking
  transport details.
- Keep the advanced public-key path available for technical users.
- Ensure pairing errors always offer a clear recovery action.
- Preserve restart behaviour so the app does not re-onboard an existing profile.

## First run

Boru treats a profile as first-run when the local room history and friend store
are both empty. On that path the GUI opens on the landing screen instead of a
blank placeholder.

The landing screen should communicate three things immediately:

1. This is a peer-to-peer app with no central server.
2. The user can start with a room invitation or with a direct friend key.
3. The current connection state and relay mode are visible, but not dominant.

The landing screen uses the existing button system and surface-card styling:

- branding heading
- short privacy tagline
- status card
- quick action buttons
- a short recent-activity feed when available

Once the user completes any first action, the profile is marked as no longer
first-run and the app returns to the normal chat list experience on future
launches.

## Pairing flows

### 1. Invitation-based room join

Recommended for most users.

Flow:

1. One peer generates a room invitation.
2. The invitation is shared as text or QR.
3. The receiving peer pastes or scans the invitation.
4. The app validates the invitation and joins the room.

Use this path when you want the shortest copy/paste journey and do not need to
exchange direct friend keys first.

### 2. Legacy ticket join

Used for backwards compatibility with older room tickets that include bootstrap
peers.

Flow:

1. One peer shares a legacy ticket string.
2. The receiving peer chooses `Join Ticket`.
3. The app parses the ticket, joins the room, and seeds transport hints if the
   ticket contains them.

### 3. Advanced public-key pairing

Used when the other peer's public key is already known.

Flow:

1. The user chooses `Add Friend`.
2. The user pastes the public key.
3. A friend request is created and tracked explicitly.
4. Accepting the friend request does not auto-open a chat; the user still picks
   the next conversation action.

This is the advanced path, not the default onboarding path.

## Invitation privacy

Stable `boru1:` invitations are intentionally compact:

- room topic
- discovery secret
- no endpoint information
- no relay URLs
- no creator identity

That makes them safe to paste into chat, render as QR, or keep in a notes app
without exposing transport details.

The privacy rule is simple: if a field is not required to join the room, do not
put it in the invitation payload.

## Recoverable errors and recovery actions

Invitation and QR parsing should fail loudly but helpfully. The UI should keep
recoverable cases on-screen and offer one of the following actions depending on
context:

- Retry
- Choose another image
- Paste instead
- Generate new invitation
- Open advanced setup
- Continue offline

The recovery action should match the failure:

- malformed QR image or no QR found -> choose another image / paste instead
- invalid or expired invitation -> generate new invitation / paste instead
- unsupported invitation version -> generate a new invitation or switch to a
  compatible peer
- self-invitation or duplicate contact -> open the existing peer/contact
- peer unreachable -> retry or continue offline
- save failure -> retry after fixing permissions or storage availability

Do not hide the reason. Show the user enough detail to choose the next step,
but do not expose secret invitation contents in the error text.

## Persistence and restart behaviour

Boru should preserve pairing state across restarts:

- a completed first-run state must stay completed
- accepted friends remain accepted
- pending requests remain pending until resolved
- conversation history and room membership should survive a restart

If the app opens again with an existing profile, it should skip the landing flow
and show the normal chat list.

## Validation and safety notes

- Reject invitations that are too short before decoding.
- Keep the invitation parser strict about version and payload size.
- Never log secret invitation material or raw QR payloads.
- Do not allocate based on untrusted invitation lengths without checking bounds
  first.
- If a QR image includes extra sensitive information, redact it from logs and
  diagnostic output.

## Related test coverage

The current repository coverage for these flows is spread across integration
and parser tests, including:

- `tests/test_room_invite_v2.rs`
- `tests/test_private_room_invitation_discovery.rs`
- `tests/test_friend_request_e2e.rs`
- `tests/test_iced_chat_flow.rs`
- `tests/test_deterministic_harness.rs`
- `tests/test_peer_lifecycle.rs`

Use this document as the design reference whenever the landing screen, invite
formats, or pairing recovery actions change.
