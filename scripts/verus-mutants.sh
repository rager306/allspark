#!/usr/bin/env bash
# Verification-depth lane: mutation-test the PROOFS, not the code.
#
# A proof that passes can still be worthless. `cargo-verus-mutants` mutates the
# executable model under a proof and re-verifies: a mutant the verifier still
# accepts is a hole in a SPEC (some clause does not constrain the behaviour it
# should), and a mutant nothing can distinguish is equivalent.
#
# MEASURED on this tree, 2026-10-27: 16 mutants, 12 killed by Verus, 3 invalid,
# 1 equivalent (documented in verus/mirrors/.verus-mutants.toml), 92.3% kill
# rate, ~40 s. The first run had 80% and every survivor was a REAL gap:
#   - `None if limit` mutated to `None if true` verified: nothing required the
#     no-gate arm to keep going when budget remained;
#   - the failed-gate-under-limit reason was unconstrained.
# Both are now clauses in verus/pa_core_gates.rs and their mutations die.
#
# The mirrors crate is inlined rather than `#[path]`-included because discovery
# walks a PACKAGE's own files. It is `exclude`d from the cargo workspace, so no
# build or fmt reads it (AGENTS.md: verus/ stays outside the workspace).
set -uo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
TOOLCHAIN="$ROOT/verus/toolchain"

if [ ! -x "$TOOLCHAIN/cargo-verus" ]; then
  echo "verus-mutants: no Verus toolchain at verus/toolchain — skipping (see verus/README.md)"
  exit 0
fi
if ! command -v cargo-verus-mutants >/dev/null 2>&1; then
  echo "verus-mutants: cargo-verus-mutants not installed — skipping (cargo install --locked cargo-verus-mutants)"
  exit 0
fi

cd "$ROOT/verus/mirrors" || exit 1
# The toolchain ships cargo-verus; put it on PATH only for this run.
if PATH="$TOOLCHAIN:$PATH" cargo verus-mutants run --jobs "${VERUS_MUTANTS_JOBS:-4}" \
    --minimum-kill-rate "${VERUS_MUTANTS_MIN_KILL_RATE:-0.9}" "$@" >"${OUT:-/dev/null}/verus-mutants.log" 2>&1; then
  rate=$(grep -o '[0-9.]*% kill rate' "${OUT:-/dev/null}/verus-mutants.log" 2>/dev/null | tail -1)
  echo "verus-mutants: ok${rate:+ ($rate)}"
else
  echo "verus-mutants: FAILED — a proof accepted a mutant it should reject"
  sed -n '1,40p' "${OUT:-/dev/null}/verus-mutants.log" 2>/dev/null || true
  exit 1
fi
