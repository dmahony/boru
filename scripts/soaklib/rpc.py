"""Bounded JSON-RPC transport used by soak actions."""
from __future__ import annotations

import json
import socket
from typing import Any


def port_open(port: int, timeout: float = 0.2) -> bool:
    with socket.socket() as sock:
        sock.settimeout(timeout)
        return sock.connect_ex(("127.0.0.1", port)) == 0


def call(port: int, method: str, params: dict[str, Any] | None = None, timeout: float = 5.0) -> dict[str, Any]:
    request = {"jsonrpc": "2.0", "method": method, "params": params or {}, "id": 1}
    with socket.create_connection(("127.0.0.1", port), timeout=timeout) as conn:
        conn.settimeout(timeout)
        conn.sendall((json.dumps(request, separators=(",", ":")) + "\n").encode())
        line = conn.makefile("rb").readline()
    if not line:
        raise RuntimeError("empty MCP response")
    value = json.loads(line)
    return value if isinstance(value, dict) else {"result": value}


rpc = call
