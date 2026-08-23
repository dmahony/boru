"""Process and filesystem metrics without third-party dependencies."""
from __future__ import annotations

import pathlib
from typing import Any


def proc_metrics(pid: int, data_dir: pathlib.Path) -> dict[str, Any]:
    result: dict[str, Any] = {"rss_kb": None, "threads": None, "fds": None, "db_bytes": 0}
    status = pathlib.Path(f"/proc/{pid}/status")
    if status.exists():
        for line in status.read_text(errors="replace").splitlines():
            if line.startswith("VmRSS:"):
                result["rss_kb"] = int(line.split()[1])
            elif line.startswith("Threads:"):
                result["threads"] = int(line.split()[1])
    fd_dir = pathlib.Path(f"/proc/{pid}/fd")
    if fd_dir.exists():
        try:
            result["fds"] = len(list(fd_dir.iterdir()))
        except OSError:
            pass
    if data_dir.exists():
        result["db_bytes"] = sum(p.stat().st_size for p in data_dir.rglob("*") if p.is_file())
    return result
