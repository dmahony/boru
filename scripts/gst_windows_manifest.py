#!/usr/bin/env python3
"""Regenerate the GStreamer Windows runtime/plugin manifests.

The manifests shipped in scripts/ are checked in so the GitHub Actions
packaging step is deterministic and does not require a PE parser on the
runner.  Regenerate them locally whenever the pinned GStreamer MSVC runtime
version changes:

  python3 scripts/gst_windows_manifest.py \
      --gst-root <extracted runtime root> \
      --plugins scripts/gstreamer-windows-plugins.txt \
      --out-bin scripts/gstreamer-windows-runtime.txt

--plugins is the curated allowlist (see comments in
scripts/gstreamer-windows-plugins.txt).  --out-bin receives the dependency
closure of bin/*.dll files reachable from the allowlisted plugins plus the
core libs the application itself links (gstreamer, gstapp, gstbase,
gstvideo, glib, gobject, gio, ...).  System DLLs (kernel32, user32,
api-ms-win-*, VCRUNTIME140) are ignored — they ship with Windows or the
VC++ redistributable.
"""
import argparse
import re
import subprocess
import sys
from pathlib import Path

READOBJ = "/usr/lib/llvm-18/bin/llvm-readobj"  # override with --readobj

CORE_LIBS = [
    "gstreamer-1.0-0.dll", "gstapp-1.0-0.dll", "gstbase-1.0-0.dll",
    "gstvideo-1.0-0.dll", "gstaudio-1.0-0.dll", "gstpbutils-1.0-0.dll",
    "gstcontroller-1.0-0.dll", "gsttag-1.0-0.dll", "gstnet-1.0-0.dll",
    "gstallocators-1.0-0.dll", "gstfft-1.0-0.dll", "gstriff-1.0-0.dll",
    "glib-2.0-0.dll", "gobject-2.0-0.dll", "gio-2.0-0.dll",
    "gmodule-2.0-0.dll", "gthread-2.0-0.dll", "intl-8.dll",
    "ffi-7.dll", "z-1.dll", "orc-0.4-0.dll",
]


def dll_imports(path: Path, readobj: str) -> set[str]:
    out = subprocess.run([readobj, "--coff-imports", str(path)],
                         capture_output=True, text=True)
    names = set()
    for m in re.finditer(r"Name:\s+([^\s]+\.dll)", out.stdout, re.IGNORECASE):
        names.add(m.group(1).lower())
    return names


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--gst-root", required=True)
    ap.add_argument("--plugins", required=True)
    ap.add_argument("--out-bin", required=True)
    ap.add_argument("--readobj", default=READOBJ)
    args = ap.parse_args()

    root = Path(args.gst_root)
    bin_dir, plugin_dir = root / "bin", root / "lib" / "gstreamer-1.0"
    plugins = [p.strip() for p in Path(args.plugins).read_text().splitlines()
               if p.strip() and not p.strip().startswith("#")]
    missing = [p for p in plugins if not (plugin_dir / p).exists()]
    if missing:
        print("MISSING PLUGINS:", ", ".join(missing), file=sys.stderr)
        return 1

    bin_dlls = {p.name.lower(): p for p in bin_dir.glob("*.dll")}
    roots = list(plugins) + [c for c in CORE_LIBS if (bin_dir / c).exists()]
    needed: set[str] = set()
    scanned: set[str] = set()
    queue = list(roots)
    while queue:
        name = queue.pop(0)
        key = name.lower()
        if key in scanned:
            continue
        if key in bin_dlls:
            needed.add(key)
        path = bin_dlls.get(key) or (plugin_dir / name)
        if not path.exists():
            continue
        scanned.add(key)
        for imp in dll_imports(path, args.readobj):
            if imp in bin_dlls and imp not in scanned:
                queue.append(imp)

    resolved = sorted(bin_dlls[k].name for k in needed)
    Path(args.out_bin).write_text("\n".join(resolved) + "\n")
    print(f"wrote {len(resolved)} bin DLLs to {args.out_bin}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
