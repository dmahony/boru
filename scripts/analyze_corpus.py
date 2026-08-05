#!/usr/bin/env python3
"""Analyze the postcard corpus dumped by the gen_dictionary_and_analyze test.

Finds the highest-frequency repeated substrings across all messages so the
dictionary can be built from the byte sequences that actually recur.

Usage: python3 scripts/analyze_corpus.py [dir]
"""
import collections
import os
import sys


def load_corpus(directory):
    entries = []
    for fn in sorted(os.listdir(directory)):
        if not fn.endswith(".bin"):
            continue
        with open(os.path.join(directory, fn), "rb") as f:
            data = f.read()
        entries.append((fn[:-4], data))
    return entries


def main():
    directory = sys.argv[1] if len(sys.argv) > 1 else "/tmp/boru_corpus"
    entries = load_corpus(directory)
    if not entries:
        print(f"no .bin files in {directory}")
        return
    total = sum(len(d) for _, d in entries)
    print(f"{len(entries)} entries, {total} total bytes\n")
    for name, data in entries:
        print(f"{name:<38} {len(data):>6} bytes  {data.hex()}")

    # Count frequent substrings of length 3..16 across all entries.
    # Weight by occurrence; a substring occurring once in a 300-byte message
    # counts once, but a substring spanning many entries is more valuable.
    counts = collections.Counter()
    per_entry = collections.Counter()
    for name, data in entries:
        seen = set()
        for n in range(3, 17):
            for i in range(len(data) - n + 1):
                sub = data[i : i + n]
                counts[sub] += 1
                seen.add(sub)
        for sub in seen:
            per_entry[sub] += 1

    print("\n=== most frequent substrings (count x length => bytes of input covered) ===")
    rows = []
    for sub, c in counts.items():
        # c = total occurrences; per_entry = distinct entries containing it.
        rows.append((c * len(sub), c, per_entry[sub], len(sub), sub))
    rows.sort(reverse=True)
    for covered, c, entries_n, n, sub in rows[:120]:
        printable = "".join(chr(b) if 32 <= b < 127 else f"\\x{b:02x}" for b in sub)
        print(f"covers {covered:>5}  occ {c:>3}  in {entries_n:>2} entries  len {n:>2}  {printable}")


if __name__ == "__main__":
    main()
