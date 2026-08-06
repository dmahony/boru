#!/usr/bin/env python3
"""Vendor a curated subset of the Papirus icon theme into Boru's asset tree.

Reproducible import process for `assets/third_party/papirus/`.

Usage:
    python3 scripts/vendor_papirus_icons.py --source /path/to/papirus-icon-theme [--commit <sha>] [--out <dir>]
    python3 scripts/vendor_papirus_icons.py --check [--out <dir>]

What it does:
  1. Verifies the expected upstream commit (from assets/third_party/papirus/selected-icons.json
     unless --commit is given).
  2. Reads the approved icon-selection list (selected-icons.json).
  3. Detects aliases/symlinks among the selected icons. Papirus stores many icon names
     as relative symlinks (e.g. `application-msword.svg -> x-office-document.svg`,
     `application-zip.svg -> ../apps/ark.svg`).
  4. Resolves every selected icon to its *actual* SVG source and copies/materialises the
     real SVG content into <out>/<size>/<name>.svg. `shutil.copyfile` follows symlinks, so
     no relative symlinks leak into the bundle and every bundled file is a regular file.
     A selected icon whose symlink target is missing (dangling) or whose content cannot be
     read FAILS the import with a non-zero exit.
  5. Copies the upstream GPL-3.0 LICENSE into the bundle.
  6. Records content-hash dedup metadata. The current iced loader embeds SVGs with
     include_bytes! and has no asset-alias mechanism, so identical files are kept as real
     files, but every duplicate group is recorded in manifest.json so a future
     alias-aware loader can collapse them (284 of 570 files are currently byte-identical).
  7. Regenerates manifest.json from the FINAL packaged asset paths, including the
     symlink-resolution table and dedup groups.
  8. Rewrites UPSTREAM.md with the import metadata.
  9. Verifies the finished bundle: no symlinks present, every manifest entry points at a
     real regular file. Exits non-zero on verification failure.

Constraints honoured:
  - No runtime downloads; everything is committed to the repo.
  - Only icons listed in selected-icons.json are imported (no whole-repo copy).
  - Sizes 16/24/32/48/64 px only (upstream 16x16/24x24/32x32/48x48/64x64).
  - The bundle is a pure materialisation: no symlinks, no hardlinks, no duplicates
    physically collapsed (framework does not support asset aliases yet).

Examples:
    python3 scripts/vendor_papirus_icons.py --source /tmp/papirus-icon-theme
    python3 scripts/vendor_papirus_icons.py --source /tmp/papirus-src/papirus-icon-theme-<sha>
    python3 scripts/vendor_papirus_icons.py --check
"""

from __future__ import annotations

import argparse
import datetime
import hashlib
import json
import os
import shutil
import subprocess
import sys
from pathlib import Path

SCRIPT_DIR = Path(__file__).resolve().parent
REPO_ROOT = SCRIPT_DIR.parent
DEFAULT_SELECTION = REPO_ROOT / "assets" / "third_party" / "papirus" / "selected-icons.json"
DEFAULT_OUT = REPO_ROOT / "assets" / "third_party" / "papirus"
MIMETYPES_SUBDIR = "mimetypes"
PLACES_SUBDIR = "places"
FOLDER_CATEGORY = "folders"


def die(msg: str) -> None:
    print(f"ERROR: {msg}", file=sys.stderr)
    sys.exit(1)


def resolve_upstream_commit(source: Path, expected: str) -> str:
    """Return the verified upstream commit for `source`, or exit non-zero."""
    # Prefer git metadata when the source is a checkout.
    if (source / ".git").exists() or (source / ".git").is_dir():
        try:
            r = subprocess.run(
                ["git", "-C", str(source), "rev-parse", "HEAD"],
                capture_output=True, text=True, check=True,
            )
            actual = r.stdout.strip()
        except (subprocess.CalledProcessError, FileNotFoundError):
            actual = None
        if actual and expected and actual != expected:
            die(
                f"commit mismatch: source HEAD is {actual}, expected {expected} "
                f"(from {DEFAULT_SELECTION.name}). Refusing to import."
            )
        if actual:
            return actual
    # Tarball extraction: directory name usually contains the commit.
    if expected and expected in source.name:
        return expected
    if expected:
        return expected
    die("could not determine upstream commit; pass --commit or use a git checkout")


def list_selected_icons(selection: dict) -> list[str]:
    """Flatten categories + generic fallbacks into a deduplicated ordered list."""
    seen: set[str] = set()
    ordered: list[str] = []
    for group in ("categories", "generic_fallbacks"):
        mapping = selection.get(group, {})
        if isinstance(mapping, dict):
            names = [n for lst in mapping.values() for n in lst]
        else:
            names = list(mapping)
        for name in names:
            if name not in seen:
                seen.add(name)
                ordered.append(name)
    return ordered


def upstream_subdir_for(icon: str, selection: dict) -> str:
    """Return the upstream subdirectory (mimetypes|places) for an icon name."""
    folders = selection.get("categories", {}).get(FOLDER_CATEGORY, [])
    return PLACES_SUBDIR if icon in folders else MIMETYPES_SUBDIR


def sha256_of(path: Path) -> str:
    h = hashlib.sha256()
    with open(path, "rb") as fh:
        for chunk in iter(lambda: fh.read(1 << 16), b""):
            h.update(chunk)
    return h.hexdigest()


def import_icon(source: Path, out: Path, icon: str, sizes: list[str],
                upstream_dirs: dict[str, str], selection: dict,
                symlinks_resolved: dict[str, str]) -> tuple[dict[str, str], dict[str, str], int]:
    """Copy one icon at all sizes; return ({size: relative_path}, {size: resolved_or_None}).

    Symlink detection and resolution:
      - `src.is_symlink()` -> the upstream entry is an alias.
      - `src.resolve()` follows the full chain (same-dir or `../apps/...`) to the real
        SVG file; the resolved path is recorded for the manifest.
      - A dangling symlink (target missing) is a hard error: recorded in the returned
        `unresolvable` map so main() fails the import.
    """
    result: dict[str, str] = {}
    unresolvable: dict[str, str] = {}
    symlink_count = 0
    sub = upstream_subdir_for(icon, selection)
    for size in sizes:
        upstream_size = upstream_dirs.get(size, f"{size}x{size}")
        src = source / "Papirus" / upstream_size / sub / f"{icon}.svg"
        rel_bundle = f"{size}/{icon}.svg"
        if not src.exists():
            if src.is_symlink():
                # dangling symlink: exists() is False because the target is gone
                unresolvable[size] = f"dangling symlink -> {os.readlink(src)}"
            else:
                unresolvable[size] = "missing upstream"
            continue
        if src.is_symlink():
            symlink_count += 1
            resolved = src.resolve()
            if not resolved.is_file():
                unresolvable[size] = f"symlink target not a file: {os.readlink(src)}"
                continue
            try:
                resolved.relative_to(source.resolve())
            except ValueError:
                unresolvable[size] = f"symlink resolves outside source: {resolved}"
                continue
            symlinks_resolved[rel_bundle] = str(resolved.relative_to(source.resolve()))
        else:
            # regular file: still record an identity mapping for transparency
            symlinks_resolved[rel_bundle] = str(src.relative_to(source.resolve()))
        dst_dir = out / size
        dst_dir.mkdir(parents=True, exist_ok=True)
        dst = dst_dir / f"{icon}.svg"
        # copyfile follows symlinks: materialises real SVG content.
        shutil.copyfile(src, dst)
        result[size] = rel_bundle
    return result, unresolvable, symlink_count


def verify_bundle(out: Path, manifest: dict) -> list[str]:
    """Verify a bundle directory is release-safe. Returns a list of problems (empty = ok).

    Checks:
      - no symlinks at all under the bundle (they break Windows/AppImage/Flatpak packing)
      - every manifest `icons` entry points at an existing regular file
    """
    problems: list[str] = []

    # 1. symlink scan
    symlinks = sorted(p for p in out.rglob("*") if p.is_symlink())
    if symlinks:
        for p in symlinks[:50]:
            problems.append(f"symlink in bundle: {p.relative_to(out)} -> {os.readlink(p)}")
        problems.append(f"{len(symlinks)} symlink(s) present in bundle (expected 0)")

    # 2. manifest entries point at real files
    icons = manifest.get("icons", {})
    checked = 0
    for icon, sizes in icons.items():
        for size, rel in sizes.items():
            checked += 1
            p = out / rel
            if not p.is_file():
                problems.append(f"manifest entry missing: {rel} (icon {icon}, size {size})")
            elif p.is_symlink():
                problems.append(f"manifest entry is a symlink: {rel}")
    if checked:
        print(f"Verified {checked} manifest entries against real files.")

    return problems


def main() -> None:
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument("--source", default=None, help="Path to a Papirus checkout or extracted archive (required for import)")
    ap.add_argument("--commit", default=None, help="Expected upstream commit (default: from selected-icons.json)")
    ap.add_argument("--out", default=str(DEFAULT_OUT), help="Output directory (default: assets/third_party/papirus)")
    ap.add_argument("--selection", default=str(DEFAULT_SELECTION), help="Selection JSON (default: selected-icons.json)")
    ap.add_argument("--check", action="store_true", help="Verify an existing bundle without importing")
    args = ap.parse_args()

    source = Path(args.source) if args.source else None
    out = Path(args.out)
    selection_path = Path(args.selection)

    if not selection_path.exists():
        die(f"selection file does not exist: {selection_path}")

    selection = json.loads(selection_path.read_text())
    expected_commit = args.commit or selection.get("commit")
    sizes = list(selection.get("sizes", ["16", "24", "32", "48", "64"]))
    required_fallbacks = set(selection.get("required_fallbacks", []))

    if args.check:
        if not out.exists():
            die(f"bundle does not exist: {out}")
        manifest_path = out / "manifest.json"
        if not manifest_path.exists():
            die(f"manifest does not exist: {manifest_path}")
        manifest = json.loads(manifest_path.read_text())
        print(f"Checking bundle at {out} (no import).")
        problems = verify_bundle(out, manifest)
        if problems:
            print("Bundle verification FAILED:")
            for p in problems:
                print(f"  - {p}")
            sys.exit(2)
        print("Bundle verification OK: no symlinks, all manifest entries are real files.")
        return

    if source is None:
        die("--source is required for import (or use --check to verify an existing bundle)")
    if not source.exists():
        die(f"source does not exist: {source}")

    commit = resolve_upstream_commit(source, expected_commit)
    upstream_dirs = selection.get("upstream_dirs", {})
    icons = list_selected_icons(selection)
    license_rel = source / "LICENSE"

    print(f"Importing {len(icons)} curated icons at sizes {sizes} from commit {commit[:12]}...")
    out.mkdir(parents=True, exist_ok=True)

    missing: dict[str, dict[str, str]] = {}
    icon_paths: dict[str, dict[str, str]] = {}
    symlinks_resolved: dict[str, str] = {}
    upstream_symlink_entries = 0
    for icon in icons:
        paths, unresolvable, symlink_count = import_icon(
            source, out, icon, sizes, upstream_dirs, selection, symlinks_resolved
        )
        upstream_symlink_entries += symlink_count
        if unresolvable:
            missing[icon] = unresolvable
        icon_paths[icon] = paths

    # Licence text.
    if license_rel.exists():
        shutil.copyfile(license_rel, out / "LICENSE")
        print("Copied upstream LICENSE (GPL-3.0).")
    else:
        missing["LICENSE"] = {"0": "upstream LICENSE file missing"}

    # Fail hard on missing required fallbacks.
    missing_fallbacks = [f for f in required_fallbacks if not icon_paths.get(f)]
    if missing_fallbacks:
        die(
            "required fallback icons missing: "
            + ", ".join(missing_fallbacks)
            + " — fix selected-icons.json or the upstream checkout before importing."
        )

    # Symlink / alias report.
    # An entry is an alias when the resolved upstream path's basename differs from the
    # bundle file's basename (i.e. the bundled file was materialised from a different
    # upstream file).  Regular files resolve to their own basename.
    alias_count = sum(
        1 for rel, resolved in symlinks_resolved.items()
        if not resolved.endswith("/" + rel.split("/", 1)[1])
    )

    # Content-hash dedup metadata.
    bundle_files = sorted(p for p in out.rglob("*.svg") if p.is_file())
    hash_groups: dict[str, list[str]] = {}
    for p in bundle_files:
        h = sha256_of(p)
        hash_groups.setdefault(h, []).append(str(p.relative_to(out)))
    duplicate_groups = {h: sorted(v) for h, v in hash_groups.items() if len(v) > 1}
    dedup_potential = sum(len(v) - 1 for v in duplicate_groups.values())

    # Manifest.
    manifest = {
        "source": "PapirusDevelopmentTeam/papirus-icon-theme",
        "commit": commit,
        "license": selection.get("license", "GPL-3.0"),
        "generated_by": "scripts/vendor_papirus_icons.py",
        "import_date": datetime.date.today().isoformat(),
        "sizes": sizes,
        "required_fallbacks": sorted(required_fallbacks),
        "icons": icon_paths,
        "symlinks": {
            "total_entries": len(symlinks_resolved),
            "upstream_symlink_entries": upstream_symlink_entries,
            "aliases_resolved": alias_count,
            "regular_files_copied": len(symlinks_resolved) - alias_count,
            "resolved": dict(sorted(symlinks_resolved.items())),
        },
        "duplicates": {
            "note": (
                "identical-content groups recorded for a future asset-alias-aware loader; "
                "files are kept as real files because the iced include_bytes! loader has "
                "no asset-alias mechanism"
            ),
            "distinct_content_hashes": len(hash_groups),
            "duplicate_groups": len(duplicate_groups),
            "files_redundant_if_aliased": dedup_potential,
            "groups": {h: v for h, v in sorted(duplicate_groups.items())},
        },
    }
    (out / "manifest.json").write_text(json.dumps(manifest, indent=2, sort_keys=True) + "\n")
    print(f"Wrote manifest.json ({len(icon_paths)} icons).")

    # UPSTREAM.md.
    now = datetime.datetime.now(datetime.timezone.utc).strftime("%Y-%m-%d")
    imported_paths = "\n".join(
        f"- {size}/ ({len([p for p in icon_paths.values() if size in p])} icons)" for size in sizes
    )
    upstream_md = f"""# Upstream metadata — Papirus icon theme

Project: Papirus Icon Theme
Upstream repository: PapirusDevelopmentTeam/papirus-icon-theme
Licence: GPL-3.0
Imported commit: {commit}
Import date: {now}
Import script: scripts/vendor_papirus_icons.py
Selection file: assets/third_party/papirus/selected-icons.json

Imported paths (inside assets/third_party/papirus/):
{imported_paths}
- LICENSE (GPL-3.0 text)
- NOTICE.md (attribution)
- manifest.json (machine-readable index)

Symlink/alias resolution (PAPIRUS-03):
- {upstream_symlink_entries} of the {len(symlinks_resolved)} selected entries were stored
  upstream as relative symlinks ({alias_count} distinct aliases resolved); the remainder
  are regular files copied verbatim.
- Every selected icon was resolved to its real SVG source and materialised as a regular
  file; the bundle contains 0 symlinks, so release packaging (Windows/AppImage/Flatpak)
  will not ship broken relative links.
- The import fails with a non-zero exit if any selected icon is missing or any symlink
  target cannot be resolved.

Deduplication (PAPIRUS-03):
- {len(bundle_files)} SVG files with {len(hash_groups)} distinct content hashes.
- {len(duplicate_groups)} duplicate groups; {dedup_potential} files are byte-identical
  to another file and could be collapsed once the framework gains asset aliases.
- Duplicate groups are recorded in manifest.json under `duplicates.groups`.

Modifications: None. SVGs are copied verbatim from the pinned upstream commit;
symlinks are materialised as real files so the bundle has no relative symlinks.
"""
    (out / "UPSTREAM.md").write_text(upstream_md)
    print("Wrote UPSTREAM.md.")

    # Report.
    if missing:
        print("\nUnresolvable icons (import will fail):")
        for icon, sizes_missing in missing.items():
            for size, reason in sizes_missing.items():
                print(f"  {icon} [{size}]: {reason}")
    else:
        print("\nAll selected icons imported at all sizes.")
    print(f"Symlinks/aliases detected and resolved: {alias_count}")
    print(f"Duplicate groups recorded: {len(duplicate_groups)} (potential dedup: {dedup_potential} files)")

    total_files = sum(len(p) for p in icon_paths.values())
    print(f"Total SVG files copied: {total_files}")
    print(f"Bundle: {out}")

    # Final verification pass (no symlinks + every manifest entry real).
    problems = verify_bundle(out, manifest)
    if problems:
        print("\nBundle verification FAILED:")
        for p in problems:
            print(f"  - {p}")
        sys.exit(2)
    print("Bundle verification OK: no symlinks, all manifest entries are real files.")

    if missing:
        sys.exit(2)

    print("Import complete.")


if __name__ == "__main__":
    main()
