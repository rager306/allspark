# Grok Build — testing methodology, improved assessment

Date: 2026-10-24. Replaces the earlier assessment (which covered layers, isolation, DSL, disposal
contract, consumer matrix, `#[ignore]` gating, scaled timeouts, freshness rule).
This revision adds the layers and semantics the first pass **missed** and corrects two claims.
No test counting: every item below is a mechanism, not a number.

Everything is cited from `xai-org/grok-build` (public tree, issues disabled). UNVERIFIED marks
anything I could not read directly.

---

## 1. What the first pass missed

### 1.1. The declarative YAML scenario layer does not exist in the old assessment

This is the **largest** part of their TUI methodology and it was absent. Two distinct test
authoring styles coexist:

| style | where | authored as |
|---|---|---|
| imperative Rust harness | `tests/pty_e2e*/*.rs`, `tests/*.rs` | `PtyHarness` calls, `#[path]` fan-in |
| **declarative YAML scenario** | `tests/scenarios/*.yaml` + `tests/scripted_scenarios.rs` | data, no Rust |

`ScriptedScenario` (`src/scripted.rs:1229`) is a *document*:

```
name, description,
terminal {rows, cols},
environment {args[], env[{key,value}]},
workspace {git_init, files{path->content}},   // ephemeral repo materialized into temp cwd
mock {response, turns[], fixture_images[]},
steps [ ... ]
```

`workspace.git_init` + `files` means a scenario can *construct a repository* before the TUI boots.
`mock.turns` is an ordered expectation list: **every listed turn must be satisfied**, falling back
to `response` when exhausted. That is a turn-level oracle, not a single canned reply.

### 1.2. The step vocabulary is a 35-action terminal DSL

`ScenarioStep` (`src/scripted.rs:7323`) — actions, not assertions-only:

- **wait**: `wait_for_text`
- **assert on screen**: `assert_contains`, `assert_not_contains`, `assert_highlight_run`,
  `assert_text_not_highlighted`
- **assert on terminal protocol**: `assert_osc52_contains`, `assert_osc52_not_contains`,
  `assert_kitty_graphics`, `assert_no_kitty_graphics`, `wait_for_kitty_graphics`
- **text entry**: `type_text`, `keys`, `paste`, `paste_prompt`, `focus_prompt`, `drop_text_prompt`
- **images**: `paste_image_paths`, `paste_image_file_urls`, `paste_escaped_image_paths`,
  `paste_clipboard_image`, `drop_images_prompt`
- **pointer**: `mouse_click`, `double_click`, `triple_click`, `click_text`, `double_click_text`,
  `triple_click_text`, `drag`, `drop_text`, `drop_text_at`, `select_text`, `select_text_range`
- **scroll**: `scroll`, `scroll_at`
- **misc**: `copy`, `wait`

The first pass described "keys + contains_text". The real surface asserts on **OSC 52 clipboard
payloads** and **kitty graphics protocol frames** — i.e. it grades the bytes a terminal multiplexer
would receive, not just visible characters.

### 1.3. The runner produces a *bug report*, not a pass/fail bit

`ScriptedRunReport` (`src/scripted.rs:45675`): `status`, `skip_reason`, `started_at_ms`,
`artifact_dir`, `steps[StepOutcome]`, `artifacts[VisualArtifact]`, **`bugs[BugFinding]`**.

`BugFinding { step, severity, message, screen_text }`, and `write_bug_markdown` emits `bugs.md`
into the run directory. Kinds of findings the runner raises on its own:

- a step's expectation failed → status `Failed`, artifacts captured, `bugs.md` written;
- **the pager exited before the scenario completed** → `BugFinding` with the full screen text;
- an `AgentTurnExpectation` was never satisfied after the settle deadline → `BugFinding` with the
  aggregated diagnostic of *both* endpoint variants.

So the harness distinguishes "assertion failed" from "the product died mid-scenario" from "the
model turn never landed" — three failure semantics with different diagnostics.

### 1.4. Screenshot = four artifact formats, not an image

`capture_artifacts` (`src/scripted.rs`, `fn capture_artifacts`) writes, per `screenshot` step,
`{step:02}-{name}` as **`.txt` (plain screen), `.html` (styled), `.svg` (rendered), `.json`
(styled run structure)**. On failure the runner additionally dumps `raw_output.bin` — the raw PTY
byte stream "for offline analysis (e.g. Kitty a=t vs a=p counts)".

This is the difference between "we saved a screenshot" and "a failure is reconstructible without
re-running".

### 1.5. `AgentTurnExpectation`: one logical turn, two wire dialects

`src/content.rs:28`. A single agent turn is represented as **two** `InferenceExpectation`s —
`/v1/responses` and `/v1/chat/completions` — and the API is:

```
wait_received()   // either backend claimed the turn  (tokio::select!)
wait_blocked()    // the active backend reached this turn's terminal barrier
release()         // open the barrier
wait_satisfied()  // the active backend completed the turn
is_satisfied() / assert_satisfied()
diagnostic()      // describes BOTH variants for aggregated failure output
```

The semantic point: the test does not know, and must not care, which wire dialect the client picks.
The turn is the unit of synchronization. This is a **dialect-agnostic oracle** — structurally the
same idea as asserting on a logical operation rather than its HTTP shape.

`#[must_use = "keep the handle to synchronize or verify the logical agent turn"]` — the type system
forces the test to consume the barrier.

### 1.6. Waits are event-driven, and quiescence is a first-class condition

`ptyctl/src/wait.rs:3`: *"Waiters never poll: the session's feeder bumps a generation watch after
each `term.feed()`, and conditions are re-checked only on bumps."*

```
enum WaitCondition { Text(String), Regex(String), Gone(String), StableMs(u64) }
```

- `Gone` is the **absence** oracle (positive assertion that a label disappeared);
- `StableMs` is **quiescence**: "no grid update for N ms" — the principled replacement for
  `sleep(400)`. (Contrast: the YAML example still uses `action: wait, millis: 400`; the primitive
  exists but is not universally used.)

On timeout, `WaitDiagnostics` carries `screen`, `cursor`, `modes`, `raw_tail` (capped),
`generation`, and **`ended: bool`** — "true when the session's output ended (child exited) before
the deadline, so the condition could never have been met". That last field is the difference
between "we timed out" and "it was never going to work".

### 1.7. The PTY controller is a standalone, HTTP-drivable service

`ptyctl` is its own crate (`keys.rs`, `pty.rs`, `server.rs`, `session.rs`, `styled.rs`, `term.rs`,
`wait.rs`), documented as *"Headless PTY controller built on alacritty_terminal … expose it all via
HTTP REST API"*, with `ptyctl-cli` (`cli.rs`, `commands/`, `registry.rs`) on top. So a TUI session
can be driven by an **agent** over HTTP, not only by a Rust test. The pager harness consumes it.

### 1.8. Frame-level performance is a test, with a regression gate

`src/timing.rs`: *"Detects frame boundaries from synchronized-update markers (`CSI ? 2026 h/l`) and
records wall-clock timing for each frame."* → `BenchResults { total_frames, avg_fps, p50_ms,
p99_ms, max_ms, jank_count, jank_rate, chars_per_frame_avg }`.

`benches/pty_baselines/README.md`: per-platform files (`linux-x86_64`, `linux-aarch64`,
`macos-aarch64`), and **CI fails if any scenario's p99 frame time grows by more than 15%**. An
intentional shift must overwrite the baseline *and* paste the bench output in the PR body so a
reviewer can sanity-check.

This is the answer to "how do you keep a paint-latency-critical TUI honest": the metric is
per-frame, the baseline is per-platform, and drift is a failing gate rather than a dashboard.

### 1.9. Orphaned children are killed by the kernel, with the residual gap documented

`tests/orphan_reap.rs`. The comment is the method:

> PTY e2e children used to survive an ungraceful test-runner death (CI timeouts deliver SIGTERM
> then SIGKILL — neither unwinds, so no Drop runs), re-parent to init, and accumulate on CI hosts.
> The fix arms `PR_SET_PDEATHSIG(SIGKILL)` on the child at spawn, so the **kernel** reaps it when
> the spawning test process dies. Residual gap: pdeathsig covers only the direct PTY child … live
> tool subprocesses it spawned can still re-parent and outlive it (the group kill covers them only
> on userspace teardown paths).

They then **test the residual gap's boundary**: `pid_is_running` is zombie-tolerant because
*"whether that init reaps it promptly is environmental; the harness's contract is that it stops
running"*. Linux-gated (`#![cfg(target_os = "linux")]`) with the reason stated.

### 1.10. Environment hygiene is an explicit deny-list tied to a named consumer

`src/pty_spawn.rs` removes, before spawning:
- `CLIPBOARD_SINK_ENV_VARS` (`GROK_OSC52_SINK`, `LC_GROK_OSC52_SINK`)
- `APPEARANCE_ENV_VARS` (`GROK_APPEARANCE`, `GROK_THEME`, `COLORFGBG`, …) — *"would make
  `theme=auto` non-deterministic in PTY tests (layout depends on the resolved palette)"*
- `HOST_TERMINAL_ENV_VARS` (CURSOR_TRACE_ID, TERM_PROGRAM, WEZTERM_VERSION, ITERM_SESSION_ID, …) —
  *"A leak reclassifies the child (tmux, Cursor before TERM_PROGRAM, nvim OSC 52). Keep in sync
  with that detection source."*

Each list names *the product code that reads it*. Determinism is tracked to its consumer.

### 1.11. A race is answered with an idempotent retry, not a longer sleep

`src/flows.rs`, `submit_turn`: *"a leader attach race can drop the first submit and leave the
prompt in the composer. Extra Enter is a no-op once the draft is taken, so it cannot double-submit."*
It re-presses Enter under its own per-attempt budget, with the argument for why the retry is safe.
That is the shape a flaky-input fix should have: prove the retry cannot double-apply.

### 1.12. Mock response resolution has an explicit precedence order

`InferenceOverride` (`inference_override`): `named-expectation` → `compat-fifo` → `auth-rejection`
→ `conversation-script` → `concurrency-cap`. Ambiguity is resolved by a documented ladder, so a
test author can predict which reply wins.

---

## 2. Corrections to the first pass

**(a) `#[path]` fan-in is not primarily a compile-time cost.** The first pass called it "increases
compile time". The stated reason is resource bounding: *"All cases are ignored for ordinary Cargo
runs; Bazel opts in and caps this process-heavy family at four concurrent libtest workers."* One
binary per family → one libtest process → Bazel can cap it at 4. The compile-time cost is the
*price* of a bounded blast radius on CI. Both readings matter; the design intent is the second.

**(b) The "huge PTY e2e surface" is deliberately not on the default path.** These are `#[ignore]`d
and only Bazel opts in. Plain `cargo test` never pays for them. Calling it a weakness without that
caveat misstates the CI cost.

**(c) `set_test_env`'s `unsafe env::set_var` is narrower than it looks.** The SAFETY comment is a
real invariant: *"the only live threads are the mock's HTTP workers, which never read env"*, and it
runs before spawning under a `LocalSet`. It is a constraint to preserve, not a free-floating race —
but it is fragile, because the invariant is about *threads that exist at that moment*.

**(d) Missing from the first pass entirely — what they do NOT have** (verified against code):
- **no build identity in the handshake.** `LeaderServerMetadata.leader_binary_version` is
  `env!("CARGO_PKG_VERSION")`; `client_version` likewise. There is no commit/build-id comparison on
  the wire (the `VERSION_WITH_COMMIT` stamp reaches `--version` and telemetry, not the handshake).
  The `x.ai/leader/version_mismatch` notification compares **version strings only**.
- **no content hash of the binary.** Integrity hashes exist for *downloaded side assets*
  (`fd`/`ripgrep`/`mingit` SHA-256), not for the shipped binary.
- **no generic "list is non-empty / has ≥N rows" primitive.** The picker test
  (`dashboard_model_list_click.yaml`) does assert non-emptiness — but by `wait_for_text:
  test-model`, i.e. by naming an expected row. There is no row-count or cardinality assertion
  available to a scenario.
- **no content-addressed fixture/release separation.** Mitigations are staging prefixes
  (`.staging-`) and build-time conditional bundling (`build.rs` bundles only in release) — not a
  provenance gate on the shipped asset.

---

## 3. Improved ontology — four dimensions, not three

The first graph had layers / components / principles. That under-describes the system: the
interesting engineering is in *how an oracle is built* and *how a failure is made reconstructible*.
Add two axes and make the edge vocabulary carry the semantics.

**Dimensions**
1. `TestLayer` — where in the stack the test runs (unit → integration → acp_e2e / headless_e2e → pty_e2e → bench).
2. `Component` — the double / owner / client / runner that implements it.
3. `Oracle` — **how truth is decided** (see 3.1).
4. `FailureSemantics` — **what a failure tells you** (see 3.2).
5. `Principle` — the organizational rule that binds them.

### 3.1. Oracle taxonomy (new)

| oracle | mechanism | strength | used by |
|---|---|---|---|
| `ScreenText` | substring / regex on decoded grid | semantic, cheap | `wait_for_text`, `assert_contains` |
| `ScreenAbsence` | `WaitCondition::Gone` | catches "stale row never cleared" | `wait_for_labels_absent` |
| `Quiescence` | `StableMs` — no grid update | replaces sleep; can't mask a hang | `ptyctl::wait` |
| `StyledRun` | highlight / SGR runs | catches "rendered but unstyled" | `assert_highlight_run` |
| `TerminalProtocol` | OSC 52 payload, kitty graphics frames | catches bytes a muxer sees | `assert_osc52_*`, `assert_kitty_graphics` |
| `LogicalTurn` | `AgentTurnExpectation` over 2 dialects | wire-dialect agnostic | `content.rs` |
| `ScriptConsumption` | `assert_no_script_violations` + Drop | a test cannot pass with unused scripts | `conversation_script` |
| `FrameMetric` | p99 from `CSI ?2026 h/l` boundaries | catches paint regression | `timing.rs` + baselines |
| `ProcessLiveness` | kernel `pdeathsig` + zombie-tolerant probe | catches leaks on SIGKILL | `orphan_reap.rs` |

**The gap this exposes in us:** we have `ScreenText`-equivalents
(`crates/pa-perf/src/bench/pty.rs: wait_screen_contains`, `screen_text`) and nothing else on this
list. No absence oracle, no quiescence, no styled-run, no terminal-protocol, no logical-turn
barrier, no script-consumption check, no frame metric, no leak probe.

### 3.2. Failure semantics (new)

| semantic | what it proves | artifact |
|---|---|---|
| `StepExpectationFailed` | the product ran, one assert failed | `.txt/.html/.svg/.json` + `raw_output.bin` |
| `ProductDiedMidScenario` | the pager exited before the scenario finished | `BugFinding` + full `screen_text` |
| `TurnNeverLanded` | the turn barrier was not satisfied after settle | diagnostic of **both** dialects |
| `CouldNotEverMatch` | `ended=true` — child gone before the deadline | `WaitDiagnostics` |
| `LeakOnUngracefulDeath` | child outlived a SIGKILLed runner | pid + starttime probe |
| `PerfRegression` | p99 grew past 15% vs platform baseline | bench JSON + PR body rule |

Each has a *different* remedy. Collapsing them into "test failed" is what makes TUI failures
expensive to debug; that is the single most transferable idea here.

### 3.3. Graph edges (semantics-carrying)

```
(Layer)        unit --escalates_to(scope-widening)--> integration
               integration --escalates_to(process-boundary)--> acp_e2e
               integration --escalates_to(full-cli)--> headless_e2e
               acp_e2e --escalates_to(real-terminal-rendering)--> pty_e2e
               pty_e2e --escalates_to(per-frame-timing)--> bench

(Oracle)       ScreenText --weaker_than--> ScreenAbsence --weaker_than--> Quiescence
               ScreenText --orthogonal_to--> StyledRun --orthogonal_to--> TerminalProtocol
               LogicalTurn --subsumes--> ScreenText                (turn vs screen)
               ScriptConsumption --guards_false_positive_of--> ScriptedResponse
               FrameMetric --requires--> PerPlatformBaseline

(Component)    ScriptedScenarioRunner --drives--> PtyHarness --drives--> ptyctl --emulates--> alacritty_terminal
               ScriptedScenarioRunner --emits--> ScriptedRunReport --contains--> BugFinding
               ScriptedScenarioRunner --captures--> VisualArtifact{txt,html,svg,json}
               ContentController --owns--> TestSandbox
               ContentController --owns--> MockInferenceServer
               MockInferenceServer --resolves_via--> InferenceOverride(--precedence-->…)
               AgentTurnExpectation --wraps--> InferenceExpectation[responses, chat_completions]
               HeadlessRunner --scaled_by--> GROK_TEST_TIMEOUT_SCALE

(Principle)    HermeticIsolation --enforced_by--> TestSandbox --denies--> {CLIPBOARD_SINK, APPEARANCE, HOST_TERMINAL}
               IgnoredExpensiveTests --governs--> pty_e2e
               BazelVsCargoSplit --schedules--> pty_e2e --cap--> 4 workers
               OrphanSafety --enforced_by--> pdeathsig --tested_by--> orphan_reap
               FreshnessRule --governs_docs_of--> xai-grok-test-support
               DeclarativeScenario --trades--> compile_time --for--> single_binary_concurrency_cap
```

**Conflicts worth naming** (the graph is not a hierarchy):
- `DeclarativeScenario` vs `CompileSpeed` — one binary per family buys a concurrency cap.
- `Quiescence` vs `ExplicitWait` — `StableMs` is principled but slow on chatty screens; the YAML
  corpus still uses `millis: 400`. The primitive exists; adoption is partial.
- `HermeticIsolation` vs `unsafe env::set_var` — process-global env is the only channel the child
  inherits; the sandbox narrows *which* vars but cannot make the mutation thread-safe.
- `ScriptConsumption` vs `Result`-returning tests — an early `?` drops the server before scripts
  finish, and the drop-time panic *replaces* the real error. They document the trap; they do not
  eliminate it.

---

## 4. Our gaps, ordered by the defect class they leave open

| gap (ours) | defect class it lets through | evidence | cost |
|---|---|---|---|
| **No build identity** — no `build.rs`; `runtime.buildId = concat!("pa-daemon-rs-", CARGO_PKG_VERSION)` (`crates/pa-daemon/src/supervisor/clients.rs:126`) == version; `DAEMON_SCHEMA_ID` is a hand-written literal (`crates/pa-types/src/daemon/mod.rs:17`) | client silently served by a **different build of the same version** | two binaries, one schemaId: sha256 `116d134699…` vs `a42a4fae58…` | ~1 day |
| **Two daemon classifiers disagree** — `probe_daemon` (protocol+schemaId, `crates/pa-cli/src/interactive_mode/daemon.rs`) vs `classify_reachable` (+ appVersion, `crates/pa-cli/src/daemon_discovery/mod.rs:416`, doctor-only) | TUI attaches to a stale daemon, `doctor` says `stale`, nobody reads it | reproduced live | ~2 h |
| **No non-empty invariant** on any TUI list | "providers present, models absent" renders as a valid screen | the incident | ~1 day |
| **No absence/quiescence oracle** — only `wait_screen_contains` (`crates/pa-perf/src/bench/pty.rs:214`) | stale rows that never disappear; sleeps that mask hangs | — | ~2 days |
| **No artifact capture on failure** (no `.txt/.html/.svg/.json` + `raw_output.bin`) | a red TUI test is not reconstructible without a rerun | — | ~1 day |
| **No frame-level perf gate** (`pa-perf` exists but no per-platform p99 baseline / 15% rule) | paint regression ships silently | — | ~2 days |
| **Fixture catalog ships as release asset** — `.github/workflows/release-fork.yml:127` `generate --fixture` | synthetic data in a real install; today it is silently dropped by `pin_catalog_models` (`crates/pa-models/src/pinning.rs:68`) | installed asset is 46 `fixture-*` models | ~2 h |
| **No leak probe on ungraceful death** (`pdeathsig` appears only in a test: `crates/pa-cli/tests/windows_runtime_triage_e2e.rs:177`) | orphans accumulate on CI hosts | — | ~half day |

## 5. What to take, in order (each with the test that proves it)

1. **Build identity** — one `build.rs` in `crates/pa-cli` (composition root, not per-crate):
   `git rev-parse HEAD` → 12 chars, `cargo:rustc-env=…`, `rerun-if-changed` on `.git/HEAD`;
   thread it through `configure_app_version` (`crates/pa-daemon/src/protocol.rs:29`) into
   `daemon_hello`. *Test*: two fake hellos differing only in build id → client must not accept.
2. **Unify the classifiers** — `probe_daemon` must use `classify_reachable`, not its own copy.
   *Test*: same hello through both paths → identical verdict (structural anti-drift test).
3. **Non-empty invariant primitive** — a `PtyChild` assertion `screen_row_count_where(pred) >= n`.
   *Test*: open `/model` with an empty catalog → red.
4. **Quiescence + absence** — port `WaitCondition::{Gone, StableMs}` into `pa-perf`.
   *Test*: a row that should disappear → `Gone` red without the fix.
5. **Failure artifacts** — dump screen text/html + raw bytes into the run dir on any pty-e2e failure.
6. **Perf baseline gate** — per-platform p99 baseline, 15% threshold, in `scripts/fork/prepush-gate.sh`.
7. **Kill the fixture from the release path** — `release-fork.yml` must fetch or vendor a real
   catalog, and `bundle_catalog.py verify` must refuse `--fixture` output for a release.
8. **`pdeathsig` in the harness** — not only in one test.

## 6. What not to port

- **Bazel/cargo dual-build.** Their split exists because the tree is synced from a monorepo;
  the `#[ignore]` + `--ignored` protocol is the price. We have one build system and a local gate.
- **A separate 100k-line test-infra crate.** Their `xai-grok-test-support` is justified by ~30
  consumer crates. We already have `pa-perf` + `qa-tasks`; grow those.
- **The full 35-action DSL.** We need the oracle taxonomy and artifact discipline first; a YAML
  layer without a non-empty primitive would reproduce the incident with more ceremony.
- **`env::set_var` under `unsafe`.** Their invariant depends on which threads exist at that moment.
  Prefer passing an explicit env map into the spawn.
- **Version-string comparison as the mismatch signal.** It is what let our two binaries pass;
  build identity replaces it.

---

## 7. The disposal contract — corrected and sharpened

The first pass said: *"`assert_no_script_violations()` and the check on Drop guarantee a test cannot
pass if not all scripts ran — protection against false-positive tests."* True, but it misses the
mechanism that makes it safe.

`MockInferenceServer::drop` (`xai-grok-test-support/src/mock_server.rs:1169`):

```rust
impl Drop for MockInferenceServer {
    fn drop(&mut self) {
        self.state.overrides.release_parked_replies();
        self.state.overrides.release_completions();
        if let Some(tx) = self.shutdown_tx.take() { let _ = tx.send(()); }
        if !std::thread::panicking() {
            self.assert_no_script_violations();
        }
    }
}
```

Three details the summary skipped, each load-bearing:

1. **`if !std::thread::panicking()`** — the drop check is suppressed while unwinding. Without it, a
   genuine assertion failure would be masked by a second panic about unused scripts, and the
   original message (usually the real bug) would be lost. The guard exists because they already hit
   that failure mode.
2. **`#[must_use = "assert on the violations; finishing disarms the drop check"]`** on
   `finish_scripts()` — the exception path is typed, not a comment. A test that *expects* a
   violation must claim it; the compiler nags otherwise.
3. **The panic message repairs the trap it creates**: *"a test that returned early through `?` has
   its error replaced by this panic; call `finish_scripts()` before returning"*. They document a
   known leak in the abstraction rather than hiding it.

`script_violations()` is also careful about multiplicity: *"Each violation once: those recorded,
then every script not served through or never opened."* — so a script that was never even opened
counts as a violation, not silently passes.

**Why this matters for us:** it is the one mechanism that makes a *mock-driven* test trustworthy.
Every test we write against a fake provider/socket can pass for the wrong reason — the fake was
never asked, or the test returned before the interesting part. A drop-time, panic-safe check turns
that into a hard failure. We have no equivalent, and our `qa-tasks/` replay layer would benefit most.

---

## 8. Two claims from the first pass that are now corrected by code

**(a) "The client does not refuse to serve data based solely on a build ID mismatch."**
Partly wrong, and the correction is the most useful thing in this document.

`xai-grok-shell/src/leader/server.rs:8`:
```rust
const LEADER_VERSION: &str = match option_env!("VERSION_WITH_COMMIT") {
    Some(v) => v,          // "<version> (<12-char git sha>)"
    None => "unknown",     // dev build → mismatch detection disabled
};
```
and `server.rs:1479`:
```rust
fn make_version_mismatch_notification(client_version: &str, leader_version: &str) -> Option<String> {
    if client_version == leader_version || leader_version == "unknown" { return None; }
    Some(json!({"method": "x.ai/leader/version_mismatch", …}))
}
```

So the comparison **does** include the commit for the leader side, and the message is prescriptive:
*"Restart the grok binary to use the same version."* They also degrade deliberately: an unstamped
dev build reports `"unknown"` and detection switches **off**, rather than producing false alarms
against a source build.

**The gap that remains — and it is exactly ours.** The client side sends
`binary_version: Some(xai_grok_version::VERSION.to_string())` (`leader/client.rs:645`) and
`leader_binary_version: env!("CARGO_PKG_VERSION")` (`leader/client.rs:1204`) — i.e. **plain
version strings, not `VERSION_WITH_COMMIT`**. So when a client compares against a leader that *is*
stamped, it compares `"1.0.45"` against `"1.0.45 (559751fdcec0)"` and reports a mismatch for two
builds that are actually identical. Asymmetric stamping is its own defect class.

**What this teaches us, precisely:** stamping the build is necessary but not sufficient; the
comparison must be *symmetric* — both sides must send the same identity format, or the check
produces noise and gets disabled. Our incident is the other failure: we send the same *shape*
(plain version) on both sides and therefore compare two different binaries as equal.

**(b) "No build-id comparison exists."** Corrected: it exists for the leader↔client handshake
(`LEADER_VERSION` / `client_version`), and is unit-tested via the extracted pure function
`make_version_mismatch_notification`. What does **not** exist is build identity for the
**daemon_hello** path that our architecture uses — the analogy to draw, not the absence to cite.

---

## 9. One-line verdict

Their methodology's real contribution is not "many layers" or "a PTY harness" — it is that
**every failure mode has its own oracle and its own artifact**: absence is an assertion
(`WaitCondition::Gone`), quiescence is an assertion (`StableMs`), a turn is an assertion
(`AgentTurnExpectation` over two dialects), an unused script is a failure (`Drop` +
`!panicking`), a slow frame is a failure (p99 vs per-platform baseline), and a leaked child is a
failure (kernel `pdeathsig`). We have exactly one of those oracles (screen text) and zero of the
artifacts.
