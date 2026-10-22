#!/usr/bin/env bash
# Optional formal-verification gate: proves the mirror specs in verus/*.rs.
# Skips silently (exit 0) when the Verus toolchain is absent — this gate is
# opt-in, never a hard requirement of `cargo test`.
set -u

VERUS_BIN="${VERUS_BIN:-/root/verus-pilot/verus-x86-linux/verus}"
DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/../verus" && pwd)"

if [ ! -x "$VERUS_BIN" ]; then
  echo "verus-check: toolchain not found at $VERUS_BIN — skipping (install: verus/README.md)"
  exit 0
fi

if [ ! -d "$DIR" ]; then
  echo "verus-check: no verus/ directory — skipping"
  exit 0
fi

fail=0
for f in "$DIR"/*.rs; do
  echo "verus-check: verifying $(basename "$f")"
  if ! "$VERUS_BIN" "$f"; then
    echo "verus-check: FAILED $(basename "$f")" >&2
    fail=1
  fi
done

if [ "$fail" -ne 0 ]; then
  echo "verus-check: one or more mirror specs failed — the mirrored logic likely drifted" >&2
  exit 1
fi
echo "verus-check: all mirror specs verified"

# A proof about a drifted mirror is a false assurance, and the verifier cannot see
# the drift: it only ever reads the mirror. Check the constants the two files must
# share, so the cheapest drift fails here rather than silently passing.
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
if ! python3 "$HERE/verus-mirror-check.py"; then
  echo "verus-check: a mirror disagrees with its source" >&2
  exit 1
fi
