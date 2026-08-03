#!/usr/bin/env python3
"""Minimal loopback JSON-RPC client for the Boru UI screenshot harness.

This intentionally uses only the Python standard library and prints one compact
JSON response. It never writes credentials or application state.
"""
from __future__ import annotations

import json
import socket
import sys


def main() -> int:
    if len(sys.argv) != 4:
        print("usage: ui_mcp.py PORT METHOD PARAMS_JSON", file=sys.stderr)
        return 2
    port = int(sys.argv[1])
    method = sys.argv[2]
    params = json.loads(sys.argv[3])
    request = {"jsonrpc": "2.0", "method": method, "params": params, "id": 1}
    payload = (json.dumps(request, separators=(",", ":")) + "\n").encode()
    with socket.create_connection(("127.0.0.1", port), timeout=5) as conn:
        conn.sendall(payload)
        stream = conn.makefile("rb")
        line = stream.readline()
    if not line:
        print("empty MCP response", file=sys.stderr)
        return 1
    response = json.loads(line)
    if "error" in response:
        print(json.dumps(response, sort_keys=True), file=sys.stderr)
        return 1
    print(json.dumps(response, separators=(",", ":")))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
