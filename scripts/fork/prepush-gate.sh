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
# The repo root, ASSIGNED and exported. `317cfc4a1` replaced `ROOT="$PWD"` with a bare
# `export ROOT` while fixing G-9, so `$ROOT` was unbound, `set -u` aborted on the next
# line, and the gate never ran again — a fix that killed the thing it was fixing. The
# export is still what G-9 needs: run_unit's per-unit disk floor runs under
# `xargs bash -c`, where an unexported variable is empty.
ROOT="$PWD"
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

# Target liveness: the gate shares one working tree with other agents, and a `cargo clean` or
# a vanished worktree copy by ANY of them leaves this run compiling against a path that no
# longer describes the sources it read. The disk floor above cannot see that: 30 GB free and
# a dead target look identical to it.
#
# The check is cheap and runs ONCE, before the first build: the marker Cargo itself writes
# into a fresh target (`CACHEDIR.TAG`) must exist and the directory must be writable. A
# failure here names the cause, because the alternative is a compile error 40 minutes in
# that reads like a code defect.
check_target_liveness() {
  local target="$ROOT/target"
  if [[ ! -d "$target" ]]; then
    echo "target missing at $target: this is a clean tree (fine), so the build starts cold" >&2
    return 0
  fi
  if [[ ! -f "$target/CACHEDIR.TAG" ]] && compgen -G "$target/debug/*" >/dev/null; then
    echo "target at $target has artifacts but no CACHEDIR.TAG: another tool recreated it" >&2
    echo "  the gate cannot tell whether its fingerprints match this tree; run cargo clean" >&2
    return 1
  fi
  if ! touch "$target/.gate-liveness" 2>/dev/null; then
    echo "target at $target is not writable: a build here fails at the first artifact" >&2
    return 1
  fi
  rm -f "$target/.gate-liveness"
  return 0
}

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

step "fork surface (native tools, fork-owned files)"
# Cheap, and it is the only detector for the class the v0.10.3-beta.1 merge produced:
# upstream deleted modules the fork consumed, and nothing went red until a human read
# `cargo check`. Runs before fmt so a dropped native tool fails in seconds.
# The guard runs twice: once against the tree, and once against a fixture it mutates
# (`--self-test`). The second run is the point — a guard that cannot fail gates nothing, and
# this one was green on a fixture that was already red (review #40).
if bash "$ROOT/scripts/fork/check-fork-surface.sh" >"$OUT/fork-surface.log" 2>&1 \
   && bash "$ROOT/scripts/fork/check-fork-surface.sh" --self-test \
        >"$OUT/fork-surface-self-test.log" 2>&1; then
  grep '^  ok' "$OUT/fork-surface.log" | sed 's/^  ok    /fork: /'
  echo "fork: self-test ok (every injected break turned its own check red)"
else
  # Exit immediately: a broken surface contract makes every later stage
  # meaningless (clippy on a tree that lost a native tool is wasted minutes), and
  # the point of this step is to fail in seconds.
  echo "fork surface: FAILED"; cat "$OUT/fork-surface.log" "$OUT/fork-surface-self-test.log"
  echo; echo "GATE RED (fork surface). Logs: $OUT/fork-surface*.log"
  exit 1
fi

step "target liveness"
if check_target_liveness; then
  echo "target: live"
else
  echo "target: DEAD (see above)"; fail=1
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

step "gate contract (the gate's own tests)"
# `scripts/test_prepush_gate.py` guards THIS script: its prologue reads its `$ROOT`,
# `run_unit` draws skip-vs-pass verdicts, the summary must not call a skipped unit
# green, and the Verus lane must stay wired and advisory. It existed and nothing ran
# it, so the file's own class of defect (a guard nobody invokes) applied to the guard
# of the gate. Cheap: measured 0.4-0.6 s for the two fast classes, and the one test
# that executes `--fast` end to end is excluded here because the gate is already
# running.
if python3 scripts/test_prepush_gate.py \
    RunUnitVerdicts VerusAdvisoryLane VerusMirrorDriftReachability TheGateRunsItsOwnTests \
    FastExitRespectsTheContractVerdict ParitySkipsAreCounted \
    >"$OUT/gate-contract.log" 2>&1; then
  echo "gate contract: ok ($(grep -E '^Ran [0-9]+ tests' "$OUT/gate-contract.log"))"
else
  echo "gate contract: FAILED (see $OUT/gate-contract.log)"; tail -20 "$OUT/gate-contract.log"; fail=1
fi

step "verus mirror drift (advisory)"
# The one layer of the Verus practice that must run on EVERY machine: it needs no
# toolchain (measured 0.05 s) and it catches the drift the verifier structurally cannot
# see — a constant changed in a source and not in its mirror, which turns the proof
# into an assurance about code that no longer exists. Advisory per AGENTS.md: the exit
# code is untouched. Placed before the `--fast` exit because that is the run developers
# actually do; the proof and mutation lanes below need the toolchain and stay in the
# full gate.
if python3 scripts/verus-mirror-check.py >"$OUT/verus-mirror.log" 2>&1; then
  echo "verus-mirror: ok ($(tail -1 "$OUT/verus-mirror.log"))"
else
  echo "verus-mirror: DRIFT (advisory — never blocks a merge; $OUT/verus-mirror.log)"
  grep -E 'DRIFT|not (found|resolvable)' "$OUT/verus-mirror.log" | sed 's/^/  /'
fi

# The accumulated verdict, not the formality of reaching this line: the gate's own
# contract tests run above and set `fail` on a red verdict, and this exit used to ignore
# it — `--fast` printed GATE GREEN and returned 0 with a broken gate contract. The
# check is here rather than repeated per lane because the full gate has its own exits
# and this one is the only path that skips them.
if [[ $fail -ne 0 ]]; then
  echo; echo "GATE RED (fast: fmt, clippy or the gate contract failed). Logs: $OUT"
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
step "verus proofs and proof depth (advisory)"
# ADVISORY BY CONTRACT. AGENTS.md ("Formal verification (Verus)"): a proof that does
# not close must never block a merge. So these lanes run and report, and never set
# `fail` — including the mutation lane, which used to be BLOCKING here and thereby
# contradicted the very rule it was written under (measured: a kill rate below the
# floor printed GATE RED with every cargo test green).
#
# Why it exists at all: this lane was wired NOWHERE except `verus-mutants.sh`.
# `scripts/verus-check.sh` — the script that proves the mirrors and diffs their
# constants — had no caller at all, so a proof could rot and no gate would notice.
# The drift half of it runs earlier (before the `--fast` exit) because it needs no
# toolchain; the proof half lives here because it does.
#
# A proof that passes can still be worthless: mutation testing asks whether the
# verifier REJECTS a mutated model, and a survivor is a spec gap. Measured
# 2026-10-27: 16 sampled mutants, 92.3% kill; measured 2026-11-01 on a loaded box:
# 3 m 49 s at 4 jobs. Both lanes skip cleanly without their toolchain.
verus_advisory_failures=0
verus_lane() {
  local label="$1" log="$2"
  shift 2
  local status=0
  "$@" >"$log" 2>&1 || status=$?
  if [[ $status -ne 0 ]] && grep -qE 'baseline failed|cargo metadata. exited with an error' "$log"; then
    # An INFRASTRUCTURE failure, not a survivor: the baseline build never ran, and
    # `verus-mutants.sh` reports that as "the kill rate fell below the floor (a real
    # survivor)" — a claim about the proofs the evidence does not support. The script
    # has a SKIPPED arm for its other infrastructure failure (`baseline timed out`);
    # this is the same class and needs the same arm. Reported here so the gate never
    # repeats the claim; the durable fix belongs in scripts/verus-mutants.sh.
    #
    # The cause measured on this tree: `cargo-verus-mutants` copies its package under
    # `$TMPDIR/.tmpXXXX/source` and runs `cargo metadata` there. This gate exports
    # TMPDIR="$ROOT/.tmp", so the copy lands INSIDE the workspace and cargo answers
    # "current package believes it's in a workspace when it's not". The mutation lane
    # therefore runs with `env -u TMPDIR` (see its call below).
    echo "$label: INFRASTRUCTURE FAILURE — the baseline build did not run, so nothing"
    echo "  was proved either way (not a survivor). Cause, from $log:"
    grep -m1 -E 'baseline failed' "$log" | sed 's/^/  /'
  elif [[ $status -ne 0 ]]; then
    verus_advisory_failures=$((verus_advisory_failures + 1))
    echo "$label: FAILED (advisory — reported, never blocks a merge; $log)"
    sed -n '1,12p' "$log" | sed 's/^/  /'
  elif grep -qiE 'skipp(ed|ing)' "$log"; then
    echo "$label: SKIPPED — $(grep -iE 'skipp(ed|ing)' "$log" | tail -1)"
  else
    # The lane's OWN verdict line, not the log's last line. `verus-mutants.sh` sets its
    # LOG to "$OUT/verus-mutants.log" and redirects cargo into it, so that file ends on
    # cargo's `summary: <path>` line; matching the script's prefix reports the kill rate
    # instead of a file name.
    local verdict
    verdict=$(grep -oE "${label%% *}: .*" "$log" | tail -1)
    echo "$label: ok — ${verdict:-$(tail -1 "$log")}"
  fi
}
verus_lane "verus-check (proofs)" "$OUT/verus-check.log" bash scripts/verus-check.sh
# `.stdout.log`, not `.log`: verus-mutants.sh sets its own LOG to "$OUT/verus-mutants.log"
# and redirects cargo into it, while this step captures the script's stdout — two streams
# on one path left interleaved garbage (measured: a first line reading `aseline: cargo
# verus build`, one byte of `baseline` already overwritten).
#
# `env -u TMPDIR` for THIS lane only (see the infrastructure arm above): the tool copies
# its package under "$TMPDIR/.tmpXXXX/source", and the gate's repo-local TMPDIR puts that
# copy inside the workspace. Every test unit keeps the repo-local scratch; only this tool
# needs the isolation it assumes it has. Measured 2026-11-01: exit 1 with TMPDIR set to
# "$ROOT/.tmp", exit 0 with it unset — same tree, same minute.
verus_lane "verus-mutants (proof depth)" "$OUT/verus-mutants.stdout.log" \
  env -u TMPDIR bash scripts/verus-mutants.sh
# An advisory lane that fails silently is a lane nobody reads. The count is printed so
# a reader of a green gate still learns that a proof needs attention.
if [[ $verus_advisory_failures -eq 0 ]]; then
  echo "verus: advisory lanes clean"
else
  echo "verus: $verus_advisory_failures advisory failure(s) — NOT blocking (AGENTS.md: a"
  echo "       proof that does not close never blocks a merge). See $OUT/verus-*.log."
fi

step "loom (pa-mem interleavings)"
# The alternative lane that DOES work today is Shuttle, and it is wired
# (`.github/workflows/shuttle.yml` runs pa-mem's DST test on a schedule). This
# step stays for the Loom variant, which needs a `loom` dev-dependency and
# cfg(loom) module copies -- matrix row MUT-loom-spinlock is `status: gap`.
# Kept honest the same way as kani above: both the dependency AND a loom-gated
# test have to exist before a run would mean anything.
if ! cargo metadata --no-deps --format-version 1 2>/dev/null | rg -q '"loom"'; then
  echo "loom: SKIPPED (no loom dev-dependency in pa-mem — matrix row MUT-loom-spinlock is a gap; shuttle covers this protocol)"
elif ! grep -rq 'cfg(loom)' crates/pa-mem/src/ 2>/dev/null; then
  echo "loom: SKIPPED (loom dep present but no cfg(loom) test in pa-mem — nothing to interleave)"
elif ! cargo test -p pa-mem --features loom -- --test-threads=1 >"$OUT/loom.log" 2>&1; then
  echo "loom: FAILED (see $OUT/loom.log)"; sed -n '1,20p' "$OUT/loom.log"; fail=1
else
  echo "loom: ok"
fi

step "kani (pa-core platform decisions)"
# The lane moved from pa-mem's robin_hood to pa-core's platform code, and for
# the reason the audit gave: RobinHoodMap has no consumer outside pa-mem's own
# tests, so a proof about it proves nothing about the product, while
# `planned_open_file_limit` decides a process-wide descriptor limit at startup.
# scripts/kani-check.sh carries the toolchain workaround and its measurements;
# a missing bundle is a SKIP with the install command, never a fake green.
if bash scripts/kani-check.sh >"$OUT/kani.log" 2>&1; then
  if grep -q 'kani-check: no Kani bundle' "$OUT/kani.log"; then
    echo "kani: SKIPPED (no Kani bundle — cargo install --locked kani-verifier)"
  elif grep -q 'kani-check: the bundle needs' "$OUT/kani.log"; then
    echo "kani: SKIPPED (bundle needs a pinned rustup toolchain that is absent)"
  else
    verified=$(grep -c 'VERIFICATION:- SUCCESSFUL' "$OUT/kani.log" 2>/dev/null || echo 0)
    echo "kani: ok ($verified harnesses)"
  fi
else
  echo "kani: FAILED (see $OUT/kani.log)"; sed -n '1,20p' "$OUT/kani.log"; fail=1
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
  # A unit that SKIPS prints its own line from a PASSING test, and both runners CAPTURE
  # the stdout/stderr of a passing test — the line never reaches `$log`, so the counter at
  # the end of this script greps a string that was never written. The flag is scoped to the
  # units that can skip because it is not free: on `pa-graph#lib` it took the log from
  # 56 KiB to 233 KiB (x4) and the unit from 2.95 s to 3.30 s, and the gate has ~300 units.
  #
  # Upstream removed the TS comparison tests (#3870), so `differential_cli`,
  # `export_live_differential` and `package_e2e` no longer exist. What still skips at
  # runtime: the upgrade-compat pair (its TS golden may be absent) and the live-network
  # lanes (jev/subq) — those are the units the counter at the end of this script needs.
  local reveal=0
  case "$unit" in
    pa-cli#test:lock_compat_e2e|pa-jev#test:live_subq|pa-core#test:jev_live_eval) reveal=1;;
  esac
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
  local ok=0 reveal_flag=""
  if (( reveal )); then reveal_flag="--success-output=final"; fi
  if [[ "$kind" == "lib" || "$kind" == "test" ]] && command -v cargo-nextest >/dev/null 2>&1; then
    # `--color never`: nextest colourises each word, which splits "Starting 0 tests"
    # into escape-interleaved fragments, so the zero-test check below never matched
    # in the gate's own log while matching in a clean one.
    # `--no-tests=warn` states the "every test is #[ignore]d" case directly: warn
    # and exit 0, instead of exit 4 that has to be told apart from a failure.
    # The reveal flag is LAST: `--success-output` takes a value, and a bare `$reveal_flag`
    # before `-p` reads as `--success-output=final -p pkg`, which is fine, but extending
    # the string later is a trap — one more word and the value is a flag.
    if [[ "$kind" == "lib" ]]; then
      cargo nextest run --locked --no-tests=warn --color never -p "$pkg" --lib \
        $reveal_flag >"$log" 2>&1
    else
      cargo nextest run --locked --no-tests=warn --color never -p "$pkg" --test "$name" \
        $reveal_flag >"$log" 2>&1
    fi
    local status=$?
    # Exit 0 alone does NOT mean the unit passed: `--no-tests=warn` turns "nothing ran"
    # into exit 0, so nextest's `Starting 0 tests` line is what says the unit had tests
    # to run. Review #53: `pa-jev#test:live_subq` and `pa-core#test:jev_live_eval` hold
    # one `#[ignore]`d test each, exited 0, printed `ok`, and the summary counted them
    # as green — a gate certifying a unit it never executed. Exit 4 stays the belt for
    # an older nextest without `--no-tests`. `grep`, not `rg`: a missing matcher must
    # not be able to answer "this unit was tested" for every unit.
    if [[ $status -eq 0 || $status -eq 4 ]] \
      && grep -qF -- 'Starting 0 tests' "$log"; then
      # A skip with its reason: every test `#[ignore]`d is untested, and so is a target
      # that lost the tests it used to have.
      if grep -qE '\([0-9]+\ tests?\ skipped\)' "$log"; then
        echo "skip $pkg $kind $name  (every test is #[ignore]d)"
      else
        echo "skip $pkg $kind $name  (the target lists no tests)"
      fi
      return 0
    fi
    if [[ $status -eq 0 ]]; then
      ok=1
    fi
  else
    # `-- --nocapture` on the cargo-test path: the same capture rule, the same reveal.
    local nocapture=()
    if (( reveal )); then nocapture=(-- --nocapture); fi
    cargo test --locked -p "$pkg" "${args[@]}" "${nocapture[@]}" >"$log" 2>&1 && ok=1
    # The same rule on the cargo-test path, so "nothing executed" is a skip whichever
    # runner produced it: `0 passed; 0 failed; N ignored` is "the tests exist and every
    # one was skipped", and `0 passed; 0 failed; 0 ignored` is a target with nothing to
    # run. Both exit 0, and exit 0 is where the gate used to stop asking.
    if [[ $ok -eq 1 ]] \
      && ! grep -qE 'test result: ok\. [1-9][0-9]* passed' "$log" \
      && grep -qE '^test result: ok\. 0 passed; 0 failed;' "$log"; then
      if grep -qE '^test result: ok\. 0 passed; 0 failed; [1-9][0-9]* ignored' "$log"; then
        echo "skip $pkg $kind $name  (every test is #[ignore]d)"
      else
        echo "skip $pkg $kind $name  (the target lists no tests)"
      fi
      return 0
    fi
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
elif [[ "$ran_ok" -eq 0 ]]; then
  # A degenerate run: every unit skipped. Before this, the line below reported "all N
  # units green (0 ok, N skipped)", which is the same sentence a healthy run prints
  # (review #53). A gate whose every unit is skipped has proved nothing, so it must be
  # RED — the one shape where "green" and "did nothing" were indistinguishable.
  echo "tests: DEGENERATE — all $units units skipped, 0 ran; the gate proved nothing"
  fail=1
else
  # "all N units green" was the same sentence for a healthy run and for one where units
  # never executed a test (review #53). A skip is not a failure — an all-`#[ignore]`d
  # live-smoke unit is legitimately untested — but it must not read as tested, so the
  # counts are printed, and the units whose tests EXIST and were skipped are named: those
  # are the ones a reader would otherwise believe were executed.
  ran_no_tests=$(grep -c '^skip .*(the target lists no tests)' "$OUT/tests/summary.log" || true)
  echo "tests: $units of $units units accounted for ($ran_ok ok, $ran_skip skipped:" \
       "$ran_no_tests with no tests to run)"
  if [[ "$ran_skip" -gt "$ran_no_tests" ]]; then
    grep -v '(the target lists no tests)' "$OUT/tests/summary.log" | grep '^skip '
  fi
fi

# A test that skips at RUNTIME returns NORMALLY: it prints its own SKIPPED line and exits
# 0, so the runner counts it as passed and the per-unit verdict above says `ok` although
# nothing was compared (review #1). Cargo has no runtime skip, so the honest surface is a
# count.
#
# One fact the first version of this counter got wrong, measured: `eprintln!` from a
# PASSING test is captured by nextest and by cargo test and never written to the log (0
# `SKIPPED` lines by default, 3 with `--success-output=final`). That is why run_unit passes
# the revealing flag for the units that can skip — a counter over logs nobody wrote to
# counts zero forever.
#
# Advisory, not red: the skip contract is deliberate, and an absent comparison must not be
# reported as a completed one. `$NF`, not `$2`: with ONE log file `grep -c` prints a bare
# count, with several it prints `path:count`, and a two-file gate would otherwise sum zero.
skipped=$(grep -hc 'SKIPPED: ' "$OUT"/tests/*.log 2>/dev/null \
  | awk '{ s += $NF } END { print s + 0 }')
if [ "${skipped:-0}" -gt 0 ]; then
  echo "skips: $skipped test(s) skipped at runtime and were counted as ok by the runner"
fi

if [[ $fail -ne 0 ]]; then
  echo; echo "GATE RED. Logs: $OUT"
  exit 1
fi
echo; echo "GATE GREEN (fmt + clippy + $units test units)"
