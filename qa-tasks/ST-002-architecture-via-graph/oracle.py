"""The chain must be reported in order, naming every hop."""
import os, re, sys

transcript = os.environ.get("QA_TRANSCRIPT", "")
text = ""
if transcript and os.path.isfile(transcript):
    text = open(transcript, encoding="utf-8", errors="replace").read()

# Normalize separators so `->`, `→`, and `>` all read the same.
flat = re.sub(r"[\s]+", " ", text)
hops = ["main", "handle", "validate", "load"]
problems = []
pos = 0
for hop in hops:
    idx = flat.find(hop, pos)
    if idx < 0:
        problems.append(f"chain hop {hop!r} missing after position {pos}")
        break
    pos = idx + len(hop)
if "CHAIN-DONE" not in flat:
    problems.append("missing CHAIN-DONE marker")
for p in problems:
    print(p)
sys.exit(1 if problems else 0)
