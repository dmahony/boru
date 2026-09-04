#!/usr/bin/env python3
"""Produce a machine-readable release-candidate gate report.

Cargo checks are deliberately invoked through ``rb`` when --run is supplied;
this keeps expensive compilation on DEBSRV.  A failed gate always yields a
non-ready disposition.
"""
from __future__ import annotations

import argparse
import json
import subprocess
from datetime import datetime, timezone
from pathlib import Path


def run(argv: list[str], root: Path, timeout: int = 900) -> dict[str, object]:
    try:
        proc = subprocess.run(argv, cwd=root, text=True, capture_output=True, timeout=timeout)
        status = "pass" if proc.returncode == 0 else "fail"
        return {"command": " ".join(argv), "status": status, "exit_code": proc.returncode,
                "stdout": proc.stdout[-4000:], "stderr": proc.stderr[-4000:]}
    except (OSError, subprocess.TimeoutExpired) as exc:
        return {"command": " ".join(argv), "status": "error", "detail": str(exc)}


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--repo", type=Path, default=Path("."))
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--run", action="store_true", help="run DEBSRV gates through rb")
    args = parser.parse_args()
    root = args.repo.resolve()
    checks: list[dict[str, object]] = []
    checks.append(run(["git", "diff", "--check"], root))
    checks.append(run(["python3", "scripts/check-i18n.py"], root))
    checks.append(run(["python3", "scripts/check-architecture.py"], root))
    if args.run:
        checks.append(run(["rb", "check", "--locked", "--bin", "boru", "--features", "gui,video-playback,terminal"], root))
        checks.append(run(["rb", "test", "--locked", "--lib"], root, timeout=1200))
        checks.append(run(["rb", "check", "--locked", "--no-default-features", "--features", "net,metrics"], root))
    else:
        checks.append({"command": "rb check --locked --bin boru --features gui,video-playback,terminal", "status": "skip", "reason": "rerun with --run on DEBSRV"})
        checks.append({"command": "rb test --locked --lib", "status": "skip", "reason": "rerun with --run on DEBSRV"})
        checks.append({"command": "rb check --locked --no-default-features --features net,metrics", "status": "skip", "reason": "rerun with --run on DEBSRV"})
    sha = run(["git", "rev-parse", "HEAD"], root)
    report = {
        "schema": "boru.release-candidate.v1",
        "generated_at": datetime.now(timezone.utc).replace(microsecond=0).isoformat().replace("+00:00", "Z"),
        "candidate_sha": subprocess.check_output(["git", "rev-parse", "HEAD"], cwd=root, text=True).strip(),
        "checks": checks,
        "disposition": "release-ready" if all(item["status"] == "pass" for item in checks) else "not-release-ready",
        "rollback": "retain prior signed release; do not promote candidate; preserve this report",
        "sha_lookup": sha,
    }
    args.output.write_text(json.dumps(report, indent=2, sort_keys=True) + "\n", encoding="utf-8")
    print(json.dumps(report, indent=2, sort_keys=True))
    return 0 if report["disposition"] == "release-ready" else 1


if __name__ == "__main__":
    raise SystemExit(main())
