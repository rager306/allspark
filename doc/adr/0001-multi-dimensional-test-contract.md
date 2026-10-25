---
id: ADR-0001
title: Multi-dimensional test contract with a machine-checked coverage governor
status: Proposed
lifecycle: "[bounded]"
date: 2026-10-24
supersedes: none
related: []
---

# ADR-0001: Multi-dimensional test contract with a machine-checked coverage governor

## Status

**Proposed [bounded]**. The contract (Part A) is specified in full and is the normative
reference for every later test decision. The governor (Part B) is specified as a *design*; its
first implementation is `scripts/testgov/` and it ships **advisory** (`--fail-on-warn` off) until
the matrix rows it must prove exist for the four P0 gaps (§C). Nothing in this ADR claims that
the product is tested: it claims that **the oracles are named, owned, and machine-verified to
exist**.

Every architectural or state claim below carries a lifecycle tag:
`[validated]` (measured, reproducible), `[bounded]` (implemented, scope stated),
`[smoke]` (local, skip-capable, not promotion proof), `[proposed]` (design only),
`[deferred]` (decided against, for now).

## Context

### The incident this ADR exists for

A user reported "the providers are there, the models are not" in the TUI, with a `429` from z.ai.
Measured outcome:

| claim | verdict | evidence |
|---|---|---|
| z.ai 429 is our defect | **false** | direct probe returns `code=1113 Insufficient balance` for `glm-5.3`, `glm-5.3-flash`, `glm-4.7` [validated] |
| the TUI was served by a different build than it is | **true** | live daemon `appVersion=0.10.1-beta.35` vs client `0.10.2-beta.3`; `doctor --json` printed `status:"stale"` in the same second [validated] |
| some test should have caught it | **true** | `rg 'appVersion' crates/pa-cli/src/interactive_mode/daemon.rs crates/pa-tui/src/daemon_client.rs` → 0 hits [validated] |
| the release ships a real model catalog | **false** | published `fork.010` `libexec/models.bundled.json` = 46 models, 46/46 `fixture-*`; the picker shows `fixture-zai-42` [validated] |

Three independent facts about **how** this stayed invisible:

1. **Three classifiers answer one question.** `probe_daemon`
   (`crates/pa-cli/src/interactive_mode/daemon.rs:37-41`), `classify_reachable`
   (`crates/pa-cli/src/daemon_discovery/mod.rs:416-425`), `validate_successor`
   (`crates/pa-cli/src/update_flow/legacy_restart.rs:439-443`) each compare a different field set.
   None compares `buildId`. [validated]
2. **A partial asset is worse than a broken one.** `chain.rs:183` `Some(bundled) => bundled` is a
   *replacement*; `pinning.rs:85` drops unpinnable rows silently. Measured, five states:
   absent → 4 rows (compiled fallback); `{"models":[]}` → same 4; **one** anthropic row → 3 rows
   with `claude-fable-5` and `glm-4.7` gone; **one** zai row → `fixture-zai-42` and anthropic gone
   entirely; one unpinnable row → 4 rows. Nothing warns; `doctor` has no catalog check. [validated]
3. **The check that would have caught it cannot fail.** `run_ps` returns `()` (`stop.rs:28`);
   `run_doctor` ends in `handled()` → exit 0 (`public_command.rs:473`); `run_reap` returns `i32`
   and is **discarded** (`public_command.rs:462-467`). Measured exit 0 on a live stale daemon.
   [validated]

### Why "more tests" is not the answer

We have 7 716 inline test attributes in 732 files, 239 integration targets, 16 PTY targets,
17 QA tasks over 4 lanes, 12 workflows, and a local gate of ~280 units. That is **5/5 on the
levels axis** against six external repositories [validated]. The defect above passed all of it.

The reason is a **conflict between practice axes**, not a missing layer:

```
N3 snapshot-testing  --✗3-->  N9 non-empty-invariant
```

A snapshot of an empty picker is a *valid* snapshot. Adopting snapshots — the practice we
appear to lack — cannot reach the invariant we actually lack. Likewise 43 headless targets
substitute for PTY coverage in review while being structurally unable to observe a capability
probe, a resize, or a process exit.

So the contract must be **multi-dimensional**: an oracle is not "a test exists", it is
*(layer, oracle kind, semantic subject, failure semantics, owner)*. A checker must prove the
named oracle exists, is not ignored, is not a tautology, and is reachable from the surface it
claims to cover.

## Decision

**Part A — the contract.** Every test obligation in this repository is a row in
`doc/testing/test-matrix.yaml`, and every row's `oracle` list names real `fn`s. The row schema is
normative; the checker enforces it (Part B). A behaviour with no row is **not tested by
contract**, regardless of how many assertions exist.

**Part D — the ontology.** The contract is a **graph** synthesised from six measured reports
(~5 900 lines): **32 practice nodes** in four planes, **seven typed edges** (`escalates_to`,
`requires`, `strengthens`, `absorbs`, `✗conflicts`, `substitutes`, `masks`, weighted 1–3), a
**19-kind oracle taxonomy** (we have 9), and **15 named gaps**. Our verdict per node is
`have` 12 / `partial` 13 / `absent` 7, and the absent nodes cluster in exactly two places —
*identity* (`N22`, `N23`, `N18`) and *failure legibility* (`N9`, `N11`, `N31`, `N32`). The single
reference implementation's eight practices are **one row** of this graph (§D.5), not the graph
itself. **The graph changes the plan order** from "cheapest first" to "dependency order"
(§D.8), and three of its edges say a practice we thought we lacked cannot reach the gap we have
(§D.1).

**Part B — the governor.** `scripts/testgov/` is a fail-closed checker with two severities
(`HIGH` fails, `LOW` advises), a `Check` protocol, and a `--changed` mode for pre-commit. It
proves rows against the tree; it does not run tests and does not report a coverage percentage.

**Part C — the first rows.** The four P0 gaps get rows whose oracles must be written red-first.

## Part A — the test contract

### A.1 Identity of a row

A row is one **invariant**, not one test. One invariant may have several oracles.

```yaml
- id: ID-build-same-build                # <PLANE>-<area>-<what>
  plane: product                          # product | harness | ui
  invariant: >                            # one sentence; what must always be true
    A client refuses to serve a session from a daemon process whose build identity
    differs from its own, and says so.
  layer: decision                         # see A.3
  axes:                                   # see A.4 — the multi-dimensional part
    code:      [pa-cli/interactive_mode/daemon.rs]
    logic:     [classify_reachable]
    semantics: [buildId, appVersion]
    ui:        [banner:version-mismatch]
    flow:      [attach]
  oracle:
    - the_mismatched_build_is_not_accepted
    - probe_and_classifier_agree_on_one_hello
  kill_mutation: >                        # what single edit makes this row red
    Delete the buildId comparison; the row's oracles must fail.
  status: gap                             # see A.5
  gate: default                           # default | serial-tail | advisory
  owner: ADR-0001
  source: incident 2026-10-24
```

`kill_mutation` is mandatory and is the anti-vacuity field: a row whose oracle stays green when
its mutation is applied is a **liar**, and the checker reports it as such
(`rust-test-tautological`). This is copied from `kutha-graph`'s `test-matrix.yaml`, where 272
rows each name `kill_mutation` and the governor fails closed on `UNPROVEN` [validated].

### A.2 Planes stay distinct

| plane | owns | must not |
|---|---|---|
| `product` | shipped behaviour of `pa-*` crates | assert on test scaffolding |
| `harness` | the test infrastructure's own contracts (a fake actually served, a harness reaps its child) | stand in for product coverage |
| `ui` | what a user sees on a real terminal (ptу-rendered screen) | assert on internal state instead of the screen |

Mixing planes in one row is a defect (`plane-collapse`), because an artefact that proves the
harness works says nothing about the product — and vice versa. The incident's root cause spans
`product` (build identity) and `ui` (empty picker), which is exactly why it needed two rows, not
one.

### A.3 Layers — ordered by what they can observe

| layer | observes | cannot observe | cost |
|---|---|---|---|
| `unit` | one function's contract | wiring, ordering, process identity | ms |
| `contract` | a trait/port implemented by two or more adapters | end-to-end wiring | s |
| `integration` | two or more real components, no model | real terminal, real process lifecycle | s |
| `wire` | exact request/response bytes | user-visible behaviour | s |
| `acp_e2e` | a real binary over real pipes | rendering, resize, exit | tens of s |
| `pty_e2e` | a real binary on a real terminal: capability probe, resize, exit, screen | nothing above it | ~1 min |
| `daemon` | supervisor/worker lifecycle, crash recovery, reattach | rendering | minutes |
| `artifact` | the **packaged** thing a user installs | source-level behaviour | ~1 min |
| `bench` | per-frame timing against a baseline | correctness | minutes |
| `crash` | behaviour under `SIGKILL`, ENOSPC, partial write | graceful paths | minutes |
| `governor` | this ADR's own rows | anything it does not name | ms |

**A row's `layer` must be the cheapest layer that can observe its invariant.** A row that claims
`unit` for a process-identity invariant is a `layer-too-cheap` finding, because no unit test can
construct two builds.

### A.4 Axes — the multi-dimensional part

Each row declares which axes it touches. The checker does not score coverage; it verifies that
**every axis named by the invariant has at least one oracle on every layer claim**, and that the
implementation surface named on an axis exists.

| axis | question it answers | failure it exposes |
|---|---|---|
| `code` | which files own this? | the row drifts from the code it describes |
| `logic` | which function decides? | two implementations of one decision (our three classifiers) |
| `semantics` | which *value* must hold? | a value is plausible but wrong (`1` context window) |
| `ui` | what does the user see? | renders correctly, content absent |
| `flow` | which sequence must hold? | an event fires in the wrong order |
| `data` | which data path feeds it? | partial data replaces complete data |

The six axes are the ontology. An invariant is only "covered" when the axes it names each have an
oracle — a `semantics` invariant with only a `code` oracle is `axis-uncovered`.

### A.5 Status vocabulary

`killed` (oracle exists, mutation recorded) · `characterization` (pins current behaviour, no
mutation possible — must say why) · `gap` (no oracle; blocks the row's plane from merging) ·
`blocked` (oracle exists but is gated on an environment) · `spec-only` (an ADR claim with no
code yet) · `gate` (enforced by the governor itself).

Only `killed` counts as coverage. `gap` is a merge blocker for its plane when the governor runs
with `--fail-on-warn`.

### A.6 The four P0 rows this ADR adds

```yaml
- id: ID-build-same-build
  plane: product
  invariant: A client refuses a session from a daemon whose build identity differs from its own, and names the mismatch.
  layer: decision
  axes: {code: [pa-cli/interactive_mode/daemon.rs, pa-cli/src/daemon_discovery/mod.rs], logic: [one identity classifier], semantics: [buildId], ui: [banner], flow: [attach]}
  oracle: []            # not written yet
  kill_mutation: delete the buildId comparison from the classifier
  status: gap
  gate: default
  owner: ADR-0001
  source: incident 2026-10-24

- id: ID-one-identity-classifier
  plane: product
  invariant: Exactly one function decides whether a daemon is usable; every call site uses it.
  layer: decision
  axes: {code: [3 call sites], logic: [classify], semantics: [same verdict]}
  oracle: [the_probe_and_the_classifier_agree_on_one_hello]
  kill_mutation: give probe_daemon its own field list again
  status: gap
  gate: default
  owner: ADR-0001

- id: DATA-partial-asset
  plane: product
  invariant: A bundled catalog that is partial or synthetic never replaces the complete compiled catalog silently.
  layer: integration
  axes: {code: [pa-models/src/chain.rs, pa-models/src/pinning.rs], semantics: [set equality vs compiled], data: [bundled asset]}
  oracle: [a_partial_asset_does_not_shrink_the_catalog]
  kill_mutation: drop one provider from the asset; the picker's set must stay equal
  status: gap
  gate: default
  owner: ADR-0001
  source: measured 5 asset states

- id: UI-picker-nonempty
  plane: ui
  invariant: The /model picker shows exactly the models the packaged catalog promises, on the settled screen.
  layer: pty_e2e
  axes: {ui: [picker], semantics: [set equality], data: [models.bundled.json], logic: [get_model_catalog]}
  oracle: []
  kill_mutation: ship the fixture asset; the row must go red on "at least one real id"
  status: gap
  gate: default
  owner: ADR-0001
  source: incident 2026-10-24
```

**Red-first is the acceptance criterion for closing any of these**: the oracle must fail on
today's tree. `UI-picker-nonempty`'s sharpest form is "at least one rendered model id is a real
model id", which is red today because every shipped id is `fixture-*` [validated].

### A.7 What the contract forbids

- **A test may not be the only record of an invariant.** A row must exist; a test without a row
  is untracked work, and the checker reports it (`untracked-oracle`) as `LOW`.
- **A snapshot is not an invariant.** A snapshot approves whatever was on screen the first time.
  Non-emptiness and set equality must be asserted explicitly; a snapshot may accompany them.
- **A count is not a set.** `>= 1 row` catches empty but not partial. Where the subject is a
  data set, the assertion is set equality against its declared source.
- **A guard must be proved to fire.** A check that cannot fail is not a check: `doctor` returns
  exit 0 by construction, and `bundle_catalog.py:842` waives its own gate. Both are
  `inert-guard` findings in the checker.
- **A version string is not an identity.** `config::version()` reads `package.json` next to the
  executable (`config.rs:67-74`); measured: a decoy `{"version":"9.9.9-probe"}` changes
  `--version`. Identity must be content-derived and compared symmetrically.

## Part B — the governor

### B.1 Shape — an interpreter over a constitution, a dictionary, and an FSM

The governor is not a script with a hard-coded list of checks. It is an **interpreter** over three
versioned artifacts, in the shape both reference implementations converged on (`kutha-graph`'s
`.kutha/META.md` + `dictionaries/fsm.yaml` + `kutha_gov/fsm.py`; `law-nexus`'s 50 `check_*`
functions behind one driver):

```
doc/testing/META.md                 the constitution (meta-prompt): what the tool is, how to add
                                    a check, the two severities, what it will not do
doc/testing/test-matrix.yaml        the dictionary: one row per invariant
doc/testing/fsm.yaml                the machine: states, kinds, transitions, budget

scripts/testgov/
  __main__.py     CLI: ci | list | explain | map | selftest | json
  protocol.py     Severity, Finding, CheckResult, Context, Check
  dictionary.py   loads + validates the matrix; vocabulary and the layer→axis reach table
  fsm.py          load_fsm / validate / walk; ALLOWED_KINDS
  kinds.py        the checks, registered in ALL_CHECKS
  rust_index.py   one pass over crates/**/*.rs: attrs, #[ignore], is_test, nesting
  fixtures/<check>/   the tree state that MUST make each check report
  test_fsm.py     7 unit tests of the interpreter itself
```

**Why an FSM and not a sequence.** The order of checks is itself a claim: shape must hold before
oracle existence means anything, and guards must be inspected before process checks are read.
Putting that order in a YAML machine makes it reviewable and testable, and makes the failure modes
explicit — an unknown state kind, a missing transition, a state naming a check that does not exist,
and a machine with no path to `fail` are all HIGH findings rather than silent skips.

The shipped machine is seven states:

```yaml
initial: idle
terminal: [ok, fail]
states:
  idle:              { kind: noop }
  load_constitution: { kind: require_file, path: doc/testing/META.md }
  load_matrix:       { kind: require_file, path: doc/testing/test-matrix.yaml }
  load_fsm:          { kind: require_file, path: doc/testing/fsm.yaml }
  run_shape:         { kind: run_checks, checks: [matrix-shape, kill-mutation-present,
                                                  axis-uncovered, plane-collapse, layer-too-cheap] }
  run_oracles:       { kind: run_checks, checks: [oracle-exists, governor-row-names-a-check] }
  run_guards:        { kind: run_checks, checks: [inert-guard] }
  run_process:       { kind: run_checks, checks: [row-stale-vs-diff, untracked-oracle,
                                                  capability-coverage] }
  run_selftest:      { kind: run_selftest }
  decide:            { kind: decide }
```

`ALLOWED_KINDS = {noop, require_file, run_checks, run_selftest, decide}`. A new kind is a kernel
change: implement it in `walk`, add a fixture that proves it can fail, and cite it here.

### B.2 Invariants of the design, each from a measured failure

1. **The checker reads the tree, not the matrix's claims.** `kutha-graph`'s `_prove_named_test`
   (`kinds.py:1434`) requires the named `fn` to exist, be `#[test]`, be non-ignored, be
   non-nested, and be non-`async` before it counts. Ours does the same and records `file:line`.
2. **Every check must be provably able to fail.** `selftest` runs each check against
   `fixtures/<name>/`, which must produce a finding; `run_selftest` is a *state in the machine*,
   so a check that cannot fail stops the walk. Measured during construction: three checks passed
   their own fixture on the first attempt — the mechanism worked before the tool was finished.
3. **An unknown kind fails closed.** `unknown-fsm-kind` is HIGH, never a skip.
4. **Severities are two, not five.** (`kutha_gov/protocol.py:11`.)
5. **`--changed` is path-scoped and fail-closed.** A row whose cited code changed but which was
   not updated is `row-stale-vs-diff` (HIGH). Its fixture supplies `changed.txt`, because a
   changed-only check with no changed-set would pass its fixture vacuously — the exact bug
   `selftest` caught in this repo's own first attempt.
6. **No coverage percentage, ever.** The matrix header says so; `map` prints counts of *rows by
   plane/layer/axis*, deliberately not a score.
7. **Citation and ontology are separate keys.** `paths` says which files a row is *about*;
   `axes.code` says the row has an oracle that *observes* code. Conflating them let an artifact
   row claim it could observe source — a real inconsistency the governor caught in this ADR's own
   first matrix, and the reason `paths` exists now.
8. **A loose oracle beats a lying one — but only after measuring.** `settings-have-consumers`
   went through three accuracy levels: `.field` only (claimed 14 dead settings; 11 were false —
   `get_default_model()`, `get_edit_hybrid_enabled()` are real consumers), then
   `get_/set_field` exact (still lied: `get_branch_summary_skip_prompt()`,
   `get_jev_graph_rerank()` carry the field as a **substring** of a compound accessor). The
   shipped rule is an identifier index with substring membership — deliberately loose, because a
   false "read" costs a missed dead field while a false "unread" costs a maintainer. Measured
   outcome: 5 genuinely unread fields.
9. **The FSM must be edited with its own failures.** Two dictionary defects the interpreter
   rejected on first run: `idle` had no outgoing transition (the machine refused to start), and
   `run_selftest` expected `event: ok` while the interpreter emits `done` on success. Both were
   HIGH findings, which is the machine working; they are recorded here because the next editor
   of `fsm.yaml` will hit the same rule — *the event name is whatever the kind emits, and the
   interpreter defines that*.

### B.3 Checks — the first set of eleven

| check | severity | proves | introduced because |
|---|---|---|---|
| `matrix-shape` | HIGH | required keys, vocabulary, unique ids, axes non-empty | a row that cannot be parsed is a silent skip |
| `oracle-exists` | HIGH | each named oracle is a real `#[test]`, not `#[ignore]`, not a helper — or, on a `governor` row, an implemented `Check` class | the reference's `_prove_named_test` |
| `kill-mutation-present` | HIGH | `kill_mutation` is concrete, not a placeholder | anti-vacuity, from `kutha-graph`'s matrix |
| `axis-uncovered` | HIGH | a row claiming `killed`/`gate`/`blocked` names at least one oracle | our own first matrix had a `blocked` row with nothing in progress |
| `governor-row-names-a-check` | HIGH | a `governor`-plane row's oracle is a check this tool implements | caught two of our own rows claiming proof that did not exist |
| `plane-collapse` | HIGH | a `ui` row cites a real ui/pty surface | the incident was a ui row with no ui surface |
| `layer-too-cheap` | HIGH | the row's layer can observe every axis it names (table in A.3) | a unit test cannot construct two builds |
| `inert-guard` | HIGH | named guard patterns that cannot fail (`self-waiving-gate`, `unexported-guard-operand`) | two measured defects: `bundle_catalog.py:842`, `prepush-gate.sh:113/133/190` |
| `untracked-oracle` | LOW | behavioural test fns matching no row | inventory, not a gate |
| `row-stale-vs-diff` | HIGH (changed mode) | cited code changed while the row did not | drift is the failure mode of every hand-maintained list |
| `capability-coverage` | LOW | crates named by no row | the analogue of law-nexus's `check_port_contract_coverage` |

Two checks were added **because the governor caught its own authors**: `governor-row-names-a-check`
(after two rows claimed proof), and the `paths`/`axes` split inside `layer-too-cheap` (after an
artifact row claimed it could observe source). Both are evidence that the mechanism works on the
people who built it, which is the only interesting test of a governor.

### B.4 Commands

```
PYTHONPATH=scripts python3 -m testgov ci             # walk the FSM; exit 1 on any HIGH
PYTHONPATH=scripts python3 -m testgov ci --changed   # path-scoped, for pre-commit
PYTHONPATH=scripts python3 -m testgov list           # rows and status
PYTHONPATH=scripts python3 -m testgov explain UI-picker-nonempty
PYTHONPATH=scripts python3 -m testgov map            # rows by plane/layer/axis (not a score)
PYTHONPATH=scripts python3 -m testgov selftest       # every check must fail its fixture
```

`ci` is the first step of `scripts/fork/prepush-gate.sh` — before fmt and before clippy — because
it costs ~7 s (the Rust index dominates) and a matrix or guard inconsistency would otherwise
surface after a forty-minute build. Measured: 7.21 s on the real tree, 13.12 s before the FSM
removed the redundant passes.

### B.5 What the governor does not do

- It does not run `cargo test`. Existence and reachability are cheap; execution is the gate's job.
- It does not count tests, lines, or percentages.
- It does not read `.research/` (gitignored) — sources must be committed to be cited.
- It does not judge whether an oracle asserts the *right* thing. That is review's job; the
  `kill_mutation` field is the reviewer's handle.

## Part D — the ontology: a synthesis graph over six research reports

This section is the **synthesis** of six independent reports written for this ADR
(`.research/TUI-BLACKBOX-TESTING-2026.md`, `AGENT-ECOSYSTEM-TESTING-2026.md`,
`RELEASE-ARTIFACT-CONTRACT-TESTING.md`, `SCHEMA-AND-INVARIANT-GATES.md`,
`GROK-TEST-ARCHITECTURE-VS-OURS.md`, `TESTING-MATRIX-MULTIDIM-2026.md`; ~5 900 lines, all claims
measured against this tree). It is not a checklist of eight practices read from one
implementation. Eight practices are one *row* of the graph; the graph has **32 nodes, typed edges,
and 15 named gaps**.

### D.1 Why a graph and not a list

Three measured facts make a list wrong:

1. **A practice we lack cannot be reached by a practice we thought we lacked.**
   `snapshot --✗3--> non-empty-invariant`: a snapshot of an empty panel is a *valid* snapshot.
   The reference implementation has snapshots; adopting them cannot express "the list must have
   rows". Measured on us: the only empty-case test, `model_picker/tests.rs:579`
   (`an_empty_catalog_opens_the_empty_panel`), asserts the empty panel is **correct**.
2. **A practice we already have actively substitutes for one we do not.**
   `headless --|3|--> pty_e2e`: 43 headless targets stand in for PTY coverage in review while
   being structurally unable to observe a capability probe, a resize, or a process exit.
3. **A comfort practice masks a detection practice.**
   `scaled-timeouts --masks--> missing-quiescence`: if budgets scale before a
   no-grid-update-for-N-ms oracle exists, a hang becomes a longer wait instead of a failure.

A checklist would list snapshots, scaled timeouts, and headless suites as things to adopt. The
graph says all three are wrong *in that order*, and two of them are wrong on their own.

### D.2 Node kinds and the semantic of each

| kind | identity | carries | example nodes |
|---|---|---|---|
| `Layer` | name | cost, observability ceiling | `unit`, `integration`, `acp_e2e`, `pty_e2e`, `daemon`, `artifact`, `bench`, `crash` |
| `Oracle` | name | what truth it decides, what it is blind to | `exact-assert`, `set-equality`, `quiescence`, `frozen-corpus`, `mutation`, `frame-metric` |
| `Fixture` | name | what it fakes, its honesty mechanism | `wire-mock-server` (measured: `crates/pa-ai/src/testing/mock_server.rs`, 778 lines — real `TcpListener`, chunked SSE, five failure modes; **no path routing** as of 2026-10-24, fixed by `queue_json_for_path`/`queue_sse_for_path`), `recorded-cassette`, `faux-script`, `bundled-asset` |
| `Isolation` | name | owned resources, kill switches | `hermetic-sandbox`, `pdeathsig`, `owned-socket` |
| `Disposal` | name | trigger, what it proves | `script-consumption`, `guard-cannot-fail` |
| `Contract` | name | both sides, symmetry requirement | `cross-build-handshake`, `wire-pin`, `schema-id` |
| `Practice` | name | what it buys, what it costs | 32 nodes below |
| `Gap` | `G-1…G-16` | defect class admitted, measured evidence, cost | see D.6 |

### D.3 The 32 practice nodes, grouped by plane

Nodes are **practices**, not tools, so the graph survives a tool substitution. Each carries our
verdict with the evidence that produced it.

**Oracle plane** (how truth is decided)

| node | what it decides | us | evidence |
|---|---|---|---|
| `N1 exact-assert` | a hand-written expectation | **have** (29 780 of them) | 7 716 attrs; the count is the problem, see `N11` |
| `N2 golden-corpus` | replay against captured real output | **partial** | 53 golden cases over 5 tools; TS differential corpus exists |
| `N3 snapshot-of-own-output` | the frame as reviewed text | **absent** | `rg 'insta::'` → 0; the substring hit was the clippy lint `manual_instant_elapsed` |
| `N4 property` | invariant over generated input | **partial** | 2 `proptest!` blocks (`pa-tui/src/view/geometry.rs:378`, `pa-daemon/src/supervisor/tests.rs:2488`); `daily-archive` has 52 |
| `N5 differential-shadow` | new path vs old at runtime | **have** | `crates/pa-cli/tests/differential_cli.rs` (skips without `PA_TS_BINARY`) |
| `N6 wire-pin` | serialized bytes pinned | **partial** | `pa-types/tests/daemon_wire_roundtrip.rs` is round-trip only — a schema change keeping the id passes |
| `N7 model-judge` | an LLM rubric, advisory | **have, advisory** | `qa-tasks/` real-model lane |
| `N8 metric-drift` | a score with a bound across runs | **partial** | `pa-perf` has 36 baseline rows, only 8 gating (`report.rs:34 REGRESSION_FACTOR = 2.0`) |
| `N9 non-empty invariant` | assert an ordinal/floor before rows | **absent** | G-3; `model_picker/tests.rs:579` asserts the opposite |
| `N10 frozen-eval-corpus` | labelled set, sealed split, promotion gate | **have — strongest axis** | `pa-core/src/jev/eval/`: 3 corpora, sha256 fingerprint, `TestAlreadyTouched`, `tune()` rejects `Split::Test` |
| `N11 mutation` | mutant survives ⇒ missing assert | **absent in gate** | `reactivegraph` (same machine) records 318 mutants / 48 missed; we have none |

**Execution plane** (where it runs)

| node | what it sees | us | evidence |
|---|---|---|---|
| `N12 unit-tier` | one function | **have** | 7 716 attrs in 732 files |
| `N13 integration-target` | two or more components | **have** | 239 targets via `cargo metadata` |
| `N14 headless-app-loop` | the product without a terminal | **have** | 43 targets |
| `N15 PTY-real-binary` | alt-screen, raw mode, SIGWINCH, exit | **partial** | 16 files by `openpty`, but **11 distinct `PtyReader` impls** and the only screen decoder is `pub(crate)` to pa-perf |
| `N16 vt-screen-model` | bytes → grid | **have** | `vt100` in `pa-perf/src/bench/pty.rs:32`; `rg -l 'vt100' crates/*/tests/` → 0, so no e2e uses it |
| `N17 declarative-scenario` | a test as data | **partial** | `qa-tasks/` (17 tasks, 4 lanes) + `PRIME_AGENT_FAUX_SCRIPT`; scripts *responses* only — no failure setters (no stall/refuse/cut/doom-loop), no step vocabulary like the reference's 44 actions |
| `N18 cross-build-handshake` | two binaries that must agree | **absent** | `interactive_daemon_e2e.rs:1340` spawns the *same* binary, so the branch is structurally unreachable |
| `N19 daemon-lane` | real supervisor + socket | **have** | 31 lifecycle probes, `qa-tasks` daemon lane |
| `N20 multi-client-cluster` | N clients, one daemon, exactly-once | **partial** | RT-006 worker-crash exactly-once exists |
| `N21 release-artifact-smoke` | install the artifact, run it | **partial** | `release-fork.yml:111,218` runs `--version` on the unpacked payload; no content assertion |

**Identity & data plane**

| node | what it guarantees | us | evidence |
|---|---|---|---|
| `N22 build-identity-stamp` | content identity via `rustc-env` | **absent** | `find . -name build.rs` → 0; `buildId = concat!("pa-daemon-rs-", CARGO_PKG_VERSION)` (`clients.rs:126`) |
| `N23 derived-schema-id` | schema id computed from the schema | **absent** | `DAEMON_SCHEMA_ID` is a hand literal whose hex suffix is decorative (`pa-types/src/daemon/mod.rs:17`) |
| `N24 hermetic-sandbox` | owned HOME/TMP/socket/port | **partial** | 4 distinct `fn sandbox(` + 9 files re-declaring `HARNESS_LOCK` |
| `N25 wire-mock-server` | real HTTP/SSE, scripted | **have — and underused** | `crates/pa-ai/src/testing/mock_server.rs` (778 lines, HTTP/1.1 + chunked SSE + `queue_hang`/`queue_sse_stalling`); used by **2** test files; `queue_hang`/`queue_sse_stalling` by **none**. Path routing added 2026-10-24 (`queue_json_for_path`) |
| `N26 recorded-cassette` | provider bytes frozen for CI | **partial** | frozen eval corpora, not provider bytes |
| `N27 generation-by-parameter` | corpus from knobs, seeded | **partial** | 2 proptest blocks |

**Process plane**

| node | what it guarantees | us | evidence |
|---|---|---|---|
| `N28 stable-hash-shard` | a unit never migrates between shards | **have** | 8 stable-hash shards, 12 workflows / 43 jobs |
| `N29 manifest-audit` | union of shards == full enumeration | **partial** | gate reports `ran N of M units`; no equality assertion |
| `N30 red-first-proof` | invert the fix, watch it fail | **have as practice** | `AGENTS.md` demands it; not machine-checked |
| `N31 flake-ledger` | named, owned, dated | **absent** | 32 `#[ignore]`s, no `docs/flaky.md`; 2 of 280 gate units are timing reds |
| `N32 failure-artifact` | screen/HTML/trace on failure | **absent** | `pty.rs:243` holds the screen but is private; CI uploads only perf streams |

**Score: 12 have, 13 partial, 7 absent.** The absents cluster in exactly two places — *identity*
(`N22`, `N23`, `N18`) and *failure legibility* (`N9`, `N31`, `N32`, `N11`) — which is why one
incident produced an empty screen, a stale daemon, and no way to tell.

### D.4 Typed edges

Weight 1–3 = how strongly the edge holds, judged from the measured evidence in the reports.

```
# escalation: the next tier observes what this one structurally cannot
N12 --escalates_to(2)--> N13 --(3)--> N14 --(3)--> N15 --(3)--> N8

# requirements: the target cannot work without the source
N15 --requires(3)--> N16          PTY bytes without a grid are unassertable at scale
N16 --requires(2)--> N9           a grid lets you count rows; bytes do not
N18 --requires(3)--> N22          a handshake needs an identity to compare
N19 --requires(2)--> N18          the daemon lane is where a cross-build test lives
N21 --requires(3)--> N22          an artifact smoke that cannot read the artifact's identity
                                  proves only "something ran"
N24 --requires(3)--> N15          PTY tests flake without an owned HOME/TMP
N25 --requires(3)--> N13          a wire mock turns provider behaviour into a near-unit test

# strengthening: raises the yield of the target
N1 --strengthens(3)--> N9         exact asserts are what a floor guards
N2 --strengthens(2)--> N3         a corpus from ANOTHER implementation outranks our snapshot
N4 --strengthens(3)--> N11        property tests raise mutant kill rate cheaply
N9 --strengthens(3)--> N8         a metric with a floor is N9 at run granularity
N10 --strengthens(3)--> N8        the eval harness IS metric drift over a labelled corpus
N11 --strengthens(3)--> N9        mutants prove a floor has teeth
N17 --strengthens(3)--> N15       scenarios-as-data let N PTY tests share one driver
N28 --strengthens(3)--> N31       the shard ledger is where a flake is first seen
N29 --strengthens(3)--> N28
N30 --strengthens(3)--> N1
N32 --strengthens(2)--> N30       a failure you can see is a red you can trust

# absorption: makes the target redundant for that surface
N2 --absorbs(3)--> N3             where a TS reference exists, a self-snapshot adds little
N5 --absorbs(2)--> N2             shadowing the old path replaces a captured corpus there
N5 --strengthens(2)--> N6

# ⛔ conflicts: adopting one makes the other wasteful or harmful  (dashed)
N3  --✗conflicts(3)--> N9         a snapshot of an empty panel is a VALID snapshot
N11 --✗conflicts(2)--> N19        a full mutation run over a daemon e2e crate is hours of spawns
N10 --✗conflicts(2)--> N15        a labelled eval corpus is model-facing; a PTY test is binary-facing
N17 --✗conflicts(2)--> N1         a scenario language growing predicates becomes a second language
N20 --✗conflicts(1)--> N12        an exactly-once cluster test re-proves a registry unit at 1000x

# substitution: covers the same surface at a lower tier, and therefore hides the gap  (dotted)
N14 --substitutes(3)--> N15       43 headless targets stand in for PTY in review
N19 --substitutes(2)--> N18       a daemon lane without N22 cannot see a build mismatch

# masking: makes a missing oracle survivable
N7  --masks(2)--> N9              a judge can approve an empty answer as "reasonable"
N8  --masks(1)--> N31             a drifting metric read as noise is a flake
scaled-timeouts --masks(3)--> N9  (see D.1.3)
```

### D.5 The eight reference practices are one row of this graph

The practices read from the single reference implementation, scored for us, with their graph
position — this is what the earlier draft mistook for the whole ontology.

| # | practice | graph nodes | us | evidence |
|---|---|---|---|---|
| P1 | layered pyramid with explicit tiers | `N12…N15` | **have** | 5/5 on the levels axis against six external repos |
| P2 | one shared isolation owner (`env_clear`+allowlist+kill switches) | `N24` | **partial** | 4 `fn sandbox(`, 9 `HARNESS_LOCK` |
| P3 | declarative dialogue DSL with failure setters (stall/refuse/cut/doom-loop) | `N17`, `N25` | **partial** | faux scripts responses per child; **no failure setters** |
| P4 | disposal contract (`assert_no_script_violations` + `Drop` under `!panicking`) | `Disposal` | **absent** | `rg 'Drop for' print_runtime/faux.rs` → nothing |
| P5 | consumer matrix for test infra | — | **absent** | no consumer table anywhere |
| P6 | explicit `#[ignore]` tiering | `N31` (pressure valve) | **have** | 32 real attributes in 22 files |
| P7 | scaled timeouts | — | **absent** | no `TIMEOUT_SCALE`; and see D.1.3 — add `Quiescence` first |
| P8 | freshness rule (README in the same PR as `src/`) | — | **absent** | only the telemetry analogue in `AGENTS.md` |

Two of these (P5, P8) are pure process and cost an hour each. One (P7) is **ordered after**
`Quiescence` by D.4's masking edge. One (P3) is `partial` in a way that matters: our faux engine
scripts *responses* but cannot script *failures*, so the retry/backoff paths have no fixture.

### D.6 The complete oracle taxonomy — what "multi-dimensional" means operationally

An `Oracle` node is the unit of truth. A `Gap` closes only when an oracle exists at a layer that
can see it.

| oracle | decides by | catches | blind to | us |
|---|---|---|---|---|
| `exact-assert` | a hand-written expectation | the author's hypothesis | anything unimagined | **have** ×29 780 |
| `golden-corpus` | replay of captured output | regression vs a known-good | both wrong the same way | **partial** |
| `snapshot` | the frame as reviewed text | layout drift | **emptiness** (conflict) | absent |
| `property` | invariant over generated input | input-space holes | integration | **partial** ×2 |
| `differential-shadow` | two implementations, same input | divergence | both wrong together | **have** |
| `wire-pin` | serialized bytes, null-presence | shape drift | semantic compatibility | **partial** |
| `model-judge` | rubric | tone, tool choice | determinism | **have** (advisory) |
| `metric-drift` | a score with a bound | regression over runs | what the metric misses | **partial** (8/36 gating) |
| `non-empty` | an ordinal/floor | empty | **partial data** | absent |
| `set-equality` | rendered set == declared set | **partial data** (our incident) | ordering, styling | absent |
| `frozen-corpus` | sha256 pin | silent eval drift | corpus quality | **have** |
| `sealed-split` | one-shot token | tuning on test | — | **have** |
| `threshold-calibration` | promotion from dev only, typed | a sub-point win | — | **have** |
| `mutation` | test reddens on a mutant | vacuous tests | missing rows | absent in gate |
| `quiescence` | no grid update for N ms | hang; assert-before-settle | correctness | absent |
| `screen-absence` | a label disappeared | stale rows never cleared | what replaced it | absent |
| `styled-run` / `terminal-protocol` | SGR runs; OSC 52; kitty frames | rendered-but-unstyled; what a muxer sees | visual result | absent |
| `frame-metric` | p99 vs per-platform baseline | paint regression | correctness | absent in gate |
| `process-liveness` | `pdeathsig` + zombie-tolerant probe | leaks on SIGKILL | graceful leaks | **partial** (1 test) |

**9 of 19 oracle kinds present.** The absent half is precisely: emptiness/partiality
(`set-equality`, `non-empty`), settle (`quiescence`, `screen-absence`), byte-level rendering
(`styled-run`, `terminal-protocol`), perf in the gate (`frame-metric`), and vacuity (`mutation`).

### D.7 Gap nodes, with the oracle that closes each

| `Gap` | defect class admitted | closes with | priority |
|---|---|---|---|
| **G-1** | same version, different code — undetectable | `N22` build-identity stamp | P0 |
| **G-2** | client served by another build (the incident) | `N18` + `Contract` symmetry | P0 |
| **G-3** | empty or synthetic list renders as valid | `set-equality` + `non-empty` | P0 |
| **G-4** | synthetic catalog ships and *replaces* real data | provenance + `set-equality` on the artifact | P0 |
| **G-5** | semantic corpus exists, nothing runs it (4 of 10 faux tasks silently skip) | wire the faux lane | P0 |
| **G-6** | one oracle kind, 29 780 times | `property`, `mutation` | P1 |
| **G-7** | PTY layer copied, not owned (11 impls) | one `N15`/`N16` owner | P1 |
| **G-8** | determinism has no ledger | `N31` | P1 |
| **G-9** | the gate's disk guard is inert | `Disposal`/guard-cannot-fail | P1 |
| **G-10** | perf gate watches the cheap scenarios (28/36 report-only) | `frame-metric` in the gate | P2 |
| **G-11** | no failure artifact | `N32` | P2 |
| **G-12** | no cross-build wire pin (`schemaId` cannot see drift) | `N23` derived schema id | P2 |
| **G-13** | duplicated harness across 6 file groups | `N24` single owner | P2 |
| **G-14** | the eval harness gates nothing (69 cases, sealed split, no merge depends on it) | `--baseline` gate on dev split | P2 |
| **G-15** | no coverage measurement at all | advisory `llvm-cov` | P3 |
| **G-16** | settings declared and never read: `max_wait_ms`, `provider_backup_model`, `subagent_default_model`, `wait_for_usage` have no mention outside `settings/types.rs`, and `onboarding_completed` is write-only | a user sets a knob, nothing changes | P1 |

### D.8 The graph reorders the plan — dependency order, not price order

| # | step | closes | red-first proof | cost |
|---|---|---|---|---|
| 1 | two-build handshake test (`cross_build_handshake_e2e.rs`) | G-2 | today the client attaches ⇒ red before the fix | 1 d |
| 2 | `build.rs` stamp → `daemon_hello.buildId` | G-1 | two builds of one version must report the same id ⇒ step 1 reddens | 2 d |
| 3 | one `DaemonCompatibility::check(hello)` used by all three call sites | G-2 | make it ignore `appVersion` ⇒ discovery test reddens | 4 h |
| 4 | `set-equality` + `non-empty` oracle on the picker | G-3 | feed an empty catalog ⇒ red; substitute one id ⇒ red | 5 h |
| 5 | real catalog in the release (or real ids in the fixture) | G-4 | `generate --fixture` then `verify` must exit 0 (red today) | 1 d |
| 6 | wire the QA faux lane; give ST-001…004 wrong-action scripts | G-5 | a wrong-action script must redden | 1 d |
| 7 | `docs/flaky.md` + record the four current reds | G-8 | process artifact | 1 h |
| 8 | export `ROOT` into the xargs child; stub-`df` test | G-9 | stub `df` returning 1 must `SKIP`; delete the export ⇒ red | 1 h |
| 9 | shared PTY harness (`pa-test-pty`): `PtyChild` + vt100 + `quiescence`/`absence` | G-7, G-3 | a row that should disappear ⇒ `absence` reddens | 1 d |
| 10 | failure artifacts (`.txt` + `.ansi` + grid JSON) | G-11 | n/a; reviewed once | 4 h |
| 11 | `frame-metric` gate: p99 per platform, threshold | G-10 | a deliberate 20% paint regression must redden | 4 h |
| 12 | `mutation` measurement on pure crates | G-6 | a surviving mutant names a missing assert | 1 d |

Steps 1–5 close the incident; 6–9 close the class; 10–12 close the legibility. **`P7 scaled
timeouts` and `P5/P8` process items are deliberately last** — D.4's masking edge says a scaled
budget added before `quiescence` converts a hang into a longer wait.

### D.8a What the graph says that no table does

- The two highest-fan-out nodes in the identity cluster are `N22` and `N18` — **we have neither**,
  and every path to `N21 artifact-smoke` runs through `N22`. That is why our release can pass a
  `--version` smoke while shipping a synthetic catalog: the smoke had no identity to check.
- `N3 --✗conflicts--> N9` is the reason the incident was invisible rather than merely untested.
- `N14 --substitutes--> N15` is the reason it stayed invisible in review: 43 cheap green targets.
- `N5 --absorbs--> N2` says our TS differential corpus should shrink, not grow, as shadowing
  covers the same surfaces.
- The absent oracle kinds (D.6) form one coherent hole: **emptiness, settle, bytes, vacuity** —
  four ways for a suite to be green about nothing.

### D.10 The verification ladder — which layers are worth adding, and why the rest are not

The ladder (clippy → Miri → fuzzing → Loom → Kani → Verus family) is a claim about *what a
layer can observe*, so it belongs in this ontology as `Layer` nodes with `Oracle` payloads. But
adopting a layer because it is strong is the same mistake as adopting a practice because a
reference implementation has it. Scored against **our** unsafe and concurrency surface:

| fact | measured |
|---|---|
| `unsafe` sites | 128 |
| of which libc/winapi FFI (`tcgetattr`, `GetStdHandle`, `sockaddr_un`) | ~90% |
| non-FFI raw-pointer/transmute sites (what Miri *can* execute) | 43 |
| hand-rolled concurrency in `pa-mem` | 1 747 lines (`spinlock.rs` 360, `shard.rs` 393, `refcount.rs` 364, `robin_hood.rs` 630) |
| `loom` / `shuttle` / `kani` dependencies | none |
| Verus mirrors present | 3 files + `scripts/verus-check.sh` + `verus-mirror-check.py` |

| layer | verdict | why, from the numbers |
|---|---|---|
| **clippy + `cargo check`** | **already have** | zero-warning posture in the gate; adds nothing |
| **Loom** | **adopt — `spinlock` + `refcount`** | ordering is chosen by hand (`compare_exchange(0, MAX, Acquire, Relaxed)`); an `Acquire`/`Release` mistake is invisible on x86 and visible on `aarch64-apple-darwin`, which we ship. This is the **one** cheap layer that observes `flow`, hence the new `interleaving` layer node |
| **Kani** | **adopt — `robin_hood` only** | the only pure module with a wraparound/index invariant and **no** Verus mirror; `ptr48` and `varint` already have mirrors, so model checking there duplicates a proof |
| **Miri** | **adopt — workflow added** | Miri cannot execute FFI, which is ~90% of our unsafe, but the 43 non-FFI sites in pure crates (pa-types, pa-jev, pa-graph) are exactly where Miri adds value. Two workflow files (.github/workflows/miri.yml, sanitizer.yml) run the pure-crate test suites under Miri and ASan on every PR |
| **sanitizers** | **adopt — workflow added** | ASan on the pure crates catches real-allocation bugs Miri's interpreted model misses; scoped to pa-types/pa-jev/pa-graph to avoid pa-core/pa-daemon FFI noise |
| **cargo-fuzz** | **reject** | our defect class is not malformed input but **empty or partial data**: a fuzzed protocol returns a valid empty list and every seed passes. Fuzzing catches panics; the incident was a 200 with nothing in it |
| **Prusti / Creusot / VeriFast / Aeneas+hax / Flux** | **reject** | each requires rewriting the code into its own subset. `AGENTS.md` states the rule: if the code must be rewritten to become provable, that is the signal not to write the mirror. Verus already fits our stack (3 mirrors, pilot done: 4 functions, ~2.1 s, spec:logic ≈ 3:1) |

**Implementation status (2026-10-25):**

| artifact | status |
|---|---|
| `crates/pa-cli/build.rs` | **landed** — stamps `ALLSPARK_BUILD_HASH` (12-hex + `.dirty`), rerun-if-changed on .git |
| `daemon_hello.runtime.buildHash` | **landed** — carried on the wire via `configure_build_hash` seam |
| `daemon_attach_compat` predicate | **landed** — one classifier, both call sites delegate |
| `.github/workflows/miri.yml` | **landed** — runs Miri on pa-types/pa-jev/pa-graph per PR |
| `.github/workflows/sanitizer.yml` | **landed** — ASan on pure crates per PR |
| `doc/testing/test-matrix.yaml` | **landed** — 27 rows including 6 ladder rows (`LADDER-*`) |
| `design/03-decisions.md` | **landed** — D1–D9, each with evidence and date |
| `docs/perf-review.md` | **landed** — baseline procedure + regression threshold |

**Two edges this adds to the graph:**

```
Kani  --strengthens(3)--> mutation      model checking finds a MISSING assert,
                                        not only a surviving mutant
Loom  --observes--> flow                the only cheap layer that does;
                                        therefore `unit` must not claim `flow`
```

The second edge is not decoration: the governor caught it in this ADR's own first attempt to add
the loom row — the row claimed `layer: unit` with `axes.flow`, and `layer-too-cheap` rejected it.
The fix was a new layer (`interleaving`), not a looser check, because a plain unit test genuinely
cannot observe an interleaving.

**Where they run.** Neither belongs in `testgov`: the governor proves an oracle *exists* in ~7 s,
while these *execute* and cost minutes. They are gate units with an honest SKIP:

- `loom` — `serial-tail` gate row; runs only when `pa-mem` carries the `loom` dev-dependency, and
  prints `SKIPPED (no loom dev-dependency)` otherwise, never a fake green.
- `Kani` — `advisory` row; runs only when the Kani toolchain is present.

Both are matrix rows (`MUT-loom-spinlock`, `MUT-kani-robin-hood`) with status `gap`, so the
governor will require them the moment their oracles land — that is the mechanism that stops this
assessment from becoming another report nobody acts on.

**What the ladder does not buy.** It cannot reach the incident: no amount of UB checking,
model checking or proof says "the picker must show what the packaged catalog promises". That is
`set-equality` (D.6), and it is a different branch of the graph entirely. The ladder hardens the
bottom of the stack; the incident lived at the top.

## Part E — consumer matrix and freshness rule (P5, P8)

Both are cheap and both exist in the reference for a reason. As a first cut, `doc/testing/consumers.md`
carries the table:

| consumer | fixture | isolation | oracle kinds used | layer |
|---|---|---|---|---|
| `crates/pa-perf` pty bench | none | `PtyChild` own pty, setsid | `FrameMetric` | bench |
| `crates/pa-cli/tests/*_pty_e2e.rs` | `PRIME_AGENT_FAUX_SCRIPT` | per-test `sandbox()` (4 copies) | `ScreenText` | pty_e2e |
| `crates/pa-cli/tests/differential_cli.rs` | TS binary on PATH | `sandbox()` | `Differential` | acp_e2e |
| `qa-tasks/` | task `fixture/` + `oracle.py` | task `setup.sh` | `ModelJudge`, task oracle | e2e |
| `crates/pa-core/src/jev/eval` | frozen corpus | none (pure) | `FrozenCorpus`, `SealedSplit`, `ThresholdCalibration` | unit |

And the freshness rule becomes a governor check rather than a note: a row whose
`axes.code` includes test infrastructure must cite `doc/testing/consumers.md`, so a change to
the fixture owner that does not update the table is `HIGH`.

## Consequences

**Positive.** A behaviour can no longer be "tested" by accident of a passing suite. The three
classifiers, the partial-asset replacement, the inert disk guard, and the self-waiving catalog
gate are all expressible as findings today. The four P0 rows are named, owned, and red-first.

**Negative.** ~1 200 lines of checker (`kutha_gov` is 7 320; law-nexus's governor is 8 360 for
50 checks — we need 12 checks, so the smaller shape applies). Every new invariant costs a row.
A matrix row that is wrong is worse than no row, which is why `oracle-exists` and
`check-cannot-fail` are `HIGH`.

**Risk we accept.** The matrix can drift from the code it describes. `row-stale-vs-diff` catches
drift only for changed paths; a row about untouched code can rot silently. Mitigation: the row
names `axes.code` files, so a future `row-file-missing` check is cheap.

## Alternatives considered

- **Coverage percentage (`cargo llvm-cov`).** Measures lines executed, not invariants observed.
  The incident had 100% of the picker's lines executed. Rejected as a gate; may report `LOW`.
- **Snapshot every screen.** A snapshot of an empty screen is valid; it cannot express
  non-emptiness. Rejected as the non-empty mechanism (usable as an accompanying artefact).
- **One big YAML with no checker.** Became stale within one release in every reference
  implementation we read; `kutha-graph` added a governor for exactly this reason.
- **Property-based testing instead of a matrix.** `proptest` is a strong oracle for pure
  functions (we have 2 blocks; `daily-archive` has 53) but cannot state "the picker shows what
  the packaged catalog promises". Complementary, not substituting.
- **A checklist of the eight reference practices.** Rejected because three of them interact in
  ways a list erases: a snapshot cannot reach non-emptiness (§D.3 S5), a dialogue DSL cannot
  reach a catalog asset (§D.3 S4), and a scaled timeout *masks* a missing quiescence oracle
  (§D.3 S8). The graph is the only form that records "this practice cannot close that gap".
- **Copying grok-build's four-way split wholesale.** Their `TestSandbox`/`TestProcess`/
  `TestProcessTree`/`ContentController` layering is justified by ~30 consumer crates; ours has
  four `fn sandbox(` copies and nine `HARNESS_LOCK` re-declarations across ~240 test files. The
  right first move is **one** owner, not four layers (§D.2 P2, §D.6 step 1).
- **Making `doctor` the gate.** It returns exit 0 by construction and its version input is a
  file — it must be fixed before it can gate anything.

## Open questions

0. Does the graph need to be *machine-readable* (a YAML node/edge file the governor validates),
   or is §D.3's prose + the matrix's `axes` enough? Leaning: prose now, and only promote the
   edges that the governor can actually check (`requires`, `✗3` via `layer-too-cheap`) rather than
   writing a hand-maintained edge list that will drift like every hand-maintained list we have
   read. `[proposed]`
1. Should `ui` rows require an artifact (screen text + raw bytes) on failure to count as `killed`?
   Leaning yes: a red `pty_e2e` row that cannot be reconstructed costs a rerun.
2. Where does the build identity live on the wire: `runtime.buildHash` (additive, keeps the
   `pa-daemon-rs-` prefix that `install-rust.sh:1985` classifies by) or a new field? Additive is
   the only non-breaking option [proposed].
3. Do `qa-tasks/` lanes become their own plane, so a `real-model` task can never be counted as
   `product` coverage?
