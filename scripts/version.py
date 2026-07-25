#!/usr/bin/env python3
"""
Boru version management.

Calculates the next semantic version from conventional commits since the
last recorded version change, and applies it to Cargo.toml.

Commands:
  check              Print proposed version without modifying files.
  apply              Update Cargo.toml and .version-state.json.
  apply --dry-run    Show what would change without writing.
  initialise         Create .version-state.json at the current commit.
"""

from __future__ import annotations

import argparse
import json
import re
import subprocess
import sys
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parent.parent
CARGO_TOML = REPO_ROOT / "Cargo.toml"
STATE_FILE = REPO_ROOT / ".version-state.json"

# Bump levels (higher = more significant)
BUMP_NONE = 0
BUMP_PATCH = 1
BUMP_MINOR = 2
BUMP_MAJOR = 3

# Ignore commits matching these patterns (version-bump commits)
_IGNORE_PATTERNS = [
    re.compile(r"^chore:\s*bump\s+version", re.IGNORECASE),
    re.compile(r"^chore:\s*bump\s+boru\s+version", re.IGNORECASE),
    re.compile(r"^chore\(release\):", re.IGNORECASE),
]


# ── Version helpers ──────────────────────────────────────────────────────


def parse_version(text: str) -> tuple[int, int, int]:
    """Parse 'MAJOR.MINOR.PATCH' from a string, optionally 'v'-prefixed."""
    text = text.removeprefix("v").strip()
    parts = text.split(".")
    return (int(parts[0]), int(parts[1]), int(parts[2]))


def format_version(v: tuple[int, int, int]) -> str:
    return f"{v[0]}.{v[1]}.{v[2]}"


def bump_patch(v: tuple[int, int, int]) -> tuple[int, int, int]:
    return (v[0], v[1], v[2] + 1)


def bump_minor(v: tuple[int, int, int]) -> tuple[int, int, int]:
    return (v[0], v[1] + 1, 0)


def bump_major(v: tuple[int, int, int]) -> tuple[int, int, int]:
    return (v[0] + 1, 0, 0)


# ── Cargo.toml I/O ──────────────────────────────────────────────────────


def read_cargo_version() -> tuple[int, int, int]:
    """Extract the current version from Cargo.toml."""
    text = CARGO_TOML.read_text()
    m = re.search(r'^version\s*=\s*"([^"]+)"', text, re.MULTILINE)
    if not m:
        print("ERROR: could not find version in Cargo.toml", file=sys.stderr)
        sys.exit(1)
    return parse_version(m.group(1))


def update_cargo_version(new_version: str, dry_run: bool = False) -> None:
    """Replace the version field in Cargo.toml."""
    text = CARGO_TOML.read_text()
    new_text = re.sub(
        r'^version\s*=\s*"[^"]+"',
        f'version = "{new_version}"',
        text,
        count=1,
        flags=re.MULTILINE,
    )
    if dry_run:
        print(f"  Cargo.toml: version → {new_version}")
    else:
        CARGO_TOML.write_text(new_text)
        print(f"  Cargo.toml: updated to {new_version}")


# ── State file I/O ──────────────────────────────────────────────────────


def read_state() -> dict | None:
    """Read .version-state.json, return None if missing."""
    if not STATE_FILE.exists():
        return None
    return json.loads(STATE_FILE.read_text())


def write_state(version: str, commit: str, dry_run: bool = False) -> None:
    """Write .version-state.json."""
    data = {"version": version, "commit": commit}
    if dry_run:
        print(f"  .version-state.json: {json.dumps(data)}")
    else:
        STATE_FILE.write_text(json.dumps(data, indent=2) + "\n")
        print(f"  .version-state.json: version={version}, commit={commit[:12]}")


# ── Git helpers ─────────────────────────────────────────────────────────


def git(*args: str) -> str:
    """Run a git command and return stdout, stripping trailing whitespace."""
    try:
        result = subprocess.run(
            ["git"] + list(args),
            capture_output=True,
            text=True,
            check=True,
            cwd=REPO_ROOT,
        )
    except subprocess.CalledProcessError as e:
        print(f"ERROR: git {' '.join(args)} failed:\n{e.stderr}", file=sys.stderr)
        sys.exit(1)
    return result.stdout.strip()


def get_current_commit() -> str:
    return git("rev-parse", "HEAD")


def log_since(commit: str) -> list[dict]:
    """Return parsed commits since (but not including) the given commit.

    Each entry: {hash, subject, body}.
    Body is the full commit message minus the subject line.
    """
    raw = git("log", f"{commit}..HEAD", "--format=%H%n%B%x1e", "--no-merges")
    if not raw:
        return []
    entries: list[dict] = []
    for block in raw.split("\x1e"):
        block = block.strip()
        if not block:
            continue
        lines = block.split("\n", 1)
        h = lines[0].strip()
        rest = lines[1].strip() if len(lines) > 1 else ""
        # Subject = first line of rest
        rest_lines = rest.split("\n")
        subject = rest_lines[0].strip() if rest_lines else ""
        body = "\n".join(rest_lines[1:]).strip() if len(rest_lines) > 1 else ""
        entries.append({"hash": h, "subject": subject, "body": body})
    return entries


# ── Commit classification ───────────────────────────────────────────────


def classify_commit(subject: str, body: str) -> int:
    """Return the bump level implied by a commit message.

    Returns one of BUMP_NONE, BUMP_PATCH, BUMP_MINOR, BUMP_MAJOR.

    Breaking changes detected via:
      - trailing `!` before `:`  (e.g. "feat!: ...")
      - scope with `!`           (e.g. "feat(scope)!: ...")
      - BREAKING CHANGE or BREAKING-CHANGE in body/footer
    """
    # Ignore version-bump commits
    for p in _IGNORE_PATTERNS:
        if p.match(subject):
            return BUMP_NONE

    # Check for breaking change marker
    is_breaking = bool(
        re.search(r"^\w+(\([^)]*\))?!:", subject)
        or re.search(
            r"^BREAKING[-\s]CHANGE:",
            (body or ""),
            re.MULTILINE | re.IGNORECASE,
        )
    )

    # Extract the conventional commit type
    kind_match = re.match(r"^(\w+)", subject)
    if not kind_match:
        return BUMP_NONE

    kind = kind_match.group(1).lower()

    if is_breaking:
        return BUMP_MAJOR
    if kind == "feat":
        return BUMP_MINOR
    if kind in ("fix", "perf", "refactor", "revert"):
        return BUMP_PATCH
    # docs, style, test, ci, chore, build -> no bump
    return BUMP_NONE


def compute_bump(commits: list[dict]) -> tuple[int, list[str]]:
    """Determine the highest bump level and list of trigger messages."""
    max_bump = BUMP_NONE
    triggers: list[str] = []
    for c in commits:
        level = classify_commit(c["subject"], c["body"])
        if level > max_bump:
            max_bump = level
            triggers = [c["subject"]]
        elif level == max_bump and level > BUMP_NONE:
            triggers.append(c["subject"])
    return max_bump, triggers


# ── Version bump resolution ─────────────────────────────────────────────


def resolve_next_version(
    current: tuple[int, int, int], bump: int
) -> tuple[int, int, int]:
    """Map a bump level to the actual version increment.

    For versions < 1.0.0:
      - breaking (MAJOR) → minor
      - feat → minor
      - fix/patch → patch

    For versions >= 1.0.0:
      - breaking → major
      - feat → minor
      - fix/patch → patch
    """
    is_pre_1_0 = current[0] == 0

    if bump == BUMP_MAJOR:
        if is_pre_1_0:
            return bump_minor(current)
        return bump_major(current)
    if bump == BUMP_MINOR:
        return bump_minor(current)
    if bump == BUMP_PATCH:
        return bump_patch(current)
    return current


# ── Commands ─────────────────────────────────────────────────────────────


def cmd_check(dry_run: bool = False) -> None:
    """Check proposed version and print results."""
    state = read_state()
    if state is None:
        print("ERROR: .version-state.json not found.", file=sys.stderr)
        print(
            "Run 'python scripts/version.py initialise' to create it.",
            file=sys.stderr,
        )
        sys.exit(1)

    current = read_cargo_version()
    version_str = format_version(current)
    last_commit = state.get("commit", "")

    if not last_commit:
        print("ERROR: .version-state.json has no commit.", file=sys.stderr)
        sys.exit(1)

    commits = log_since(last_commit)

    if not commits:
        print(f"Current version:  {version_str}")
        print(f"Proposed version: {version_str} (no changes since last version)")
        return

    bump, triggers = compute_bump(commits)
    next_ver = resolve_next_version(current, bump)
    next_str = format_version(next_ver)
    same = current == next_ver

    print(f"Current version:  {version_str}")
    print(f"Proposed version: {next_str if not same else f'{version_str} (no change)'}")
    print()

    bump_name = {BUMP_NONE: "none", BUMP_PATCH: "patch", BUMP_MINOR: "minor", BUMP_MAJOR: "breaking"}
    print(f"Bump type:        {bump_name.get(bump, 'unknown')}")

    if triggers:
        print()
        print("Triggering commits:")
        for t in triggers:
            print(f"  - {t}")

    # Build the explanatory message for GitHub summary
    if not same:
        print()
        print("Explanation:")
        print(f"  {version_str} → {next_str}")
        for t in triggers:
            print(f"    {t}")


def cmd_apply(dry_run: bool = False) -> None:
    """Calculate next version and update files."""
    state = read_state()
    if state is None:
        print("ERROR: .version-state.json not found.", file=sys.stderr)
        print(
            "Run 'python scripts/version.py initialise' to create it.",
            file=sys.stderr,
        )
        sys.exit(1)

    current = read_cargo_version()
    version_str = format_version(current)
    last_commit = state.get("commit", "")

    if not last_commit:
        print("ERROR: .version-state.json has no commit.", file=sys.stderr)
        sys.exit(1)

    commits = log_since(last_commit)
    bump, triggers = compute_bump(commits)
    next_ver = resolve_next_version(current, bump)
    next_str = format_version(next_ver)

    if current == next_ver:
        print(f"Version unchanged ({version_str}) — nothing to apply.")
        return

    head = get_current_commit()

    print(f"Current version:  {version_str}")
    print(f"Proposed version: {next_str}")
    print(f"Bump:             {['none', 'patch', 'minor', 'breaking'][bump]}")
    print()
    print("Changes to apply:")

    update_cargo_version(next_str, dry_run=dry_run)
    write_state(next_str, head, dry_run=dry_run)

    if dry_run:
        print()
        print("(dry run — no files modified)")
    else:
        print()
        print(f"Version updated from {version_str} to {next_str}.")
        print("Review the diff, then commit:")


def cmd_initialise() -> None:
    """Create .version-state.json at the current commit."""
    if STATE_FILE.exists():
        print(
            f".version-state.json already exists. Delete it first to re-initialise.",
            file=sys.stderr,
        )
        sys.exit(1)

    version = read_cargo_version()
    version_str = format_version(version)
    commit = get_current_commit()

    write_state(version_str, commit)
    print()
    print(
        f"Initialised .version-state.json: version={version_str}, "
        f"commit={commit[:12]}"
    )
    print("Future version checks will inspect commits after this one.")


# ── CLI entry point ─────────────────────────────────────────────────────


def main() -> None:
    parser = argparse.ArgumentParser(
        description="Boru version management",
    )
    parser.add_argument(
        "command",
        choices=["check", "apply", "initialise"],
        help="Command to run",
    )
    parser.add_argument(
        "--dry-run",
        action="store_true",
        help="Show what would change without modifying files (for 'apply')",
    )

    args = parser.parse_args()

    if args.command == "check":
        cmd_check(dry_run=args.dry_run)
    elif args.command == "apply":
        cmd_apply(dry_run=args.dry_run)
    elif args.command == "initialise":
        if args.dry_run:
            print("--dry-run is not supported for 'initialise'", file=sys.stderr)
            sys.exit(1)
        cmd_initialise()


if __name__ == "__main__":
    main()
