# Advertisement authentication (BORU-DIR-03, PDF Phase 1 Task 1.3)

Status: implemented on `wt/t_ea7d5e83` (BORU-DIR-03).

This note answers the three design questions the PDF Task 1.3 asks, and
documents the signature scheme, the receive-path verification, and the
authority model. It complements `advertisement-metadata.md` (BORU-DIR-02).

## 1. How advertisements are authenticated

Every `PUBLIC_ROOM_ADVERTISEMENT` control-plane message now carries a
`signature` field on the `PublicRoomAdvertisement` payload: a 64-byte
Ed25519 signature produced by the **publisher** — the node that sends the
message, whose identity is the control-plane envelope's `sender_node_id`.

The signature covers the canonical framing of every security-relevant
advertisement field **except** the signature itself, using the crate-wide
`crate::protocol_signing` convention (the same primitives as mailbox acks,
tunnel capabilities, and download descriptors):

```text
canonical bytes =
  postcard((
    "boru/public-room-advertisement/v1",   # domain-separation tag
    advert_version,                        # signed payload version
    (
      publisher,                           # the signer's public key (embedded
                                           # so a signature is self-describing)
      advert_version,
      room_id,
      room_name,
      short_description,
      room_protocol_version,
      owner_peer_id,
      visibility,
      expires_after_secs,
      tags,
      last_active_hint_secs,
      approximate_member_count,
      room_avatar_hash,
      feature_flags,
    ),
  ))
```

* Publisher API: `PublicRoomAdvertisement::sign(&mut self, &SecretKey)`
  (the node signs with its own node key; the service never holds a secret).
* Verify API: `PublicRoomAdvertisement::verify_signed(&self, &PublicKey) ->
  AdvertisementAuth`, against the **claimed publisher** (the envelope
  `sender_node_id`).
* Domain separation: a signature over this tag can never verify as a
  signature over any other Boru protocol object family.

This is layered on top of the existing transport attribution (BORU-CP-03):
the control-plane guard already verifies that the envelope's claimed
`sender_node_id` equals the authenticated gossip delivery source before a
frame is accepted, and the advertisement-level signature then binds the
*payload* to that same key so the advertisement is verifiable at cache time,
not just at receive time.

## 2. Verification path (before the trusted directory view)

In `DiscoveryService::handle_incoming` (the only place advertisements are
decoded — BORU-DIR-01), every accepted PUBLIC_ROOM_ADVERTISEMENT payload is
verified against the envelope's `sender_node_id`:

| Auth state                          | Disposition                                          |
|-------------------------------------|------------------------------------------------------|
| `AdvertisementAuth::Verified`       | Emitted as `ControlEvent::RoomAdvertisement` with `auth = Verified { publisher }` — may enter the trusted directory view. |
| `AdvertisementAuth::MissingSignature` | Emitted with `auth = MissingSignature` — **clearly untrusted** (may be listed as unverified, never canonical). |
| `AdvertisementAuth::InvalidSignature` | **Discarded** — `IncomingOutcome::AdvertisementAuthRejected`, counted as a malformed packet, never enters the directory view, never affects gossip or chat processing. |

Failed verification never panics and never touches the peer registry or the
gossip actor: the receive path is a pure function, and a rejected
advertisement is simply dropped (regression-tested).

## 3. Is `owner_peer_id` descriptive or cryptographically proven?

`owner_peer_id` is **descriptive metadata**. A valid signature proves who
*published* the advertisement; it does not, by itself, prove who *owns* the
room. Room ownership is cryptographically proven **only when**:

```text
verify_signed(publisher) == Verified   AND   publisher == owner_peer_id
```

i.e. the advertisement verifies as signed by the key named in
`owner_peer_id`. `PublicRoomAdvertisement::is_authoritative_publisher`
implements the second half of that test. An unsigned advertisement that
names an owner proves nothing (test: `unsigned_advertisement_is_untrusted`).

## 4. Multiple-member advertising: independent endorsement vs designated room authority

**Only the designated room authority may publish canonical metadata.** The
designated room authority is the key named in `owner_peer_id` — the room
creator/owner. Concretely:

* An advertisement that verifies as **signed by the room authority**
  (`Verified` + `is_authoritative_publisher`) may establish or update the
  room's **canonical** directory entry.
* An advertisement that verifies as signed by **any other member** is an
  **independent endorsement** of the room's existence: it may appear in the
  directory as a member-endorsed listing, but it can **never replace** the
  authority's canonical metadata.
* An advertisement with **no signature** is untrusted and can never be
  canonical.

This is the deterministic conflict rule the directory cache (BORU-DIR-04 /
PDF Phase 4 Task 4.2) applies per `room_id`: prefer the newer verified
advertisement from the room's recognized authority; retain a conflict state
rather than silently trusting arbitrary metadata; and never allow rapid
name/description oscillation from unauthenticated or non-authority peers.
The rule is pinned by `spoofed_advertisement_cannot_overwrite_canonical`
(three attack shapes: wrong-publisher signature, forged owner signature,
stolen-signature-on-tampered-payload — all fail at least one half of the
rule).

## 5. Authentication never replaces room-level authorization

Nothing in this design grants permission. A verified advertisement only
attributes *metadata* to a publisher; the join flow (PDF Phase 6) still uses
the room's normal join/permission/moderation logic, and the PDF Core rule
stands: the discovery network advertises that a room exists — it does not
join the room, subscribe to its chat topic, download its history, or grant
membership or moderation. `AdvertisementAuth` has no authorization surface
(no join/moderate methods), mirroring the "no authorisation by presence"
guarantee of the control-plane privacy layer.

## Wire compatibility

The `signature` field is appended at the END of the payload struct with
`#[serde(default)]` and the envelope decoder discards trailing bytes, so:

* older clients decode the known prefix and ignore the signature;
* newer clients treat a missing signature as `MissingSignature` (untrusted,
  never canonical) — an unsigned advertisement from an older peer is not a
  crash, and it cannot overwrite canonical metadata.
