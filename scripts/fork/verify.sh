#!/usr/bin/env bash
# Batched verification: ONE cargo invocation per stage across all six crates.
# Cargo serialises on the target/ lock, so N parallel `cargo test -p …` calls
# gain nothing — they queue. One call with multiple `-p` flags keeps a single
# build plan and a single freshness pass; the parallelism lives *inside* the
# invocation (CARGO_BUILD_JOBS for the build, nextest/--test-threads for tests).
#
# Usage: verify.sh [quick|full]
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
cd "$ROOT"

CRATES="-p pa-types -p pa-models -p pa-ai -p pa-core -p pa-daemon -p pa-cli"
OUT="${ROOT}/.research/gate"
mkdir -p "$OUT"

step() { printf '\n=== %s\n' "$*"; }

fail=0

step "test contract (governor, ~7 s, first: a matrix error must not wait for a build)"
if PYTHONPATH="$ROOT/scripts" python3 -m testgov ci >"$OUT/testgov.log" 2>&1; then
  echo "testgov: ok"
else
  echo "testgov: FAILED (HIGH findings; see $OUT/testgov.log)"
  grep -E '^\[HIGH\]' "$OUT/testgov.log" | head -20
  fail=1
fi

step "fmt"
if cargo fmt --all -- --check >"$OUT/fmt.log" 2>&1; then
  echo "fmt: ok"
else
  echo "fmt: FAILED (cargo fmt --all fixes it)"; tail -20 "$OUT/fmt.log"; fail=1
fi

step "clippy (batched: one invocation, six crates)"
if cargo clippy $CRATES --all-targets -- -D warnings >"$OUT/clippy.log" 2>&1; then
  echo "clippy: ok"
else
  echo "clippy: FAILED"; grep -E '^error' -A6 "$OUT/clippy.log" | head -40; fail=1
fi

if [[ "${1:-full}" == "quick" ]]; then
  echo; if [[ $fail -ne 0 ]]; then echo "VERIFY RED (quick)"; else echo "VERIFY GREEN (quick)"; fi
  exit $fail
fi

free_gb=$(df -BG --output=avail "$ROOT" | tail -1 | tr -dc '0-9')
if (( free_gb < 15 )); then
  echo "VERIFY SKIPPED: ${free_gb}G free, need 15G (clean target artifacts)"
  exit 2
fi

step "tests (batched: one invocation, all six crates, --lib)"
if cargo test -q $CRATES --lib >"$OUT/tests-lib.log" 2>&1; then
  echo "tests --lib: ok ($(grep -oE '[0-9]+ passed' "$OUT/tests-lib.log" | tail -1))"
else
  echo "tests --lib: FAILED"; grep -E 'panicked|FAILED|^failures' "$OUT/tests-lib.log" | head -20; fail=1
fi

echo
if [[ $fail -ne 0 ]]; then echo "VERIFY RED"; else echo "VERIFY GREEN"; fi
exit $fail
