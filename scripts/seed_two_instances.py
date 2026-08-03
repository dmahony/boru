#!/usr/bin/env python3
"""Seed TWO Boru data directories that know each other as friends, so two
instances can discover each other over local mDNS and exchange real messages
(development/test only; not part of the app).

Instance A's peer is B's real public key and vice-versa. Each side gets a
friends.json entry with an Active direct conversation whose topic is the
deterministic direct topic of the two public keys.

Usage: python3 seed_two_instances.py <dir_a> <dir_b> [--history N]
"""
import json
import os
import sys
import time

from cryptography.hazmat.primitives.asymmetric.ed25519 import Ed25519PrivateKey


def blake3(data: bytes) -> bytes:
    from blake3 import blake3 as _b3
    return _b3(data).digest()


def gen_keypair(seed: bytes):
    priv = Ed25519PrivateKey.from_private_bytes(seed)
    return priv, priv.public_key().public_bytes_raw().hex()


def direct_topic(hex_a: str, hex_b: str):
    a = bytes.fromhex(hex_a)
    b = bytes.fromhex(hex_b)
    first, second = (a, b) if a <= b else (b, a)
    return list(blake3(b"iroh-gossip-chat/direct/v1" + first + second))


def write_dir(path: str, local_pk: str, peer_pk: str, peer_label: str, now_ms: int,
              peer_addr: object | None = None):
    os.makedirs(path, exist_ok=True)
    topic = direct_topic(local_pk, peer_pk)
    known_addrs = [] if peer_addr is None else [peer_addr]
    friends = {
        "schema_version": 4,
        "friends": {
            peer_pk: {
                "label": peer_label,
                "status": {"online": False, "last_offline_at_unix_ms": now_ms - 60_000},
                "relationship": "friends",
                "known_addrs": known_addrs,
                "direct_conversation": {"topic": topic, "state": "Active"},
            }
        },
    }
    conversations = {
        "schema_version": 1,
        "conversations": [
            {
                "topic": topic,
                "peer_id": peer_pk,
                "name": peer_label,
                "kind": "Direct",
                "created_at_unix_ms": now_ms - 2 * 86_400_000,
                "last_seen_at_unix_ms": now_ms - 60_000,
                "last_message_preview": "",
                "unread_count": 0,
                "archived": False,
            }
        ],
    }
    with open(os.path.join(path, "friends.json"), "w") as f:
        json.dump(friends, f, indent=2)
    with open(os.path.join(path, "conversations.json"), "w") as f:
        json.dump(conversations, f, indent=2)
    return topic


def main() -> None:
    if len(sys.argv) < 3:
        print("usage: seed_two_instances.py <dir_a> <dir_b> [--bind-port-a P] [--bind-port-b P]",
              file=sys.stderr)
        return 2
    dir_a, dir_b = sys.argv[1], sys.argv[2]
    port_a = port_b = None
    if "--bind-port-a" in sys.argv:
        port_a = int(sys.argv[sys.argv.index("--bind-port-a") + 1])
    if "--bind-port-b" in sys.argv:
        port_b = int(sys.argv[sys.argv.index("--bind-port-b") + 1])

    now_ms = int(time.time() * 1000)

    # Deterministic secrets unless overridden (keeps topics stable across runs).
    seed_a = bytes.fromhex(os.environ.get("BORU_SEED_A", "aa" * 32))
    seed_b = bytes.fromhex(os.environ.get("BORU_SEED_B", "bb" * 32))
    priv_a, pk_a = gen_keypair(seed_a)
    priv_b, pk_b = gen_keypair(seed_b)

    os.makedirs(dir_a, exist_ok=True)
    os.makedirs(dir_b, exist_ok=True)
    with open(os.path.join(dir_a, "secret_key.txt"), "w") as f:
        f.write(seed_a.hex() + "\n")
    with open(os.path.join(dir_b, "secret_key.txt"), "w") as f:
        f.write(seed_b.hex() + "\n")

    # Seed each side with the other's direct QUIC address so two instances on
    # one host connect deterministically without relying on flaky mDNS
    # discovery (which conflicts when two iroh endpoints bind 5353 on the
    # same machine). The JSON shape mirrors EndpointAddr serde:
    # { "id": <peer pk hex>, "addrs": [ { "Ip": "127.0.0.1:<port>" } ] }.
    addr_b = None if port_b is None else {"id": pk_b, "addrs": [{"Ip": f"127.0.0.1:{port_b}"}]}
    addr_a = None if port_a is None else {"id": pk_a, "addrs": [{"Ip": f"127.0.0.1:{port_a}"}]}
    topic_ab = write_dir(dir_a, pk_a, pk_b, "BPeer", now_ms, addr_b)
    topic_ba = write_dir(dir_b, pk_b, pk_a, "APeer", now_ms, addr_a)
    assert topic_ab == topic_ba, "direct topic must be symmetric"

    print(f"instance A: pk={pk_a} peer={pk_b} topic={topic_ab}")
    print(f"instance B: pk={pk_b} peer={pk_a} topic={topic_ba}")
    print(f"BORU_SEED_A={seed_a.hex()} BORU_SEED_B={seed_b.hex()}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
