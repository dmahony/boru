#!/usr/bin/env python3
"""Convert cargo metadata into a dependency-scoped SPDX 2.3 SBOM."""
from __future__ import annotations

import argparse
import json
from datetime import datetime, timezone
from pathlib import Path
import re


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("metadata", type=Path)
    parser.add_argument("output", type=Path)
    args = parser.parse_args()
    data = json.loads(args.metadata.read_text(encoding="utf-8"))
    packages = data.get("packages", [])
    if not packages:
        raise SystemExit("cargo metadata contains no resolved packages")

    document = {
        "spdxVersion": "SPDX-2.3",
        "dataLicense": "CC0-1.0",
        "SPDXID": "SPDXRef-DOCUMENT",
        "name": "boru-resolved-cargo-dependencies",
        "documentNamespace": "https://github.com/dmahony/boru/releases/sbom/" + data.get("resolve", {}).get("root", "unknown"),
        "creationInfo": {
            "created": datetime.now(timezone.utc).replace(microsecond=0).isoformat().replace("+00:00", "Z"),
            "creators": ["Tool: scripts/generate-spdx-sbom.py"],
        },
        "packages": [],
        "relationships": [],
    }
    for package in sorted(packages, key=lambda item: (item["name"], item["version"], item["id"])):
        package_id = "SPDXRef-Package-" + re.sub(r"[^A-Za-z0-9.-]", "-", package["id"])
        document["packages"].append({
            "SPDXID": package_id,
            "name": package["name"],
            "versionInfo": package["version"],
            "downloadLocation": package.get("source") or "NOASSERTION",
            "licenseConcluded": "NOASSERTION",
            "licenseDeclared": package.get("license") or "NOASSERTION",
            "filesAnalyzed": False,
            "externalRefs": [{
                "referenceCategory": "PACKAGE-MANAGER",
                "referenceType": "purl",
                "referenceLocator": f"pkg:cargo/{package['name']}@{package['version']}",
            }],
        })
        document["relationships"].append({
            "spdxElementId": "SPDXRef-DOCUMENT",
            "relatedSPDXElement": package_id,
            "relationshipType": "DESCRIBES",
        })
    args.output.write_text(json.dumps(document, indent=2, sort_keys=True) + "\n", encoding="utf-8")
    print(f"wrote SPDX SBOM for {len(packages)} resolved Cargo packages: {args.output}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
