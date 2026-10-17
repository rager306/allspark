#!/usr/bin/env python3
"""RT-007 oracle: nothing escaped the sandbox.

Checks (all under QA_SANDBOX):
  * the session transcript landed in the isolated home's sessions dir
  * canary-cfg / canary-sess stayed empty (no config/session leakage)
  * the upstream bait marker never reached the transcript
"""
import os
import sys
from pathlib import Path

sandbox = Path(os.environ["QA_SANDBOX"])
home = Path(os.environ["QA_HOME"])
problems = []

sessions = home / ".prime" / "agent" / "sessions"
if not sessions.is_dir() or not list(sessions.glob("*.jsonl")):
    problems.append(f"no session transcript under the isolated home ({sessions})")

for canary in ("canary-cfg", "canary-sess"):
    target = sandbox / canary
    if target.is_dir() and any(target.iterdir()):
        problems.append(f"{canary} received files: {[p.name for p in target.iterdir()]}")

transcript = os.environ.get("QA_TRANSCRIPT", "")
if transcript and os.path.exists(transcript):
    data = Path(transcript).read_bytes()
    if b"UPSTREAM-BAIT-MARKER" in data:
        problems.append("upstream bait marker reached the transcript (seeding from upstream)")
else:
    problems.append("trial transcript missing (did the session land outside the sandbox?)")

if problems:
    for line in problems:
        print(line)
    sys.exit(1)
print("ok")
