#!/usr/bin/env python3
"""UI-18 responsive / high-DPI evidence fixture (kanban t_f75e5521).

Builds deterministic long-value stress states on top of `figure4_fixture`
(the Figure 4 chat timeline): long friend labels, long conversation and
group names, unbroken long messages, long system events and a long
last-message preview.  These exercise the truncation / reflow paths that
UI-18 must verify at every supported viewport.

QA only: writes exclusively into the data directory passed in and refuses
to overwrite directories that already hold app data (same guard as
figure4_fixture).  Cleanup is delegated to `figure4_fixture.py cleanup`.

CLI:
  ui18_fixture.py stress <data_dir> [--now-ms N]   # figure-4 base + long-value overlay
  ui18_fixture.py base <data_dir> [--now-ms N]     # plain figure-4 base (alias)
  ui18_fixture.py cleanup <data_dir>
"""
from __future__ import annotations

import argparse
import json
import os
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import figure4_fixture as f4

DAY_MS = 86_400_000
HOUR_MS = 3_600_000
MINUTE_MS = 60_000

# ── Long-value stress content ─────────────────────────────────────────────

# A friend label long enough to overflow a 304 px sidebar row at 14 px.
LONG_LABEL_A = (
    "28d7ee8656 — extremely long friend display name used to stress sidebar "
    "truncation and the online peers rail card"
)
LONG_LABEL_B = (
    "b7c9f0a3e5 — second friend with a deliberately verbose label that must "
    "never wrap the conversation row or escape the rail card"
)
LONG_GROUP_NAME = (
    "Project Nebula — weekly sync for the distributed mesh working group, "
    "with a name long enough to force clipping in the sidebar groups section"
)
LONG_PREVIEW = (
    "Last message preview that is intentionally far longer than any sidebar "
    "conversation row can display, so the row must ellipsize instead of "
    "expanding horizontally or pushing the timestamp off the row…"
)
# One unbroken 1600-char "word" — the worst case for bubble wrapping.
LONG_UNBROKEN_WORD = "supercalifragilisticexpialidocious" * 64  # 34*64 = 2176 chars
LONG_URL = (
    "https://example.com/some/very/deep/path/with/a/long/query?"
    + "&".join(f"param_{i}=value_{i}" for i in range(30))
)
LONG_EVENT = (
    "Alice renamed the group conversation from \u201cProject Nebula\u201d to "
    "\u201cProject Nebula — weekly sync for the distributed mesh working group, "
    "with a name long enough to force clipping in the sidebar groups section\u201d"
)


def _entry(eid: int, sender_hex: str, body: str, ts: int, topic: list, kind: str = "text",
           delivery: str = "Delivered") -> dict:
    return {
        "event_id": eid,
        "hash": format(eid, "064x"),
        "sender": sender_hex,
        "timestamp": ts,
        "kind": kind,
        "topic": topic,
        "text_preview": body,
        "signed_bytes": [],
        "delivery_state": delivery,
    }


def _group_topic() -> list:
    """Deterministic 32-byte gossip topic for the seeded group room."""
    return list(bytes(range(32, 64)))


def stress(data_dir: str, now_ms: int | None = None, force: bool = True) -> dict:
    """figure-4 base + long-value overlay: labels, names, messages, events."""
    base = f4.inject(data_dir, now_ms=now_ms, with_today_anchor=False, force=force)
    now = int(now_ms if now_ms is not None else base.get("now_ms") or __import__("time").time() * 1000)
    topic = base["topic"]
    local_pk = base["local_pk_hex"]
    remote_pk = base["remote_pk_hex"]

    # 1) friends.json — two friends, both with very long labels.
    friends_path = os.path.join(data_dir, "friends.json")
    with open(friends_path, "r", encoding="utf-8") as f:
        friends = json.load(f)
    friend_b_pk = "b7c9f0a3e5" + "cd" * 27  # 64-hex
    friends["friends"][remote_pk]["label"] = LONG_LABEL_A
    friends["friends"][friend_b_pk] = {
        "label": LONG_LABEL_B,
        "status": {"online": True, "last_seen_at_unix_ms": now - 90_000},
        "relationship": "friends",
        "known_addrs": [],
    }
    with open(friends_path, "w", encoding="utf-8") as f:
        json.dump(friends, f, indent=2)

    # 2) conversations.json — long-named direct row + long-named group row.
    conv_path = os.path.join(data_dir, "conversations.json")
    with open(conv_path, "r", encoding="utf-8") as f:
        conversations = json.load(f)
    conversations["conversations"][0]["name"] = LONG_LABEL_A
    conversations["conversations"][0]["last_message_preview"] = LONG_PREVIEW
    conversations["conversations"].append(
        {
            "topic": _group_topic(),
            "name": LONG_GROUP_NAME,
            "kind": "Group",
            "created_at_unix_ms": now - 5 * DAY_MS,
            "last_seen_at_unix_ms": now - HOUR_MS,
            "last_message_preview": "Let\u2019s sync the mesh topology changes.",
            "unread_count": 2,
            "archived": False,
        }
    )
    with open(conv_path, "w", encoding="utf-8") as f:
        json.dump(conversations, f, indent=2)

    # 3) chat_history.json — append long-value entries to the figure-4 rows.
    hist_path = os.path.join(data_dir, "chat_history.json")
    with open(hist_path, "r", encoding="utf-8") as f:
        store = json.load(f)
    entries = store["entries"]
    next_id = max((e["event_id"] for e in entries), default=0) + 1
    yesterday = now - DAY_MS
    entries.append(
        _entry(next_id, remote_pk, LONG_URL, yesterday + 8 * HOUR_MS + 1 * MINUTE_MS, topic)
    )
    next_id += 1
    entries.append(
        _entry(next_id, local_pk, LONG_UNBROKEN_WORD, yesterday + 8 * HOUR_MS + 2 * MINUTE_MS,
               topic, delivery="Seen")
    )
    next_id += 1
    entries.append(
        _entry(next_id, remote_pk, LONG_EVENT, yesterday + 8 * HOUR_MS + 3 * MINUTE_MS,
               topic, kind="system", delivery="Sent")
    )
    next_id += 1
    entries.append(
        _entry(
            next_id,
            remote_pk,
            "A normal-length paragraph that still has to coexist with the very "
            "long unbroken token above it, proving bubble width stays capped at "
            "560 px or 68 % of the timeline while long content wraps rather than "
            "overflowing the conversation column.",
            yesterday + 8 * HOUR_MS + 4 * MINUTE_MS,
            topic,
        )
    )
    store["entries"] = entries
    with open(hist_path, "w", encoding="utf-8") as f:
        json.dump(store, f, indent=2)

    return {
        **base,
        "entries": len(entries),
        "long_labels": [LONG_LABEL_A, LONG_LABEL_B],
        "long_group_name": LONG_GROUP_NAME,
        "friend_b_pk": friend_b_pk,
    }


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    sub = parser.add_subparsers(dest="cmd", required=True)

    p_stress = sub.add_parser("stress", help="figure-4 base + long-value overlay")
    p_stress.add_argument("data_dir")
    p_stress.add_argument("--now-ms", type=int, default=None)
    p_stress.add_argument("--force", action="store_true", default=True)

    p_base = sub.add_parser("base", help="plain figure-4 base")
    p_base.add_argument("data_dir")
    p_base.add_argument("--now-ms", type=int, default=None)
    p_base.add_argument("--force", action="store_true", default=True)

    p_clean = sub.add_parser("cleanup", help="remove injected files")
    p_clean.add_argument("data_dir")

    args = parser.parse_args(argv)
    if args.cmd == "stress":
        summary = stress(args.data_dir, now_ms=args.now_ms, force=args.force)
        print(json.dumps(summary, indent=2, sort_keys=True))
    elif args.cmd == "base":
        summary = f4.inject(args.data_dir, now_ms=args.now_ms,
                            with_today_anchor=False, force=args.force)
        print(json.dumps(summary, indent=2, sort_keys=True))
    elif args.cmd == "cleanup":
        removed = f4.cleanup(args.data_dir)
        print(json.dumps({"removed": removed}, indent=2, sort_keys=True))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
