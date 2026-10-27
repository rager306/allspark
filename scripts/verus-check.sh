#!/usr/bin/env bash
# Optional formal-verification gate: proves the mirror specs in verus/*.rs.
# Skips silently (exit 0) when the Verus toolchain is absent — this gate is
# opt-in, never a hard requirement of `cargo test`.
set -u

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

# The toolchain lives WITH the specs, inside verus/ (gitignored: it is 1.6 GB of
# binaries). One folder holds the mirrors, their toolchain, and the runner, so a
# reader looking for "where is the proof" finds one directory.
VERUS_BIN="${VERUS_BIN:-$ROOT_DIR/verus/toolchain/verus}"
DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/../verus" && pwd)"

if [ ! -x "$VERUS_BIN" ]; then
  echo "verus-check: toolchain not found at $VERUS_BIN — skipping (install: verus/README.md)"
  exit 0
fi

if [ ! -d "$DIR" ]; then
  echo "verus-check: no verus/ directory — skipping"
  exit 0
fi

# Two classes, and the difference is the point:
#
#  * NO-TRUST mirrors model the property over data the verifier understands
#    (a `Seq<u8>`, an enum), so they need no `assume` and no `external_body`.
#    They run under `--no-cheating`, which turns any such shortcut into an
#    error instead of a silent axiom.
#  * TRUSTED mirrors wrap a std conversion Verus cannot see into (from_le_bytes,
#    to_le_bytes). Their `external_body` posts a spec the body is not checked
#    against; that surface is declared in each file's header. They run normally,
#    because `--no-cheating` would reject the declared trust, not a defect.
#
# A new mirror starts in NO-TRUST. Moving one to TRUSTED is a decision that has
# to be written down in the file, which is why the list is explicit here.
NO_TRUST=(pa_core_truncate.rs)
TRUSTED=(pa_core_gates.rs pa_mem_bucket.rs pa_mem_ptr48.rs)

fail=0
for f in "$DIR"/*.rs; do
  base="$(basename "$f")"
  strict=0
  for name in "${NO_TRUST[@]}"; do
    if [ "$name" = "$base" ]; then strict=1; fi
  done
  flags=()
  if [ "$strict" -eq 1 ]; then
    flags+=(--no-cheating)
    echo "verus-check: verifying $base (no-cheating)"
  else
    echo "verus-check: verifying $base (declared trust surface)"
  fi
  if ! "$VERUS_BIN" "${flags[@]}" "$f"; then
    echo "verus-check: FAILED $base" >&2
    fail=1
  fi
done

# Every mirror file must be classified: an unlisted file silently opts out of
# the strict mode the list exists to enforce.
for f in "$DIR"/*.rs; do
  base="$(basename "$f")"
  known=0
  for name in "${NO_TRUST[@]}" "${TRUSTED[@]}"; do
    if [ "$name" = "$base" ]; then known=1; fi
  done
  if [ "$known" -eq 0 ]; then
    echo "verus-check: $base is in neither NO_TRUST nor TRUSTED — classify it" >&2
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
