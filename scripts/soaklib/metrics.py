"""Process, process-tree, and profile database metrics using procfs."""
from __future__ import annotations

import pathlib
from typing import Any, Iterable


def _proc_int(path: pathlib.Path, prefix: str) -> int | None:
    try:
        for line in path.read_text(errors="replace").splitlines():
            if line.startswith(prefix):
                return int(line.split()[1])
    except (OSError, ValueError, IndexError):
        return None
    return None


def proc_children(pid: int) -> list[int] | None:
    """Return direct children, or None when procfs is unavailable."""
    path = pathlib.Path(f"/proc/{pid}/task/{pid}/children")
    try:
        return [int(value) for value in path.read_text().split()]
    except (OSError, ValueError):
        return None


def _proc_parent(pid: int) -> int | None:
    return _proc_int(pathlib.Path(f"/proc/{pid}/status"), "PPid:")


def detect_orphan_children(pid: int, expected: Iterable[int] = ()) -> list[int] | None:
    """Find direct children not owned by the expected process set.

    This is intentionally conservative: without procfs, return None so callers
    can report SKIP rather than treating an unsupported metric as healthy.
    """
    children = proc_children(pid)
    if children is None:
        return None
    expected_set = set(expected)
    return [child for child in children if child not in expected_set and _proc_parent(child) in (pid, 1)]


def profile_db_size(data_dir: pathlib.Path) -> int | None:
    """Return bytes for SQLite profile files, or None when the directory is absent."""
    if not data_dir.exists():
        return None
    total = 0
    try:
        for path in data_dir.rglob("*"):
            if path.is_file() and (path.suffix.lower() in {".db", ".sqlite", ".sqlite3"}
                                   or path.name.lower().endswith((".db-wal", ".db-shm"))):
                total += path.stat().st_size
    except OSError:
        return None
    return total


def proc_metrics(pid: int, data_dir: pathlib.Path) -> dict[str, Any]:
    """Return RSS, thread, FD, DB, and child metrics with explicit None support."""
    status = pathlib.Path(f"/proc/{pid}/status")
    rss_kb = _proc_int(status, "VmRSS:")
    threads = _proc_int(status, "Threads:")
    fd_dir = pathlib.Path(f"/proc/{pid}/fd")
    fds: int | None = None
    if fd_dir.exists():
        try:
            fds = len(list(fd_dir.iterdir()))
        except OSError:
            pass
    db_bytes = profile_db_size(data_dir)
    total_bytes: int | None = None
    if data_dir.exists():
        try:
            total_bytes = sum(path.stat().st_size for path in data_dir.rglob("*") if path.is_file())
        except OSError:
            pass
    # Preserve db_bytes as the historical field while exposing its availability.
    return {
        "rss_kb": rss_kb,
        "threads": threads,
        "fds": fds,
        "profile_db_bytes": db_bytes,
        "db_bytes": total_bytes if total_bytes is not None else 0,
        "child_pids": proc_children(pid),
        "orphan_children": detect_orphan_children(pid),
    }
