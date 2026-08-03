#!/usr/bin/env python3
"""Seed chat_history.json with deterministic text entries for scroll-behavior
evidence (development/test only; not part of the app).

Writes entries for the Alice direct topic into an already-seeded Boru data
directory (run seed_boru_data.py first).  The entries alternate Local ("You")
and Remote ("Alice") text messages with distinctive numbered bodies so OCR
can verify which part of the timeline is on screen.

Usage: python3 seed_chat_history.py <data_dir> [count]
"""
import json
import os
import sys

def blake3(data: bytes) -> bytes:
    from blake3 import blake3 as _b3
    return _b3(data).digest()

def direct_topic(local_hex: str, peer_hex: str):
    local = bytes.fromhex(local_hex)
    peer = bytes.fromhex(peer_hex)
    first, second = (local, peer) if local <= peer else (peer, local)
    return list(blake3(b"iroh-gossip-chat/direct/v1" + first + second))

def main() -> None:
    if len(sys.argv) not in (2, 3):
        print("usage: seed_chat_history.py <data_dir> [count]", file=sys.stderr)
        return 2
    data_dir = sys.argv[1]
    count = int(sys.argv[2]) if len(sys.argv) == 3 else 60

    secret_path = os.path.join(data_dir, "secret_key.txt")
    if not os.path.exists(secret_path):
        print("missing secret_key.txt — run seed_boru_data.py first", file=sys.stderr)
        return 1
    with open(secret_path) as f:
        secret_hex = f.read().strip()
    from cryptography.hazmat.primitives.asymmetric.ed25519 import Ed25519PrivateKey
    priv = Ed25519PrivateKey.from_private_bytes(bytes.fromhex(secret_hex))
    local_pk_hex = priv.public_key().public_bytes_raw().hex()

    alice_pk = "a1" * 32
    topic_alice = direct_topic(local_pk_hex, alice_pk)

    import time
    now_ms = int(time.time() * 1000)
    entries = []
    for i in range(1, count + 1):
        if i % 2 == 0:
            sender = alice_pk
            kind = "text"
            label_prefix = "Alice"
        else:
            sender = local_pk_hex
            kind = "text"
            label_prefix = "You"
        body = f"Seed msg {i:03d} - {label_prefix} timeline entry number {i}"
        entries.append({
            "event_id": i,
            "hash": "0" * 64,
            "sender": sender,
            "timestamp": now_ms - (count - i) * 60_000,
            "kind": kind,
            "topic": topic_alice,
            "text_preview": body,
            "signed_bytes": [],
            "delivery_state": "Delivered" if i % 2 == 0 else "Seen",
        })

    store = {
        "schema_version": 1,
        "entries": entries,
    }
    path = os.path.join(data_dir, "chat_history.json")
    with open(path, "w") as f:
        json.dump(store, f, indent=2)
    print(f"wrote {count} history entries to {path}")
    return 0

if __name__ == "__main__":
    raise SystemExit(main())
