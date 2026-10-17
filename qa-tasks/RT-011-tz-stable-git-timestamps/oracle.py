#!/usr/bin/env python3
"""RT-011 oracle: commits.txt equals the TZ-independent reference
(`git log --format='%h %cI'`) computed under two different TZ settings."""
import os
import subprocess
import sys
from pathlib import Path

work = Path(os.environ["QA_WORK"])
repo = work / "repo"
target = repo / "commits.txt"
if not target.is_file():
    print("commits.txt was not created in the repo")
    sys.exit(1)
produced = target.read_text().strip()


def reference(tz):
    env = dict(os.environ, TZ=tz)
    out = subprocess.run(["git", "-C", str(repo), "log", "--format=%h %cI"],
                         capture_output=True, text=True, env=env)
    return out.stdout.strip()


ref_utc = reference("UTC")
ref_kras = reference("Asia/Krasnoyarsk")
if ref_utc != ref_kras:
    print("internal error: reference itself is TZ-dependent (%cI must be stable)")
    sys.exit(1)
if produced != ref_utc:
    print("commits.txt does not match the TZ-stable reference:")
    print(f"  produced: {produced!r}")
    print(f"  expected: {ref_utc!r}")
    sys.exit(1)
print("ok")
