#!/usr/bin/env python3
"""Generate the deterministic files used by the desktop download tests."""
from pathlib import Path

ROOT = Path(__file__).resolve().parent

(ROOT / "zero-byte.txt").write_bytes(b"")
(ROOT / "small-message.txt").write_bytes(b"Boru download fixture: small text file\n")
(ROOT / "imported-document.json").write_bytes(
    b'{"fixture":"imported-document","version":1,"message":"import through the File Library workflow"}\n'
)
(ROOT / "referenced-record.csv").write_bytes(
    b"id,kind,value\n1,reference,deterministic\n2,reference,fixture\n"
)
for name in ("duplicate-a.txt", "duplicate-b.txt"):
    (ROOT / name).write_bytes(b"Boru download fixture: identical content\n")

# The byte at offset n is n modulo 251. The repeated 1 MiB block makes the
# output deterministic without storing generated binary data in this script.
block = bytes(i % 251 for i in range(1024 * 1024))
with (ROOT / "large-deterministic.bin").open("wb") as output:
    for _ in range(8):
        output.write(block)

print(f"generated fixtures in {ROOT}")
