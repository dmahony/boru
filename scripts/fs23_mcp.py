#!/usr/bin/env python3
"""FS-23 helper: query boru MCP over loopback JSON-RPC, print compact JSON."""
from __future__ import annotations
import json, socket, sys

def rpc(port: int, method: str, params: dict) -> dict:
    req = {"jsonrpc": "2.0", "method": method, "params": params, "id": 1}
    payload = (json.dumps(req, separators=(",", ":")) + "\n").encode()
    with socket.create_connection(("127.0.0.1", port), timeout=15) as conn:
        conn.sendall(payload)
        stream = conn.makefile("rb")
        line = stream.readline()
    if not line:
        return {"error": "empty response"}
    return json.loads(line)

def summarize_discovery(events):
    kinds = {}
    for e in events:
        k = e["kind"]
        t = k.get("type", "?")
        kinds.setdefault(t, 0)
        kinds[t] += 1
    return kinds

if __name__ == "__main__":
    port = int(sys.argv[1]); method = sys.argv[2]
    params = json.loads(sys.argv[3]) if len(sys.argv) > 3 else {}
    resp = rpc(port, method, params)
    print(json.dumps(resp, separators=(",", ":"), sort_keys=True))
