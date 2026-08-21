#!/usr/bin/env python3
"""Validate a release version against Cargo.toml and an optional Git tag."""
from __future__ import annotations

import argparse
import re
import sys
from pathlib import Path

VERSION_RE = re.compile(r"^v(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)(?:-[0-9A-Za-z.-]+)?(?:\+[0-9A-Za-z.-]+)?$")
CARGO_VERSION_RE = re.compile(r'^version\s*=\s*"([^"]+)"\s*$', re.MULTILINE)


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("version", help="release tag, for example v0.225.0")
    parser.add_argument("--cargo-toml", default="Cargo.toml")
    parser.add_argument("--tag", help="tag to compare with version")
    args = parser.parse_args()

    if not VERSION_RE.fullmatch(args.version):
        print("release version must be a non-empty semantic version tag like v1.2.3", file=sys.stderr)
        return 2
    if args.tag is not None and args.tag != args.version:
        print(f"release tag {args.tag!r} does not match requested version {args.version!r}", file=sys.stderr)
        return 2

    cargo_text = Path(args.cargo_toml).read_text(encoding="utf-8")
    match = CARGO_VERSION_RE.search(cargo_text)
    expected = args.version[1:]
    if match is None or match.group(1) != expected:
        actual = match.group(1) if match else "<missing>"
        print(f"Cargo.toml version {actual!r} does not match {expected!r}", file=sys.stderr)
        return 2

    print(f"release version validated: {args.version}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
