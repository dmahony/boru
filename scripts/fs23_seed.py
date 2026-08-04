#!/usr/bin/env python3
"""FS-23 clean-run seed: isolated sender/receiver profiles with deterministic keys.

Writes exactly the files the app itself writes (secret_key.txt, friends.json,
conversations.json) plus a share fixture file. Nothing else touches the dirs;
the app creates boru.db / blobs / downloads / logs on first launch.
"""
from __future__ import annotations
import json
import os
import sys
import time

from cryptography.hazmat.primitives.asymmetric.ed25519 import Ed25519PrivateKey


def make_key_pair(seed_byte: int) -> tuple[str, str]:
    priv = Ed25519PrivateKey.from_private_bytes(bytes([seed_byte] * 32))
    pub = priv.public_key().public_bytes_raw().hex()
    secret_hex = (bytes([seed_byte] * 32)).hex()
    return secret_hex, pub


def seed_profile(data_dir: str, seed_byte: int, label: str, peer_pk: str, peer_label: str) -> str:
    os.makedirs(data_dir, exist_ok=True)
    secret_hex, local_pk = make_key_pair(seed_byte)
    key_path = os.path.join(data_dir, "secret_key.txt")
    if not os.path.exists(key_path):
        with open(key_path, "w") as f:
            f.write(secret_hex + "\n")
        os.chmod(key_path, 0o600)

    now_ms = int(time.time() * 1000)
    friends = {
        "schema_version": 4,
        "friends": {
            peer_pk: {
                "label": peer_label,
                "status": {"online": False, "last_seen_at_unix_ms": 0},
                "relationship": "friends",
                "known_addrs": [],
                "addrs_updated_at_unix_ms": 0,
                "rooms": [],
            }
        },
    }
    friends_path = os.path.join(data_dir, "friends.json")
    if not os.path.exists(friends_path):
        with open(friends_path, "w") as f:
            json.dump(friends, f, indent=2)

    conv_path = os.path.join(data_dir, "conversations.json")
    if not os.path.exists(conv_path):
        with open(conv_path, "w") as f:
            json.dump({"schema_version": 1, "conversations": []}, f, indent=2)

    print(f"{label}: local_pk={local_pk} dir={data_dir}")
    return local_pk


def main() -> int:
    base = "/tmp/fs23-clean"
    sender_dir = os.path.join(base, "sender")
    receiver_dir = os.path.join(base, "receiver")

    # Fixed QUIC bind ports — two iroh endpoints on one host conflict on the
    # mDNS port 5353, so direct address seeding (known_addrs) with a fixed
    # port is the deterministic connection path (see seed_two_instances.py).
    # The launcher passes the same ports via --bind-port.
    sender_port = int(os.environ.get("FS23_SENDER_PORT", "41001"))
    receiver_port = int(os.environ.get("FS23_RECEIVER_PORT", "41002"))

    # Derive both keys first so each friends.json can reference the other.
    sender_secret, sender_pk = make_key_pair(0x51)  # 0x51 = 'Q'… deterministic
    receiver_secret, receiver_pk = make_key_pair(0x52)

    os.makedirs(sender_dir, exist_ok=True)
    os.makedirs(receiver_dir, exist_ok=True)

    # Sender profile
    sp = os.path.join(sender_dir, "secret_key.txt")
    if not os.path.exists(sp):
        with open(sp, "w") as f:
            f.write(sender_secret + "\n")
        os.chmod(sp, 0o600)
    fp = os.path.join(sender_dir, "friends.json")
    if not os.path.exists(fp):
        with open(fp, "w") as f:
            json.dump(
                {
                    "schema_version": 4,
                    "friends": {
                        receiver_pk: {
                            "label": "Receiver",
                            "status": {"online": False, "last_seen_at_unix_ms": 0},
                            "relationship": "friends",
                            # Direct QUIC address on the fixed bind port. JSON
                            # shape mirrors EndpointAddr serde:
                            # { "id": <pk hex>, "addrs": [ { "Ip": "127.0.0.1:<port>" } ] }
                            "known_addrs": [
                                {"id": receiver_pk, "addrs": [{"Ip": f"127.0.0.1:{receiver_port}"}]}
                            ],
                            "addrs_updated_at_unix_ms": 0,
                            "rooms": [],
                        }
                    },
                },
                f,
                indent=2,
            )

    # Receiver profile
    rp = os.path.join(receiver_dir, "secret_key.txt")
    if not os.path.exists(rp):
        with open(rp, "w") as f:
            f.write(receiver_secret + "\n")
        os.chmod(rp, 0o600)
    fp2 = os.path.join(receiver_dir, "friends.json")
    if not os.path.exists(fp2):
        with open(fp2, "w") as f:
            json.dump(
                {
                    "schema_version": 4,
                    "friends": {
                        sender_pk: {
                            "label": "Sender",
                            "status": {"online": False, "last_seen_at_unix_ms": 0},
                            "relationship": "friends",
                            "known_addrs": [
                                {"id": sender_pk, "addrs": [{"Ip": f"127.0.0.1:{sender_port}"}]}
                            ],
                            "addrs_updated_at_unix_ms": 0,
                            "rooms": [],
                        }
                    },
                },
                f,
                indent=2,
            )

    for d in (sender_dir, receiver_dir):
        c = os.path.join(d, "conversations.json")
        if not os.path.exists(c):
            with open(c, "w") as f:
                json.dump({"schema_version": 1, "conversations": []}, f, indent=2)

    # Share fixture: small text file with known content + a second larger one.
    share_dir = os.path.join(base, "share")
    os.makedirs(share_dir, exist_ok=True)
    hello = os.path.join(share_dir, "hello-boru.txt")
    if not os.path.exists(hello):
        with open(hello, "w") as f:
            f.write("FS-23 end-to-end file sharing test\n" * 40)
    big = os.path.join(share_dir, "big-blob.bin")
    if not os.path.exists(big):
        with open(big, "wb") as f:
            f.write(os.urandom(8 * 1024 * 1024))  # 8 MiB for progress throttling

    print(f"sender_pk={sender_pk}")
    print(f"receiver_pk={receiver_pk}")
    print(f"share_dir={share_dir}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
