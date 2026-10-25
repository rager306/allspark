# testgov — the ADR-0001 test-contract governor

Proves the rows in `doc/testing/test-matrix.yaml` against this tree. It does not run tests and
reports no coverage percentage: it answers "does the named oracle exist, is it non-ignored, and
does every seat of the contract hold".

    PYTHONPATH=scripts python3 -m testgov ci           # all checks; exit 1 on any HIGH
    PYTHONPATH=scripts python3 -m testgov ci --changed  # only rows whose cited code changed
    PYTHONPATH=scripts python3 -m testgov list
    PYTHONPATH=scripts python3 -m testgov explain UI-picker-nonempty
    PYTHONPATH=scripts python3 -m testgov map
    PYTHONPATH=scripts python3 -m testgov selftest     # every check must fail its fixture

## Why a real index, not a regex per check

One pass over `crates/**/*.rs` records, per function, its attributes, `#[ignore]`, whether it is
an attributed test, and its nesting. Doing that per check meant four passes and four chances to
disagree — the same shape as the three daemon classifiers this ADR exists to remove.

## The load-bearing command is `selftest`

A check that cannot fail is the exact defect class this tool exists to catch, so `selftest` runs
every check against `fixtures/<check-name>/`, which must produce a finding. A green selftest is a
precondition for trusting `ci`. Two checks initially passed their own fixture; that is the
failure mode working as intended.

## Current state (2026-10-24)

`ci` exits 1 on four `inert-guard` findings, all real and all in the tree:

| file:line | what cannot fail |
|---|---|
| `scripts/release/bundle_catalog.py:842` | the producer waives its own gate (`allow_small = … or options.fixture`) while `verify` does not, so `generate --fixture` then `verify` exits 1 |
| `scripts/fork/prepush-gate.sh:100,120,177` | `$ROOT` is read inside an `xargs bash -c` child but never exported, so `df` gets an empty operand and the per-unit disk floor never fires |

Both are recorded as matrix rows (`GATE-inert-guard`) and both are P1.

## What it deliberately does not do

- No `cargo` invocation. Existence and reachability are cheap; execution is the gate's job.
- No count of tests, lines, or percentages. The matrix is a map of oracles.
- No reading of `.research/` (gitignored): a cited source must be committed.
- No judgement of whether an oracle asserts the right thing. `kill_mutation` is the reviewer's
  handle, and `mutation` testing (G-6) is the machine's.
