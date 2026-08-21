#!/usr/bin/env python3
"""Fail closed unless every supported release target has smoke evidence."""
from __future__ import annotations

import argparse
import hashlib
import json
from pathlib import Path


TARGETS = {
    "linux-x86_64": "boru-linux-x86_64.tar.gz",
    "windows-x86_64": "boru-windows-x86_64.zip",
    "macos-arm64": "boru-macos-aarch64.tar.gz",
}


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("directory", type=Path)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()

    evidence: list[dict[str, object]] = []
    for platform, artifact_name in TARGETS.items():
        artifact = args.directory / artifact_name
        base_name = artifact_name.removesuffix(".tar.gz").removesuffix(".zip")
        smoke = args.directory / f"{base_name}-smoke.json"
        if not artifact.is_file():
            evidence.append({"platform": platform, "status": "UNVALIDATED", "detail": f"missing artifact: {artifact_name}"})
            continue
        if not smoke.is_file():
            evidence.append({"platform": platform, "status": "UNVALIDATED", "detail": f"missing smoke evidence: {smoke.name}"})
            continue
        try:
            document = json.loads(smoke.read_text(encoding="utf-8"))
        except (OSError, json.JSONDecodeError) as exc:
            evidence.append({"platform": platform, "status": "FAIL", "detail": str(exc)})
            continue
        expected_hash = hashlib.sha256(artifact.read_bytes()).hexdigest()
        status = document.get("status")
        if document.get("platform") != platform or document.get("artifact") != artifact_name:
            status, detail = "FAIL", "smoke evidence identity does not match artifact"
        elif document.get("sha256") != expected_hash:
            status, detail = "FAIL", "smoke evidence checksum does not match artifact"
        elif status != "PASS":
            status, detail = "FAIL", f"smoke status is {status!r}"
        else:
            detail = "smoke evidence and archive checksum match"
        evidence.append({"platform": platform, "status": status, "detail": detail, "evidence": smoke.name})

    result = {"schema": 1, "status": "PASS" if all(item["status"] == "PASS" for item in evidence) else "FAIL", "targets": evidence}
    args.output.write_text(json.dumps(result, indent=2) + "\n", encoding="utf-8")
    print(json.dumps(result, indent=2))
    return 0 if result["status"] == "PASS" else 1


if __name__ == "__main__":
    raise SystemExit(main())
