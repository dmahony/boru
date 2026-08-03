#!/usr/bin/env python3
"""Deterministic Figure 4 QA timeline fixture (UI-13, kanban t_6814630e).

Consumes `docs/ui-redesign/evidence/ui-13-fixture/figure4-timeline-spec.json`
(the extraction output of t_49713d7a) and injects its message spec into the
Boru chat timeline data store (`chat_history.json`) in deterministic store
order, inside an **isolated** data directory.  The fixture never touches
production data paths: it writes only into the data directory you pass in,
and `cleanup()` removes exactly the files it created.

What gets written (Boru JSON schemas, same as the app writes):

- `secret_key.txt`        — fixed deterministic identity (seed = bytes(range(32)))
- `friends.json`          — remote peer `28d7ee8656` (online, Direct conversation)
- `conversations.json`    — the Direct conversation row (sidebar CHATS entry)
- `chat_history.json`     — the Figure 4 timeline: 4 system chips + 7 user
                            bubbles (11 entries) in spec order, plus an
                            optional today-anchor entry that reproduces the
                            figure's "Today" divider above "Yesterday"

Determinism guarantees:

- Fixed identity seed, fixed remote key, fixed message order/content/times.
- Timestamps are computed relative to `now` so Today/Yesterday resolve on
  the run date; pass `--now-ms` to pin the clock for byte-identical reruns.
- No randomness anywhere in the pipeline.

CLI:

    python3 scripts/figure4_fixture.py inject <data_dir> [--spec PATH]
        [--no-today-anchor] [--now-ms MS] [--force]
    python3 scripts/figure4_fixture.py cleanup <data_dir>
    python3 scripts/figure4_fixture.py selfcheck
    python3 scripts/figure4_fixture.py validate <data_dir>

Module API:

    from figure4_fixture import inject, cleanup, load_spec
    result = inject("/tmp/boru-qa-data")
    cleanup("/tmp/boru-qa-data")
"""

from __future__ import annotations

import argparse
import json
import os
import sys
import time

from cryptography.hazmat.primitives.asymmetric.ed25519 import Ed25519PrivateKey

REPO_ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
DEFAULT_SPEC_PATH = os.path.join(
    REPO_ROOT, "docs", "ui-redesign", "evidence", "ui-13-fixture", "figure4-timeline-spec.json"
)

DAY_MS = 86_400_000
HOUR_MS = 3_600_000
MINUTE_MS = 60_000

# Files the fixture writes; `cleanup` removes exactly these.
INJECTED_FILES = (
    "secret_key.txt",
    "friends.json",
    "conversations.json",
    "chat_history.json",
)

# Fixed deterministic identity seed (same convention as seed_boru_data.py).
IDENTITY_SEED = bytes(range(32))

# Remote peer key: 64-hex, starts with the figure's truncated prefix.
# Any 64-hex string parses as an iroh PublicKey; the label is what the UI
# shows, so we set friends.json label to "28d7ee8656".
REMOTE_PK_HEX = "28d7ee8656" + "ab" * 27  # 10 + 54 = 64 hex chars


def blake3(data: bytes) -> bytes:
    """Real blake3 when available; the app needs exact blake3 topics."""
    try:
        from blake3 import blake3 as _b3

        return _b3(data).digest()
    except ImportError:
        import hashlib

        print(
            "WARNING: python blake3 unavailable; direct topics will not match",
            file=sys.stderr,
        )
        return hashlib.blake2b(data, digest_size=32).digest()


def direct_topic(local_hex: str, peer_hex: str) -> list:
    """Direct-conversation topic: blake3('iroh-gossip-chat/direct/v1' + sorted keys).

    Must match the app's derivation exactly so the conversation row opens and
    chat_history entries replay under the right topic.
    """
    local = bytes.fromhex(local_hex)
    peer = bytes.fromhex(peer_hex)
    first, second = (local, peer) if local <= peer else (peer, local)
    return list(blake3(b"iroh-gossip-chat/direct/v1" + first + second))


def load_spec(spec_path: str | None = None) -> dict:
    """Load and validate the Figure 4 timeline spec JSON."""
    path = os.path.abspath(spec_path or DEFAULT_SPEC_PATH)
    if not os.path.exists(path):
        raise FileNotFoundError(f"spec not found: {path}")
    with open(path, "r", encoding="utf-8") as f:
        spec = json.load(f)
    messages = spec.get("messages")
    if not isinstance(messages, list) or len(messages) < 2:
        raise ValueError(f"spec {path} has no usable messages[]")
    return spec


def parse_spec_time(time_str: str) -> tuple[int, int]:
    """Parse a spec 'time' like '10:31 AM' into (hour, minute)."""
    stripped = time_str.strip().upper()
    if not stripped.endswith(("AM", "PM")):
        raise ValueError(f"unrecognised spec time: {time_str!r}")
    suffix = stripped[-2:]
    hhmm = stripped[:-2].strip()
    hour_s, minute_s = hhmm.split(":")
    hour, minute = int(hour_s), int(minute_s)
    if suffix == "PM" and hour != 12:
        hour += 12
    elif suffix == "AM" and hour == 12:
        hour = 0
    return hour, minute


def build_chat_history(
    spec: dict,
    local_pk_hex: str,
    remote_pk_hex: str,
    topic: list,
    now_ms: int,
    with_today_anchor: bool = True,
) -> list:
    """Build chat_history entries from the spec in deterministic store order.

    The spec's `messages[]` is already store order; date_separator entries
    are *derived* by the app from timestamps (they are not stored rows), so
    they are skipped here.  All real content is day_offset=1 (Yesterday);
    with `with_today_anchor` we prepend one today-dated system entry so the
    renderer shows the figure's "Today" divider above "Yesterday".
    """
    today_start_ms = now_ms - (now_ms % DAY_MS)  # start of the current UTC day
    entries: list[dict] = []
    event_id = 1

    def push(kind: str, sender_hex: str, body: str, ts: int, delivery: str) -> None:
        nonlocal event_id
        entries.append(
            {
                "event_id": event_id,
                "hash": format(event_id, "064x"),
                "sender": sender_hex,
                "timestamp": ts,
                "kind": kind,
                "topic": topic,
                "text_preview": body,
                "signed_bytes": [],
                "delivery_state": delivery,
            }
        )
        event_id += 1

    # Optional today anchor: reproduces the figure's empty "Today" section.
    # The anchor chip sits between the Today divider and the Yesterday
    # divider; capture with the timeline scrolled so it sits just above the
    # viewport top (see spec reproduction_notes[1]).
    if with_today_anchor:
        anchor_ts = today_start_ms + 9 * HOUR_MS + 5 * MINUTE_MS  # 09:05 today
        push("system", local_pk_hex, "Conversation started.", anchor_ts, "Sent")

    # Real spec content: 4 system chips + 7 user bubbles, all Yesterday.
    for msg in spec["messages"]:
        mtype = msg.get("type")
        if mtype == "date_separator":
            continue  # derived by the app from timestamps
        day_offset = int(msg.get("day_offset", 1))
        hour, minute = parse_spec_time(msg.get("time", "10:31 AM"))
        # Seconds offset keeps same-minute entries distinct and ordered.
        seconds = (int(msg.get("id", 0)) * 3) % 60
        ts = (
            today_start_ms
            - day_offset * DAY_MS
            + hour * HOUR_MS
            + minute * MINUTE_MS
            + seconds * 1000
        )
        body = msg["content"]
        if mtype == "system_chip":
            push("system", local_pk_hex, body, ts, "Sent")
        elif mtype == "text":
            sender = (
                local_pk_hex if msg.get("sender") == "local" else remote_pk_hex
            )
            # Spec delivery_state wins when present (UI-14 states evidence);
            # fall back to the Figure 4 defaults otherwise.
            delivery = msg.get("delivery_state") or (
                "Seen" if msg.get("sender") == "local" else "Delivered"
            )
            push("text", sender, body, ts, delivery)
        else:
            raise ValueError(f"unrecognised spec message type: {mtype!r}")
    return entries


def _inject_to_dir(
    data_dir: str,
    spec_path: str | None,
    now_ms: int,
    with_today_anchor: bool,
    force: bool,
) -> dict:
    """Write all injected files into an isolated data dir. Returns summary."""
    os.makedirs(data_dir, exist_ok=True)

    # Safety guard: refuse to touch a directory that already holds real app
    # data (production paths), unless --force was passed explicitly.
    existing = [
        name
        for name in INJECTED_FILES
        if os.path.exists(os.path.join(data_dir, name))
    ]
    if existing and not force:
        raise SystemExit(
            f"refusing to overwrite existing fixture files in {data_dir}: "
            f"{', '.join(existing)} (pass --force to overwrite; production "
            f"data dirs should never be used as fixture targets)"
        )

    spec = load_spec(spec_path)
    today_start_ms = now_ms - (now_ms % DAY_MS)

    # Deterministic identity.
    priv = Ed25519PrivateKey.from_private_bytes(IDENTITY_SEED)
    secret_hex = IDENTITY_SEED.hex()
    local_pk_hex = priv.public_key().public_bytes_raw().hex()
    with open(os.path.join(data_dir, "secret_key.txt"), "w") as f:
        f.write(secret_hex + "\n")

    topic = direct_topic(local_pk_hex, REMOTE_PK_HEX)

    # friends.json (schema 4): remote peer online, Direct conversation active.
    friends = {
        "schema_version": 4,
        "friends": {
            REMOTE_PK_HEX: {
                "label": "28d7ee8656",
                "status": {
                    "online": True,
                    "last_seen_at_unix_ms": now_ms - 60_000,
                },
                "relationship": "friends",
                "direct_conversation": {"topic": topic, "state": "Active"},
            }
        },
    }
    with open(os.path.join(data_dir, "friends.json"), "w") as f:
        json.dump(friends, f, indent=2)

    # conversations.json (schema 1): the Direct conversation row.
    conversations = {
        "schema_version": 1,
        "conversations": [
            {
                "topic": topic,
                "peer_id": REMOTE_PK_HEX,
                "name": "28d7ee8656",
                "kind": "Direct",
                "created_at_unix_ms": today_start_ms - DAY_MS,
                "last_seen_at_unix_ms": today_start_ms
                - DAY_MS
                + 10 * HOUR_MS
                + 35 * MINUTE_MS,
                "last_message_preview": "Great work!",
                "unread_count": 0,
                "archived": False,
            }
        ],
    }
    with open(os.path.join(data_dir, "conversations.json"), "w") as f:
        json.dump(conversations, f, indent=2)

    # chat_history.json (schema 1): the Figure 4 timeline.
    entries = build_chat_history(
        spec, local_pk_hex, REMOTE_PK_HEX, topic, now_ms, with_today_anchor
    )
    store = {"schema_version": 1, "entries": entries}
    with open(os.path.join(data_dir, "chat_history.json"), "w") as f:
        json.dump(store, f, indent=2)

    summary = {
        "data_dir": os.path.abspath(data_dir),
        "local_pk_hex": local_pk_hex,
        "remote_pk_hex": REMOTE_PK_HEX,
        "topic": topic,
        "entries": len(entries),
        "system_entries": sum(1 for e in entries if e["kind"] == "system"),
        "text_entries": sum(1 for e in entries if e["kind"] == "text"),
        "with_today_anchor": with_today_anchor,
        "files": [
            os.path.join(data_dir, name) for name in INJECTED_FILES
        ],
    }
    return summary


def inject(
    data_dir: str,
    spec_path: str | None = None,
    now_ms: int | None = None,
    with_today_anchor: bool = True,
    force: bool = False,
) -> dict:
    """Run the fixture: inject the Figure 4 timeline into an isolated data dir.

    Returns a summary dict (paths, entry counts, identities).  See module
    docstring for CLI usage and the determinism/cleanup guarantees.
    """
    return _inject_to_dir(
        data_dir,
        spec_path,
        int(now_ms if now_ms is not None else time.time() * 1000),
        with_today_anchor,
        force,
    )


def cleanup(data_dir: str) -> list[str]:
    """Remove the injected data: exactly the files the fixture wrote.

    If the data directory becomes empty afterwards it is removed too.
    Returns the list of removed paths.  Never touches files outside the
    injected set.
    """
    removed = []
    for name in INJECTED_FILES:
        path = os.path.join(data_dir, name)
        if os.path.exists(path):
            os.remove(path)
            removed.append(path)
    # Remove the directory only if it is now empty (or was created by us).
    if os.path.isdir(data_dir) and not os.listdir(data_dir):
        os.rmdir(data_dir)
        removed.append(os.path.abspath(data_dir))
    return removed


# ── Self-check / validation ──────────────────────────────────────────────

def _expected_sequence(spec: dict, with_today_anchor: bool) -> list[str]:
    seq = []
    if with_today_anchor:
        seq.append("Conversation started.")
    for msg in spec["messages"]:
        if msg.get("type") == "date_separator":
            continue
        seq.append(msg["content"])
    return seq


def selfcheck() -> int:
    """Prove determinism: two injects with the same clock are byte-identical."""
    import tempfile

    spec = load_spec()
    fixed_now = 1_752_000_000_000
    with tempfile.TemporaryDirectory() as d1, tempfile.TemporaryDirectory() as d2:
        inject(d1, now_ms=fixed_now)
        inject(d2, now_ms=fixed_now)
        for name in INJECTED_FILES:
            p1 = os.path.join(d1, name)
            p2 = os.path.join(d2, name)
            b1 = open(p1, "rb").read()
            b2 = open(p2, "rb").read()
            if b1 != b2:
                print(f"SELFCHECK FAIL: {name} differs between identical runs")
                return 1
            print(f"SELFCHECK OK: {name} byte-identical ({len(b1)} bytes)")

        # Order/content must match the spec sequence.
        store = json.load(open(os.path.join(d1, "chat_history.json")))
        seq = [e["text_preview"] for e in store["entries"]]
        expected = _expected_sequence(spec, with_today_anchor=True)
        if seq != expected:
            print("SELFCHECK FAIL: entry order/content does not match spec")
            print(f"  expected {expected}")
            print(f"  got      {seq}")
            return 1
        print(f"SELFCHECK OK: {len(seq)} entries in spec order")

        # Cleanup must remove every injected file.
        removed = cleanup(d1)
        for name in INJECTED_FILES:
            if os.path.exists(os.path.join(d1, name)):
                print(f"SELFCHECK FAIL: cleanup left {name}")
                return 1
        print(f"SELFCHECK OK: cleanup removed {len(removed)} paths")
    return 0


def validate(data_dir: str) -> int:
    """Validate an injected data dir: schema, order, content vs the spec.

    Anchor-aware: accepts both the default (with today anchor, 12 entries)
    and --no-today-anchor (11 entries) layouts.
    """
    spec = load_spec()
    hist_path = os.path.join(data_dir, "chat_history.json")
    if not os.path.exists(hist_path):
        print(f"validate: {hist_path} missing", file=sys.stderr)
        return 1
    store = json.load(open(hist_path))
    if store.get("schema_version") != 1:
        print(f"validate: unexpected schema_version {store.get('schema_version')}")
        return 1
    entries = store.get("entries", [])
    seq = [e["text_preview"] for e in entries]
    anchor = bool(seq and seq[0] == "Conversation started.")
    expected = _expected_sequence(spec, with_today_anchor=anchor)
    if seq != expected:
        print("validate FAIL: order/content mismatch")
        print(f"  expected ({len(expected)}): {expected}")
        print(f"  got      ({len(seq)}): {seq}")
        return 1
    ids = [e["event_id"] for e in entries]
    if ids != list(range(1, len(entries) + 1)):
        print(f"validate FAIL: event_ids not contiguous 1..N: {ids}")
        return 1
    topics = {tuple(e["topic"]) for e in entries}
    if len(topics) != 1:
        print(f"validate FAIL: multiple topics in timeline: {len(topics)}")
        return 1
    print(
        f"validate OK: {len(entries)} entries "
        f"({sum(1 for e in entries if e['kind']=='system')} system, "
        f"{sum(1 for e in entries if e['kind']=='text')} text) in spec order"
    )
    return 0


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(
        prog="figure4_fixture",
        description="Deterministic Figure 4 QA timeline fixture (UI-13).",
    )
    sub = parser.add_subparsers(dest="command", required=True)

    p_inject = sub.add_parser("inject", help="inject the Figure 4 timeline into <data_dir>")
    p_inject.add_argument("data_dir")
    p_inject.add_argument("--spec", default=None, help="path to figure4-timeline-spec.json")
    p_inject.add_argument("--now-ms", type=int, default=None, help="pin clock (determinism tests)")
    p_inject.add_argument("--no-today-anchor", action="store_true", help="skip the Today anchor entry")
    p_inject.add_argument("--force", action="store_true", help="overwrite existing fixture files")

    p_cleanup = sub.add_parser("cleanup", help="remove injected data from <data_dir>")
    p_cleanup.add_argument("data_dir")

    sub.add_parser("selfcheck", help="run the determinism/cleanup self-check")
    p_validate = sub.add_parser("validate", help="validate an injected data dir")
    p_validate.add_argument("data_dir")

    args = parser.parse_args(argv)

    if args.command == "inject":
        summary = _inject_to_dir(
            args.data_dir,
            args.spec,
            args.now_ms if args.now_ms is not None else int(time.time() * 1000),
            not args.no_today_anchor,
            args.force,
        )
        print(f"injected {summary['entries']} entries into {summary['data_dir']}")
        print(f"  local key : {summary['local_pk_hex']}")
        print(f"  remote key: {summary['remote_pk_hex']}")
        for path in summary["files"]:
            print(f"  wrote {path}")
        return 0
    if args.command == "cleanup":
        removed = cleanup(args.data_dir)
        for path in removed:
            print(f"removed {path}")
        if not removed:
            print(f"nothing to clean in {args.data_dir}")
        return 0
    if args.command == "selfcheck":
        return selfcheck()
    if args.command == "validate":
        return validate(args.data_dir)
    parser.error(f"unknown command: {args.command}")
    return 2


if __name__ == "__main__":
    try:
        raise SystemExit(main())
    except BrokenPipeError:
        # Downstream consumer (e.g. `head`) closed the pipe: exit quietly.
        devnull = os.open(os.devnull, os.O_WRONLY)
        os.dup2(devnull, sys.stdout.fileno())
        sys.exit(0)
