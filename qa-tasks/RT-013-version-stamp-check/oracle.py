#!/usr/bin/env python3
"""Check the deployed binary reports a fork.NNN stamp (not legacy, dev, or bare)."""
import os, re, subprocess, sys

# The QA suite runs against the debug build; the version check needs the
# STAMPED dist build (ALLSPARK_FORK_BUILD at compile time). Prefer the
# dist binary when present.
candidates = [
    os.environ.get("QA_DIST_BIN"),
    os.path.join(os.environ.get("QA_REPO", "."), "target", "dist", "prime-agent"),
    "/root/prime-agent/target/dist/prime-agent",
    sys.argv[1] if len(sys.argv) > 1 else None,
]
binary = next((c for c in candidates if c and os.path.isfile(c) and os.access(c, os.X_OK)), None)
if not binary:
    print("FAIL: no stamped binary found (build with ALLSPARK_FORK_BUILD=fork.NNN)")
    sys.exit(1)

try:
    out = subprocess.run([binary, "--version"], capture_output=True, text=True, timeout=5)
    version = out.stdout.strip()
except Exception as e:
    print(f"FAIL: cannot run {binary}: {e}")
    sys.exit(1)

if not version.startswith("allspark "):
    print(f"FAIL: version does not name the fork: {version!r} (from {binary})")
    sys.exit(1)

m = re.search(r"fork build fork\.(\d+)", version)
if not m:
    print(f"FAIL: no fork build stamp: {version!r}")
    sys.exit(1)

build = m.group(1)
if len(build) < 3:
    print(f"FAIL: legacy single-digit build id fork.{build} (expected NNN): {version!r}")
    sys.exit(1)
if build == "dev":
    print(f"FAIL: unstamped build (dev): {version!r}")
    sys.exit(1)

print(f"OK: {version}")
