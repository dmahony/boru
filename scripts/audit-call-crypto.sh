#!/usr/bin/env bash
# Fail if call code grows an application-level cryptographic layer.
# QUIC/TLS encryption belongs to Iroh's transport, not this source tree.
set -euo pipefail

repo_root=$(CDPATH= cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)
cd "$repo_root"

pattern='aes|encrypt|decrypt|xchacha|chacha|cipher|crypto|ratchet|nonce|seal|hkdf|hmac'
if matches=$(grep -Einr --include='*.rs' -E "$pattern" src/call); then
    printf '%s\n' "$matches" >&2
    printf 'custom call cryptography markers found in src/call\n' >&2
    exit 1
fi

printf 'call crypto audit passed: no application-level crypto markers in src/call\n'
