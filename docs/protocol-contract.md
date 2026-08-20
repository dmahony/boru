# Boru wire protocol contract

Status: authoritative implementation contract

## Single source of truth

The only authoritative definitions for the chat gossip wire format are in
`src/chat_core/protocol.rs`:

- `Message` is the postcard payload schema.
- `SignedMessage` is the authenticated transport envelope.
- `NetEvent` is the decoded event boundary used by both frontends.
- `RoomAdvertisement` and the invitation types are protocol-owned extension
  payloads.

The TUI, iced GUI, storage, and network handlers must consume these shared
Rust types. They must not define parallel wire enums or deserialize gossip
bytes directly into frontend-specific types.

## Encoding and authentication

Chat payloads are postcard-encoded and carried inside `SignedMessage`. New
messages are produced with `SignedMessage::sign_and_encode`; compressed
messages use `sign_and_encode_compressed`. Receivers must use
`SignedMessage::verify_and_decode` (or `verify_and_decode_with_id`) before
handling a payload. Decode and verification failures are controlled errors and
must be dropped at the network boundary; they must not disconnect or panic the
frontend.

`SIGNED_MESSAGE_PROTOCOL` and `SIGNED_MESSAGE_VERSION` identify the canonical
signed framing. The signature covers the sender, timestamp, compression mode,
message id, and payload bytes. Legacy envelopes signed under the previous
framing remain accepted by the compatibility verifier.

## Compatibility rules

1. Preserve existing `Message` variant order. Postcard enum discriminants are
   part of the wire format; never reorder or remove existing variants.
2. Add new message variants only at the end of `Message`.
3. Add compatible fields only at the end of a struct-like payload. A missing
   trailing field must be handled explicitly by a manual `Deserialize` visitor
   when postcard cannot invoke `#[serde(default)]` at EOF.
4. Older peers may ignore trailing extension bytes. New peers must supply a
   documented default for fields absent from legacy payloads.
5. Optional extension fields use `#[serde(default)]` where postcard always
   emits the option tag. Do not use `skip_serializing_if` on postcard payloads.
6. Unknown required variants, invalid field encodings, truncated envelopes,
   bad signatures, unsupported compression, and invalid payloads are rejected
   with an error. They are not interpreted as a different message.
7. Extension-shaped data must be bounded by the existing transport limits;
   protocol consumers must not allocate unbounded state before verification and
   decoding succeeds.

Current compatibility extensions include the trailing `compression` and
`message_id` fields of `SignedMessage`, the trailing TTL field of
`RoomAdvertisement`, and the optional/trailing file-share metadata fields.
Their legacy defaults and tests live beside the definitions in
`src/chat_core/protocol.rs` and `src/chat_core/tests.rs`.

## Frontend contract

Both frontends receive the same decoded `NetEvent` and dispatch through the
shared `ChatCallbacks` trait. A protocol change is complete only when the
shared protocol tests pass and both the TUI and iced GUI compile against the
same re-exports from `chat_core`.
