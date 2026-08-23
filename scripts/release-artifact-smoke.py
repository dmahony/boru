#!/usr/bin/env python3
"""Run platform-neutral smoke checks against one Boru release archive.

The output is deliberately JSON so workflow evidence can be consumed without
parsing human-oriented build logs.  A missing required file or failed process
is a failure; checks that cannot run are reported as UNVALIDATED.
"""
from __future__ import annotations

import argparse
import hashlib
import json
import subprocess
import tarfile
import tempfile
import zipfile
from pathlib import Path


def check(name: str, status: str, detail: str) -> dict[str, str]:
    return {"name": name, "status": status, "detail": detail}


def extract(archive: Path, destination: Path) -> None:
    if archive.suffix == ".zip":
        with zipfile.ZipFile(archive) as handle:
            for member in handle.infolist():
                target = (destination / member.filename).resolve()
                if destination.resolve() not in target.parents:
                    raise ValueError(f"archive member escapes extraction directory: {member.filename}")
            handle.extractall(destination)
    elif archive.name.endswith(".tar.gz"):
        with tarfile.open(archive, "r:gz") as handle:
            for member in handle.getmembers():
                target = (destination / member.name).resolve()
                if destination.resolve() not in target.parents:
                    raise ValueError(f"archive member escapes extraction directory: {member.name}")
            handle.extractall(destination)
    else:
        raise ValueError(f"unsupported archive type: {archive.name}")


def run_command(command: list[str], cwd: Path, timeout: int) -> tuple[str, str]:
    try:
        result = subprocess.run(
            command,
            cwd=cwd,
            stdout=subprocess.PIPE,
            stderr=subprocess.STDOUT,
            text=True,
            timeout=timeout,
            check=False,
        )
    except (OSError, subprocess.TimeoutExpired) as exc:
        return "FAIL", str(exc)
    if result.returncode != 0:
        return "FAIL", f"exit {result.returncode}: {result.stdout[-500:]}".strip()
    return "PASS", result.stdout[-500:].strip()


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("archive", type=Path)
    parser.add_argument("--platform", required=True)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--signing", choices=("signed", "unsigned"), default="unsigned")
    args = parser.parse_args()

    checks: list[dict[str, str]] = []
    archive = args.archive
    if not archive.is_file():
        checks.append(check("archive", "FAIL", f"missing archive: {archive}"))
        result = {"schema": 1, "platform": args.platform, "artifact": archive.name, "signing": args.signing, "checks": checks, "status": "FAIL"}
        args.output.write_text(json.dumps(result, indent=2) + "\n", encoding="utf-8")
        print(json.dumps(result, indent=2))
        return 1

    checks.append(check("archive", "PASS", f"{archive.stat().st_size} bytes"))
    with tempfile.TemporaryDirectory(prefix="boru-release-smoke-") as directory:
        root = Path(directory)
        payload = root / "payload"
        payload.mkdir()
        try:
            extract(archive, payload)
            checks.append(check("archive-extract", "PASS", "archive extracted safely"))
        except (OSError, ValueError, tarfile.TarError, zipfile.BadZipFile) as exc:
            checks.append(check("archive-extract", "FAIL", str(exc)))
            payload = None

        if payload is not None:
            files = {path.relative_to(payload).as_posix() for path in payload.rglob("*") if path.is_file()}
            required = {"THIRD_PARTY_NOTICES.md", "assets/third_party/papirus", "assets/emoji/twemoji"}
            missing = [entry for entry in required if not any(path == entry or path.startswith(entry + "/") for path in files)]
            checks.append(check("assets-and-notices", "PASS" if not missing else "FAIL", "required assets present" if not missing else f"missing: {', '.join(missing)}"))

            executable = next((path for path in payload.rglob("boru.exe" if args.platform.startswith("windows") else "boru") if path.is_file()), None)
            if executable is None:
                checks.append(check("executable", "FAIL", "boru executable missing from archive"))
            else:
                checks.append(check("executable", "PASS", str(executable.relative_to(payload))))
                for label, command in (("help", [str(executable), "--help"]), ("version", [str(executable), "--version"])):
                    status, detail = run_command(command, executable.parent, 30)
                    checks.append(check(f"executable-{label}", status, detail))
                support = root / "support-bundle.json"
                status, detail = run_command([str(executable), "support-bundle", str(support)], executable.parent, 30)
                if status == "PASS" and not support.is_file():
                    status, detail = "FAIL", "support-bundle command returned successfully but wrote no output"
                elif status == "PASS":
                    try:
                        document = json.loads(support.read_text(encoding="utf-8"))
                        status = "PASS" if document.get("schema_version") else "FAIL"
                        detail = "headless support bundle exported and parsed" if status == "PASS" else "support bundle lacks schema_version"
                    except (OSError, json.JSONDecodeError) as exc:
                        status, detail = "FAIL", str(exc)
                checks.append(check("support-bundle", status, detail))

    checks.append(check("signing-state", "PASS", args.signing))
    failed = any(item["status"] == "FAIL" for item in checks)
    result = {
        "schema": 1,
        "platform": args.platform,
        "artifact": archive.name,
        "sha256": hashlib.sha256(archive.read_bytes()).hexdigest(),
        "signing": args.signing,
        "checks": checks,
        "status": "FAIL" if failed else "PASS",
    }
    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_text(json.dumps(result, indent=2) + "\n", encoding="utf-8")
    print(json.dumps(result, indent=2))
    return 1 if failed else 0


if __name__ == "__main__":
    raise SystemExit(main())
