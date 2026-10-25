#!/bin/bash
# Local mirror of ci.yml's mandatory gate. Run before every push.
#
# Why: a push costs 18 GitHub jobs (8 test shards + fmt + clippy + windows
# cross + deny + packaging + summary). Every red push burns that budget and
# a 15-minute round trip. This script runs the same checks locally first.
#
# Usage:
#   scripts/fork/prepush-gate.sh            # full gate
#   scripts/fork/prepush-gate.sh --fast     # fmt + clippy only (~3 min)
#   GATE_JOBS=6 scripts/fork/prepush-gate.sh
#
# Exit codes: 0 green, 1 red (details in .research/gate/*.log).
set -uo pipefail

cd "$(dirname "$0")/../.." || exit 1
# Exported: run_unit's per-unit disk floor runs under `xargs bash -c`, and an unexported
# variable is empty there — `df` got an empty operand and the floor never fired (G-9).
export ROOT
OUT="$ROOT/.research/gate"
mkdir -p "$OUT"
FAST=0
[[ "${1:-}" == "--fast" ]] && FAST=1
JOBS="${GATE_JOBS:-4}"
# Disk guard: parallel test binaries are large (the workspace has 275 units).
# Below this floor the run aborts instead of filling the disk and taking the
# machine's other work down with it.
MIN_FREE_GB="${GATE_MIN_FREE_GB:-15}"
# Floor checked before every unit (see run_unit): the workspace's parallel test binaries can
# consume several GB while the gate is already running.
UNIT_MIN_FREE_GB="${GATE_UNIT_MIN_FREE_GB:-2}"

# The pinned toolchain (rust-toolchain.toml) is what CI uses; no override.
export CARGO_TERM_COLOR=always
export CARGO_INCREMENTAL=0
# No CARGO_PROFILE_*_DEBUG overrides: the gate compiles the SAME profile the developer's
# editor, `cargo check` and `cargo test` use (line tables for members, none for
# dependencies — see [profile.dev] in Cargo.toml). Overriding debuginfo here used to build
# a second, disjoint artifact set beside the working one: the same pa_core existed as
# 352 MB (dev) and 114 MB (gate) copies, wasting 14.7 GB across 351 crates. CI keeps its
# own overrides because it pays for the disk once; this gate shares the tree with the
# work it is checking.
# Run scratch lives INSIDE the repo, not /tmp: systemd-tmpfiles deletes /tmp entries by
# age (d /tmp ... 10d) and Debian 13 mounts it tmpfs, so a long gate run can lose a
# worktree's fixtures to a cleaner — and a repo-level `rm -rf .tmp` is then the whole
# cleanup story. The 194 integration tests that call TempDir::new() honour TMPDIR; the
# nine that set TMPDIR themselves still win, so the isolation tests keep their shape.
mkdir -p "$ROOT/.tmp"
# The same cache-directory marker Cargo writes into target/: backup tools that honour
# CACHEDIR.TAG (tar, restic, borg) then skip run scratch instead of archiving it.
if [ ! -f "$ROOT/.tmp/CACHEDIR.TAG" ]; then
  printf 'Signature: 8a477f597d28d172789f06886806bc55\n# This file is a cache directory tag created by prepush-gate.sh.\n# For information about cache directory tags see https://bford.info/cachedir/\n' \
    > "$ROOT/.tmp/CACHEDIR.TAG"
fi
export TMPDIR="$ROOT/.tmp"
export TEMP="$TMPDIR"
export TMP="$TMPDIR"
# Scrub the ambient agent-session environment: this gate often runs from
# inside a live agent session, whose PRIME_AGENT_* overrides leak into the
# test binaries and point their spawned daemons at the live session root
# (~42 daemon e2e suites then fail on isolation). CI has no such env.
unset PRIME_AGENT_SESSION_DIR PRIME_AGENT_DAEMON_SOCKET \
      PRIME_AGENT_CODING_AGENT_DIR PRIME_AGENT_INTERNAL_DAEMON_WORKER \
      PRIME_AGENT_INTERNAL_DAEMON_WORKER_ACTIVE_SESSION_ID \
      PRIME_AGENT_INTERNAL_DAEMON_WORKER_INSTANCE_ID \
      PRIME_AGENT_INTERNAL_DAEMON_WORKER_RECOVERY_JOURNAL \
      PRIME_AGENT_INTERNAL_DAEMON_WORKER_CWD \
      PRIME_AGENT_INTERNAL_DAEMON_WORKER_TOKEN \
      PRIME_AGENT_INTERNAL_DAEMON_WORKER_SOCKET \
      PRIME_AGENT_INTERNAL_DAEMON_SUPERVISOR_SOCKET \
      PRIME_AGENT_INTERNAL_SESSION_LEASE_OWNER_ID \
      PRIME_AGENT_INTERNAL_SESSION_LEASES \
      PRIME_AGENT_INTERNAL_ORPHAN_PROCESS_JOURNAL

fail=0
step() { printf '\n=== %s\n' "$*"; }

# The test-contract governor (doc/adr/0001). Runs FIRST because it costs ~2s and catches a
# matrix/gate inconsistency that would otherwise surface after a forty-minute build. Its
# findings are real defects in the tree, not style: a guard that cannot fail is the class that
# produced both the inert disk floor below and the self-waiving catalog gate.
step "test contract (governor)"
if PYTHONPATH="$ROOT/scripts" python3 -m testgov ci >"$OUT/testgov.log" 2>&1; then
  echo "testgov: ok"
else
  echo "testgov: FAILED (HIGH findings; see $OUT/testgov.log)"
  grep -E '^\[HIGH\]' "$OUT/testgov.log" | head -20
  fail=1
fi

step "fmt"
if cargo fmt --all --check >"$OUT/fmt.log" 2>&1; then
  echo "fmt: ok"
else
  echo "fmt: FAILED (cargo fmt --all fixes it)"; tail -20 "$OUT/fmt.log"; fail=1
fi

step "clippy (workspace, all targets)"
if cargo clippy --workspace --all-targets -- -D warnings >"$OUT/clippy.log" 2>&1; then
  echo "clippy: ok"
else
  echo "clippy: FAILED"; grep -E '^error' -A6 "$OUT/clippy.log" | head -40; fail=1
fi

if [[ $fail -ne 0 ]]; then
  echo; echo "GATE RED (fmt/clippy). Logs: $OUT"
  exit 1
fi

if [[ $FAST -eq 1 ]]; then
  echo; echo "GATE GREEN (fast: fmt + clippy)"
  exit 0
fi

# Verification-ladder layers below clippy (doc/testing/test-matrix.yaml: MUT-loom-spinlock,
# MUT-kani-robin-hood). Both are opt-in: loom needs the `loom` dev-dependency wired into
# pa-mem's spinlock/refcount, Kani needs the kani toolchain. Each prints SKIPPED with the
# reason instead of a fake green — a guard that cannot fail is the class testgov exists for.
step "loom (pa-mem interleavings)"
if cargo metadata --no-deps --format-version 1 2>/dev/null | rg -q '"loom"'; then
  if ! cargo test -p pa-mem --features loom -- --test-threads=1 >"$OUT/loom.log" 2>&1; then
    echo "loom: FAILED (see $OUT/loom.log)"; sed -n '1,20p' "$OUT/loom.log"; fail=1
  else
    echo "loom: ok"
  fi
else
  echo "loom: SKIPPED (no loom dev-dependency in pa-mem yet — matrix row MUT-loom-spinlock)"
fi

step "kani (pa-mem robin_hood)"
if command -v cargo-kani >/dev/null 2>&1 || cargo kani --version >/dev/null 2>&1; then
  if ! cargo kani -p pa-mem --harness verify_robin_hood_delete >"$OUT/kani.log" 2>&1; then
    echo "kani: FAILED (see $OUT/kani.log)"; sed -n '1,20p' "$OUT/kani.log"; fail=1
  else
    echo "kani: ok"
  fi
else
  echo "kani: SKIPPED (toolchain absent — matrix row MUT-kani-robin-hood, advisory)"
fi

if [[ $fail -ne 0 ]]; then
  echo; echo "GATE RED (verification ladder). Logs: $OUT"
  exit 1
fi

free_gb=$(df -BG --output=avail "$ROOT" | tail -1 | tr -dc '0-9')
if (( free_gb < MIN_FREE_GB )); then
  echo "GATE SKIPPED: ${free_gb}G free, need ${MIN_FREE_GB}G for the parallel test run"
  echo "(clean target artifacts, or set GATE_MIN_FREE_GB to override)"
  exit 2
fi

step "test binaries (one build for every unit)"
# ONE build for the whole workspace before any unit runs. Each of the ~230 units
# otherwise pays its own `cargo` freshness check: measured on the 2026-10-23 gate
# run, 220 units spent 1540s in "Finished `test` profile ... in Ns" against 1692s
# of actual test time — half the suite's wall clock was the cargo CLI re-deciding
# there was nothing to do. `--all-targets` builds test/bin/example targets;
# doctests stay on `cargo test` below, which builds its own.
# `--all-targets` links EVERY test binary at once, which is more concurrent disk
# than the per-unit scheme ever needed: measured 2026-10-23, the build filled the
# last 13G with "rustc-LLVM ERROR: IO failure on output stream: No space left on
# device" and the gate reported a red that was not about the code. Refuse up front
# with a number instead of letting a compiler crash stand in for the diagnosis.
BUILD_MIN_FREE_GB="${GATE_BUILD_MIN_FREE_GB:-25}"
build_free_gb=$(df -BG --output=avail "$ROOT" | tail -1 | tr -dc '0-9')
if (( build_free_gb < BUILD_MIN_FREE_GB )); then
  echo "GATE SKIPPED: ${build_free_gb}G free, need ${BUILD_MIN_FREE_GB}G to build the test targets"
  echo "(clean target artifacts, or set GATE_BUILD_MIN_FREE_GB to override)"
  exit 2
fi
# NOT `--all-targets`: that also links every example and bench, which this gate
# never runs (the units are lib/test/bin, enumerated below) — and the extra link
# jobs are what exhausted the disk on 2026-10-23. `--lib --tests --bins` is exactly
# the set the sharded run then executes.
if ! cargo build --workspace --lib --tests --bins --locked >"$OUT/build-tests.log" 2>&1; then
  echo "test build: FAILED (see $OUT/build-tests.log)"
  tail -20 "$OUT/build-tests.log"
  exit 1
fi

step "tests (sharded, $JOBS parallel, ${free_gb}G free)"
# ci_test_shard.py enumerates exactly what `cargo test --workspace` covers;
# we split that list across worker processes instead of shards-per-runner.
rm -rf "$OUT/tests"; mkdir -p "$OUT/tests"
# The enumeration's exit status decides whether the run is meaningful. It was
# discarded, so a failing enumerator produced an empty list and the gate printed
# "GATE GREEN" with zero units — a green that proves nothing (audit F16; measured
# with a stub: enumeration exit 42 -> "GATE GREEN (0 test units)" rc=0).
if ! python3 scripts/ci_test_shard.py --shard 1 --total 1 --list 2>"$OUT/enumerate.err" \
  | grep 'shard 1:' | sed 's/^ *shard 1: //' > "$OUT/tests/units.txt"; then
  echo "GATE RED (test enumeration failed; see $OUT/enumerate.err)"
  sed -n '1,10p' "$OUT/enumerate.err"
  exit 1
fi
units=$(wc -l < "$OUT/tests/units.txt")
echo "units: $units"
# An enumeration that succeeds but lists nothing is a broken enumerator or a
# filter bug, never "no tests to run": the workspace always has units.
if [ "$units" -eq 0 ]; then
  echo "GATE RED (test enumeration listed 0 units; see $OUT/tests/units.txt)"
  exit 1
fi

run_unit() {
  local unit="$1" pkg rest kind name args
  pkg="${unit%%#*}"; rest="${unit#*#}"; kind="${rest%%:*}"; name="${rest#*:}"
  case "$kind" in
    lib) args=(--lib);;
    bin) args=(--bin "$name");;
    doc) args=(--doc);;
    test) args=(--test "$name");;
    example) args=(--example "$name");;
    examples) args=(--examples --no-run);;
    *) return 0;;
  esac
  local log="$OUT/tests/${pkg}-${kind}-${name//\//_}.log"
  # Free-space floor PER UNIT, not just at the gate's start: a long parallel run fills the
  # disk mid-flight, and an ENOSPC then reads as a code failure in whichever unit happened
  # to be writing (2026-10-21: thirteen "failures" that were all `No space left on
  # device`). Refusing to run is honest; a fake red is not.
  local free_gb
  free_gb=$(df -BG --output=avail "$ROOT" | tail -1 | tr -dc '0-9')
  if (( free_gb < UNIT_MIN_FREE_GB )); then
    echo "SKIP $pkg $kind $name (only ${free_gb}G free, need ${UNIT_MIN_FREE_GB}G)"
    return 1
  fi
  # lib and integration-test units go through nextest: one process per test instead of one
  # shared binary, which is 2.8x on the measured suite (pa-daemon --lib: 384.8s -> 138.7s,
  # 1195 tests). nextest reuses the artifacts cargo already built and keeps its own state
  # under target/nextest, so it neither rebuilds nor perturbs the unit's dependencies.
  # Doc/bin/example units stay on cargo test: nextest does not run doctests.
  local ok=0
  if [[ "$kind" == "lib" || "$kind" == "test" ]] && command -v cargo-nextest >/dev/null 2>&1; then
    # `--color never`: nextest colourises each word, which splits "Starting 0 tests"
    # into escape-interleaved fragments, so the zero-test check below never matched
    # in the gate's own log while matching in a clean one.
    # `--no-tests=warn` states the "every test is #[ignore]d" case directly: warn
    # and exit 0, instead of exit 4 that has to be told apart from a failure.
    if [[ "$kind" == "lib" ]]; then
      cargo nextest run --locked --no-tests=warn --color never -p "$pkg" --lib >"$log" 2>&1
    else
      cargo nextest run --locked --no-tests=warn --color never -p "$pkg" --test "$name" >"$log" 2>&1
    fi
    local status=$?
    if [[ $status -eq 0 ]]; then
      ok=1
    elif [[ $status -eq 4 ]] && rg -q --fixed-strings 'Starting 0 tests' "$log"; then
      # Belt for an older nextest without `--no-tests`: an empty unit is reported,
      # never silently green, because a file that LOST its tests must stay visible.
      echo "skip $pkg $kind $name  (every test is #[ignore]d)"
      return 0
    fi
  else
    cargo test --locked -p "$pkg" "${args[@]}" >"$log" 2>&1 && ok=1
  fi
  if [[ $ok -eq 1 ]]; then
    echo "ok   $pkg $kind $name"
  else
    echo "FAIL $pkg $kind $name  -> $log"
    grep -E '^\s*FAIL|panicked at|^test .* FAILED' "$log" | head -5
    return 1
  fi
}
# Suites that are correct but CPU-timing-sensitive: their pty pacing and
# kernel-dispose deadlines are calibrated for a runner with the whole machine
# (CI gives each shard its own 4-core runner). Under this gate's local
# parallelism they flake while passing alone; run them one at a time at the
# end so the gate still covers them.
SERIAL_UNITS="pa-cli#test:slow_drain_exit_guard_e2e
pa-cli#test:termios_process_exit_e2e
pa-cli#test:kitty_early_typing_e2e
pa-daemon#test:kernel_dispose_e2e
pa-daemon#test:idle_passivation_e2e"

export -f run_unit
export OUT

# jobs: one unit per process, JOBS at a time; the timing-sensitive suites
# are held back for a serial tail.
printf '%s\n' "$SERIAL_UNITS" > "$OUT/tests/serial.txt"
grep -vxF -f "$OUT/tests/serial.txt" "$OUT/tests/units.txt" > "$OUT/tests/parallel.txt"
echo "parallel: $(wc -l < "$OUT/tests/parallel.txt"), serial tail: $(wc -l < "$OUT/tests/serial.txt")"

# `xargs` exits 123 when any invocation failed, 124/125 for its own failures.
# The status was discarded, so a launcher that died without writing a FAIL line
# still produced a green gate (audit F16; measured with a stub: one unit listed,
# launcher exit 42 -> "GATE GREEN (1 test units)" rc=0). The grep below can only
# see units that RAN.
xargs -P "$JOBS" -I{} bash -c 'run_unit "$@"' _ {} < "$OUT/tests/parallel.txt" \
  >"$OUT/tests/summary.log" 2>&1
xargs_status=$?
echo "--- serial tail" >> "$OUT/tests/summary.log"
while IFS= read -r unit; do
  [[ -z "$unit" ]] && continue
  run_unit "$unit" >> "$OUT/tests/summary.log" 2>&1
done < "$OUT/tests/serial.txt"
test_fail=$(grep -c '^FAIL' "$OUT/tests/summary.log" || true)
# Every unit must have produced a verdict, and the launcher must have succeeded.
# "No FAIL line" is not proof that a unit ran: the verdict is now the COUNT of
# results against the count of units, plus the launcher's own status.
ran_ok=$(grep -c '^ok ' "$OUT/tests/summary.log" || true)
ran_skip=$(grep -c '^skip ' "$OUT/tests/summary.log" || true)
ran=$((ran_ok + ran_skip))
tail -3 "$OUT/tests/summary.log"
if [[ "$xargs_status" -ne 0 ]]; then
  echo "tests: the parallel launcher failed (exit $xargs_status) after $ran of $units units"
  fail=1
elif [[ "$test_fail" -gt 0 ]]; then
  echo "tests: $test_fail failing units"
  grep '^FAIL' "$OUT/tests/summary.log"
  fail=1
elif [[ "$ran" -ne "$units" ]]; then
  echo "tests: $ran of $units units reported a verdict — the fleet lost units"
  grep -E '^FAIL' "$OUT/tests/summary.log" | head -5
  fail=1
else
  echo "tests: all $units units green ($ran_ok ok, $ran_skip skipped)"
fi

if [[ $fail -ne 0 ]]; then
  echo; echo "GATE RED. Logs: $OUT"
  exit 1
fi
echo; echo "GATE GREEN (fmt + clippy + $units test units)"
