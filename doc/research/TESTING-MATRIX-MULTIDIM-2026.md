<!-- FORK-ONLY: research note for the allspark fork; not shipped -->
# The testing matrix, multidimensional — allspark against this machine and against 2026 practice

**Date:** 2026-10-24 · **Tree:** `/root/prime-agent` @ `01de64db2` · **Mode:** read-only
(no build, no test run, no install; `cargo metadata` only). Public repos read via `gh api` + DeepWiki.

**Scope.** Axis-by-axis evaluation of our test architecture (not a test count), compared against the
eight other repositories on this machine and against five external references where a specific practice
matters: `xai-org/grok-build`, `openai/codex`, `All-Hands-AI/OpenHands`, `SWE-agent/SWE-agent`,
`ratatui/ratatui`. External structural claims are verified against the public git tree (`gh api
…/git/trees/HEAD?recursive=1`, all `truncated=false`); anything from DeepWiki prose that I could not
confirm in a file is marked **UNVERIFIED**.

---

## 0. Method — and the three wrong counts this report replaces

Every number below names its pattern. Three earlier figures were wrong because the pattern was too wide;
they are recorded because a matrix on a bad count is worse than no matrix.

| earlier claim | why it was wrong | correct figure |
|---|---|---|
| "`insta`: 0 everywhere" (first pass) | the pattern `insta` matched the *substring* in `install`, `instant`, and the clippy lint `manual_instant_elapsed` (`Cargo.toml:203`) | 0 is right, but it is proved by `^insta\s*=` (dependency) + `assert_snapshot!` (macro), not by the substring |
| "PTY tests: 12 files with `pty` in the name" | `pty` matches inside `empty` — `crates/pa-tui/src/agents_view/tests/empty_sessions.rs` has no PTY | **16** files call `openpty` under `crates/*/tests` (+1 bench harness) |
| "`#[ignore]`: 47" | 37 textual hits under `crates/`, five of which are prose (`#![ignore]`d and double-gated…`) | **32** attribute-anchored, in 22 files |

The same trap exists in the comparison repos and changes their ranking: `hermes-agent` has **76** files
matching `*pty*` but **42** after excluding `empty` (`rg --files -g '*pty*' | rg -v empty`). Reporting 76
would have made its PTY lead look larger than it is.

**Counting unit.** Inline tests are counted as attributes (`^\s*#\[(tokio::)?test\b`), integration tests as
`cargo metadata` targets (a directory target with submodules is one target, several files), PTY as `openpty`
call sites. Appendix A gives the exact command for each figure.

---

## 1. What allspark actually has

### 1.1 Tier inventory

Workspace size for context: **17 crates**, **1,512** `.rs` files under `crates/` (the brief's "~1,655" counts
files elsewhere too). grok-build has 3,689 `.rs` files, codex 5,515 — our test-to-source ratios in §3 are read
against these.

| tier | size | where |
|---|---|---|
| inline unit | **7,716** attributes (5,818 `#[test]` + 1,898 `#[tokio::test]`) in **732** files | `crates/*/src`, `#[cfg(test)]` |
| integration targets | **239** (`cargo metadata`) | `crates/*/tests/` |
| — of which end-to-end | **118** | `*_e2e`, `*_e2e/main.rs` |
| — of which headless TUI | **43** | `crates/pa-tui/tests/*_headless.rs` |
| PTY | **16** files (11 copies of the same reader, §6) | `crates/pa-cli/tests`, `crates/pa-daemon/tests` |
| QA task corpus | **17** tasks × 4 lanes (faux 10, real-model 8, daemon 3, mock-provider 2) | `qa-tasks/` |
| lifecycle probes | **31** Python checks + 29 self-tests | `scripts/check_*.py` |
| performance scenarios | **9** × 4 corpus profiles | `crates/pa-perf` |
| golden / differential corpus | **53** cases | `crates/pa-core/tests/golden/corpus/` |
| frozen eval corpora | **3** corpora, 69 cases, sealed test splits | `crates/pa-core/src/jev/eval/corpus.rs` |
| formal proofs | **3** mirrors, 14 obligations (advisory) | `verus/` |
| CI | **12** workflows, **43** jobs, 8 shards | `.github/workflows/` |
| local gate | **280** units, 19m22s measured | `scripts/fork/prepush-gate.sh` |

### 1.2 The QA matrix (`qa-tasks/`, `scripts/qa/run_suite.py`)

This is the only place in the repo that asserts *which tool the agent chose*.

- **Four lanes**, all converging on one grader (`run_suite.py:1570 grade_trial`): `faux` (scripted model via
  `PRIME_AGENT_FAUX_SCRIPT`), `mock-provider` (loopback wire mocks), `real-model` (live provider), `daemon`
  (ACP/JSON-RPC socket: create → attach → prompt → wait_for_idle → shutdown).
- **Two oracle types**: `state` (9 tasks — file contents, durations, request counts) and `script` (8 tasks —
  a task-local `oracle.py`).
- **Three oracle *layers***, which is the interesting part: task oracle + `must`/`must_not` tool-call
  patterns (`run_suite.py:601-639`) + level-2 log invariants (`logs:` — `max_requests`,
  `max_consecutive_failed_requests`, `max_consecutive_error_turns`, `max_total_request_ms`) + output regexes
  + an **advisory LLM judge** whose verdict lands in the manifest and never in the blocking gate
  (`run_suite.py:22-24`).
- **Regression gate** on a baseline manifest: −3pp trial pass-rate, **or ≥2 "stable" tasks losing pass-all**
  (`run_suite.py:1713-1743`). The second clause is a pass^k rule expressed at task granularity: a task earns
  `pass_all` only when every trial passed, so one task flaking across 3 trials is tolerated and two is red.
- **Measured gap:** nothing runs it. `rg 'qa-tasks\|run_suite'` over `.github/workflows/`, `Makefile` and
  `prepush-gate.sh` is empty; the only consumer is the suite's own self-test. And **4 of the 10 faux-lane
  tasks cannot run in that lane** — `ST-001…004` have no `faux-script.json`, so `run_suite.py:1848` silently
  skips them. The four tasks written to catch the 132×`python_repl` / 15×`lsp` failure report as *skips*.

### 1.3 The eval harness — a tier that was missing from the earlier inventory

`crates/pa-core/src/jev/eval/` implements an offline evaluation harness, and it is the most methodologically
advanced thing in the repo:

- **Three frozen corpora** (`corpus.rs:20-38`): auto-thinking 28 cases (dev 16 / test 12), graph re-rank 26
  (14/12), tool-arbiter 15 (9/6) — **69 cases**, hand-labelled blind, dev/test split.
- **Fingerprint pinning**: each corpus carries a SHA-256 over `(id, split, label, input)` and a test asserts
  it (`tests.rs:482-505`: *"the auto-thinking corpus changed: re-freeze only with a measured re-run"*).
- **A sealed one-shot test split**: `unseal_test()` returns a token that errors on a second call
  (`mod.rs:244-260`, `EvalError::TestAlreadyTouched`), and `TunedThresholds::tune` takes **only dev
  reports** — passing a test report is a type error, not a review comment (`mod.rs:623-640`).
- **A promotion gate with a latency ceiling**: `TuningPolicy` (default grid 1/2/3/5 pp, `max_p95_latency_ratio
  2.0`, `dev_margin_points 1.0`, `min_cases 12`), and a test named
  `the_ruflo_lesson_a_sub_point_win_does_not_promote` (`tests.rs:375`).
- **It runs in the gate**: `pa-core jev::eval::tests::the_thinking_corpus_is_frozen_and_balanced` PASS in
  0.046 s (`.research/gate/tests/pa-core-lib-pa_core.log:222`).

What it does **not** do: it is not wired to any *release* decision (nothing gates a merge on a live-candidate
run), and the only `Candidate` implementations outside tests are the live baselines in
`crates/pa-core/tests/jev_live_eval.rs`, which are `#[ignore]`d behind `SUBQ_LIVE=1` + an API key.

### 1.4 PTY infrastructure

`crates/pa-perf/src/bench/pty.rs` is the best PTY harness in the tree: `openpty` + `setsid` with no
controlling tty, `F_SETFL O_NONBLOCK`, `poll(2)`-driven reads, a **vt100 `Parser`** screen model
(`:32,84`), `wait_screen_contains(needle, what, deadline)` and a general `wait_screen(what, deadline,
settled: impl Fn(&str) -> bool)` (`:214-256`), a deterministic kitty-probe answer (`KITTY_QUERY`/`KITTY_ANSWER`,
`:19-21`), and failure messages that attach the **decoded screen** (`:243`: `"timeout waiting for {what};
screen:\n{}"`).

Two limits make it bench-only:
- `PtyChild` is `pub(crate)` and `screen_text()` is private (`:257`) — no integration test can use it.
- Consequently **0 of the 16 PTY test files decode a screen**: `rg -l 'vt100' crates/*/tests/` is empty. They
  assert on raw bytes with hand-rolled readers, and their failure messages print an undecoded `pty tail`
  (`cursor_visibility_e2e.rs:400`, `interactive_fork_e2e.rs:345`).

### 1.5 Identity — three classifiers, one literal

The incident this report was commissioned after has a precise shape, and it is worse than two sources of
truth: there are **three**, and none of them tests the others.

| path | compares | file:line | used by |
|---|---|---|---|
| interactive launch | protocol version + `schemaId` | `crates/pa-cli/src/interactive_mode/daemon.rs:37-41` | **the TUI's daemon acquisition** (`ensure_daemon_running` → `:59`) |
| `doctor` / discovery | protocol + `schemaId` + **`appVersion`** | `crates/pa-cli/src/daemon_discovery/mod.rs:416-425` | discovery + stop |
| legacy restart | `appVersion` + `schemaId` + **`supervisorPid`** | `crates/pa-cli/src/update_flow/legacy_restart.rs:437-445` | the restart funnel |

The identity they compare is weak by construction:
- `buildId = concat!("pa-daemon-rs-", env!("CARGO_PKG_VERSION"))` (`crates/pa-daemon/src/supervisor/clients.rs:126`)
  — a function of the version. Same version ⇒ same id, different code ⇒ same id.
- `DAEMON_SCHEMA_ID` is a hand-written literal, `"protocol-7-schema-30-8e4b17c2a9f5"`
  (`crates/pa-types/src/daemon/mod.rs:17`). It carries a random-looking suffix, but it is maintained by hand:
  one commit ever touched it (`git log -S 'pub const DAEMON_SCHEMA_ID'` → one hit). The 12-hex suffix is not
  derived from the schema, so it cannot detect schema drift; only a human bumping it can.
- `ALLSPARK_FORK_BUILD` exists (`crates/pa-cli/src/config.rs:62`) and is **never read by `pa-daemon`**
  (`rg fork_build crates/pa-daemon/src` → empty), so the daemon side of the handshake cannot name the fork
  build at all.
- **No `build.rs` in the workspace**: `find . -name build.rs -not -path './target/*' -not -path './vendor/*'`
  → empty. (The one hit is `vendor/tree-sitter-go/bindings/rust/build.rs`, vendored third-party code.)

No test runs **two different builds** against each other. The nearest thing,
`interactive_daemon_e2e.rs:1278 ensure_daemon_running_spawns_supervisor_and_tui_attaches`, spawns the daemon
from `env!("CARGO_BIN_EXE_prime-agent")` (`:1340`) — the same binary as the client — so the mismatch branch
is unreachable in every test that exists.

### 1.6 Release: sha256 is real, provenance is not, and the shipped catalog is synthetic

Strong, and must not be re-listed as a gap: `sha256` validation with a 64-hex gate and `sha256_hex()`
(`crates/pa-core/src/update/release.rs:133,164-165`), and **install-and-smoke inside the release workflow**
(`release-fork.yml:111` builds and runs `--version`; `:218` runs the *unpacked* `bin/allspark --version`;
`:222` checks `models.bundled.json` is present in the payload).

The weak half is the asset those checks cover. Measured by running the generator:

```
python3 scripts/release/bundle_catalog.py generate --fixture --out …
→ 46 models, 46 of them synthetic (id = fixture-<provider>-<n>), 32 providers, 43 tuples, 73 services
```

`release-fork.yml:127` ships exactly that file, and so do `release.yml:211,352,468` and
`continuous.yml:189,276` — **six release paths, all `--fixture`**. At runtime `ModelCatalog::resolve`
prefers the bundled asset over the compiled catalog (`crates/pa-models/src/chain.rs:166-187`), so a fresh
install resolves 45 pinned synthetic rows + 110 compiled prime-inference offline entries: **155 models,
none of them real**, instead of the compiled 152 real ones. The packer's only gate is
`len(tuples) >= 42` plus a per-entry schema check (`bundle_catalog.py:185-189`); 46 synthetic rows satisfy
both. Nothing in the tree parses the shipped asset through `load_bundled_models` and asserts a real model id.

(If the user sees an empty model list rather than a synthetic one, the incident's own cause is the daemon
mismatch above — the synthetic-catalog path matters when the *right* daemon is running and the picker is
still wrong. Both are the same class: **a payload that is structurally valid and semantically empty, with no
test that asks whether it has content.**)

### 1.7 Gate, CI, and what is advisory

- Local gate: fmt → clippy `-D warnings` → **one** build for all units (measured 5m06s) → enumerate 280
  units via `ci_test_shard.py` → nextest one process per unit, `JOBS=4` → 5 timing-sensitive units in a
  serial tail (`prepush-gate.sh:224`). Last recorded run: **276 ok / 4 FAIL / 0 skip**, 19m22s.
- CI: stable-hash sharding (`ci_test_shard.py:96 crc32(unit) % total`) across **8** shards, with a summary
  job that audits the union of shard manifests against the full enumeration — coverage cannot silently
  regress when a target is renamed. nextest is **not** used in CI, only locally.
- Advisory by policy: Verus (`verus/`, 3 mirrors, 298 lines; `scripts/verus-check.sh` exits 0 without the
  toolchain), TS parity diffs, and the QA corpus. The perf gate is real but narrow: **28 of 36** baseline
  rows are `report_only`, including the two slowest surfaces (`session-switch/large` 1034 ms,
  `agents-view/large` 13,459 ms); only 8 rows can fail.
- Gate self-defect: the per-unit disk guard cannot fire. `run_unit` executes inside `xargs bash -c`, which
  does not inherit the non-exported `ROOT`/`UNIT_MIN_FREE_GB`; `df -BG --output=avail ""` fails and the digit
  strip yields empty, which compares as "not less than 2". Measured: **275** `df: '': No such file or
  directory` lines in the last run's summary, one per unit.

### 1.8 Integration tooling present / absent

| tool | status |
|---|---|
| `proptest` | **present** — workspace dep `Cargo.toml:62` (v1.11, `default-features=false`, `features=["std"]`), used by pa-core, pa-tui, pa-daemon; **2 `proptest!` blocks** exist |
| `vt100` | **present** (pa-tui + pa-perf, v0.15) — used only by the bench harness (§1.4) |
| `nextest` | present locally, absent from CI |
| `insta` (snapshot) | **absent** — 0 deps, 0 macros, 0 `.snap` files |
| `cargo-mutants` | **absent** — no dep, no `mutants.out`, no CI step |
| `quickcheck`, `loom`, `shuttle`, `turmoil`, `trybuild`, `assert_cmd`, `rexpect`, `expectrl`, `criterion`, `dhat`, `mockall`, `wiremock` | **absent** from every `Cargo.toml` |
| `libfuzzer-sys` / `cargo-fuzz`, Miri, sanitizers | **absent** — no `fuzz/` dir, no `cfg(miri)`/`cfg(sanitize)` anywhere |
| coverage (`llvm-cov`, `tarpaulin`) | **absent** — no coverage step in any workflow |
| `TestBackend` (ratatui) | **0 uses**; a widget is rendered into a `Buffer` in exactly one test (`crates/pa-tui/src/markdown/tests.rs:1903-1912`) |

---

## 2. Matrix A — repository × test level

Local repos: counts from `rg` with the anchored patterns in Appendix A. External repos: counts from the
public git tree (`gh api …/git/trees/HEAD?recursive=1`, `truncated=false` for all five), so they are exact
file counts, not estimates.

| repo | unit | integration | e2e / cross-process | PTY-TUI | daemon lane | snapshot | property | mutation | golden / differential | release artifact | bench | fuzz |
|---|---|---|---|---|---|---|---|---|---|---|---|---|
| **allspark** (`/root/prime-agent`) | 7,716 attrs | 239 targets | 118 e2e | **16** files | **68** targets | none | **2** | none | 53 golden cases + 2 diff targets | sha256 + install-and-smoke in `release-fork.yml` | 9 scenarios × 4 profiles | none |
| **grok-build** | 3,689 `.rs` files (count not taken) | 252 test `.rs` in the pty-harness crate (217 in `tests/pty_e2e/`, 28 top-level, 7 in `tests/leader_pty_e2e/`) | **217** PTY files | **217** + **45 YAML scenarios** + 7 `leader_pty` | leader/client handshake | **12** `.snap` | **UNVERIFIED** (not measured) | none found | YAML scenario predicates | `VERSION_WITH_COMMIT` stamp + `SOURCE_REV` | `benches/` in harness crate | none |
| **codex** | 5,515 `.rs` files | 661 `tests/suite/` files | PTY via `PtyCodex` | ~110 pty-named paths | app-server + daemon probe | **1,618** `.snap` | **UNVERIFIED** | **UNVERIFIED** (none found in tree) | `context_snapshot` + `.snap` baselines | `codex-rs/build-info` (`STABLE_GIT_COMMIT`, sha256) | `benches/` | none in tree |
| **OpenHands** | Vitest | Vitest integration | Playwright **mock-LLM** + live | 10 pty-named paths | ACP docker e2e | 1 `.snap` | none | none | scripted trajectories | UNVERIFIED | none | none |
| **SWE-agent** | pytest | `tests/test_run_single.py`, `test_run_batch.py` | `SweBenchEvaluate` hook | none | none | none | none | none | trajectory `.traj`/`.pred` files | none | none | none |
| **daily-archive** | 7,653 `#[test]` | 1,038 test files | 143 `*_contract*`, 25 `*shadow*` | none | none | none | **52** | none | 11 parity-named, 10 conformance | none | none | none |
| **samyama-graph** | 7,198 | 446 test files (411 `.rs`) | TCK via `examples/tck_runner.rs` | none | none | none | none | none | **TCK 3,845/3,847** + baseline-is-honest test | `check_release_gate.py` (nightly verdict + ancestry) | 37 benches | none |
| **law-nexus** | 1,979 Rust + 1,351 py | 578 test files | `ln-testkit` port-contract suite | none | none | none | none | none | 10 golden | none | none | none |
| **reactivegraph** | 911 | 122 test files | — | none | none | none | **6** | **318 mutants: 216 caught / 48 missed** | none | none | none | none |
| **kutha-graph** | 713 | 193 test files | — | none | none | none | none | matrix rows name `kill_mutation` | 1 golden | none | none | none |
| **hermes-agent** | 50,443 `def test_` | 151 e2e files | **42** real PTY files | **42** | none | none | none | none | none | none | none | none |
| **ruvector-blas / -test** | 264 / 270 | 18 | none | none | none | none | none | none | none | none | none | none |

Reading the table by column:

- **PTY is grok-build's dimension and nobody else's.** 217 PTY test files in one directory, plus the 28
  top-level harness tests and 7 `leader_pty_e2e`, plus **45 declarative YAML scenarios**. We have 16 files and
  zero declarative scenarios.
- **Snapshot is codex's dimension**: 1,618 `.snap` files, concentrated in `codex-rs/tui`. Every other repo
  on this machine has **0**, which is why snapshot adoption here is a comparison against codex, not against
  our neighbours.
- **Property is daily-archive's**: 52 blocks vs our 2 and reactivegraph's 6.
- **Mutation exists once on the machine** (reactivegraph) and is not gated.
- **Only two repos verify the release artifact's *meaning***: samyama (a nightly verdict must vouch for the
  tagged commit, `scripts/check_release_gate.py`) and us (sha256 + install-and-smoke). samyama's is
  stronger on provenance-of-quality, ours on provenance-of-bytes.

---

## 3. Matrix B — axis × score, with the evidence for each cell

Scores 0-5. A cell is only as good as its citation.

| axis | allspark | grok-build | codex | ratatui (reference) | best-in-class here |
|---|---|---|---|---|---|
| **1. Levels / breadth** | **5** | 5 | 5 | 2 | tie |
| **2. Oracle method** | **3** | 4 | **5** | 3 | codex |
| **3. Isolation / determinism** | **4** | **5** | 4 | 5 | grok |
| **4. Data (fixtures/mocks/recorded)** | **4** | 5 | 5 | 1 | tie |
| **5. Surface coverage** | **4** | 5 | 4 | 2 | grok |
| **6. Gating & cost** | **4** | 3 | 4 | 5 | ratatui |
| **7. Non-empty invariants** | **1** | 3 | 3 | 2 | grok |
| **8. Identity / consistency** | **1** | **5** | **5** | n/a | tie |
| **9. Observability of failures** | **2** | **5** | 4 | 3 | grok |
| **10. Formal / property** | **3** | 2 | 3 | 2 | us |
| **11. Maintenance of the test layer** | **2** | 4 | 4 | 4 | codex |

### Justifications

**1. Levels — 5.** Seven tiers, all real and all cited: 7,716 inline; 239 integration targets in 6 kinds;
118 e2e; 43 headless (`pa-tui/src/interactive/headless.rs:24-51`, 13 step kinds); 16 PTY; 17 QA tasks over
4 lanes; 31 lifecycle probes; 9 perf scenarios; 3 frozen eval corpora. grok: 5 (PTY tiers by concern —
smoke/queue/scroll/config_ui/persistence/clipboard/shell_tools/minimal/auto_mode, plus `leader_pty_e2e`).
codex: 5 (`tests/suite` 661 files, `core_test_support` crate, TUI snapshots, PTY). ratatui: 2 (widget only).

**2. Oracle method — 3.** We have a genuinely strong method in one place and hand-written asserts everywhere
else. Strong: frozen corpora with a SHA-256 fingerprint test, a sealed one-shot test split, a promotion gate
whose `tune()` refuses a test report *by type* (`pa-core/src/jev/eval/mod.rs:623-640`), and 53 golden cases
replayed from the real TS tools (`pa-core/src/tools/golden_replay.rs:1-7`). Weak: **0 snapshots**, **2
property blocks**, 29,780 hand-written assert macros, and the golden corpus covers 5 tools. codex: 5 (1,618
snapshots + `context_snapshot` request-history baselines + mock-server + PTY + a judge-free deterministic
core). grok: 4 (YAML predicates + screen assertions, no snapshot culture). ratatui: 3 (`TestBackend` +
`assert_buffer_lines`, recommends snapshots but ships none).

**3. Isolation / determinism — 4.** Strong: `TMPDIR=<repo>/.tmp` + `CACHEDIR.TAG`, 14 ambient
`PRIME_AGENT_*` vars scrubbed (`prepush-gate.sh:57-71`), per-trial QA sandboxes, fixed proptest seeds
(`geometry.rs:380`, `supervisor/tests.rs:2490`). Weak: **32 `#[ignore]` attributes** (each with a reason —
good), but no flake ledger, and the last gate run produced 2 timing/environment reds out of 280
(`kitty_exit_routes_e2e` 48.7 s timeout, `suspend_signal_e2e` EPERM). grok: 5 (`TestSandbox` owns an
isolated `HOME`/`GROK_HOME`/workspace/tmp per test, and PTY tests are `#[ignore]`d out of plain `cargo test`
so Bazel controls concurrency — an explicit determinism decision). codex: 4 (`TestCodexBuilder` temp
workspaces).

**4. Data — 4.** We have more data mechanisms than most: a wire-level mock HTTP/SSE server with
mid-stream drop, truncation, raw frames, a hanging response and a request log
(`crates/pa-ai/src/testing/mock_server.rs:195-300`, 778 lines, used by 12 files); a deterministic corpus
generator with 4 profiles (`pa-perf/src/corpus/mod.rs:24-47`); 107 pa-edit fixtures; 53 golden cases; 3
frozen eval corpora; per-task `faux-script.json`. Missing: recorded provider cassettes (no VCR/replay), and
the shipped catalog asset is synthetic (§1.6). grok: 5 (`MockInferenceServer` + `ScriptedScenario` YAML as
data). codex: 5 (`wiremock`-based `responses.rs`, `context_snapshot`).

**5. Surface coverage — 4.** Covered: TUI screen (43 headless + 16 PTY), wire protocol (68 daemon targets),
catalog/config (model-flag tests, `live_catalog_chain`), install/update (`installer_platform_map`,
`installer_stop_gate`, `packaged_layout_e2e`, `release_workflow`), kernel/Python REPL (15 `kernel_*` targets).
Not covered: **a client and a daemon from different builds** (§1.5) — which is a *surface* (the upgrade
path), not a test. grok: 5 (adds clipboard, image input, mermaid, IME paste, leader/cluster).

**6. Gating & cost — 4.** 280 units in 19m22s locally with one build instead of one per unit, a stable-hash
shard split that keeps flake attribution, and a summary job that audits shard-manifest union — that is
better engineering than most. Deductions: perf gate 8/36 gating rows; QA corpus ungated; the per-unit disk
guard is inert (275 `df: ''` lines). ratatui: 5 (milliseconds). grok: 3 (PTY tier is `#[ignore]`d, so
`cargo test` cannot fail on it; Bazel is the gate).

**7. Non-empty invariants — 1.** See §5 G-1. Two instances, both measured: the rendered picker never sees
the real catalog (`rg 'compiled_models\|from_compiled' crates/pa-tui/src/model_picker/` → empty; 22 call
sites use a 4-model `battery_catalog()`), and the only empty-case test asserts that the empty panel is
correct (`model_picker/tests.rs:579`). The shipped catalog is 46/46 synthetic. codex: 3 (snapshot of an empty
panel is still valid, but the mock server's expectations pin model ids). grok: 3 (YAML predicates can assert
*a specific model row*, e.g. `dashboard_model_list_click.yaml`).

**8. Identity / consistency — 1.** The incident's exact axis, and our worst. `buildId` is a function of the
version (`clients.rs:126`); `DAEMON_SCHEMA_ID` is a hand-maintained literal with a decorative hex suffix
(`pa-types/src/daemon/mod.rs:17`); `ALLSPARK_FORK_BUILD` never reaches `pa-daemon`; **no `build.rs` in the
workspace**; **three** divergent classifiers (§1.5); and no test runs two builds against each other.
grok: 5 — `crates/codegen/xai-grok-pager-bin/build.rs` (verified from source) emits
`cargo:rustc-env=VERSION_WITH_COMMIT={version} ({commit})` from `git rev-parse HEAD` truncated to 12 chars,
with `rerun-if-changed` on `.git/HEAD`, `.git/logs/HEAD` and the resolved head ref, and a `rerun-if-env-changed=GROK_VERSION`;
a separate `crates/codegen/xai-grok-version/build.rs` exists; `crates/codegen/xai-grok-pager/src/acp/version_mismatch.rs`
renders `⚠ Version mismatch: client {c}, leader {l}. Restart grok to match`, with the wire `message` field
deliberately ignored so the client writes its own restart hint, and a dedicated
`version_mismatch_tests.rs` + `app/acp_handler/tests/version_mismatch.rs`.
codex: 5 — `codex-rs/build-info/src/lib.rs` (verified from source): `BuildInfo { version, build_commit,
target }`, commit from `option_env!("STABLE_GIT_COMMIT")`, plus a sha256 helper; `codex-rs/tui/src/app/server_version_notice.rs`
tells "older" from "different", stable clients compare release precedence while prerelease clients compare
only within the same release line, and there are named snapshot tests for it
(`connected_server_version_notice_snapshot.snap`, `local_daemon_version_notice_snapshot.snap`).

**9. Observability of failures — 2.** We have the best failure message in the tree and cannot reach it: the
bench harness attaches the decoded screen (`pty.rs:243`), but `screen_text()` is private, so the 16 PTY
tests print undecoded bytes (`cursor_visibility_e2e.rs:400`: `"timeout waiting for {what} (needle
{needle:?}); pty tail since mark:\n{text}"` — and `text` there is the raw stream, not a screen). CI uploads
artifacts only for perf failure streams (`perf.yml:153`) and lifecycle receipts (`ci.yml:376`); no step
uploads a screen on an ordinary test failure. grok: 5 — the harness captures text **plus** HTML, SVG and
styled-JSON representations of the screen "at various steps for bug triage" (UNVERIFIED in file form; the
`screen_contents()`/`contains_text()`/`wait_for_text` API is confirmed). codex: 4 (`insta` diffs are
reviewable inline).

**10. Formal / property — 3.** We are the only repo on this machine with a formal-verification directory
(`verus/`: `pa_core_gates.rs` 75, `pa_mem_bucket.rs` 115, `pa_mem_ptr48.rs` 108 lines; a mirror-constant
checker `scripts/verus-mirror-check.py`; advisory by design, exits 0 without the toolchain). kutha has one
Verus file with a governor cross-check. But property testing is 2 blocks against 7,716 tests, and
`proptest` is `default-features=false, features=["std"]` — no `fork`, no `timeout`, so a failing case can be
neither replayed in a child nor bounded in wall time.

**11. Maintenance — 2.** 14 files ≥ 2,000 lines (largest `crates/pa-cli/tests/interactive_daemon_e2e.rs`
at 5,878); **11** independent `struct PtyReader` definitions with 38-182-line impls and 11 distinct hashes;
`fn spawn_supervisor` defined **65** times across 64 files; `fn spawn_daemon` 27 times; `read_line` in 76
files; `write_json` 51; `attach_data` 48; `env!("CARGO_BIN_EXE_…")` in 108 files. No test-support crate
exists (`ls -d crates/*test*` → nothing); only pa-core, pa-edit and pa-models have a `tests/common/mod.rs`.
codex: 4 (a `core_test_support` crate, and `codex-rs` has 23 paths naming `test_support`). grok: 4
(`xai-grok-test-support`).

---

## 4. The practice graph

Nodes are **practices** (not tools), so the graph survives a tool substitution. Edges are typed:
`+` strengthens (raises the yield of the target), `⊃` absorbs (makes the target redundant),
`✗` conflicts (adopting one makes the other wasteful or harmful), `→` requires (target cannot work without
the source). Weight 1-3 = how strongly the edge holds, judged from evidence in §1-3.

### 4.1 Nodes (32)

**Oracle layer**
- `N1 exact-assert` (hand-written expectation)
- `N2 golden-corpus` (byte replay against captured real output)
- `N3 snapshot-of-own-output` (`insta`)
- `N4 property` (`proptest`/`quickcheck`: invariant over generated input)
- `N5 differential-shadow` (new path vs old path, compared at runtime)
- `N6 wire-pin` (serialized bytes pinned, incl. null-presence)
- `N7 model-judge` (LLM rubric, advisory)
- `N8 metric-drift` (a score with a bound, tracked across runs)
- `N9 non-empty invariant` (assert an ordinal/floor before asserting on rows)
- `N10 frozen-eval-corpus` (labelled set, sealed test split, promotion gate)
- `N11 mutation` (mutant survives ⇒ missing assert)

**Execution layer**
- `N12 unit-tier`
- `N13 integration-target`
- `N14 headless-app-loop`
- `N15 PTY-real-binary`
- `N16 vt-screen-model` (decode bytes into a grid)
- `N17 declarative-scenario` (YAML/JSON test as data)
- `N18 cross-build-handshake` (run two binaries that must agree)
- `N19 daemon-lane` (real supervisor + socket)
- `N20 multi-client-cluster` (N clients, one daemon, exactly-once)
- `N21 release-artifact-smoke` (install the artifact, run it)

**Identity & data**
- `N22 build-identity-stamp` (`build.rs` + git sha via `rustc-env`)
- `N23 derived-schema-id` (schema id computed from the schema)
- `N24 hermetic-sandbox` (owned HOME/TMP/socket/port)
- `N25 wire-mock-server` (real HTTP/SSE, scripted)
- `N26 recorded-cassette` (provider bytes frozen for CI)
- `N27 generation-by-parameter` (corpus generated from knobs, seeded)

**Process layer**
- `N28 stable-hash-shard` (unit → shard never migrates)
- `N29 manifest-audit` (union of shards == full enumeration)
- `N30 red-first-proof` (invert the fix, watch it fail)
- `N31 flake-ledger` (named, owned, dated)
- `N32 failure-artifact` (screen/HTML/trace attached on failure)

### 4.2 Edges

Grouped by what they say, so the drawing can be a layered DAG with the conflict edges dashed.

```
# Oracle layer strengthens/absorbs
N1 --+3--> N9        exact asserts are what a non-empty invariant guards
N2 --+2--> N3        a golden corpus from ANOTHER implementation outranks a snapshot of ours
N2 --⊃3--> N3        where a TS reference exists, a self-snapshot adds little
N4 --+3--> N11       property tests raise mutant kill rate cheaply
N5 --+2--> N6        a shadow test is the cheapest way to notice wire drift
N5 --⊃2--> N2        shadowing the old path replaces a captured corpus for that surface
N6 --+2--> N4        a pinned wire shape is a property worth generating cases for
N8 --+2--> N31       a drifting metric is how a flake is first seen
N9 --+3--> N8        a metric with a floor is N9 at run granularity
N10 --+3--> N8       the eval harness IS metric-drift with a labelled corpus
N11 --+3--> N9       mutants on a surface prove its floor has teeth
N11 --+2--> N1       mutants are a list of asserts that do not assert

# Execution layer
N12 --+2--> N13      unit coverage shrinks what integration must re-prove
N13 --+3--> N14      integration proves components; the app loop proves the product
N14 --+3--> N15      headless cannot see alt-screen, raw mode, SIGWINCH, exit codes
N15 --→3--> N16      PTY bytes without a screen model are unassertable at scale
N16 --+3--> N32      a decoded grid is the failure artifact for a TUI
N16 --+2--> N9       a grid lets you count rows (an ordinal), bytes do not
N17 --+3--> N15      scenarios-as-data let N PTY tests share one driver
N17 --+2--> N32      a scenario step is a natural artifact capture point
N18 --+3--> N22      a handshake needs an identity to compare
N19 --+2--> N20      a cluster needs a daemon lane
N19 --→2--> N18      the daemon lane is where a cross-build test lives
N21 --+3--> N22      the artifact must report its stamp, or the smoke is blind
N21 --→2--> N25      the artifact must reach a mock, not the network

# Identity & data
N22 --+3--> N18      build id is the payload of the handshake
N23 --+2--> N18      a derived schema id is a second, cheaper discriminator
N24 --+3--> N15      PTY tests flake without an owned HOME/TMP
N24 --+2--> N14      ambient env leaks into the app loop
N25 --+3--> N13      a wire mock turns "provider behaviour" into a unit-ish test
N25 --+2--> N6       the mock's request log is what a wire pin asserts on
N26 --+2--> N25      a cassette is a mock with recorded bytes
N27 --+3--> N8        a metric needs a controllable corpus
N27 --+2--> N15      PTY timing needs seeded corpora

# Process
N28 --+3--> N31      stable sharding is what makes a flake attributable at all
N29 --+3--> N28      without the union audit, a shard can silently lose units
N30 --+3--> N1        an assert that never failed is documentation
N30 --+3--> N9        a floor added without a red-first proof may be vacuous
N31 --+2--> N30       the ledger is the list of tests that need a red-first pass
N32 --+2--> N30       a failure you can see is a red you can trust

# Conflicts (dashed)
N3 --✗3--> N9        a snapshot of an empty panel is a VALID snapshot: N3 cannot
                      express "the list must have rows". Adopting N3 as a general
                      policy actively hides the N9 class.
N11 --✗2--> N19      a full mutation run over a daemon e2e crate is hours of
                      process spawns; run it on pure crates only.
N10 --✗2--> N15      a labelled eval corpus is model-facing; a PTY test is
                      binary-facing. One harness cannot serve both.
N17 --✗2--> N1        a scenario language that grows predicates becomes a second
                      language with no type checking of its own.
N20 --✗1--> N12       an exactly-once cluster test re-proves what the registry unit
                      test already pins, at 1000x the cost.
```

### 4.3 The same graph as Mermaid

```mermaid
graph LR
  subgraph Oracle
    N1[exact-assert]:::or; N2[golden-corpus]:::or; N3[snapshot]:::or
    N4[property]:::or; N5[differential-shadow]:::or; N6[wire-pin]:::or
    N7[model-judge]:::or; N8[metric-drift]:::or; N9[non-empty-invariant]:::or
    N10[frozen-eval-corpus]:::or; N11[mutation]:::or
  end
  subgraph Execution
    N12[unit]:::ex; N13[integration]:::ex; N14[headless-loop]:::ex
    N15[PTY-real-binary]:::ex; N16[vt-screen-model]:::ex
    N17[declarative-scenario]:::ex; N18[cross-build-handshake]:::ex
    N19[daemon-lane]:::ex; N20[multi-client]:::ex; N21[artifact-smoke]:::ex
  end
  subgraph IdentityData
    N22[build-identity-stamp]:::id; N23[derived-schema-id]:::id
    N24[hermetic-sandbox]:::id; N25[wire-mock-server]:::id
    N26[recorded-cassette]:::id; N27[generation-by-parameter]:::id
  end
  subgraph Process
    N28[stable-hash-shard]:::pr; N29[manifest-audit]:::pr
    N30[red-first-proof]:::pr; N31[flake-ledger]:::pr; N32[failure-artifact]:::pr
  end
  N2 ==>|3| N3
  N4 ==>|3| N11
  N9 ==>|3| N8
  N10 ==>|3| N8
  N13 ==>|3| N14
  N14 ==>|3| N15
  N15 -->|3| N16
  N17 ==>|3| N15
  N19 -->|2| N18
  N18 ==>|3| N22
  N21 ==>|3| N22
  N24 ==>|3| N15
  N25 ==>|3| N13
  N28 ==>|3| N31
  N29 ==>|3| N28
  N30 ==>|3| N1
  N30 ==>|3| N9
  N3 -.->|✗3| N9
  N11 -.->|✗2| N19
  N17 -.->|✗2| N1
  classDef or fill:#eef,stroke:#557; classDef ex fill:#efe,stroke:#575
  classDef id fill:#fee,stroke:#755; classDef pr fill:#ffe,stroke:#775
```

### 4.4 What the graph says that the tables do not

Reading the edges as a dependency structure, the two nodes with the largest downstream fan-out in the
*identity* cluster are `N22 build-identity-stamp` and `N18 cross-build-handshake` — and **we have neither**.
Every path through `N18` requires `N22`; every path from `N21 artifact-smoke` back to a meaningful verdict
requires `N22` too (a smoke test that cannot read the artifact's identity can only prove "something ran").

The graph also explains why our incident class is invisible rather than merely untested:
`N3 --✗3--> N9` is a *conflict*, so the practice we lack (`N9`) cannot be reached by adding the practice we
thought we lacked (`N3`). And `N14 ==>|3| N15` means the 43 headless targets actively *substitute* for PTY
coverage in review — they are cheap, they usually pass, and they cannot see a capability probe, a resize, or
a process exit.

---

## 5. Gaps — what each one hides, what it costs, and its priority

Priority: **P0** = it has already cost us a user-visible defect; **P1** = it will, on the next release;
**P2** = it taxes every change; **P3** = polish.

| # | gap | defect class it lets through | evidence (measured) | cost to close | priority |
|---|---|---|---|---|---|
| **G-1** | **No build identity.** No `build.rs` in the workspace; `buildId` = version; `DAEMON_SCHEMA_ID` a hand-maintained literal with a decorative hex suffix | "same version, different code" is undetectable; a stale daemon is indistinguishable from a current one | `clients.rs:126`; `pa-types/src/daemon/mod.rs:13-17`; `find . -name build.rs` → 0 in-workspace | 2-3 d (build.rs + handshake field + a two-build test) | **P0** |
| **G-2** | **No client/daemon same-build check.** Three classifiers compare three different tuples; the interactive one compares the *weakest* pair | the shipped incident: TUI attaches to a 0.10.1-beta.35 daemon under a 0.10.2-beta.3 client → empty model list | `interactive_mode/daemon.rs:37-41` vs `daemon_discovery/mod.rs:416-425` vs `update_flow/legacy_restart.rs:437-445`; no test runs two builds (`interactive_daemon_e2e.rs:1340` spawns the same binary) | 3-4 d (needs G-1) | **P0** |
| **G-3** | **No non-empty invariant on any rendered data surface.** The picker is only ever fed a 4-model fixture; the empty panel is the *documented* behaviour | "the panel rendered, plausibly, out of nothing" — an empty or synthetic list looks correct | `rg 'compiled_models\|from_compiled' crates/pa-tui/src/model_picker/` → empty; `battery_catalog()` 22 uses; `model_picker/tests.rs:579`; only `(1/9)` (`:290`) and `rows.len()==18` (`:311`) exist, both on fixtures | hours (2 tests) | **P0** |
| **G-4** | **The shipped catalog is a synthetic fixture** (`--fixture` on six release paths), and it *replaces* the compiled catalog at runtime | a structurally valid, semantically empty payload ships | `release-fork.yml:127,129`; `release.yml:211,352,468`; `continuous.yml:189,276`; `bundle_catalog.py:481-525`; `chain.rs:166-187`; measured 46/46 synthetic | 1 h (real-catalog path) + 3 h (a test that parses the shipped asset through `load_bundled_models` and asserts a real id) | **P0** |
| **G-5** | **The semantic corpus exists and nothing runs it** — and 4 of its 10 faux tasks silently skip | "the agent chose the wrong tool" (132×`python_repl` vs 15×`lsp`); also oracle rot, because a `must_not` that never fired is untested | `rg 'qa-tasks\|run_suite'` over CI/Makefile/gate → empty; `run_suite.py:1848` skips for a missing `faux-script.json`; ST-001…004 have none | 4 h (wire the faux lane) + 1 d (wrong-action scripts, so the oracle can go red) | **P0** |
| **G-6** | **One oracle kind, 29,780 times**: 0 snapshots, 2 property blocks, 53 golden cases over 5 tools | asserts encode what the author thought of; nothing generates the case that breaks it | anchors in Appendix A | 2 d for 3 property blocks; 1 d for one mutation measurement | **P1** |
| **G-7** | **PTY layer is copied, not owned**: 11 `PtyReader` impls (38-182 lines, 11 distinct hashes); the one screen-decoding harness is `pub(crate)` to pa-perf | every PTY fix is made 11 times; PTY failures print escape soup, so a 48-second timeout is undiagnosable | `rg 'struct PtyReader'` → 11; `pty.rs:28,257`; `rg -l 'vt100' crates/*/tests/` → 0 | 1 d (extract a shared harness), one file per commit | **P1** |
| **G-8** | **Determinism has no ledger.** 32 well-annotated `#[ignore]`s, but no record of "passes alone, fails together" | a flake is re-diagnosed from scratch each time; the honest alternative (retry-to-green) is forbidden, so it is ignored instead | 2 of 280 gate units are timing/environment reds; no `docs/flaky.md` | 1 h (the file) | **P1** |
| **G-9** | **The gate's per-unit disk guard is inert** | `No space left on device` reads as a code failure — the exact incident the guard's own comment describes | 275 `df: ''` lines in `summary.log`; `run_unit` runs under `xargs bash -c` without `ROOT` exported (`prepush-gate.sh:160-180`) | 1 h + a red-first stub-`df` test | **P1** |
| **G-10** | **The perf gate watches the cheap scenarios.** 28 of 36 baseline rows are `report_only` | the two slowest user-visible surfaces can regress 10× and stay green | `crates/pa-perf/baseline.json` (36 entries, 8 gating); `report.rs:34 REGRESSION_FACTOR = 2.0` | 4 h (spread-bounded thresholds) or a written exemption | **P2** |
| **G-11** | **No failure artifact.** No decoded screen, no HTML/SVG, no trace on a failed test; CI uploads only perf streams and lifecycle receipts | a TUI regression is triaged by re-running it locally | `pty.rs:243` has the screen but is private; `perf.yml:153`; `ci.yml:376` | half a day (share the decoder, write `.txt` + `.ansi` per failure) | **P2** |
| **G-12** | **No cross-build wire pin.** `DAEMON_SCHEMA_ID` cannot detect schema drift; the wire test is round-trip only | a schema change that keeps the id passes every test | `pa-types/src/daemon/mod.rs:17`; `pa-types/tests/daemon_wire_roundtrip.rs` | 1 d | **P2** |
| **G-13** | **Duplicated harness** across 64 + 26 + 76 + 51 + 48 + 108 files | a harness bug is fixed in one copy; reviewers cannot see the shared contract | Appendix A.6 | 2-3 d (incremental, one file per commit) | **P2** |
| **G-14** | **The eval harness gates nothing.** 69 labelled cases, a sealed split and a promotion gate, none of which a merge depends on | a judgment consumer can be switched on without its measured win | `pa-core/src/jev/eval/`; the only live `Candidate` is `#[ignore]`d (`tests/jev_live_eval.rs`) | 1 d (a `--baseline` gate on the dev split) | **P2** |
| **G-15** | **No coverage measurement at all** | cannot answer "did this PR test the lines it changed"; makes G-6 prioritization guesswork | no `llvm-cov`/`tarpaulin` anywhere | 2 h (one advisory CI job) | **P3** |

Four of these are the ones the brief named, and my measurement agrees with all four while sharpening two:
(a) build identity — confirmed, and *worse* than "no hash": there are **three** classifiers; (b) no non-empty
invariants — confirmed, and the empty panel is *deliberately* asserted; (c) no same-build check — confirmed,
and the nearest test structurally cannot reach the branch; (d) the fixture catalog in the release — confirmed,
and it is not merely present but **preferred over the compiled catalog at runtime**.

---

## 6. Plan — twelve steps, cheapest first, first one is the incident

Every step names the file, the red-first proof, the price and the defect class.

| # | step | file(s) | red-first proof | cost | catches |
|---|---|---|---|---|---|
| **1** | **Two-build handshake test.** Serve a hand-written `daemon_hello` fixture on a socket (an old `buildId`/`schemaId`), start the new client against it, assert it *replaces* the daemon instead of attaching | new `crates/pa-cli/tests/cross_build_handshake_e2e.rs` | today the client attaches ⇒ the test fails before the fix; after it, the old daemon must be shut down and a new one spawned | 1 d | **the incident** |
| **2** | **`build.rs` build identity.** Stamp `ALLSPARK_GIT_SHA` / `ALLSPARK_DIRTY` / `ALLSPARK_FORK_BUILD` into `pa-types`; `buildId = version + sha12 (+dirty)`; `pa-daemon` reads it; `daemon_hello.buildId` carries it | new `crates/pa-types/build.rs`; `pa-types/src/daemon/mod.rs`; `pa-daemon/src/supervisor/clients.rs:126` | with the stamp unset, two builds of one version must report the *same* id ⇒ step 1's test goes red for the identity reason | 2 d | same-version-different-code |
| **3** | **One classifier.** Extract `DaemonCompatibility::check(hello) -> Compat` and call it from `probe_daemon`, `classify_reachable` and `validate_successor` | `interactive_mode/daemon.rs:37`; `daemon_discovery/mod.rs:416`; `update_flow/legacy_restart.rs:437` | make the shared fn ignore `appVersion` ⇒ the discovery test must go red | 4 h | three-sources-of-truth |
| **4** | **Non-empty invariants.** A headless test that opens `/model` against `transports::compiled_models()` and asserts `(1/N)`, `N ≥ 150`; a unit test that the bundled asset yields ≥ 111 *and* contains a named real id | `crates/pa-tui/tests/model_picker_real_catalog_headless.rs`; `crates/pa-models/src/bundled.rs` tests | feed an empty catalog ⇒ red; skip every id containing `-` in `pinning.rs` ⇒ the floor drops ⇒ red | 5 h | silent-empty payload |
| **5** | **Ship a real catalog, or make the fixture carry real ids**, plus the composition test | `scripts/release/bundle_catalog.py:434`; `.github/workflows/release-fork.yml:127` | run the release's own pair: `generate --fixture` then `verify --out …` must exit 0 (red today — see §9) | 1 d | synthetic release asset |
| **6** | **Wire the QA faux lane**, and give `ST-001…004` wrong-action scripts | `scripts/qa/run_suite.py`; `.github/workflows/ci.yml`; `qa-tasks/ST-*/faux-script.json` | a wrong-action script must go RED; delete a task's script and the job must count the skip | 1 d | wrong-tool choice; oracle rot |
| **7** | **`docs/flaky.md`** + record the four current gate reds with owner and environment | new `docs/flaky.md` | n/a (process artifact); the review rule is "no new flake without a row" | 1 h | re-diagnosis; silent quarantine |
| **8** | **Export `ROOT`/`UNIT_MIN_FREE_GB`** into the `xargs` child; add `scripts/fork/test_prepush_gate.sh` with a stub `df` | `scripts/fork/prepush-gate.sh:160-180` | stub `df` returning 1 must produce `SKIP … need 2G`; delete the export and the same stub runs ⇒ red | 1 h | ENOSPC misread as a code failure |
| **9** | **Shared PTY harness**: extract `PtyChild` + the vt100 screen into a dev-only crate (`pa-test-pty`), migrate the 11 copies **one file per commit** | new `crates/pa-test-pty`; `crates/pa-perf/src/bench/pty.rs:28,257` | shrink the shared settle window to 0 ⇒ the migrated test must go red | 1 d | 11x maintenance; undecodable failures |
| **10** | **Failure artifacts**: on a PTY timeout write `<test>.screen.txt` (decoded) + `.ansi` (raw) under the target dir; upload them on failure | `crates/pa-test-pty`; `.github/workflows/ci.yml` | inject a needle that never appears ⇒ the artifact exists and contains the decoded screen | 4 h | untriageable TUI failures |
| **11** | **Three property blocks where the invariant is already prose**: pa-edit apply idempotence, **pa-graph incremental == full rebuild**, pa-types JSONL byte-stability | `crates/pa-edit/tests/apply_patch_parity.rs`; `crates/pa-graph/tests/integration.rs`; `crates/pa-types/tests/session_jsonl_roundtrip.rs` | the `pa-graph` one is expected to **find the reindex gap immediately** (`crates/pa-graph/src/incremental.rs:316-324`, audit F07) | 1 d | incremental drift |
| **12** | **One mutation measurement** (`cargo mutants -p pa-edit`, no gate) and publish caught/missed | `pa-edit` only | n/a — the survivor list is the output | 1 d compute | asserts that do not assert |

**Sequencing rationale.** 1-3 are one thread: the incident, its identity, and the three-way divergence. Step 1
is first because it is the only one that reproduces the incident *without* the fix, and because it constrains
the shape of step 2 (the handshake payload). 4-5 close the "structurally valid, semantically empty" class,
which is the same class as the incident reached from the data side. 6 makes the semantic tier real. 7-10 are
repairs to the measuring instrument, which is why they precede 11-12: you cannot trust a mutation list
gathered by an instrument with an inert guard and undecodable failures.

**The plan adds no snapshot framework, no second matrix language, and no mutation gate.** Five of twelve steps
delete code; the rest replace a hand-written assert with a generated one or make an existing signal
trustworthy.

---

## 7. What not to port

| not this | why (with evidence) | do this instead |
|---|---|---|
| **`insta` as a general policy** | (i) 0 in every repo on this machine; it is codex's practice (1,618 `.snap`), not a local norm. (ii) It cannot express our worst class: a snapshot of an empty panel is a **valid** snapshot. (iii) Re-record decay: the first diff is repaired with `cargo insta accept` and the contract ships with the new snapshot. | 3-5 reviewed frames at most, over `Buffer::empty` + `Widget::render` (the pattern already used once at `pa-tui/src/markdown/tests.rs:1903`). For data surfaces use G-3's ordinal+floor. If adopted, `CI=true` must make writes fail. |
| **grok's 217-file PTY corpus as a target number** | We have 16 PTY files and 68 daemon targets; their ratio follows from a TUI-only product with a hand-rolled pager. Copying the count produces PTY tests for surfaces that already have a cheaper oracle. | copy the **two mechanisms**: declarative `ScriptedScenario` YAML (45 files, one driver) and the multi-representation failure artifact. Steps 9-10. |
| **grok's `#[ignore]`-everything PTY tier** | Their PTY tests are `#[ignore]`d and run by Bazel, so plain `cargo test` cannot fail on the TUI. That is a deliberate trade for a Bazel shop; under our nextest gate it would silently remove 16 units from 280. | keep PTY tests in the gate; move only genuinely timing-sensitive units to the serial tail (we have 5, each with a reason at `prepush-gate.sh:224`). |
| **codex's `tests/suite` breadth (661 files)** | It is the shape of a product with 5,515 Rust files and a Bazel+cargo hybrid. Our 239 targets already cover our 17 crates at a higher ratio. | copy `context_snapshot` (request-history baselines) and **the version-notice snapshot tests** — exactly G-2's missing assertion: `local_daemon_version_notice_snapshot.snap` pins the client-version vs server-version matrix. |
| **OpenHands' Playwright stack** | Browser e2e for a web product; 0 PTY, 34 testlib paths, 5 Playwright configs. Nothing transfers to a terminal UI. | its transferable idea is `mock-llm` as an HTTP server with admin endpoints for trajectory registration — we already have the equivalent in `pa-ai/src/testing/mock_server.rs`. |
| **SWE-agent's `SweBenchEvaluate` as a gate** | A benchmark harness, not a regression gate: containers plus a dataset, and its pass rate is a research metric. | its `PredeterminedTestModel` / `DummyRuntime` idea is already better served by our faux provider + `PRIME_AGENT_FAUX_SCRIPT`. |
| **kutha's 3,172-line test matrix** | 272 rows maintained as a second codebase; the governor verifies oracle **names exist**, not that they still assert the invariant. At our 239 targets the YAML would exceed the tests it indexes. | the **row schema** for the 4-6 contract breaks we already wrote down (`.research/AUDIT-VERDICTS-AND-PLAN.md` F01-F18): `invariant`, `oracle`, `kill_mutation`, `status`. Start at 6 rows, not 272. |
| **a full `cargo-mutants` gate** | 318 mutants in one crate (reactivegraph) is minutes-to-hours; on our workspace it is hours plus disk, and it gates on `target/`. | one measurement per crate, published, survivors filed as missing asserts (step 12). |
| **samyama's `kill -9` loop as a default gate** | It needs a nightly bare-metal host and a durable log; under our parallel gate it would flake. | the **prefix** idea where we already crash-test (`worker_crash_exactly_once`, `RT-006`): a count is satisfiable by scattered survivors, a prefix is not. |
| **`--test-threads=1` as a general answer** | It hides ordering bugs. It is correct in exactly one place for us (`windows-runtime-triage.yml:127`). | keep it there with the reason at the test. |
| **A grep gate for `sleep`** | Our PTY readers use `std::thread::sleep` inside deliberate settle windows (`drain_until_quiet`), so a grep would flag the good ones and miss a retry-to-green loop. | the existing rule already works: a wait must name an observable readiness condition. Review it; do not grep it. |
| **Shipping `--fixture` in a release at all** | §1.6: it is preferred over the compiled catalog at runtime and contains no real model. Six release paths do it today. | either a credentialed `--network` path, or a fixture whose ids are real; plus the composition test in step 5. |

---

## 8. Summary

**Levels: 5/5.** Seven tiers, all real, all cited. No repo on this machine has our breadth, and grok and codex
match rather than exceed it.

**Oracle method: 3/5** — the number that surprises me. It was 1 in my first pass because I had not read
`crates/pa-core/src/jev/eval/`: three frozen corpora, fingerprint-pinned, a sealed one-shot test split, and a
promotion gate where "tune on test" is a *type error*. That is better methodology than anything in the five
external repos on the eval axis. What drags the score down is that it covers 69 cases of judgment routing
while 29,780 hand-written asserts cover everything else, and the corpus that *would* catch the incident class
(non-empty rendering) does not exist.

**Identity: 1/5** — our worst axis, and the incident's axis. Three classifiers, a version-valued `buildId`, a
hand-maintained schema literal, no `build.rs`, and no test that runs two builds against each other. grok and
codex are both 5 here, and both were verified from source, not prose.

**The single most valuable change** is not a new tier: it is **step 1**, a two-build handshake test, because it
is the only step that reproduces the incident before the fix — and it needs no build identity to be written,
only a hand-written `daemon_hello` fixture and `PRIME_AGENT_DAEMON_SOCKET`.

---

## 9. Live note — an uncommitted change that its own release path cannot pass

While verifying §1.6 I found **two modified, uncommitted files** in the working tree (not mine):

```
 M .github/workflows/release-fork.yml
 M scripts/release/bundle_catalog.py
```

`bundle_catalog.py` gains `FORK_PROVIDERS` and rejects an asset missing
`muse`/`codebuddy`/`genspark`/`subquadratic`/`ninerouter` unless `allow_small_fixture` is set. The check is
**correct** and names the right mechanism ("chain.rs step 2 replaces the compiled catalog with this file").

It does not compose with the release path. Measured on this tree:

| command | result |
|---|---|
| `bundle_catalog.py generate --fixture --out …` | **rc 0** (46 synthetic models, 43 tuples, 73 services) |
| `bundle_catalog.py verify --out …` (no waiver) | **rc 1** — missing the fork's providers |
| the same two commands against `git show HEAD:scripts/release/bundle_catalog.py` | generate 0, **verify 0** |

The mechanism is a one-line asymmetry: `generate` waives its own gate with
`allow_small = options.allow_small_fixture or options.fixture` (`bundle_catalog.py:842`), while `verify` reads
only `args.allow_small_fixture` (`:873-875`). `release-fork.yml:129` runs `verify` with no waiver, so the fork
release would be **red at the packaging step**. `assemble_artifacts.py:333` performs the same waiver-less
validation after `generate --fixture` in `release.yml` and `continuous.yml`, so six paths break.

Its own battery agrees exactly: `test_catalog_assets.py` is **10 failures / 24** on the tree vs **6 / 24** on
`git archive HEAD` — 5 new failures, 1 no-longer-failing, 5 common. The `test_network_*` failures are the same
cause sideways: those tests generate in network mode and compare byte-for-byte against
`fixture_catalog_bodies()` (`test_catalog_assets.py:462-472`).

It belongs in a testing report because the gate is right about the defect and wrong about the artifact: the
fork ships `--fixture` deliberately (`release-fork.yml:123`), so "reject a fixture" and "ship a fixture" cannot
both be policy. The fix is not to weaken the gate but to make the fixture carry real ids — and to add the
composition test of step 5: *the command a workflow runs must exit 0 on the artifact that workflow generates.*
I did not touch these files (another agent's uncommitted work).

---

## Appendix A — the commands

**A.1 inline tests (7,716; 5,818 `#[test]` + 1,898 `#[tokio::test]`)**
```bash
python3 - <<'EOF'
import os,re
n=0
for r,_,fs in os.walk('crates'):
    for f in fs:
        if f.endswith('.rs'):
            n+=len(re.findall(r'(?m)^[ \t]*#\[(?:tokio::)?test\b',
                  open(os.path.join(r,f),encoding='utf-8',errors='replace').read()))
print(n)   # 7716
EOF
rg -F --count-matches '#[test'        -g '*.rs' crates/ | awk -F: '{s+=$NF} END{print s}'  # 5819
rg -F --count-matches '#[tokio::test' -g '*.rs' crates/ | awk -F: '{s+=$NF} END{print s}'  # 1899
```

**A.2 integration targets (239) — never count files**
```bash
cargo metadata --format-version 1 --no-deps --locked | python3 -c \
 'import json,sys;d=json.load(sys.stdin);print(sum(1 for p in d["packages"] for t in p["targets"] if "test" in t["kind"]))'
# 235 = `crates/*/tests/*.rs`; 259 = `crates/*/tests/**/*.rs` (adds submodules + fixtures).
# Both are right, they count different things; 239 is the number of TEST TARGETS.
```

**A.3 property (2), snapshots (0), assert macros (29,780)**
```bash
rg -F --count-matches 'proptest! {'      -g '*.rs' crates/ | awk -F: '{s+=$NF} END{print s}'  # 2
rg -F --count-matches 'assert_snapshot!' -g '*.rs' .       | awk -F: '{s+=$NF} END{print s}'  # 0
rg --files -g '*.snap' | wc -l                                                                 # 0
rg -F --count-matches 'assert_eq!' -g '*.rs' crates/ | awk -F: '{s+=$NF} END{print s}'  # 17420
rg -F --count-matches 'assert!('   -g '*.rs' crates/ | awk -F: '{s+=$NF} END{print s}'  # 12098
rg -F --count-matches 'assert_ne!' -g '*.rs' crates/ | awk -F: '{s+=$NF} END{print s}'  # 262
```

**A.4 `#[ignore]` (32 attributes / 22 files; 37 textual, 5 of them prose)**
```bash
rg -F --count-matches '#[ignore' -g '*.rs' crates/ | awk -F: '{s+=$NF} END{print s}'   # 37
python3 - <<'EOF'
import os,re
print(sum(len(re.findall(r'(?m)^[ \t]*#\[ignore',
      open(os.path.join(r,f),encoding='utf-8',errors='replace').read()))
      for r,_,fs in os.walk('crates') for f in fs if f.endswith('.rs')))   # 32
EOF
```

**A.5 PTY (16 test files; 18 including the bench harness and a Cargo.toml comment)**
```bash
rg -l 'openpty' -g '*.rs' crates/*/tests | wc -l    # 16
rg --files -g '*pty*' | wc -l                       # 12 — MISLEADING: 'empty' contains 'pty'
rg -l 'vt100' -g '*.rs' crates/*/tests | wc -l      # 0  — no PTY test decodes a screen
```

**A.6 duplicated harness (files that DEFINE the helper, in `crates/*/tests`)**
```bash
for n in spawn_supervisor spawn_daemon read_line write_json attach_data run_plan drain_until_quiet; do
  printf '%-18s %s\n' "$n" "$(rg -l "fn $n" -g '*.rs' crates/*/tests | wc -l)"
done
# spawn_supervisor 64 (65 definitions) · spawn_daemon 26 (27) · read_line 76 · write_json 51
# attach_data 48 · run_plan 34 · drain_until_quiet 10
rg -l 'struct PtyReader' -g '*.rs' crates/ | wc -l                    # 11
rg -l 'env!\("CARGO_BIN_EXE_' -g '*.rs' crates/ | wc -l               # 108 (pa-daemon 69, pa-cli 39)
```
`read_line` is *mentioned* in 154 test files but *defined* in 76 — a count that does not separate these is the
§0 error class again.

**A.7 golden / eval corpora**
```bash
python3 -c 'import json,glob;[print(f,len(json.load(open(f)).get("cases",[])))
  for f in sorted(glob.glob("crates/pa-core/tests/golden/corpus/*.json"))]'                 # 53 total
rg -n 'THINKING_FINGERPRINT|RERANK_FINGERPRINT' crates/pa-core/src/jev/eval/tests.rs         # frozen pins
```

**A.8 QA corpus (17 tasks, 8 faux scripts, ungated)**
```bash
find qa-tasks -name task.yaml | wc -l                     # 17
find qa-tasks -name faux-script.json | wc -l              # 8  (ST-001..004 have none)
rg 'qa-tasks|run_suite' .github/workflows/ Makefile scripts/fork/prepush-gate.sh   # EMPTY
```

**A.9 the inert disk guard (275) and the gate's own result**
```bash
grep -c "df: ''" .research/gate/tests/summary.log   # 275
grep -c '^ok '   .research/gate/tests/summary.log   # 276
grep -c '^FAIL'  .research/gate/tests/summary.log   # 4
bash -c 'ROOT=$PWD; f(){ df -BG --output=avail "$ROOT" | tail -1 | tr -dc 0-9; }; export -f f; \
         printf x | xargs -P1 -I{} bash -c "f" _ {}'   # → df: '': No such file or directory
```

**A.10 the local repos (exact patterns)**
```bash
for d in /root/{daily-archive,samyama-graph,law-nexus,reactivegraph,kutha-graph,hermes-agent}; do
  echo "== $d"
  ( cd "$d" && for p in 'proptest! {' 'assert_snapshot!' 'openpty'; do
      printf '  %-16s %s\n' "$p" "$(rg -F --count-matches "$p" -g '*.rs' . 2>/dev/null | awk -F: '{s+=$NF} END{print s+0}')"
    done; ls mutants.out 2>/dev/null | wc -l; find . -name build.rs -not -path './target/*' | wc -l )
done
# daily-archive prop=52 · reactivegraph prop=6, mutants.out=10 files, 318 mutants / 216 caught / 48 missed
# all others prop=0 · snap=0 everywhere · hermes `*pty*` 76 → 42 after `rg -v empty`
```

**A.11 external repos — exact tree counts (check `truncated` is false)**
```bash
gh api repos/<owner>/<repo>/git/trees/HEAD?recursive=1 --jq '.tree[].path' > tree.txt
gh api repos/<owner>/<repo>/git/trees/HEAD?recursive=1 --jq '.truncated'      # all five: false
grep -c '\.snap$' tree.txt          # codex 1618 · grok 12 · others 0
grep -c 'build\.rs$' tree.txt       # grok 8 · codex 7
grep -c 'tests/scenarios/.*yaml$' tree.txt    # grok 45
grep -c 'pty_e2e/' tree.txt                   # grok 217
grep -c 'tests/suite/' tree.txt               # codex 661
```

**A.12 the release asset, measured**
```bash
python3 scripts/release/bundle_catalog.py generate --fixture --out /tmp/cat-assets
python3 -c 'import json;d=json.load(open("/tmp/cat-assets/models.bundled.json"));
print(len(d["models"]), sum(m["id"].startswith("fixture-") for m in d["models"]))'   # 46 46
python3 scripts/release/bundle_catalog.py verify --out /tmp/cat-assets               # see §9
```
