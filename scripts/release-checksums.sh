#!/usr/bin/env bash
# Create and verify one SHA-256 manifest for release payloads.
# Usage: scripts/release-checksums.sh <artifact-directory>
set -euo pipefail

DIR=${1:?usage: release-checksums.sh <artifact-directory>}
[[ -d "$DIR" ]] || { echo "artifact directory does not exist: $DIR" >&2; exit 2; }
cd "$DIR"
mapfile -t files < <(find . -maxdepth 1 -type f \( -name '*.tar.gz' -o -name '*.zip' -o -name '*.json' \) -printf '%f\n' | sort)
((${#files[@]} > 0)) || { echo "no release artifacts found" >&2; exit 1; }
sha256sum "${files[@]}" > SHA256SUMS
sha256sum --strict --check SHA256SUMS
[[ -s SHA256SUMS ]] || { echo "empty checksum manifest" >&2; exit 1; }
printf 'verified %d release artifacts with %s\n' "${#files[@]}" "$PWD/SHA256SUMS"
