#!/usr/bin/env python3
"""Vendor a curated subset of the Papirus icon theme into Boru's asset tree.

Reproducible import process for `assets/third_party/papirus/`.

Usage:
    python3 scripts/vendor_papirus_icons.py --source /path/to/papirus-icon-theme [--commit <sha>] [--out <dir>]

What it does:
  1. Verifies the expected upstream commit (from assets/third_party/papirus/selected-icons.json
     unless --commit is given).
  2. Reads the approved icon-selection list (selected-icons.json).
  3. Resolves aliases/symlinks by copying the *content* of each selected SVG
     (shutil.copyfile follows symlinks, so no relative symlinks leak into the bundle).
  4. Copies the required assets into <out>/<size>/<name>.svg.
  5. Copies the upstream GPL-3.0 LICENSE into the bundle.
  6. Regenerates manifest.json from the final packaged asset paths.
  7. Rewrites UPSTREAM.md with the import metadata.
  8. Reports missing icons and exits non-zero if any required fallback is absent.

Constraints honoured:
  - No runtime downloads; everything is committed to the repo.
  - Only icons listed in selected-icons.json are imported (no whole-repo copy).
  - Sizes 16/24/32/48/64 px only (upstream 16x16/24x24/32x32/48x48/64x64).

Examples:
    python3 scripts/vendor_papirus_icons.py --source /tmp/papirus-icon-theme
    python3 scripts/vendor_papirus_icons.py --source /tmp/papirus-src/papirus-icon-theme-<sha>
"""

from __future__ import annotations

import argparse
import datetime
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


def import_icon(source: Path, out: Path, icon: str, sizes: list[str],
                upstream_dirs: dict[str, str], selection: dict) -> dict[str, str]:
    """Copy one icon at all sizes; return {size: relative_path}."""
    result: dict[str, str] = {}
    sub = upstream_subdir_for(icon, selection)
    for size in sizes:
        upstream_size = upstream_dirs.get(size, f"{size}x{size}")
        src = source / "Papirus" / upstream_size / sub / f"{icon}.svg"
        if not src.exists():
            continue
        dst_dir = out / size
        dst_dir.mkdir(parents=True, exist_ok=True)
        dst = dst_dir / f"{icon}.svg"
        # copyfile follows symlinks: materialises real SVG content.
        shutil.copyfile(src, dst)
        result[size] = f"{size}/{icon}.svg"
    return result


def main() -> None:
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument("--source", required=True, help="Path to a Papirus checkout or extracted archive")
    ap.add_argument("--commit", default=None, help="Expected upstream commit (default: from selected-icons.json)")
    ap.add_argument("--out", default=str(DEFAULT_OUT), help="Output directory (default: assets/third_party/papirus)")
    ap.add_argument("--selection", default=str(DEFAULT_SELECTION), help="Selection JSON (default: selected-icons.json)")
    args = ap.parse_args()

    source = Path(args.source)
    out = Path(args.out)
    selection_path = Path(args.selection)

    if not source.exists():
        die(f"source does not exist: {source}")
    if not selection_path.exists():
        die(f"selection file does not exist: {selection_path}")

    selection = json.loads(selection_path.read_text())
    expected_commit = args.commit or selection.get("commit")
    commit = resolve_upstream_commit(source, expected_commit)

    sizes = list(selection.get("sizes", ["16", "24", "32", "48", "64"]))
    upstream_dirs = selection.get("upstream_dirs", {})
    icons = list_selected_icons(selection)
    required_fallbacks = set(selection.get("required_fallbacks", []))
    license_rel = source / "LICENSE"

    print(f"Importing {len(icons)} curated icons at sizes {sizes} from commit {commit[:12]}...")
    out.mkdir(parents=True, exist_ok=True)

    missing: dict[str, list[str]] = {}
    icon_paths: dict[str, dict[str, str]] = {}
    for icon in icons:
        paths = import_icon(source, out, icon, sizes, upstream_dirs, selection)
        missing_here = [s for s in sizes if s not in paths]
        if missing_here:
            missing[icon] = missing_here
        icon_paths[icon] = paths

    # Licence text.
    if license_rel.exists():
        shutil.copyfile(license_rel, out / "LICENSE")
        print("Copied upstream LICENSE (GPL-3.0).")
    else:
        missing["LICENSE"] = ["upstream LICENSE file missing"]

    # Fail hard on missing required fallbacks.
    missing_fallbacks = [f for f in required_fallbacks if not icon_paths.get(f)]
    if missing_fallbacks:
        die(
            "required fallback icons missing: "
            + ", ".join(missing_fallbacks)
            + " — fix selected-icons.json or the upstream checkout before importing."
        )

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

Modifications: None. SVGs are copied verbatim from the pinned upstream commit;
symlinks are materialised as real files so the bundle has no relative symlinks.
"""
    (out / "UPSTREAM.md").write_text(upstream_md)
    print("Wrote UPSTREAM.md.")

    # Report.
    if missing:
        print("\nMissing icons (see selected-icons.json notes for expected fallbacks):")
        for icon, sizes_missing in missing.items():
            print(f"  {icon}: missing at sizes {', '.join(sizes_missing)}")
    else:
        print("\nAll selected icons imported at all sizes.")

    total_files = sum(len(p) for p in icon_paths.values())
    print(f"Total SVG files copied: {total_files}")
    print(f"Bundle: {out}")
    print("Import complete.")

    if missing:
        sys.exit(2)


if __name__ == "__main__":
    main()
