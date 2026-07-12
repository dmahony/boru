#!/usr/bin/env python3
"""Ad-hoc verification: confirm no emoji remain in app.rs UI code."""
import unicodedata
import json
import os

src = "/home/dan/iroh-gossip-chat/examples/iced_chat/app.rs"
result_path = "/home/dan/iroh-gossip-chat/hermes-verify-emoji.json"

def is_target_emoji(cp):
    # Emoji ranges that should NOT appear in UI code
    if 0x2600 <= cp <= 0x27BF: return True   # misc symbols
    if 0x1F000 <= cp <= 0x1FFFF: return True  # emoji
    if 0x2300 <= cp <= 0x23FF: return True    # misc technical
    return False

findings = {"emoji_in_code": [], "pass": True}

with open(src, "r", encoding="utf-8") as f:
    lines = f.readlines()

for i, line in enumerate(lines, 1):
    stripped = line.strip()
    is_comment = stripped.startswith("//") or stripped.startswith("/*")
    
    for c in line:
        cp = ord(c)
        if not is_target_emoji(cp):
            continue
        
        # Known skips
        if cp == 0x2713 and ("AA" in line or "~" in line):  # ✓ in contrast comments
            continue
        if is_comment:
            continue
            
        try:
            name = unicodedata.name(c, f"U+{cp:04X}")
        except:
            name = f"U+{cp:04X}"
        
        findings["emoji_in_code"].append({
            "line": i, "char": f"U+{cp:04X}", "name": name,
            "snippet": line.rstrip()[:100]
        })
        findings["pass"] = False

if findings["pass"]:
    print(f"PASS: Zero emoji in app.rs UI code ({len(lines)} lines checked)")
else:
    print(f"FAIL: {len(findings['emoji_in_code'])} emoji in UI code:")
    for e in findings["emoji_in_code"]:
        print(f"  L{e['line']}: {e['name']} ({e['char']})  {e['snippet']}")

with open(result_path, "w") as f:
    json.dump(findings, f, indent=2)
print(f"Report: {result_path}")
