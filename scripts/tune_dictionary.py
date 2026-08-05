#!/usr/bin/env python3
"""Dictionary tuning harness using ctypes against system libz.

Raw RFC1951 deflate (windowBits=-15) with a preset dictionary — the same
zlib backend that flate2 wraps — so ratios measured here match what boru's
wire_compression::compress does with the preshared dictionary.

Usage:
  python3 scripts/tune_dictionary.py                    # baseline comparisons
  python3 scripts/tune_dictionary.py --candidate FILE   # measure a candidate dict
  python3 scripts/tune_dictionary.py --gate             # measure the gate corpus path
"""
import ctypes
import ctypes.util
import glob
import os
import re
import sys
import zlib

libz = ctypes.CDLL(ctypes.util.find_library("z"))

# z_stream layout (zlib.h)
class ZStream(ctypes.Structure):
    _fields_ = [
        ("next_in", ctypes.c_void_p),
        ("avail_in", ctypes.c_uint),
        ("total_in", ctypes.c_ulong),
        ("next_out", ctypes.c_void_p),
        ("avail_out", ctypes.c_uint),
        ("total_out", ctypes.c_ulong),
        ("msg", ctypes.c_char_p),
        ("state", ctypes.c_void_p),
        ("zalloc", ctypes.c_void_p),
        ("zfree", ctypes.c_void_p),
        ("opaque", ctypes.c_void_p),
        ("data_type", ctypes.c_int),
        ("adler", ctypes.c_ulong),
        ("reserved", ctypes.c_ulong),
    ]


Z_NO_FLUSH = 0
Z_FINISH = 4
Z_OK = 0
Z_STREAM_END = 1
Z_DEFAULT_COMPRESSION = -1
Z_DEFLATED = 8
Z_DEFAULT_STRATEGY = 0


def raw_deflate_with_dict(data: bytes, dictionary: bytes) -> bytes:
    """Raw deflate (no zlib header) with a preset dictionary, level 6 (flate2
    Compression::default() is level 6)."""
    stream = ZStream()
    src = ctypes.create_string_buffer(data, len(data))
    stream.next_in = ctypes.cast(src, ctypes.c_void_p)
    stream.avail_in = len(data)

    out_cap = max(len(data) * 2 + 256, 256)
    dst = ctypes.create_string_buffer(out_cap)

    # deflateInit2_(strm, level, method, windowBits, memLevel, strategy, version, stream_size)
    rc = libz.deflateInit2_(
        ctypes.byref(stream), Z_DEFAULT_COMPRESSION, Z_DEFLATED, -15, 8,
        Z_DEFAULT_STRATEGY, b"1.2.12", ctypes.sizeof(ZStream))
    if rc != Z_OK:
        raise RuntimeError(f"deflateInit2 failed rc={rc}")
    try:
        if dictionary:
            rc = libz.deflateSetDictionary(
                ctypes.byref(stream),
                ctypes.cast(ctypes.create_string_buffer(dictionary, len(dictionary)),
                            ctypes.c_void_p),
                len(dictionary))
            if rc != Z_OK:
                raise RuntimeError(f"deflateSetDictionary failed rc={rc}")
        stream.next_out = ctypes.cast(dst, ctypes.c_void_p)
        stream.avail_out = out_cap
        rc = libz.deflate(ctypes.byref(stream), Z_FINISH)
        if rc != Z_STREAM_END:
            raise RuntimeError(f"deflate failed rc={rc}")
        return dst.raw[: stream.total_out]
    finally:
        libz.deflateEnd(ctypes.byref(stream))


def load_current_dictionary():
    src = open('src/wire_compression.rs').read()
    m = re.search(r'pub static DICTIONARY: &\[u8\] = &\[\n(.*?)\n\];', src, re.S)
    body = m.group(1)
    return bytes(int(b, 16) for b in re.findall(r'0x([0-9a-fA-F]{2})', body))


def load_corpus(directory='/tmp/boru_corpus'):
    entries = []
    for fn in sorted(glob.glob(os.path.join(directory, '*.bin'))):
        with open(fn, 'rb') as f:
            entries.append((os.path.basename(fn)[:-4], f.read()))
    return entries


def measure(entries, dictionary, label):
    raw_total = 0
    comp_total = 0
    rows = []
    for name, data in entries:
        c = raw_deflate_with_dict(data, dictionary)
        raw_total += len(data)
        comp_total += len(c)
        rows.append((name, len(data), len(c), len(data) / len(c)))
    print(f"\n=== {label} (dictionary {len(dictionary)} bytes) ===")
    for name, r, c, ratio in rows:
        print(f"{name:<38} raw {r:>6}  comp {c:>6}  ratio {ratio:>5.2f}")
    print(f"TOTAL raw {raw_total} comp {comp_total} ratio {raw_total / comp_total:.2f}")
    return raw_total, comp_total


def main():
    dict_bytes = load_current_dictionary()
    entries = load_corpus()
    print(f"{len(entries)} corpus entries, current DICTIONARY = {len(dict_bytes)} bytes")

    measure(entries, b"", "plain deflate (no dictionary)")
    measure(entries, dict_bytes, "current DICTIONARY")

    if len(sys.argv) > 2 and sys.argv[1] == '--candidate':
        with open(sys.argv[2], 'rb') as f:
            cand = f.read()
        measure(entries, cand, "candidate")


if __name__ == '__main__':
    main()
