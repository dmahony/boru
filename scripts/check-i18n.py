#!/usr/bin/env python3
"""Validate Boru locale dictionaries and interpolation contracts.

The checker is intentionally dependency-free so it can run before Cargo.  It
checks JSON syntax, English/locale key parity, and placeholder parity.  Use
--strict to reject untranslated/missing entries; the default reports findings
without making an existing partial locale unusable.
"""
from __future__ import annotations

import argparse
import json
import re
import sys
from pathlib import Path

PLACEHOLDER = re.compile(r"\{([A-Za-z][A-Za-z0-9_]*)\}")


def load(path: Path) -> dict[str, str]:
    value = json.loads(path.read_text(encoding="utf-8"))
    if not isinstance(value, dict) or not all(isinstance(k, str) and isinstance(v, str) for k, v in value.items()):
        raise ValueError(f"{path}: expected a JSON object of string values")
    return value


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--dir", type=Path, default=Path("src/bin/boru/locales"))
    parser.add_argument("--strict", action="store_true")
    args = parser.parse_args()
    en_path = args.dir / "en.json"
    try:
        english = load(en_path)
    except (OSError, ValueError, json.JSONDecodeError) as exc:
        print(f"ERROR: {exc}", file=sys.stderr)
        return 2
    errors: list[str] = []
    findings: list[str] = []
    for path in sorted(args.dir.glob("*.json")):
        if path.name == "en.json":
            continue
        try:
            locale = load(path)
        except (OSError, ValueError, json.JSONDecodeError) as exc:
            errors.append(str(exc))
            continue
        missing = sorted(set(english) - set(locale))
        extra = sorted(set(locale) - set(english))
        placeholder_mismatch = sorted(
            key for key in set(english) & set(locale)
            if sorted(PLACEHOLDER.findall(english[key])) != sorted(PLACEHOLDER.findall(locale[key]))
        )
        if missing:
            findings.append(f"{path}: missing {len(missing)} keys (first: {', '.join(missing[:5])})")
        if extra:
            errors.append(f"{path}: unknown keys: {', '.join(extra[:5])}")
        if placeholder_mismatch:
            errors.append(f"{path}: placeholder mismatch: {', '.join(placeholder_mismatch[:5])}")
    if errors:
        for item in errors:
            print(f"ERROR: {item}", file=sys.stderr)
        return 1
    for item in findings:
        print(f"FINDING: {item}")
    print(f"OK: English catalogue has {len(english)} keys; checked {len(list(args.dir.glob('*.json')))} locale files")
    return 1 if args.strict and findings else 0


if __name__ == "__main__":
    raise SystemExit(main())
