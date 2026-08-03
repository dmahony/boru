#!/usr/bin/env python3
"""UI-16 evidence fixture helpers (kanban t_dfbde136).

Builds deterministic chat data states for the chat-footer / composition
evidence captures (empty / one-message / long-history) on top of
`figure4_fixture`'s identity + friends + conversations injection.

QA only: writes exclusively into the data directory passed in and refuses to
overwrite directories that already hold app data (same guard as
figure4_fixture). Cleanup is delegated to `figure4_fixture.py cleanup`.
"""
from __future__ import annotations

import argparse
import json
import os
import sys
import time

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import figure4_fixture as f4

DAY_MS = 86_400_000
HOUR_MS = 3_600_000
MINUTE_MS = 60_000


def _entry(eid: int, sender_hex: str, body: str, ts: int, topic: list) -> dict:
    """One chat_history text entry in the schema the app writes."""
    return {
        "event_id": eid,
        "hash": format(eid, "064x"),
        "sender": sender_hex,
        "timestamp": ts,
        "kind": "text",
        "topic": topic,
        "text_preview": body,
        "signed_bytes": [],
        "delivery_state": "Delivered",
    }


def _write_history(data_dir: str, entries: list) -> str:
    path = os.path.join(data_dir, "chat_history.json")
    with open(path, "w", encoding="utf-8") as f:
        json.dump({"schema_version": 1, "entries": entries}, f, indent=2)
    return path


def _base(data_dir: str, now_ms: int, force: bool) -> dict:
    return f4.inject(
        data_dir,
        now_ms=now_ms,
        with_today_anchor=False,
        force=force,
    )


def empty(data_dir: str, now_ms: int | None = None, force: bool = True) -> dict:
    """Fresh direct conversation with NO history (empty timeline state)."""
    summary = _base(data_dir, now_ms if now_ms is not None else int(time.time() * 1000), force)
    _write_history(data_dir, [])
    return summary


def one(data_dir: str, now_ms: int | None = None, force: bool = True) -> dict:
    """One-message conversation (shortest non-empty timeline)."""
    now = now_ms if now_ms is not None else int(time.time() * 1000)
    summary = _base(data_dir, now, force)
    ts = now - DAY_MS + 10 * HOUR_MS + 31 * MINUTE_MS
    _write_history(
        data_dir,
        [_entry(1, summary["remote_pk_hex"], "Hi! A single-message conversation.", ts, summary["topic"])],
    )
    return summary


def long(data_dir: str, n: int = 400, now_ms: int | None = None, force: bool = True) -> dict:
    """Long history (default 400 messages, ~4 minutes apart, alternating sides)."""
    now = now_ms if now_ms is not None else int(time.time() * 1000)
    summary = _base(data_dir, now, force)
    local = summary["local_pk_hex"]
    remote = summary["remote_pk_hex"]
    topic = summary["topic"]
    entries = []
    for i in range(1, n + 1):
        ts = now - DAY_MS + 9 * HOUR_MS + i * 4 * MINUTE_MS
        sender = remote if i % 2 == 0 else local
        body = f"Long-history message {i}: verifying the timeline scrolls cleanly and stays pinned to the latest message with the composer and status footer below."
        entries.append(_entry(i, sender, body, ts, topic))
    _write_history(data_dir, entries)
    summary["entries"] = len(entries)
    return summary


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(prog="ui16_fixture", description=__doc__)
    sub = parser.add_subparsers(dest="command", required=True)
    for name in ("empty", "one"):
        p = sub.add_parser(name, help=f"inject the {name} conversation into <data_dir>")
        p.add_argument("data_dir")
        p.add_argument("--now-ms", type=int, default=None)
    p_long = sub.add_parser("long", help="inject a long history into <data_dir>")
    p_long.add_argument("data_dir")
    p_long.add_argument("--count", type=int, default=400)
    p_long.add_argument("--now-ms", type=int, default=None)
    args = parser.parse_args(argv)

    now = args.now_ms if args.now_ms is not None else int(time.time() * 1000)
    if args.command == "empty":
        summary = empty(args.data_dir, now)
        print(f"empty conversation injected into {summary['data_dir']}")
    elif args.command == "one":
        summary = one(args.data_dir, now)
        print(f"one-message conversation injected into {summary['data_dir']}")
    elif args.command == "long":
        summary = long(args.data_dir, args.count, now)
        print(f"long history ({summary['entries']} entries) injected into {summary['data_dir']}")
    else:
        parser.error(f"unknown command: {args.command}")
        return 2
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
