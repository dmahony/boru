#!/usr/bin/env python3
"""Seed a Boru data directory with deterministic fixture data for UI-06
sidebar evidence screenshots (development/test only; not part of the app).

Creates friends.json, conversations.json, rooms.json and friend_requests.json
using the same JSON schemas the app writes, so the running GUI renders a
populated sidebar: chats with previews/timestamps/unread counts, a group,
friends with online/offline state, and a pending request.

Usage: python3 seed_boru_data.py <data_dir>
"""
import json
import os
import sys
import time

from cryptography.hazmat.primitives.asymmetric.ed25519 import Ed25519PrivateKey

def blake3(data: bytes) -> bytes:
    # Small blake3 implementation fallback: use hashlib if the module is
    # unavailable.  The direct-topic derivation only needs a stable hash for
    # fixture data; exact blake3 output is required for the app to select the
    # seeded conversation row, so we need the real blake3.
    import hashlib
    try:
        from blake3 import blake3 as _b3
        return _b3(data).digest()
    except ImportError:
        # Fallback is NOT cryptographically equivalent; the seed script prints
        # a warning and the caller should install blake3 for correct topics.
        print("WARNING: python blake3 unavailable; direct topics will not match", file=sys.stderr)
        return hashlib.blake2b(data, digest_size=32).digest()

def main() -> None:
    if len(sys.argv) != 2:
        print("usage: seed_boru_data.py <data_dir>")
        sys.exit(2)
    data_dir = sys.argv[1]
    os.makedirs(data_dir, exist_ok=True)

    now_ms = int(time.time() * 1000)
    hour_ms = 3_600_000
    day_ms = 86_400_000

    # Deterministic local identity: write a real ed25519 secret key so the
    # app loads a stable local public key, and the pending request below can
    # target it.  The seed is fixed for reproducibility (test-only fixture).
    seed = bytes(range(32))
    priv = Ed25519PrivateKey.from_private_bytes(seed)
    secret_hex = seed.hex()
    local_pk_hex = priv.public_key().public_bytes_raw().hex()
    with open(os.path.join(data_dir, "secret_key.txt"), "w") as f:
        f.write(secret_hex + "\n")

    # Deterministic but valid-format identifiers: any 64-hex string parses as
    # an iroh PublicKey.  TopicId serialises as a 32-element byte array in
    # JSON (newtype around [u8; 32]), so emit those as lists.
    alice_pk = "a1" * 32
    bob_pk = "b2" * 32
    long_pk = "c3" * 32
    group_topic = [0xD4] * 32

    def topic_list(hex_str: str) -> list:
        return list(bytes.fromhex(hex_str))

    # Direct conversations use direct_topic(local, peer) so the MCP
    # open-conversation action selects the seeded row.
    def direct_topic(peer_hex: str) -> list:
        local = bytes.fromhex(local_pk_hex)
        peer = bytes.fromhex(peer_hex)
        first, second = (local, peer) if local <= peer else (peer, local)
        hasher = blake3(b"iroh-gossip-chat/direct/v1" + first + second)
        return list(hasher)

    topic_alice = direct_topic(alice_pk)
    topic_bob = direct_topic(bob_pk)
    topic_long = direct_topic(long_pk)

    # ── friends.json (schema 4) ────────────────────────────────────────
    friends = {
        "schema_version": 4,
        "friends": {
            alice_pk: {
                "label": "Alice",
                "status": {
                    "online": True,
                    "last_seen_at_unix_ms": now_ms - 60_000,
                },
                "relationship": "friends",
                "direct_conversation": {
                    "topic": topic_alice,
                    "state": "Active",
                },
            },
            bob_pk: {
                "label": "Bob",
                "status": {
                    "online": False,
                    "last_offline_at_unix_ms": now_ms - hour_ms,
                },
                "relationship": "friends",
                "direct_conversation": {
                    "topic": topic_bob,
                    "state": "Active",
                },
            },
            long_pk: {
                # Very long identity to verify graceful truncation + tooltip.
                "label": "a-very-long-display-name-for-truncation-test-peer-42",
                "status": {
                    "online": True,
                    "last_seen_at_unix_ms": now_ms - 60_000,
                },
                "relationship": "friends",
                "direct_conversation": {
                    "topic": topic_long,
                    "state": "Active",
                },
            },
        },
    }
    with open(os.path.join(data_dir, "friends.json"), "w") as f:
        json.dump(friends, f, indent=2)

    # ── conversations.json (schema 1) ──────────────────────────────────
    conversations = {
        "schema_version": 1,
        "conversations": [
            {
                "topic": topic_alice,
                "peer_id": alice_pk,
                "name": "Alice",
                "kind": "Direct",
                "created_at_unix_ms": now_ms - 3 * day_ms,
                "last_seen_at_unix_ms": now_ms - 2 * hour_ms,
                "last_message_preview": "See you at the demo!",
                "unread_count": 2,
                "archived": False,
            },
            {
                "topic": topic_bob,
                "peer_id": bob_pk,
                "name": "Bob",
                "kind": "Direct",
                "created_at_unix_ms": now_ms - 5 * day_ms,
                "last_seen_at_unix_ms": now_ms - day_ms,
                "last_message_preview": "Did you get the file?",
                "unread_count": 0,
                "archived": False,
            },
            {
                "topic": topic_long,
                "peer_id": long_pk,
                "name": "a-very-long-display-name-for-truncation-test-peer-42",
                "kind": "Direct",
                "created_at_unix_ms": now_ms - day_ms,
                "last_seen_at_unix_ms": now_ms - 30 * 60_000,
                # Long preview to verify single-line truncation.
                "last_message_preview": (
                    "This is a deliberately long message preview that should "
                    "truncate gracefully with an ellipsis in the sidebar row "
                    "instead of wrapping to multiple lines."
                ),
                "unread_count": 12,  # two-digit count badge
                "archived": False,
            },
            {
                "topic": group_topic,
                "peer_id": "",
                "name": "Weekend Trip Planning",
                "kind": "Group",
                "created_at_unix_ms": now_ms - 2 * day_ms,
                "last_seen_at_unix_ms": now_ms - 10 * 60_000,
                "last_message_preview": "Who's bringing the tent?",
                "unread_count": 5,
                "archived": False,
            },
        ],
    }
    with open(os.path.join(data_dir, "conversations.json"), "w") as f:
        json.dump(conversations, f, indent=2)

    # ── rooms.json (schema 1) — room history previews for chat rows ────
    rooms = {
        "schema_version": 1,
        "rooms": [
            {
                "topic": topic_alice,
                "name": "Alice",
                "last_seen": int(now_ms / 1000) - 7200,
                "last_preview": "See you at the demo!",
                "last_sender_name": "Alice",
                "member_count": 0,
                "is_owner": False,
            },
            {
                "topic": topic_bob,
                "name": "Bob",
                "last_seen": int(now_ms / 1000) - day_ms,
                "last_preview": "Did you get the file?",
                "last_sender_name": "Bob",
                "member_count": 0,
                "is_owner": False,
            },
            {
                "topic": topic_long,
                "name": "a-very-long-display-name-for-truncation-test-peer-42",
                "last_seen": int(now_ms / 1000) - 1800,
                "last_preview": (
                    "This is a deliberately long message preview that should "
                    "truncate gracefully with an ellipsis in the sidebar row "
                    "instead of wrapping to multiple lines."
                ),
                "last_sender_name": "a-very-long-display-name-for-truncation-test-peer-42",
                "member_count": 0,
                "is_owner": False,
            },
            {
                "topic": group_topic,
                "name": "Weekend Trip Planning",
                "last_seen": int(now_ms / 1000) - 600,
                "last_preview": "Who's bringing the tent?",
                "last_sender_name": "Alice",
                "member_count": 3,
                "is_owner": True,
            },
        ],
    }
    with open(os.path.join(data_dir, "rooms.json"), "w") as f:
        json.dump(rooms, f, indent=2)

    # ── friend_requests.json (schema 1) — pending incoming request ─────
    requests = {
        "schema_version": 1,
        "requests": {
            "req_ui06_alice": {
                "id": "req_ui06_alice",
                "requester": alice_pk,
                "recipient": local_pk_hex,
                "status": "Pending",
                "created_at_unix_ms": now_ms - hour_ms,
                "updated_at_unix_ms": now_ms - hour_ms,
                "message": None,
            }
        },
    }
    with open(os.path.join(data_dir, "friend_requests.json"), "w") as f:
        json.dump(requests, f, indent=2)

    print(f"seeded {data_dir}")
    for name in ("friends.json", "conversations.json", "rooms.json", "friend_requests.json"):
        print(f"  {name}: {os.path.getsize(os.path.join(data_dir, name))} bytes")

if __name__ == "__main__":
    main()
