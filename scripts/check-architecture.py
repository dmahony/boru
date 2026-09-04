#!/usr/bin/env python3
"""Run cheap architecture and dependency boundary checks as JSON.

This is a review gate, not a substitute for Cargo.  It makes the boundaries
machine-readable and gives CI a stable, diffable result before expensive builds.
"""
from __future__ import annotations

import argparse
import json
import re
import subprocess
from pathlib import Path


def command(argv: list[str], cwd: Path) -> dict[str, object]:
    try:
        proc = subprocess.run(argv, cwd=cwd, text=True, capture_output=True, timeout=30)
    except (OSError, subprocess.TimeoutExpired) as exc:
        return {"command": argv, "status": "error", "detail": str(exc)}
    return {"command": argv, "status": "pass" if proc.returncode == 0 else "fail", "exit_code": proc.returncode,
            "stdout": proc.stdout[-2000:], "stderr": proc.stderr[-2000:]}


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--repo", type=Path, default=Path("."))
    parser.add_argument("--output", type=Path)
    args = parser.parse_args()
    root = args.repo.resolve()
    forbidden: list[str] = []
    for path in sorted((root / "src/bin/boru/app").glob("*.rs")):
        text = path.read_text(encoding="utf-8")
        if re.search(r"\buse\s+super::\*", text):
            forbidden.append(str(path.relative_to(root)))
    graph_path = root / "docs/architecture-refactor/dependency-graph.json"
    graph_valid = graph_path.is_file()
    checks: dict[str, dict[str, object]] = {
        "app_domains_forbid_use_super_glob": {"status": "pass" if not forbidden else "fail", "files": forbidden},
        "machine_readable_dependency_graph": {"status": "pass" if graph_valid else "fail", "path": str(graph_path.relative_to(root))},
    }
    commands = [command(["git", "diff", "--check"], root)]
    result: dict[str, object] = {
        "schema": "boru.architecture-gate.v1",
        "repository": str(root),
        "checks": checks,
        "commands": commands,
    }
    result["status"] = "pass" if all(item["status"] == "pass" for item in checks.values()) and all(item["status"] == "pass" for item in commands) else "fail"
    payload = json.dumps(result, indent=2, sort_keys=True) + "\n"
    if args.output:
        args.output.write_text(payload, encoding="utf-8")
    print(payload, end="")
    return 0 if result["status"] == "pass" else 1


if __name__ == "__main__":
    raise SystemExit(main())
