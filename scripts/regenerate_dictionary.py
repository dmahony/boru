#!/usr/bin/env python3
"""Regenerate the DICTIONARY static from the gen_dictionary sections.

Replicates the ignored gen_dictionary test exactly (verified byte-for-byte
against the current static), then writes the updated static to
/tmp/boru_dictionary_new.rs with section comments, for pasting into
src/wire_compression.rs.
"""
import re
import sys

import blake3 as b3

sys.path.insert(0, 'scripts')
from replicate_gate_corpus import varint, str_field, opt_str, opt_bytes

SRC = 'src/wire_compression.rs'


def extract_rust_string(src, var):
    m = re.search(rf'let {var} = "\\\n(.*?)";', src, re.S)
    raw = m.group(1)
    lines = raw.split('\n')
    out = []
    for i, line in enumerate(lines):
        line = line.lstrip()
        if line.endswith('\\'):
            out.append(line[:-1])
        else:
            out.append(line)
            if i < len(lines) - 1:
                out.append('\n')
    s = ''.join(out)
    s = s.replace('\\\\', '\\').replace('\\"', '"').replace('\\n', '\n').replace('\\t', '\t')
    return s


def blob(words):
    b = bytearray()
    for w in words.split():
        b.extend(w.encode())
        b.append(0x20)
    return bytes(b)


def build():
    src = open(SRC).read()
    sa = extract_rust_string(src, 'section_a')
    sa2 = extract_rust_string(src, 'section_a2')
    sb = extract_rust_string(src, 'section_b')

    ba, ba2, bb = blob(sa), blob(sa2), blob(sb)

    # Section C
    c = bytearray()
    hash_fn = lambda s: b3.blake3(s.encode()).digest()
    tick = lambda s: b"blob:iroh:" + b3.blake3(s.encode()).hexdigest().encode() + b":3:200:1000"

    def add_msg(m):
        kind = m[0]
        if kind == 'Msg':
            return b"\x01" + str_field(m[1])
        if kind == 'PWT':
            return b"\x05" + str_field(m[1])
        if kind == 'AboutMe':
            return b"\x00" + str_field(m[1]) + opt_str(m[2])
        if kind == 'FileShare':
            return b"\x02" + str_field(m[1]) + str_field(m[2]) + varint(m[3]) + opt_bytes(m[4])
        if kind == 'Edit':
            return b"\x07" + m[1] + str_field(m[2])
        raise ValueError(kind)

    for t in ["hi", "hello", "ok", "thanks", "thank you", "lol",
              "good morning", "what's up", "how are you", "see you later"]:
        c += add_msg(('Msg', t))
    c += add_msg(('PWT', tick('general')))

    sections = [
        ("Section A: common English chat words and phrases", ba),
        ("Section A2: common multi-word chat phrases", ba2),
        ("Section B: domain vocabulary", bb),
        ("Section C: postcard structural exemplars (one per Message variant)", bytes(c)),
    ]

    out = []
    total = 0
    for title, data in sections:
        out.append(f"    // ── {title} ──\n    ")
        for i, b in enumerate(data):
            out.append(f"0x{b:02x}, ")
            if (i + 1) % 8 == 0:
                out.append("\n    ")
        out.append("\n")
        total += len(data)
    static = "pub static DICTIONARY: &[u8] = &[\n" + "".join(out) + "];\n"
    return static, total


def main():
    static, total = build()
    print(f"DICTIONARY total {total} bytes")
    with open('/tmp/boru_dictionary_new.rs', 'w') as f:
        f.write(static)
    print("wrote /tmp/boru_dictionary_new.rs")


if __name__ == '__main__':
    main()
