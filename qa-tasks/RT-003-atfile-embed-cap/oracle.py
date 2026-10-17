#!/usr/bin/env python3
"""RT-003 oracle: the embedded @file content in the initial user message is
bounded (256KiB cap + marker), and the tail marker never reached the wire."""
import json
import sys

transcript = sys.argv[0] if False else None
import os
path = os.environ["QA_TRANSCRIPT"]
if not path or not os.path.exists(path):
    print("no transcript")
    sys.exit(1)

first_user = None
with open(path, "rb") as handle:
    for line in handle:
        if not line.strip():
            continue
        try:
            row = json.loads(line)
        except Exception:
            continue
        msg = row.get("message", {})
        if row.get("type") == "message" and msg.get("role") == "user":
            first_user = msg
            break

if first_user is None:
    print("no initial user message found")
    sys.exit(1)

blocks = first_user.get("content", [])
file_block = next((b for b in blocks if isinstance(b, dict) and b.get("type") == "text"
                   and "big.log" in str(b.get("text", ""))[:2000]), None)
if file_block is None:
    print("no <file name=big.log> block in the initial message")
    sys.exit(1)

text = file_block.get("text", "")
if "[file truncated at" not in text:
    print("truncation marker missing from the embedded file block")
    sys.exit(1)
if len(text) > 300_000:
    print(f"embedded file block is {len(text)} chars (> 300k: cap+marker exceeded)")
    sys.exit(1)
if "RT003-TAIL-MARKER-39499" in text:
    print("tail marker reached the prompt")
    sys.exit(1)
print("ok")
