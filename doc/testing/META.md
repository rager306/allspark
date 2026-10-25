# testgov meta-prompt (constitution)

Version: 1
Not an ADR. Not product SoT. No model executes these checks: the governor is an interpreter.

This is the process analog of ADR-0001: a versioned constitution, a dictionary layer
(`doc/testing/*`), and one FSM (`doc/testing/fsm.yaml`). Adding a *check* is a dictionary edit.
Adding a *kind* is a kernel change: append it to `ALLOWED_KINDS` in `scripts/testgov/fsm.py`,
implement it, and add a fixture that proves it fails.

## Fail-closed cycle

```text
load META (this file)                 -> missing: HIGH, stop
load matrix (doc/testing/test-matrix.yaml)
load fsm (doc/testing/fsm.yaml)
walk FSM                              -> unknown kind or missing transition: HIGH, stop
run checks named by reached states
run selftest                          -> a check that cannot fail is HIGH
decide                                -> any HIGH: terminal fail; else ok
```

Unknown state kind -> HIGH (`unknown-fsm-kind`). Missing dictionary -> HIGH. A transition with no
`event:` is HIGH. Terminal `fail` is reachable from every check state: the machine must not be
able to skip a failing check by taking a path that omits it.

## How to add a check

A "must" becomes a governor requirement only when it has a matrix row naming it as an `oracle`
and a `Check` subclass with a fixture. Protocol:

1. Add the row to `doc/testing/test-matrix.yaml` with `kill_mutation` (the edit that must redden it).
2. Add the `Check` subclass in `scripts/testgov/kinds.py` and register it in `ALL_CHECKS`.
3. Add `scripts/testgov/fixtures/<check-name>/` holding the tree state that must produce a finding.
4. Run `selftest`: it must report the check as failing its fixture. A check that passes its own
   fixture is itself a finding — that is the class this tool exists to catch.

Silence is not a requirement: a row with no oracle is `gap`, never `killed`.

## Severities

Two, never three. HIGH fails `ci`; LOW advises. A third severity invites negotiation at review.

## What the interpreter does not do

- It does not run `cargo`. Execution is the gate's job; existence and reachability are cheap.
- It does not count tests, lines, or percentages. The matrix is a map of oracles.
- It does not read `.research/` (gitignored): a cited source must be committed.
- It does not judge whether an oracle asserts the right thing. `kill_mutation` is the reviewer's
  handle; `mutation` testing (G-6) is the machine's.
