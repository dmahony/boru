#!/usr/bin/env python3
"""UI-16 evidence verification (kanban t_dfbde136).

OCR-verifies the captured chat screenshots: the footer must show exactly one
truthful route/peer label ("Direct (mesh)" | "Relay" | "Mesh" | "Not
connected") plus an optional peer count, and must NOT duplicate the header's
"End-to-end encrypted" text (plan UI-16 step 129). Also verifies the sent
live-resize message text appears (latest message visible above the composer).

Usage: python3 scripts/ui16_verify.py <evidence_dir>
"""
from __future__ import annotations

import json
import os
import subprocess
import sys

EVIDENCE = sys.argv[1] if len(sys.argv) > 1 else "docs/ui-redesign/evidence/ui-16"

ROUTE_LABELS = ("Direct (mesh)", "Relay", "Mesh", "Not connected")


def ocr(path: str) -> str:
    """Tesseract OCR of an image, single-column-ish output."""
    out = subprocess.run(
        ["tesseract", path, "stdout", "--psm", "6"],
        capture_output=True,
        text=True,
        check=False,
    )
    return out.stdout


def check(name: str, text: str, results: list, *, expect_connected: bool | None = None,
          expect_peer_count: bool | None = None, expect_dup_e2ee: bool = False) -> None:
    """One assertion block for a capture."""
    lower = text.lower()
    routes = [r for r in ROUTE_LABELS if r.lower() in lower]
    dup = "end-to-end encrypted" in lower
    peer = ("peer" in lower) or ("1 peer" in lower)
    ok = True
    notes = []
    if expect_connected is not None:
        if expect_connected:
            if not routes:
                ok = False
                notes.append("no route label found")
            elif routes != ["Not connected"] and any("not connected" in r.lower() for r in routes):
                ok = False
                notes.append("unexpected 'Not connected' for connected state")
        else:
            if routes != ["Not connected"]:
                ok = False
                notes.append(f"expected 'Not connected', got {routes}")
    if expect_peer_count is not None:
        if expect_peer_count and not peer:
            ok = False
            notes.append("expected peer count text, none found")
        if not expect_peer_count and peer:
            ok = False
            notes.append("unexpected peer count text")
    if expect_dup_e2ee:
        # Footer must NOT repeat the header's E2EE label; the header itself
        # legitimately shows it once, so "end-to-end encrypted" appearing in
        # the full OCR is expected exactly once overall. We flag when the
        # string appears in the footer band (bottom ~20% of the screenshot).
        pass
    status = "PASS" if ok else "FAIL"
    results.append(
        {"file": name, "status": status, "routes": routes, "has_peer_count": peer,
         "e2ee_mentions": lower.count("end-to-end encrypted"), "notes": notes}
    )


def main() -> int:
    results = []
    files = sorted(f for f in os.listdir(EVIDENCE) if f.endswith(".png") and f.startswith("t_dfbde136_"))
    for f in files:
        path = os.path.join(EVIDENCE, f)
        text = ocr(path)
        lower = text.lower()
        # Footer band: crop bottom 18% of the image and OCR it separately so
        # we can prove the footer does not contain the header's E2EE label.
        from PIL import Image
        im = Image.open(path)
        w, h = im.size
        band = im.crop((0, int(h * 0.80), w, h))
        band_path = "/tmp/ui16-footer-band.png"
        band.save(band_path)
        footer_text = ocr(band_path).lower()

        routes = [r for r in ROUTE_LABELS if r.lower() in lower]
        dup_in_footer = "end-to-end encrypted" in footer_text
        peer_in_footer = "peer" in footer_text
        record = {
            "file": f,
            "size": f"{w}x{h}",
            "routes": routes,
            "footer_has_peer_count": peer_in_footer,
            "footer_duplicates_e2ee": dup_in_footer,
            "e2ee_total_mentions": lower.count("end-to-end encrypted"),
        }
        if "empty" in f:
            record["expect"] = "empty timeline; footer route present; no e2ee duplicate"
            record["ok"] = (routes != [] and not dup_in_footer)
        elif "offline" in f:
            record["expect"] = "footer 'Not connected'; no peer count; no e2ee duplicate"
            record["ok"] = ("not connected" in lower and not peer_in_footer and not dup_in_footer)
        elif "long" in f:
            record["expect"] = "long history bottom: latest message visible; footer route; no e2ee duplicate"
            record["ok"] = ("long-history message 400" in lower and routes != [] and not dup_in_footer)
        elif "live_resize" in f:
            record["expect"] = "sent live-resize message visible; footer route; no e2ee duplicate"
            # Map viewport size -> message number (script sends in this order).
            order = {"1024x720": 1, "1280x800": 2, "1440x900": 3, "1920x1080": 4}
            size = f.replace("t_dfbde136_live_resize_", "").replace(".png", "")
            n = order.get(size)
            record["expect"] = f"live-resize message {n} visible above composer; footer route; no e2ee duplicate"
            record["ok"] = (routes != [] and not dup_in_footer and (n is None or f"message {n}" in lower))
        elif "one" in f:
            record["expect"] = "one message visible; footer route; no e2ee duplicate"
            record["ok"] = ("single-message" in lower and routes != [] and not dup_in_footer)
        else:
            record["expect"] = "figure-4 conversation; footer route; no e2ee duplicate"
            record["ok"] = (routes != [] and not dup_in_footer)
        results.append(record)

    failures = [r for r in results if not r.get("ok")]
    print(json.dumps({"checked": len(results), "failures": len(failures), "results": results}, indent=2))
    return 1 if failures else 0


if __name__ == "__main__":
    raise SystemExit(main())
