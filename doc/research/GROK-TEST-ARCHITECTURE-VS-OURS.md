# Grok Build's test architecture vs ours — what to take, what to skip, and what it costs

Date: 2026-10-24. Scope: read-only. Every claim about `xai-org/grok-build` is read from the
public tree at `SOURCE_REV 559751fdcec02d413e4c57c8832ab275e4f44980` through `gh api`
(contents + git tree); every claim about us is read from `/root/prime-agent` at `01de64db2`
plus the working tree. Nothing was built, run, cloned or committed; no process was started.

Paths under "Grok" are relative to the repo root of `xai-org/grok-build`. Paths under "ours"
are relative to `/root/prime-agent`. `UNVERIFIED` marks a claim I could not read directly
(usually because the file lives outside the public subset, or the code-search API rate-limited).

## 0. What bounds this report

1. **There is no public tracker.** `gh api repos/xai-org/grok-build` does not even carry the
   `hasIssuesEnabled` key (it reads `null`), the issues endpoint returns `[]`, `gh issue list`
   answers "the repository has disabled issues", and `hasDiscussionsEnabled` is `false`.
   `CONTRIBUTING.md:1-20` says the repository "does **not** accept external pull requests or
   unsolicited patches". So every judgement below about *problems* in Grok's suite comes from code
   or from third-party write-ups — never from a maintainer's issue. Anything of the form "they
   probably struggle with X" is UNVERIFIED.
2. **There is no public CI.** `crates/...` is present, but `.github/` returns 404 and
   `filename:BUILD.bazel` returns 0 hits. The only in-tree statements about CI are comments and
   `#[ignore]` attributes (cited below). **How the pty suite is scheduled, what it costs, and
   what a red run blocks are UNVERIFIED.** I mark each such statement.
3. **The tracker is not merely empty, it is switched off.** `hasIssuesEnabled` is absent/false on
   the repo object, `gh issue list -R xai-org/grok-build` answers "the repository has disabled
   issues", `gh api repos/xai-org/grok-build/issues` returns `[]`, `hasDiscussionsEnabled` is
   `false`, and `releases` is `[]` for the public mirror. So **no issue-derived evidence exists for
   any claim in §E below**; every one of them is marked UNVERIFIED and sourced from external
   analyses. What *is* observable about the release cadence: the mirror's newest commit is
   `2bdd1d6a6369` "Synced from monorepo" (2026-09-29), i.e. the public tree lags the monorepo and
   carries no history of its own.
4. **Three sources, deliberately cross-checked against each other.** DeepWiki
   (`read_wiki_structure` + ~12 `ask_wiki_question` calls) was used for navigation only; every claim
   it made that appears here was re-read in the raw file through `gh api`, and **three DeepWiki
   answers were wrong or imprecise** and are corrected in place: it said `ScreenTracker` is
   "wrapped by ptyctl" (right) but placed the empty-state row in `views/picker.rs` as a *unit test
   in `settings_e2e.rs`* (right, but it omitted that the model picker's own empty path has no test
   at all — §6); it claimed `build.rs`'s stamp reaches a wire message via `LogEntry.ver`
   (`unified_log.rs:17-23,72-75` — that field is stamped with `xai_grok_version::VERSION` at
   `xai-grok-shell/src/agent/init.rs:461` and `agent/app.rs:233,320,717`, i.e. the *unstamped*
   version, so the commit does **not** reach the wire by that route either); and it asserted
   row-count assertions exist generally, which is true at two tiers and false at the PTY tier over
   backend data (§6). Treat DeepWiki here as an index, not an authority.

---

## 1. The shape of their test architecture

Three crates carry the whole methodology; everything else is per-suite constants.

| Piece | Size (bytes / files) | Role |
|---|---|---|
| `crates/codegen/xai-grok-pager-pty-harness` | 1,755,136 B / 335 files | pty harness + declarative scenario runner + benchmarks + scroll matrix |
| `crates/codegen/xai-grok-test-support` | 849,860 B / 74 files | mock inference server, sandbox/env owner, ACP clients, headless runner, OTLP doubles |
| `crates/codegen/ptyctl` (+ `ptyctl-cli`) | not sized here | headless terminal (alacritty_terminal) + HTTP driver |

Inside the harness crate, the split by directory (from the git tree listing, `size` per blob):

```
899,331 B  176 files  tests/pty_e2e            (the imperative PTY suite, incl. minimal/ 41)
228,658 B   12 files  src                      (harness + scripted runner)
195,835 B   28 files  tests                    (roots, standalone suites, scripted_scenarios.rs)
122,627 B    8 files  src/scroll_matrix        (log-invariant matrix)
 70,912 B   45 files  tests/scenarios          (declarative YAML)
 29,924 B   10 files  src/scenarios            (bench workloads)
 27,562 B    7 files  tests/leader_pty_e2e
 22,967 B    2 files  benches
 18,694 B    4 files  src/bin
```

The layering is stated in `src/lib.rs:1-20` and is worth quoting because it is the engineering
claim, not just a file list:

- **L1 `pty`** — spawn, inject keys, resize, drain (`src/pty.rs:74-91`).
- **L2a `screen`** — "what the user sees" via `alacritty_terminal` (`src/screen.rs:1-8`).
- **L2b `timing`** — per-frame durations from `CSI ?2026 h/l` markers (`src/timing.rs:1-4`).
- **L3 `content`** — mock inference server driving *real* content into the pager
  (`src/content.rs:102-131`).
- **`scenarios`** — named workloads returning `BenchResults` (`src/scenarios/mod.rs:1-5`).
- **`results`** — statistics + baseline compare (`src/results.rs:1-2`).
- **`scroll_matrix`** — JSONL ingestion for a scroll validation matrix (`src/scroll_matrix/`).
- **`env`** — binary resolution (`src/env.rs:66-90`).
- **`flows`** — cross-suite drive/seed helpers (`src/flows.rs:1-6`).

### 1.0 The full crate map, and who consumes whom

The brief expected six groups; the tree has them plus two more that matter. From
`xai-grok-test-support/README.md`'s consumer matrix and `Cargo.toml` manifests:

| Crate | Test role | Consumed by | Which defect class it exists for |
|---|---|---|---|
| `xai-grok-test-support` | doubles + sandbox + clients + headless runner | `xai-grok-shell` tests, `xai-grok-pager-pty-harness` (`ContentController`), `xai-grok-sampler`, `xai-grok-telemetry` | "the test cannot reach the failure without a controllable peer" |
| `xai-grok-pager-pty-harness` | pty harness, YAML runner, benches, scroll matrix | pager e2e + benches only (never the product) | "the defect only exists in a real terminal" |
| `ptyctl` (+ `ptyctl-cli`) | headless terminal on `alacritty_terminal`; event-driven waits; HTTP driver | the pty harness | "screen assertions without a screen" |
| `xai-grok-shell/tests/*` | **43** files: ACP stdio, leader fixtures, signed managed config, headless | — | protocol/registration errors, signed-policy trust |
| `xai-grok-tools` | **6** integration files + in-module unit tests | — | per-tool argument/result shape |
| `xai-grok-update/tests/*` | **10** files: install.sh blitz, downgrade matrix, concurrent convergence, install_internal | — | "an install that bricks the machine" |
| `xai-grok-telemetry/tests/*` | **16** files: external OTLP over HTTP/gRPC/TLS/mTLS, gates, guard, session ctx | — | "a secret leaves the machine" |
| `xai-grok-bundle` (unit) | per-file checksum manifest, prune rules | — | "a user-edited file is overwritten" |

Two of these are worth naming as *separate tiers* that the six-crate picture hides:
`xai-grok-update/tests/` (an installer correctness suite) and `xai-grok-telemetry/tests/` (an egress
correctness suite). Both assert invariants nobody can see from the UI, and both are small.

### 1.1 Layer separation is enforced by dependency direction, not by convention

`Cargo.toml:12-16` says the crate is **not** imported into the production pager binary, and that
"spawn-only pager tests live here so `cargo test -p xai-grok-pager` never compiles this crate".
In other words: the harness depends on the product; the product never depends on the harness.
That is the same rule we hold for `pa-perf` (`crates/pa-perf/Cargo.toml` is dev/CI tooling), and
it is the rule that keeps a test helper from becoming a production dependency.

### 1.2 The suite is split into independently schedulable roots

`Cargo.toml:70-156` declares 11 `[[test]]` roots, 4 `[[bin]]`s and 2 `[[bench]]`es. Each root is
a *family* that fans in per-case modules with `#[path]`:

```rust
// tests/pty_e2e_smoke.rs:5-12
#[allow(dead_code, unused_imports)]
#[path = "pty_e2e/common.rs"]
mod common;
#[path = "pty_e2e/agent_response.rs"]
mod agent_response;
```

`sources/pty_e2e_smoke.rs` fans in 37 modules; `pty_e2e_scroll_selection.rs` 39;
`pty_e2e_queue.rs` 31; `pty_e2e_config_ui.rs` 31; `pty_e2e_shell_tools.rs` 24;
`pty_e2e_clipboard.rs` 12; `pty_e2e_persistence.rs` 9; `pty_e2e_minimal.rs` 3.

The stated reason is resource bounding, not compile speed — `tests/pty_e2e_smoke.rs:3`:
"All cases are ignored for ordinary Cargo runs; Bazel opts in and caps this process-heavy family
at four concurrent libtest workers." One root = one libtest process = a cap you can hold.

**Verdict for us: skip.** The split exists because their tree is synced from a monorepo with a
second build system. We have one build system and `cargo nextest` with a serial tail
(`scripts/fork/prepush-gate.sh:224-247`); a per-family binary would buy us nothing and would add
11 `[[test]]` stanzas to maintain.

---

## 2. The declarative scenario layer (the part we have no equivalent of)

`src/scripted.rs` is a **document-driven** runner: a YAML file is the whole test.

```rust
// src/scripted.rs:33-50
pub struct ScriptedScenario {
    pub name: String,
    #[serde(default)] pub description: String,
    #[serde(default)] pub terminal: TerminalConfig,
    #[serde(default)] pub environment: EnvironmentConfig,
    #[serde(default)] pub workspace: Option<WorkspaceConfig>,
    #[serde(default)] pub mock: MockConfig,
    #[serde(default)] pub steps: Vec<ScenarioStep>,
}
```

### 2.1 The document schema, field by field

| Field | Type | Semantics | Cite |
|---|---|---|---|
| `name` | string | also the artifact directory name (`safe_name`) | `:33`, `:480` |
| `description` | string | free text, lands in `report.json` | `:35` |
| `terminal` | `{rows, cols, respond_to_queries}` | 50x120 default; `respond_to_queries` feeds terminal replies back to the child | `:88-107` |
| `environment.os` / `.arch` | `Vec<String>` | allow-lists; a mismatch turns the run into `Skipped`, not `Failed` | `:110-137`, `:487-496` |
| `environment.env` | `[{key, value}]` | per-scenario env on top of the sandbox baseline | `:126-130` |
| `environment.args` | `[string]` | argv before the pager's own | `:132-134` |
| `environment.config_toml` | string | written to `$GROK_HOME/config.toml` before spawn | `:136-139`, `:512-518` |
| `workspace` | `{git_init, files{path: content}}` | an ephemeral temp dir, optionally `git init`-ed, used as cwd | `:147-157`, `:650-696` |
| `mock.response` | string | default assistant text | `:159-161` |
| `mock.turns` | `[string]` | **ordered** per-turn expectations; **every** one must be satisfied | `:162-164`, `:502-510`, `:617-633` |
| `mock.fixture_images` | `[{name, extension, kind}]` | synthetic PNGs (standard / tiny 1x1 / CRC-corrupt) | `:169-190`, `:1290-1373` |
| `steps` | `[ScenarioStep]` | the action list | `:204-368` |

Two of these are load-bearing and have no analogue in our tree:

- **`mock.turns` is a turn-level oracle.** A scenario can say "I expect three agent turns, in
  order"; the runner then reports a `BugFinding` whose message aggregates the diagnostics of the
  turns that never landed (`:617-633`). That is stricter than "the response text appeared once".
- **`workspace` constructs a repository.** `git_init: true` plus a declared `.mcp.json` is how
  `folder_trust_prompt.yaml` reaches the trust prompt at all. A scenario owns its *preconditions*,
  not only its keystrokes.

### 2.2 The step vocabulary — 44 actions

`ScenarioStep` (`src/scripted.rs:204-368`), `#[serde(tag = "action", rename_all = "snake_case")]`.
Grouped by what they prove, with the dispatch arm:

| Group | Actions | Dispatch |
|---|---|---|
| wait | `wait_for_text` | `:707-709` |
| screen assert | `assert_contains`, `assert_not_contains` | `:710-719` |
| type | `type_text`, `keys`, `paste` | `:720-738` |
| semantic focus | `focus_prompt`, `paste_prompt`, `drop_text_prompt` | `:751-777` |
| images | `paste_image_paths`, `paste_image_file_urls`, `paste_escaped_image_paths`, `paste_clipboard_image`, `drop_images_prompt` | `:739-765`, `:885-897` |
| pointer coords | `mouse_click`, `double_click`, `triple_click`, `drag`, `select_text`, `drop_text` | `:778-795`, `:820-825`, `:867-876` |
| pointer semantic | `click_text`, `double_click_text`, `triple_click_text`, `select_text_range`, `scroll_at`, `drop_text_at` | `:826-884` |
| scroll | `scroll` | `:796-806` |
| clipboard | `copy`, `assert_osc52_contains`, `assert_osc52_not_contains` | `:898-920` |
| styling | `assert_highlight_run`, `assert_text_not_highlighted` | `:921-972` |
| terminal protocol | `assert_kitty_graphics`, `wait_for_kitty_graphics`, `assert_no_kitty_graphics` | `:973-987` |
| wire | `assert_request_image_count`, `assert_inline_images`, `assert_request_contains` | `:988-1003` |
| filesystem | `assert_no_temp_artifacts` | `:1004` |
| control | `resize`, `wait`, `screenshot`, `assert_running`, `assert_chat_completion` | `:1005-1026` |

The three assert families that matter are **not** screen-text asserts:

- **OSC 52 decoding** (`:1744-1765`): the runner base64-decodes every `\x1b]52;` payload and
  asserts on the clipboard text. This is how `tool_header_path_selection.yaml` proves a copy took
  the path alone and not the `Read ` label.
- **Kitty graphics counting** (`:1769-1792`): APC frames live outside the cell grid, so no screen
  snapshot can see them; the counter excludes `a=d`/`a=q` so a capability probe cannot satisfy a
  "graphics were emitted" assert.
- **Highlight runs** (`:921-972`): "the contiguous highlighted run at the first occurrence of
  `at_text` renders exactly `equals` over one uniform background". The comment at `:945`
  explains the failure it exists for: "Untrimmed: a highlight claiming padding or borders must
  fail."

### 2.3 How the runner executes a scenario

`ScriptedScenarioRunner::run` (`src/scripted.rs:474-645`), in order:

1. `validate()` — non-empty name, non-empty steps (`:75-85`).
2. Create `artifact_dir/<safe_name>/<epoch_ms>/`; write nothing yet (`:477-483`).
3. Materialize image fixtures into `<run_dir>/fixtures/` (`:485`, `:1290-1307`).
4. Platform filter → `Skipped` with a `skip_reason`, still writing `report.json` (`:487-496`).
5. Start `ContentController` (mock inference on a random loopback port + isolated `$HOME`) and
   register the default response and the ordered turns (`:498-510`).
6. Optional `config.toml` into `$GROK_HOME` (`:512-518`).
7. Optional workspace materialization (`:535-539`).
8. `PtyHarness::new_in_sandbox(...)` with terminal size, args, env, cwd (`:541-551`).
9. **Step loop** (`:553-595`). On the first error: build a `BugFinding` with the full screen text,
   mark `Failed`, write `raw_output.bin`, capture artifacts named `NN-failure`, write
   `report.json` and `bugs.md`, `quit()`, return (`:567-593`).
10. After the steps: if the pager is not running, a `BugFinding` "pager exited before scenario
    completed" and `Failed` (`:597-605`).
11. Settle the turn expectations: pump for up to `EXPECTATION_SETTLE_TIMEOUT` (10 s, `:29`)
    until every `mock.turns` entry is satisfied (`:607-616`); unsatisfied ones become a
    `BugFinding` listing both wire dialects (`:617-633`).
12. `quit()`, write `raw_output.bin`, `report.json`, `bugs.md` (`:635-643`).

Two details worth copying:

- **Failure is classified, not just reported.** `BugFinding { step, severity, message, screen_text }`
  (`:1246-1252`) distinguishes "assertion failed", "product died mid-scenario" and "the turn never
  landed". `write_bug_markdown` (`:1260-1288`) turns the run into a readable report with the screen
  and the artifact list.
- **Every wait carries its own budget and reports the screen on timeout.** `PtyHarness::wait_until`
  (`src/lib.rs:365-398`) bails with `process running`, `process tree diagnostics` and the whole
  screen. `wait_until_stable` (`:403-444`) additionally requires the predicate to hold for a
  window — the principled replacement for `sleep`.

### 2.4 A screenshot is four artifacts plus the raw byte stream

`capture_artifacts` (`src/scripted.rs:1031-1063`) writes, per `screenshot` step:

| Extension | Content | Producer |
|---|---|---|
| `.txt` | plain screen text | `packages.…screen_contents()` |
| `.html` | styled screen as HTML | `screen_html()` |
| `.svg` | rendered frame with the cursor box, drawn by `render_svg` (`:1065-1130`) | hand-rolled SVG |
| `.json` | styled runs (fg/bg/bold/italic/underline) | `screen_styled()` |
| `raw_output.bin` | the whole PTY byte stream | `raw_output()`, on failure and at the end (`:578`, `:640`) |

The comment at `:639` names the reason for the raw dump: "for offline analysis (e.g. Kitty
`a=t` vs `a=p` counts)". A red run is reconstructible without a rerun — and `path_space_hyperlink`
and `inline_image_memory` are tests whose *oracle* is a byte-level property of that file
(`tests/scripted_scenarios.rs:188-215`, `:278-300`).

### 2.5 How the scenarios are wired into `cargo test`

`tests/scripted_scenarios.rs` is 531 lines: 40 `#[tokio::test]` functions, **all `#[ignore]`d**,
38 of which are `run_scenario("<file>.yaml")` calls (`:19-42`); two are bespoke wrappers that open
the report and inspect `raw_output.bin` (`:191-215`, `:279-300`).

The gate that *does* run by default is a **parse gate** (`:482-514`): `scenarios_parse()` reads 26
YAML files through `ScriptedScenario::from_file` and asserts `steps` is non-empty, with the stated
reason "so a malformed YAML breaks CI immediately, not only when the scripted runner is opted in".
Two more single-file parse tests follow (`:516-531`).

Measured from the tree: **45 YAML files** exist; **38** are driven by a `run_scenario` call;
**28** are parse-checked at plain `cargo test` time. Five YAMLs (`html_entities.yaml`,
`html_entities_rendering.yaml`, `minimal_flush_left.yaml`, `minimal_transcript_hint.yaml`,
`minimal_transcript_hint_apple_terminal.yaml`) are referenced *nowhere* in the crate — dead
scenario files that neither run nor parse-check. This is a real, verifiable gap in their own
housekeeping, and it is the failure mode of any corpus that grows by copy-paste.

### 2.6 Which binary a test runs — three sources, in order

`src/env.rs:66-90` (`pager_binary`):

1. `PAGER_BINARY`, absolutized (Bazel passes a runfiles-relative path, and portable-pty
   resolves a relative path through `PATH` instead of the cwd — hence `std::path::absolute`
   at `:72-79`); a missing path is a hard error, never a silent fallback;
2. `CARGO_BIN_EXE_xai-grok-pager` when it exists;
3. else `cargo build -p xai-grok-pager-bin --bin xai-grok-pager` with stdin closed and the
   child detached from the TTY (`:30-64`), then `target/debug/xai-grok-pager`.

The third branch is what lets a bare `cargo test -- --ignored` work with no setup, and it is
also why the suite is not free: the first ignored test in a cold tree pays a full debug build of
the pager. **Ours** resolves the binary the same way in spirit — `crates/pa-cli/tests` use
`env!("CARGO_BIN_EXE_prime-agent")`-style paths and `crates/pa-cli/tests/packaged_layout_e2e.rs`
stages a release layout — but there is no single `pager_binary()` equivalent, so each suite
re-decides. That is a small, real duplication (one function, ~20 lines) and it belongs in the
shared harness of §9.3.

### 2.7 Gating: `#[ignore]`, and what the numbers say about cost

| Fact | Value | Source |
|---|---|---|
| `[[test]]` roots in the crate | 11 | `Cargo.toml:70-118` |
| `#[path]` module entries across the 8 pty roots | 37+31+39+3+31+24+9+12 = **186** entries → 178 distinct paths (the shared `common.rs` is declared 8×; `scroll.rs` 2×) | the eight root files |
| Case files under `tests/pty_e2e/` | **176** top-level + the `minimal/` subtree, whose `mod.rs` declares **39** more | the git tree |
| One case declared from a *different* root than its subtree | `pty_e2e/minimal/minimal_ctrl_o_send_now_queued_apple_terminal.rs` is declared by `pty_e2e_queue` (it is an apple-terminal paste case), not from `minimal/mod.rs` | `tests/pty_e2e_queue.rs` (the `minimal/` path entry) |
| Test functions in `scripted_scenarios.rs` | 40 `#[tokio::test]` (all `#[ignore]`d) + 3 plain `#[test]` parse checks (always run) | the file |
| Scenario YAMLs driven by a test | 38 | `run_scenario` calls |
| Scenarios parse-checked at plain `cargo test` | 28 of 45 | `:482-531` |
| Curated scroll cells, **not** ignored | 8 + 1 tripwire | `tests/scroll_matrix_curated.rs` |
| Bench scenarios | 6 | `src/scenarios/mod.rs:40-47` |
| Sampled test functions per `pty_e2e` file | ~1 (12 files sampled: 14 fns / 12 files) | measured by reading 12 files |

So the gating story is: **`#[ignore]` by default, opt in with `--ignored`, except the curated
scroll cells and the parse tests, which always run.** Their stated reasons: cost
(`tests/pty_e2e_smoke.rs:3`), and opt-in for anything that spawns the real binary
(`tests/exit_timeout.rs:20-21`). Wall-clock for a full `--ignored` run is **UNVERIFIED** (no CI
config, and I did not build their tree); the honest statement is "186 pty cases + 38 scenarios,
each spawning a real TUI and a mock server", which is the same order as our 16 pty files.

One consequence worth naming, because it is the trap we would fall into too: **a suite that is
`#[ignore]`d by default is a suite that is green because it did not run.** They mitigate it with
the parse gate (28 of 45 files) and the curated-cell tripwire, and still left five YAMLs that
neither run nor parse (§2.5). Any declarative layer we add needs both lists *derived from the
directory*, not enumerated.

### 2.8 The standalone driver

`src/bin/pty_scenario.rs` (75 lines) exposes the runner as a CLI: `--scenario`, `--binary`,
`--artifacts` (default `target/pty-scenarios`), prints the report as JSON, and maps
`Passed | Skipped → 0`, `Failed | Running → 1`, usage errors `→ 2`. That is what makes a scenario
usable **outside** the test binary — a reviewer can run one YAML against a release artifact.

---

## 3. The PTY harness itself

### 3.1 `PtyHarness` — one struct, three consumers

`src/lib.rs:83-99`. The same type serves (a) regression scenarios, (b) benchmarks, (c) ad-hoc
reproduction runs — stated at `:1-20`. Its surface:

| Capability | Method | Cite |
|---|---|---|
| spawn inside a sandbox | `new_in_sandbox[_ops]` | `:123-158` |
| spawn with inherited env | `new_inherited_env` | `:103-121` |
| pump output | `update(timeout)` / `pump_one` | `:291-322` |
| screen text | `screen_contents` | `:342-344` |
| screen HTML | `screen_html` | `:350-352` |
| styled runs | `screen_styled` | `:346-348` |
| any predicate wait | `wait_until(desc, timeout, f)` | `:365-398` |
| stable-for-a-window wait | `wait_until_stable(desc, timeout, hold, f)` | `:403-444` |
| text wait / text-absent wait | `wait_for_text`, `wait_for_text_absent` | `:447-460` |
| turn-idle wait | `wait_for_turn_idle` | `:465-476` |
| scrollback-inclusive text | `scrollback_text`, `full_text`, `wait_for_full_text` | `:519-560` |
| kitty graphics | `count_kitty_graphics`, `wait_for_kitty_graphics` | `:562-587` |
| raw bytes | `raw_output` | `:479-482` |
| asciinema cast | `write_cast` | `:484-517` |
| resize | `resize` | `:282-286` |
| terminal modes | `terminal_modes` | `:360-362` |

Two design decisions to note:

- **Pumping is inline so inter-chunk timing survives** (`:290`): buffering first would erase the
  spacing the frame parser needs.
- **`respond_to_queries` is opt-in** (`:174-177`): the emulator can answer DSR/DA probes by writing
  them back to the child, and the doc says exactly why it is off by default — a startup
  cursor-position query that goes unanswered silently downgrades `--minimal` to full-screen inline
  (`src/screen.rs:28-31`).

### 3.2 Screen model: alacritty, not vt100

`src/screen.rs:9-...` wraps `ptyctl::term::Terminal` (an `alacritty_terminal` model) and adds the
three things tests actually need: styled runs, HTML, and scrollback. `full_text()` (`:102-...`)
joins scrollback and screen because minimal mode's committed blocks live in either region — the
docstring states that as the reason assertions must use `full_text`, not `contents`.

**Verdict for us:** we use `vt100` 0.15 (`crates/pa-perf/Cargo.toml:21`,
`crates/pa-tui/Cargo.toml:36`). `vt100` is adequate for text + a screen grid; it cannot produce
styled runs or a faithful reflow. Whether that difference is worth an `alacritty_terminal`
dependency for us is a separate judgement (§8, item 3): the *oracle taxonomy* does not need
alacritty, only styled-run asserts do.

### 3.3 Frame timing from the escape stream

`src/timing.rs:13-19` — `FrameTiming { duration, chars }`; `:21-...` a `vte::Parser` whose `Perform`
implementation watches `CSI ? 2026 h/l` (`:96`) and timestamps begin→end. crossterm emits exactly
those markers for `BeginSynchronizedUpdate`/`EndSynchronizedUpdate`, which our TUI also uses
(`crates/pa-tui/src/app.rs:279,308`). **The signal exists in our binary already**; we simply never
parse it.

### 3.4 Process ownership: own fork/exec, PDEATHSIG, bounded teardown

`src/pty.rs:120-194` — they do not use portable-pty's `spawn_command`; they fork/exec themselves:

```rust
// src/pty.rs:131
// Own fork/exec: portable-pty has no pre_exec, and the child must arm PDEATHSIG so a SIGKILLed
// runner (no Drop) cannot leak it.
```

`src/pty_spawn.rs:192` arms `PR_SET_PDEATHSIG(SIGKILL)` via `xai_tty_utils`, and the crate bans the
plain spawn through lint policy: `clippy.toml` disallows
`portable_pty::SlavePty::spawn_command` — "an unenrolled pty child outlives its session; enroll
the shell with `xai_tty_utils::ProcessScope::enroll_terminal_pid`". Teardown is bounded:
`quit()` sends `q`, waits 5 s, then kills the tree and waits 1 s more (`src/pty.rs:238-255`);
`Drop` runs the same cleanup (`:457-481`).

Their residual gap is documented rather than hidden (`tests/orphan_reap.rs:1-20`): pdeathsig covers
only the direct child, so a SIGKILLed runner can still leave the pager's own tool subprocesses
behind; the test is Linux-gated for that reason and asserts the *observable* property ("stops
running", zombie-tolerant) rather than the mechanism.

**Ours:** `pdeathsig` appears exactly once in the test tree
(`crates/pa-cli/tests/windows_runtime_triage_e2e.rs:177`) and once in
`crates/pa-cli/tests/interactive_daemon_e2e.rs:415`; `pa-perf` sets a new session but does not arm
pdeathsig (`crates/pa-perf/src/bench/pty.rs:75`). 16 files spawn a pty, 10,949 lines, each with
its own teardown.

### 3.5 Environment hygiene is an explicit, consumer-named deny list

`src/pty_spawn.rs:141-...` (`apply_child_env`) removes, before the caller's own overrides:

| List | Examples | Stated reason |
|---|---|---|
| clipboard sinks | `GROK_OSC52_SINK`, `LC_GROK_OSC52_SINK` | a harness under `grok wrap` would confirm clipboard delivery with no sink |
| appearance | `GROK_APPEARANCE`, `GROK_THEME`, `COLORFGBG` | "`theme=auto` non-deterministic; layout depends on the resolved palette" |
| host terminal | `CURSOR_TRACE_ID`, `TERM_PROGRAM`, `WEZTERM_VERSION`, `TMUX`, `NVIM`, … | "a leak reclassifies the child (tmux, Cursor before TERM_PROGRAM, nvim OSC 52)" |

Each list says which product code reads it, and `:28-32` says "Keep in sync with that detection
source" — hygiene is tracked to its consumer, not to a vibe.

### 3.6 Every timeout is multiplied by one env knob

`xai-grok-test-support/src/lib.rs:30-39`:

```rust
/// Multiply a harness timeout by `GROK_TEST_TIMEOUT_SCALE` (positive integer, default 1).
pub fn scaled(base: Duration) -> Duration { ... }
```

and the headless runner's deadline is `crate::scaled(Duration::from_secs(60))`
(`headless.rs:23-25`). The comment gives the motive: "CI lanes on shared runner pools raise it so
pool load slows tests instead of failing them."

**Ours: nothing.** `Duration::from_secs(N)` is spelled inline in every pty test — 15 literal
timeouts in `crates/pa-cli/tests/daemon_discovery_e2e.rs`, 13 in `interactive_daemon_e2e.rs`, 7 in
`termios_process_exit_e2e.rs`. A loaded runner therefore turns into a red suite, and the only
remedy is editing N literals. One `fn scaled(base) -> Duration` reading
`PRIME_AGENT_TEST_TIMEOUT_SCALE` (name ours, do not copy theirs) is ~8 lines and makes every
existing timeout honest on a loaded host without touching an assertion.

### 3.7 The sandbox is a shared, hermetic env owner

`crates/codegen/xai-grok-test-support/src/sandbox.rs:294-379` builds the child env from a platform
allow-list plus pinned overrides:

- `HOME`, `USERPROFILE`, `GROK_HOME`, `TMPDIR`, `TMP`, `TEMP` all point inside one temp root
  (`:323-332`);
- **every** network side-channel is switched off — telemetry, Mixpanel, feedback, trace upload,
  OTel (`:333-347`) — so a test cannot reach production;
- auto-updater disabled, prompt suggestions off, turn summary off, `NO_PROXY` pinned to loopback,
  git made hermetic (`GIT_CONFIG_NOSYSTEM`, `GIT_TERMINAL_PROMPT=0`, `GIT_ASKPASS=""`,
  `GIT_CONFIG_GLOBAL` inside the sandbox) (`:347-371`);
- `GROK_AGENT_ID` pinned because an empty `GROK_HOME` otherwise recomputes a machine id, which on
  Windows is a ~30 s WMI call that blew the harness deadlines (`:349-352`, with the incident id).

`TestSandbox::diagnostic_summary` (`:157-180`) redacts credentials and non-loopback endpoints —
so the failure message can be pasted into a PR.

**Ours:** `crates/pa-cli/tests` has 16 pty-spawning files and 11 that take their own
`HARNESS_LOCK`; the hermetic env is re-built per file (e.g.
`crates/pa-cli/tests/print_runtime_e2e.rs:119` `fn isolated_home()`,
`crates/pa-cli/tests/package_e2e.rs:234` `struct Sandbox`). There is no single owner, so the
deny-list cannot be kept in sync with the consumer.

---

### 3.8 Telemetry and upload: the layer that grades what leaves the machine

This deserves its own section because it is the one place their suite asserts a *negative* over
real egress, and because it is the closest structural analogue to our incident's failure surface.

### 3.8.1 A mock OTLP collector, and a recorder you can grep

`MockOtelServer::start()` serves `POST /v1/logs` and `/v1/metrics` on loopback
(`xai-grok-test-support/README.md`, `mock_otel_server` row), records every accepted message into an
`OtelRecorder`, and offers `exporter_env()` — the variable map that aims the product's real exporter
at it. The recorder's readers are `events()`, `log_record(name)`, `metric_points_named(name)`,
`event_names()`, `exports()`, `faults()` and, load-bearing for leak tests, **`body_text()`** — every
body as lossy text, which errors if a fault or an eviction means it would miss bytes
(`otel_recorder` row). Waits are `wait_for_events`, `wait_for_signals`, and
`wait_for_silence(window, is_broken)`, whose predicate sees only events recorded *since the call*.

Three assertions become possible that no other layer can make:

| Assertion | Mechanism | What it catches |
|---|---|---|
| "no secret on the wire" | `body_text()` + `assert!(!wire.contains(SECRET))` | a canary planted in env or a tool result reaching an exporter |
| "the exporter is actually dead" | `wait_for_silence(window, \|_\| true)` | a shutdown that leaves a flush loop running |
| "this export carries no auth header" | `exports()` → headers map | credential forwarding to a third-party collector |

The collector has a second, subtler property: a **decode failure is a recorded fault, and still a
200**, so the exporter does not retry a poison body (`mock_otel_server` row: "An undecodable body is
a fault and still a 200"). A malformed export therefore shows up as evidence rather than as a retry
storm that hides it.

`crates/codegen/xai-grok-telemetry/tests/` is where this is exercised: 15 files including
`external_otlp.rs`, `external_otlp_gates_on.rs`, `external_otlp_guard.rs`,
`external_otlp_session_ctx.rs`, `external_otlp_grpc{,_tls}.rs`, `external_otlp_mtls_{grpc,http}.rs`
and `otlp_collector/mod.rs`. The README states the shape: the real exporter over HTTP posts to the
collector; `flush`/`shutdown` block, so a case runs them through `spawn_blocking`; and the
collector case asserts that no export carries an `authorization` header.

### 3.8.2 Scrubbing is a *tested* allowlist, not a convention

`crates/codegen/xai-grok-telemetry/src/otel_layer/redact.rs` (777 lines) is the enforcement:
`enforce_allowlist` (`:209`), `scrub_attributes` (`:215`), `neuter_event_name` (`:226`),
`reduce_url_to_origin` (`:252`), `enforce_enum_values` (`:302`), and the enum sets that pin what a
free-form string may become (`SOURCE_STATUS`, `READ_SKILL_SOURCE`, `GREP_REASON`, … at `:260-300`).

Its own test module is the part to copy. The names read as a defect catalogue:

- `redact_value_scrubs_secret_string` (`:383`) — asserts `[REDACTED_SECRET]` replaced the value;
- `allowlist_drops_nonallowlisted_content_keeps_safe_and_numeric` (`:422`) — plants
  `KeyValue::new("command", "echo CANARY_SECRET")`, then asserts the serialized **blob does not
  contain `CANARY_SECRET`** (`:452`) while the safe and numeric keys survive;
- `allowlist_contents_are_pinned` (`:458`) — the list itself is pinned, so *widening* the allowlist
  is a failing test, not a silent privacy regression;
- `url_value_reduced_to_origin_dropping_path_and_query` (`:674`) and
  `url_valued_keys_reduced_to_origin_but_storage_paths_kept` (`:690`) — a path is not a URL origin;
- `error_key_value_is_secret_and_path_scrubbed` (`:760`) — the error path is scrubbed too;
- `event_name_neutered_to_callsite_drops_message_content` (`:399`) — a format string in an event
  name becomes a callsite marker, because `Event::new("SECRET {x:?}", …)` (`:416`) would otherwise
  ship the interpolated value.

`:458` is the single most transferable line in their whole telemetry suite: **pinning the allowlist
turns "we must remember not to add PII" into a red test.** Ours is a policy in AGENTS.md
("Telemetry properties never carry prompt, session, or file content (primitives only)"), enforced by
a *type* (`crates/pa-telemetry/src/properties.rs:3-23` rejects non-primitives with a warn) and by
`catalog::sanitize` dropping keys a rule does not name
(`crates/pa-telemetry/src/catalog.rs`, called on the send path at
`crates/pa-telemetry/src/client.rs:396,457`). What we do not have is the canary test: nothing plants
a secret and asserts it is absent from the serialized batch.

### 3.8.3 The egress tests that *did* find a real incident

`tests/pty_e2e/storage_upload_parks_on_401_and_drains_after_recovery.rs` (103 lines) is the pattern
worth stealing for any queued side channel. It sets storage to 401, submits a turn, and asserts
four separate properties:

1. the chat still works while storage is down (the product does not block on the side channel);
2. **nothing is accepted while the gate is closed** — `content.storage_uploads().is_empty()`;
3. the parked queue **does not spam**: `after <= parked + MAX_PARKED_WORKERS * REQUESTS_PER_WAKE *
   WAKE_CYCLE_HEADROOM` (`:69-79`), with the comment "This checks that requests stay bounded while
   parked, not that the queue goes fully quiet" — an honest statement of what the bound means;
4. after recovery, the parked artifact lands and `uploads.iter().all(|u| u.size > 0)`.

Externally, this surface is the one that produced a real, public incident: a wire-level analysis
reported that Grok Build 0.2.93 uploaded whole repositories as git bundles to xAI storage
(`thehackernews.com/2026/07/grok-build-uploads-entire-git.html`, `gist.github.com/cereblab/…`,
`news.ycombinator.com/item?id=48877371` — all UNVERIFIED as to cause and as to the fix; the
follow-up reports "no `/v1/storage` uploads across six runs" on the same client, which does not
distinguish a server-side change from a client one). What is verifiable in the tree *today* is the
test above: the upload path has a bounded-queue oracle and a "no upload while unauthorized" oracle,
and no test in the crate asserts **what** an uploaded artifact contains. That is the same shape as
our catalog defect — the mechanism is tested, the payload contract is not.

### 3.8.4 Why this matters for us specifically

Our `pa-telemetry` has `MockSink` (`crates/pa-telemetry/src/sinks/mock.rs:20,50,72`) and a wire test
(`crates/pa-telemetry/tests/analytics_wire.rs`) that pins the exact request body shape. What it does
not have is (a) a canary assertion over the serialized batch, (b) a pinned allowlist that fails when
widened, or (c) a live egress test through a real exporter. We rely on a type system plus a
key-dropping sanitiser, both of which are good — and neither of which observes the bytes.

## 4. Scripted model turns

### 4.1 One logical turn, two wire dialects

`src/content.rs:28-98` — `AgentTurnExpectation` registers **the same named turn on both**
`/v1/responses` and `/v1/chat/completions` (`:194-267`) and the API is
`wait_received` / `wait_blocked` / `release` / `wait_satisfied` / `is_satisfied` /
`assert_satisfied` / `diagnostic`, with `#[must_use = "keep the handle to synchronize or verify
the logical agent turn"]`. A test says "a turn happened"; it does not say which dialect the client
picked. Nothing in our tree is dialect-agnostic in this sense: our scripted loop tests are
per-provider-wire.

### 4.2 Four ways to pace a turn

`src/content.rs:169-171` `set_chunk_delay` — "each SSE event is emitted after `delay`"; the doc
names the use: "hold a turn visibly streaming long enough to interact with it (Esc-cancel tests)".
Combined with `expect_agent_turn_blocked` (`:210-224`, "blocked immediately before its terminal
event") a test can sit deterministically *inside* a turn rather than racing it.

### 4.3 Precedence is documented, so a test author can predict the answer

`xai-grok-test-support/README.md` (module map, `inference_override` row): named expectation
matching endpoint and kind → the endpoint's compatibility FIFO → auth rejection → the request's
conversation script → the concurrency cap. And a **script-consumption guard**: dropping the server
runs `assert_no_script_violations` (`src/mock_server.rs:1169`, `:482`) unless a test panicked, so
"the test cannot pass without its scripts having run". The README also names the trap that guard
creates: a `Result`-returning test must not let an early `?` drop the server before its scripts
finish, because the panic replaces the real error.

**That is the single most transferable idea in their whole suite for us**: a double that fails the
test when it was never exercised turns a vacuous pass into a red.

### 4.4 Request-side oracles

`ContentController::request_bodies()` (`src/content.rs:284`) plus
`assert_request_contains` (`src/scripted.rs:1486-1517`) give a scenario the ability to assert on
what the *model* received — e.g. `reasoning_efforts_model_menu_opens_on_default` asserts that the
second turn's body carries `model: grok-4.6` and `reasoning.effort: high`
(`tests/pty_e2e/reasoning_efforts_model_menu_opens_on_default.rs:75-89`). "The UI said it switched"
and "the request said so" are two different oracles, and they check both.

---

## 5. Benchmarks and the scroll matrix

### 5.1 Frame-time baselines with a failing gate

`src/results.rs:14-45` aggregates `total_frames, avg_fps, p50_ms, p99_ms, max_ms, jank_count,
jank_rate, chars_per_frame_avg`; jank is "frame time over 2x the median" (`:53-56`).
`DEFAULT_REGRESSION_THRESHOLD = 0.15` (`:86`) and `compare_baseline` fails a scenario whose p99 grew
more than 15% (`:125-150`). `benches/pty_baselines/README.md` pins per-platform files and the
review rule: an intentional shift must overwrite the baseline **and** put the bench output in the
PR body.

`benches/pty_bench.rs` is the driver: `--scenario`/`--all`, `--binary`, `--baseline`,
`--write-baseline`, `--threshold` (`:43-100`); a failed scenario is recorded as zeroed results
rather than aborting the sweep, and the JSON goes to stdout for CI.

**Ours:** `crates/pa-perf` has the same shape of measurement (9 named scenarios,
`crates/pa-perf/src/bench/mod.rs:8-46`) but its gate is **2x median** (`crates/pa-perf/src/report.rs:34`)
and it is not wired into `scripts/fork/prepush-gate.sh` (0 grep hits) — it is a `make perf`
target only (`Makefile:168-175`). 15% on p99 catches a regression that 2x on
median cannot.

### 5.2 The scroll matrix: invariants over a producer's own log

`src/scroll_matrix/invariants.rs:1-15` states the rule that makes a timing invariant safe:

> Stall-safety: every timing predicate reads the RECORDER's clock … never the test process's. CI
> load can stretch host-side gesture delays but can only ever *widen* the producer-measured
> spacings. So no invariant here can false-fail on a loaded machine.

Then 14 `InvariantId`s (I-ORD, I-CAP, I-DROP-EQ, I-CADENCE, I-CONS-W, I-CONS-A, I-ACCEL, I-CARRY,
I-CFG, I-MUX-NO-OVER, I-SMOOTH-COAST, I-NO-DROP, I-SCREEN, I-QUIET) with `is_log_side()` splitting
log-derived from harness-derived (`:60-66`).

`tests/scroll_matrix_curated.rs:1-30` runs the curated cells as ordinary (not `#[ignore]`d) tests
serialized by a `tokio::sync::Mutex`, artifacts under `TEST_TMPDIR` so Bazel's per-action tmp does
not collide, and a **floor-and-ceiling tripwire** (`:101-120`): `curated_cells_all_have_a_test`
asserts the curated list equals the covered list, so a cell added without a test cannot silently
skip CI.

That tripwire is worth stealing verbatim. It is the cheapest guard against a corpus that quietly
shrinks.

---

## 6. What they do NOT have (verified against code)

This section is answered by reading, not by trusting the survey that preceded it.

| Claimed gap | Verdict | Evidence |
|---|---|---|
| **A count oracle over DAEMON-SERVED list data** | **CONFIRMED — at the PTY tier only** | Count oracles exist at two other tiers, and my first pass missed them. Widget tier: `views/dashboard/render_tests.rs:2304` `fn idle_row_count`, with `assert_eq!(idle_row_count(&lines), 9)` at `:2329` and six more uses; `slash/commands/model.rs:667-688` `assert_eq!(items.len(), 2, "model phase: one row per logical model")`. PTY tier over a *locally produced* list: `tests/pty_e2e/dock_crowded_sections.rs:104-133` `assert!(dock.len() <= 8)` plus "section starts at its first row" / "the last rows start out hidden"; `tests/prompt_history_browse_order.rs:167-172` `assert_eq!(rows.len(), EXPECTED_NEWEST_FIRST.len(), "browse holds entries nobody typed")`. What does **not** exist: a PTY-tier count over rows the *backend* served. `ScenarioStep` (`src/scripted.rs:204-368`) has no count action, and the one model-list scenario proves non-emptiness by *naming* a row (`tests/scenarios/dashboard_model_list_click.yaml:29-34`). A picker that renders 2 of 196 models passes. |
| **A test for the `/model` empty-catalog path** | **CONFIRMED, and the hole is precise** | `slash/commands/model.rs:35-37` returns `None` — no dropdown at all — when `ctx.models.is_empty()`. No test covers that path. Its sibling does: `slash/commands/effort.rs:302-317` `suggest_args_none_without_current_or_support` asserts `is_none()` on an empty `ModelState`. So the pattern was known and applied to one command and not the other, which is the same "two owners of one fact" shape as our two daemon classifiers. |
| **No content hash of the shipped binary** | **CONFIRMED** | SHA-256 appears only for *side assets*: `windows_payload.rs:65` (`.sha256` sidecar parse), `:127` (`sha256_hex_of_file` for the MinGit zip), and `xai-grok-bundle/src/lib.rs:285-300` (per-file bundle manifest). No digest of `grok` itself. |
| **No client/backend build-id comparison** | **CONFIRMED with a nuance** | They do have a *commit stamp* (`xai-grok-pager-bin/build.rs`), but it does not reach the handshake: `LeaderServerMetadata.leader_binary_version` is set from `xai_grok_version::VERSION` (`xai-grok-shell/src/agent/app.rs:794-800`), and `CLIENT_LEADER_VERSION` is the same constant (`xai-grok-shell/src/leader/mod.rs:96-97`). `LEADER_VERSION` *is* `VERSION_WITH_COMMIT` (`leader/server.rs:8-11`), and the production call site passes `None` for `leader_version_override` (`xai-grok-shell/src/agent/app.rs:817-833`, the 12th argument; the parameter is `leader/server.rs:1517`), so in production the comparison falls back to the constant — which is stamped — while `client_version` is not. `x.ai/leader/version_mismatch` compares **strings only** (`leader/server.rs:1479-1501`), and is *disabled* when the stamp is absent (`:1483`). |
| **No provenance gate on the shipped catalog asset** | **CONFIRMED** | Their catalog is compiled-in (`xai-grok-models/src/lib.rs:12` `include_str!("../default_models.json")`), which is the opposite failure mode from ours: it cannot be substituted by a file, but it also cannot be refreshed without a rebuild. Neither side has a "the asset that shipped is the asset we verified" check. |
| **The version string is the identity of a build** | **CONFIRMED, and it is their main weakness** | `xai_grok_version::VERSION` is `option_env!("GROK_VERSION").unwrap_or(CARGO_PKG_VERSION)` (`xai-grok-version/src/lib.rs:11-14`), and `IS_DEV_BUILD` is "was GROK_VERSION injected". Two builds of the same tag with different code report the same version. Their `--version` adds a 12-char commit (`build.rs`), so `--version` is honest — but nothing on the wire is. |

Their strongest build-identity ideas, by contrast, are (a) the *single* `build.rs` in the
composition root with a documented reason — "so the lib crates don't recompile on every commit"
(`xai-grok-version/src/lib.rs:19-21`); (b) `rerun-if-changed` on `.git/HEAD`, `logs/HEAD` and the
current branch ref (`xai-grok-pager-bin/build.rs:12-24`), which is what makes a commit change
actually rebuild; (c) `SOURCE_REV`, a 40-char monorepo SHA at the repo root (`SOURCE_REV`,
`README.md:32-34`) that names the *published source* rather than the built artifact.

---

## 6.5 Their known testing problems (§E of the brief) — what is evidence and what is not

The brief asks for real problems with flaky PTY, timings, env leaks, CI time, and empty-screen
assertions. **None of it can be sourced from a tracker** (§0.3). Here is the split honestly.

### 6.5.1 Evidence from the tree (verifiable)

| Problem | Where it shows | What it tells us |
|---|---|---|
| **Orphaned PTY children on a killed runner** | `tests/orphan_reap.rs:1-20` says it happened: children "re-parent to init, and accumulate on CI hosts" when CI delivers SIGTERM then SIGKILL | this is the class *every* pty suite hits; the fix was kernel-level (`PR_SET_PDEATHSIG`), not a timeout |
| **A residual leak they *document* instead of fixing** | same file: "pdeathsig covers only the direct PTY child … live tool subprocesses it spawned can still re-parent" | their accepted gap is ours too — and worse for us, because a session worker spawns tool subprocesses |
| **Timeouts that had to become configurable** | `xai-grok-test-support/src/lib.rs:30-39` `scaled()`, used for the 60 s headless budget (`headless.rs:23-25`) | under pool load, a fixed timeout is a false red; the knob is the acknowledgement |
| **Wall-clock-sized families** | 186 case files fanned into 8 roots, all `#[ignore]`d, capped at 4 libtest workers per family (`tests/pty_e2e_smoke.rs:3`) | the cost is real enough to need a scheduler knob; they chose Bazel to hold it |
| **A silent-downgrade class in the product that a test must catch** | `src/screen.rs:28-31`: without forwarded cursor-position replies, "`--minimal` silently downgrades to full-screen inline" | an unanswered terminal query changes behaviour with no error — the harness had to grow `respond_to_queries` (`src/scripted.rs:93-95`) to keep it observable |
| **Flaky input through a real race** | `src/flows.rs:28-44` `submit_turn` re-presses Enter because "a leader attach race can drop the first submit" | their flake answer is an idempotent retry with a written argument for why it cannot double-apply — not a longer sleep |
| **A scenario corpus that rots** | 45 YAMLs, 38 driven, 28 parse-checked, **5 referenced nowhere** (§2.5) | their own housekeeping gap; the mitigation (a hand-written parse list) is why it happened |
| **An empty-catalog path with no test** | `slash/commands/model.rs:35-37` returns `None`; its sibling `/effort` has the test (`effort.rs:302-317`) | the "empty is normal, so nothing notices" class, exactly as with us |

### 6.5.2 Reported externally, cause UNVERIFIED

The repository-upload incident is the only public test-adjacent failure I could source, and it is
*not* a test-suite defect — it is a product egress defect that a test suite **could have caught at
the payload-contract tier**. Reported by third parties (all UNVERIFIED as to mechanism):

- `thehackernews.com/2026/07/grok-build-uploads-entire-git.html` — "uploaded entire Git
  repositories … full commit history" to an xAI Cloud Storage bucket;
- `gist.github.com/cereblab/dc9a40bc26120f4540e4e09b75ffb547` — a wire capture claiming the upload is
  "independent of what the agent reads";
- `news.ycombinator.com/item?id=48877371`, `reddit.com/r/LocalLLaMA/comments/1ut7tis/…`;
- a follow-up on `penligent.ai` reporting "no `/v1/storage` uploads across six runs" for the same
  client version — which is equally consistent with a server-side change and with a client fix.

What I can verify in the tree **today**: the upload path has a bounded-queue oracle and a
"nothing accepted while unauthorized" oracle
(`tests/pty_e2e/storage_upload_parks_on_401_and_drains_after_recovery.rs:59-100`), and the mock
server *does* record `StorageUpload { path, size, body ≤ 256 KiB, authorization header }`
(`xai-grok-test-support/README.md`, `storage_endpoint` row) — so the machinery to assert "what was
uploaded is only what was asked for" exists and is unused for that purpose. That is the same defect
shape as ours: **the transport is tested, the payload contract is not.**

### 6.5.3 What I could not establish at all

- Whether their pty suite is flaky in practice, and at what rate. No CI logs, no issue numbers.
- Whether the 4-worker cap is sufficient on their runners, or what a full `--ignored` run costs.
- Whether the `--minimal` downgrade has ever shipped because a probe went unanswered in production.
Every statement of the form "they struggle with X" would be invented; I have not written any.

## 7. Build & release architecture (Grok) vs ours

This section answers the four questions the parent asked, in order, and then the one that matters:
what is portable.

### 7.1 Two build systems, one artifact contract

`README.md:110-113`: "The root `Cargo.toml` … is **generated** — treat it as read-only. Prefer
editing per-crate `Cargo.toml` files." Cargo is the public/local path; Bazel is the internal one.
The contract between them is stated as a security requirement in `.cargo/config.toml:40-58`:

```
# IMPORTANT: If changing build system (e.g. from cargo to Bazel-only),
# ensure equivalent linker hardening is applied to production binaries.
```

with `-Wl,-z,relro,-z,now,-z,noexecstack` for musl targets, `+crt-static` for msvc, and
`force-unwind-tables=yes` everywhere. **Verdict: not portable** — we have one build system, and
that sentence exists because they have two.

### 7.2 Profiles: the shipping artifact keeps symbols so CI can extract debug sidecars

`Cargo.toml:402-462`:

| Profile | Settings | Why |
|---|---|---|
| `release` | `incremental = true`, `panic = "abort"` | dev/default release |
| `release-dist` | inherits release; `lto = "thin"`, `codegen-units = 1`, `strip = false`, `debug = 1`, `split-debuginfo = "off"` | **the shipping profile**: "debug=1 emits line-tables DWARF; strip=false keeps symbols so CI can extract .debug sidecars (Linux) or .dSYM bundles (macOS) before stripping post-build" |
| `release-dist-jemalloc` | alias of `release-dist` | "alpha and stable — one profile, so stable can pointer-swap without a rebuild" |
| `x-prod` | `panic = "unwind"` | server-side latency services keep unwinding |
| `admin-cli-dist` | `opt-level = "z"`, `lto = true`, `strip = true` | size-optimized admin tool |
| `dev` | `panic = "abort"`, `split-debuginfo = "unpacked"`, `codegen-units = 128`, `debug = "line-tables-only"`, deps `debug = false` | build speed |

**Ours** (`Cargo.toml:245-300`): `dev` line-tables-only + deps without debug; `debugging` for real
debugger sessions; `release` thin LTO / CGU 4; `dist` inherits release with `strip = "symbols"`,
`debug = false`, `lto = "fat"`, CGU 1, and `panic = "unwind"` with a stated reason (a panicking
protocol-reader task must settle, not abort the worker). We already made the `panic` decision
deliberately — good. We strip symbols in `dist` and do **not** emit a sidecar; that is a choice,
not a defect, but it means a user crash report cannot be symbolized. Worth one line in a design
note, not a change.

### 7.3 Build ID: one `build.rs`, in the composition root

`crates/codegen/xai-grok-pager-bin/build.rs` (the entire file, 37 lines) is the whole mechanism:

```rust
println!("cargo:rerun-if-env-changed=GROK_VERSION");
// watch .git/HEAD, .git/logs/HEAD and the current branch ref
let commit = git_stdout(&["rev-parse", "HEAD"]).map(|s| s.chars().take(12).collect::<String>())
    .filter(|s| s.len() == 12).unwrap_or_else(|| "unknown".to_string());
let version = std::env::var("GROK_VERSION").or_else(|_| std::env::var("CARGO_PKG_VERSION"))...;
println!("cargo:rustc-env=VERSION_WITH_COMMIT={version} ({commit})");
```

and the runtime half is a `OnceLock` in the **library** crate
(`xai-grok-version/src/lib.rs:19-31`):

```rust
static FULL_VERSION: OnceLock<&'static str> = OnceLock::new();
pub fn set_full_version(v: &'static str) { let _ = FULL_VERSION.set(v); }
pub fn full_version() -> &'static str { FULL_VERSION.get().copied().unwrap_or(VERSION) }
```

`main()` injects it once: `xai_grok_version::set_full_version(env!("VERSION_WITH_COMMIT"))`
(`xai-grok-pager-bin/src/main.rs:2037`). The docstring says why the lock exists: "Only the release
binary stamps the commit in its own `build.rs` and injects it here at startup, so the lib crates
don't recompile on every commit."

Consumers of the stamp, all verified:

| Consumer | Site |
|---|---|
| `--version` text | `main.rs:1991-1999` (`version_text`) |
| `--version --json` | `main.rs:2213-2219` (`currentVersion`) |
| crash handler report | `main.rs:2100` (`app_version`) |
| Sentry release | `main.rs:2085` |
| telemetry service version | `main.rs:249-258` |
| OTel `ExternalClientInfo` (service + client version) | `xai-grok-pager/src/app/mod.rs:1076-1080` |
| startup-failure context | `xai-grok-pager/src/app/mod.rs:630-638` |
| leader mismatch compare | `xai-grok-shell/src/leader/server.rs:8-11` (but see §7.4) |

**Ours:** no `build.rs` in any workspace crate (`git ls-files | grep 'build\.rs$'` → one hit,
`vendor/tree-sitter-go/bindings/rust/build.rs`). `ALLSPARK_FORK_BUILD` is a *release-pipeline*
env var read with `option_env!` in exactly three places — `crates/pa-cli/src/config.rs:61-63`
(`fork_build`, splash), `crates/pa-core/src/update/installer.rs:284` and `:762` (update refusal and
version record) — plus the release workflow that sets it
(`.github/workflows/release-fork.yml:107`). `--version` prints it
(`crates/pa-cli/src/lib.rs:236-241` → `allspark 0.10.2-beta.3 (fork build dev; upstream prime-agent 0.10.2-beta.3)`).
**It never reaches the daemon handshake.**

### 7.4 How the build id reaches the handshake — it does not, and here is the exact seam

Grok's leader registration carries `client_version`
(`xai-grok-shell/src/leader/protocol.rs:195-198`) and the pager fills it from
`PAGER_CLIENT_VERSION = xai_grok_version::VERSION`
(`xai-grok-pager/src/client_identity.rs:3`, used at `xai-grok-pager/src/acp/mod.rs:306-317` and
`:428`). The leader compares `client.capabilities.client_version` against
`leader_version_override.unwrap_or(LEADER_VERSION)` and emits
`x.ai/leader/version_mismatch` on inequality (`leader/server.rs:1623-1638`). Because the pager
calls `run_leader_server(..., None, ...)`, the override is `None` and `LEADER_VERSION` — the
commit-stamped constant — is used. **So one side of the compare is stamped and the other is not**:
the client sends `VERSION`, the leader compares against `VERSION_WITH_COMMIT`. On a release build
those differ *by construction*, which means the mismatch toast either fires for every client or is
masked by the `"unknown"` early return in dev. I can read both sides but cannot see a test that
pins the release-build combination, so the practical behaviour is **UNVERIFIED**. What is
verifiable: **no commit or build id crosses the wire in both directions.**

There is a second, weaker identity check: leader eviction. `should_evict`
(`leader/mod.rs:115-118`) evicts a leader only when its `leader_binary_version` is a *strictly
older, parseable* semver than the client's (`leader_is_older_than`, `:105-112`); an unparseable or
missing version keeps the leader. That is the right *shape* (directional, fail-open on unknown) and
it is the one place where a handshake field is actually acted on.

**Ours: the same hole, twice.**

1. `crates/pa-daemon/src/supervisor/clients.rs:125-129` publishes
   `build_id: concat!("pa-daemon-rs-", env!("CARGO_PKG_VERSION"))` — a *function of the version*,
   so "same version, different code" is invisible. Worse, the field is **serialized but never
   read**: the only consumers are `crates/pa-cli/src/daemon_discovery/mod.rs:363-370` (fills
   `ProbeResult.build_id`) and `:539` (copies it into `DaemonState`), and
   `crates/pa-cli/src/daemon_discovery/plan.rs:173` / `format.rs:197` set it to `None`. `doctor`
   can print it; nothing acts on it.
2. The **worker's** hello sets `runtime: None` (`crates/pa-daemon/src/worker/connection.rs:336`),
   while the supervisor's sets `Some(...)`. So the same struct, `DaemonRuntimeIdentity`
   (`crates/pa-types/src/daemon/outbound.rs:143-155`), means "known" for one side and "unknown" for
   the other, and no code path distinguishes them.
3. The attach gate is `probe_daemon` → `classify_reachable`
   (`crates/pa-cli/src/daemon_discovery/mod.rs:336-395`, `:416-428`): protocol version + `schemaId`
   + `appVersion`. `appVersion` is `configure_app_version(crate::config::version())`
   (`crates/pa-cli/src/lib.rs:148-151` → `crates/pa-daemon/src/protocol.rs:31-46`), i.e. the
   packaged manifest version — again a function of the version.
   `crates/pa-cli/src/interactive_mode/daemon.rs:31-45` does its own probe and checks `schemaId`
   only. Two classifiers, two answers, one live daemon — already recorded in
   `.research/TUI-BLACKBOX-TESTING-2026.md` §6.5, and I re-read both sites to confirm.

### 7.5 Is the downloaded artifact verified? Yes — by checksum *or* by smoke test, depending on the path

Three different answers, which is itself the finding:

| Path | Integrity check | Cite |
|---|---|---|
| **`install.sh` (the public curl pipe)** | **none** — no sha256 anywhere in 518 lines | `crates/codegen/xai-grok-pager/scripts/install.sh` (grep for `sha256|checksum|shasum` → 0 hits) |
| **`grok update` (in-product, "internal" installer)** | **a smoke test, explicitly not a checksum**: "downloads are verified by a smoke test, not a checksum, so redirecting to an arbitrary base could serve a hijacked install" — which is why `GROK_CLI_BASE_URL` is restricted to loopback | `xai-grok-update/src/version.rs:28`, `:37-47` (parsed loopback check, not a prefix match); smoke at `auto_update.rs:1501-1604` (`SMOKE_TEST_TIMEOUT` 30 s, `SmokeTestFailure::{Timeout,Spawn,NonZero}`, `--version` run detached) |
| **Windows side payload (MinGit)** | **SHA-256 sidecar**, verified against the downloaded zip | `windows_payload.rs:65-75` (`parse_sha256_sidecar`), `:127-136` (`sha256_hex_of_file`), `:357-370` (fetch `.zip.sha256`, warn on unparsable/absent) |

Two mechanics worth naming because they are what make an install *not* brick a machine:

- **Atomic publish with rollback.** `atomic_symlink_swap` creates a versioned temp link and
  renames over the live one (per-racer temp name, `auto_update.rs:2030-2040`), with
  `LinkRollback` capturing prior state so a partially failed two-link swap (`grok` and `agent`)
  restores both (`:1895-2020`). The shell installer's equivalent is "run the new binary's
  `--version` before `mv -f`" (`install.sh:302-306`: "downloaded grok failed to run; keeping the
  existing install").
- **A blitz harness that proves the invariant.** `tests/test_install_sh.rs:1-20` states it: "After
  any install attempt, `$BIN_DIR/grok` resolves to a binary that runs, OR is still the previous-good
  binary, never a partial/garbage binary", and runs the **real shipped** `install.sh` against a fake
  `curl` with three modes — `full`, `truncate` (4 NUL bytes), `garbage` (right length, `exit 1`)
  (`:63-105`), plus `assert_active_grok_runs` (`:124-141`) and a seeded previous-good install
  (`:109-122`). The Rust side has the same idea with a controllable raw HTTP server that can
  truncate, hang mid-transfer, or serve right-length garbage (`tests/common/artifact_server.rs:17-40`,
  `:184-200`), including `set_slow` so concurrent installers genuinely overlap (`:102-110`).

**Ours:** `install-rust.sh` *does* verify a checksum — `SHA256SUMS` from the release prefix,
cross-checked against the channel manifest's row, then `sha256sum -c` (`install-rust.sh:1568-1604`),
and it probes the payload's `--version` before publishing (`:2743-2799`,
`:3604-3613`). The release workflow writes `SHA256SUMS` and carries the digest in
`dist/latest.json` (`.github/workflows/release-fork.yml:183-210`). So our *integrity* story is
stronger than Grok's public one on the artifact, and weaker on the **content contract**: the
workflow packages `--fixture` catalog assets
(`.github/workflows/release-fork.yml:117-129`, `generate --fixture`), and `verify` passes because
`--fixture` exempts the minimum gates — which is exactly the defect `.research/TUI-BLACKBOX-TESTING-2026.md`
§2.2 (Probe A2) reproduced, and the uncommitted `scripts/release/bundle_catalog.py` change already
turns red. Our `installer_update_e2e.rs` (743 lines), `installer_stop_gate.rs` (755) and
`installer_platform_map.rs` (1290) cover the install flow; none of them asserts that the packaged
catalog is real (i.e. that the user's providers have models).

### 7.6 Install-and-smoke: yes, but the *load test* is a separate thing

Their smoke test after every update is `--version` with a 30 s timeout and a detailed failure
(`auto_update.rs:1557-1604`). That proves "executes"; it does not prove "serves a session". The
`install.sh` equivalent is the same (`install.sh:302-306`). Nothing I could read asserts that a
freshly installed binary *serves a model catalog* — which is why §6's "no row-count oracle" and
"no asset provenance gate" combine into the same class of defect we hit.

### 7.7 What is portable, and what is not

| Grok mechanism | Portable to us? | Why |
|---|---|---|
| One `build.rs` in the composition root, `rerun-if-changed` on `.git/HEAD`, commit → `rustc-env`, injected at startup through a `OnceLock` | **Yes, almost verbatim** | We have no `build.rs` at all; `crates/pa-cli` is our composition root; the `OnceLock` trick is exactly what keeps `pa-types`/`pa-daemon` from recompiling per commit |
| Build identity **on the wire** (both directions) | **Yes, and better than theirs** | They compare two different constants; a stamped-vs-stamped compare is the fix, and we already have `DaemonRuntimeIdentity` to carry it |
| Directional eviction from a *parseable* version, fail-open on unknown | **Yes** | `leader_is_older_than` is 8 lines; our `classify_reachable` is stricter and therefore brittle |
| Fail the test when the double was never exercised (`assert_no_script_violations` on Drop) | **Yes** | Pure test-side; kills the "vacuous pass" class we keep hitting |
| Turn-level ordered expectations over two wire dialects | **Yes** | We have providers per dialect already; the *handle* shape is what we lack |
| Bounded teardown + PDEATHSIG in the harness | **Yes** | We arm it in one test and set `setsid` in `pa-perf`; it belongs in the shared harness |
| Consumer-named env deny lists | **Yes** | We re-derive the sandbox per test file today |
| Four-format artifact capture + raw bytes on failure | **Yes** | We have `write_cast` in `pa-perf` and nothing in the pty e2e files |
| `CSI ?2026` frame timing + 15% p99 baseline | **Yes** (we emit the markers) | `crates/pa-tui/src/app.rs:279,308` |
| 44-action DSL | **No, not as a first step** | Without a non-empty oracle it reproduces the incident with more ceremony; and each action is code we must own |
| Bazel + `#[path]` family roots | **No** | One build system; the cap buys nothing locally |
| `ptyctl` as an HTTP service | **No** | It is a product surface for agents; we would import a server to gain a controller |
| Generated root `Cargo.toml` | **No** | Our hand-maintained workspace is the source of truth |
| `env::set_var` under `unsafe` | **No** | Pass an env map into spawn instead (we already do) |

---

## 8. Practice-by-practice comparison

Legend for "ours": **have** = a real equivalent exists and is used; **partial** = the primitive
exists but is not wired to the place that would catch the defect; **none**.

| # | Practice (Grok) | Cite | Ours | Where ours stands | Price to close | Verdict |
|---|---|---|---|---|---|---|
| 1 | Declarative YAML scenario file | `src/scripted.rs:33-50` | none | No data-driven TUI test anywhere | 2-3 d for a runner over the existing pty harness | **Take, later** (after items in §9.1-9.3) |
| 2 | Parse-gate every scenario at `cargo test` time | `tests/scripted_scenarios.rs:482-514` | none | n/a | ~20 lines + a test that globs the dir | **Take with the runner** |
| 3 | Step-level assert vocabulary | `:204-368` | partial | `wait_screen_contains` only (`crates/pa-perf/src/bench/pty.rs:214`) | the 6 asserts we need: contains, not-contains, stable, gone, count-where, request-contains | **Take the 6, not the 44** |
| 4 | Four artifacts + `raw_output.bin` on failure | `:1031-1063`, `:640` | partial | we have no cast writer and no failure artifact at all (`rg write_cast` over `crates/` → 0 hits; `pa-perf` keeps bytes in memory only) | ~1 d | **Take** |
| 5 | `BugFinding` classification (assert vs died vs turn-never-landed) | `:1246-1258`, `:567-593` | none | A red pty test is one string | ~0.5 d | **Take** |
| 6 | Ordered turn expectations that must all be satisfied | `src/content.rs:194-267`, `:617-633` | partial | `PRIME_AGENT_FAUX_SCRIPT` drives turns but does not require each to be consumed | ~1 d | **Take** |
| 7 | Double fails the test when never exercised | `mock_server.rs:1169`, `:482` | none | A scripted test can pass with an unused script | ~1 d | **Take** |
| 8 | Quiescence wait (`StableMs`) instead of `sleep` | `ptyctl/src/wait.rs:20-30`, `:1-5` | none | 11 files hand-roll their own wait loops; sleeps are common | ~0.5 d (it is a predicate over two screen snapshots) | **Take** |
| 9 | Absence oracle (`Gone`) | same | none | "label disappeared" is hand-written where needed | ~0 | **Take with 8** |
| 10 | Styled-run asserts (highlight, not-highlighted) | `src/scripted.rs:921-972` | none | no styled extraction in any test harness | needs an emulator with styles (`vt100` cannot) | **Defer** — see 12 |
| 11 | Terminal-protocol asserts (OSC 52, kitty APC) | `:1744-1792` | none | we assert on raw bytes ad hoc in kitty e2es | ~0.5 d as a helper over `raw_output` | **Take the OSC 52 decoder**; kitty counter already exists in spirit |
| 12 | `alacritty_terminal` screen model | `src/screen.rs:9`, `Cargo.toml:23-27` | partial | `vt100` 0.15 in `pa-perf` + `pa-tui` dev-dep | a heavy dep, unstable API | **Skip until a styled assert needs it** |
| 13 | Frame timing from `CSI ?2026` + 15% p99 baseline per platform | `src/timing.rs:13-96`, `src/results.rs:86` | partial | `pa-perf` measures 9 scenarios, 2x median, not in the gate | ~2 d for per-platform baselines + gate wiring | **Take (later)** — the markers already exist in our binary |
| 14 | Corpus tripwire (`curated_cells_all_have_a_test`) | `tests/scroll_matrix_curated.rs:101-120` | none | n/a | ~30 lines | **Take immediately** — cheapest anti-rot |
| 15 | PDEATHSIG + bounded teardown in the harness | `src/pty.rs:131`, `:238-255` | partial | one test arms it (`windows_runtime_triage_e2e.rs:177`) | ~0.5 d in a shared harness | **Take** |
| 16 | Hermetic env with consumer-named deny lists | `pty_spawn.rs:33,141`, `sandbox.rs:294-379` | partial | `pa-daemon`/`pa-cli` sandboxes exist; pty files re-derive | ~1 d (consolidate) | **Take** |
| 17 | One `build.rs` stamping the commit | `pager-bin/build.rs` | none | `ALLSPARK_FORK_BUILD` is pipeline-injected only | ~0.5 d | **Take** |
| 18 | Build identity **on the handshake** | *they do not have it* | none (field exists, unread) | `runtime.buildId == version`; worker sends `None` | ~1 d | **Take and go further** — this is our incident's second cause |
| 19 | Install invariant under corruption (blitz) | `test_install_sh.rs:1-20`, `artifact_server.rs:17-40` | partial | `installer_update_e2e.rs` exercises paths but not truncate/garbage bodies | ~1 d | **Take the three corruption modes** |
| 20 | Checksum of the shipped artifact | *they do not have it*; we do | have | `install-rust.sh:1597` | — | **Keep**; add the content contract (item 24) |
| 21 | `SOURCE_REV` naming the published source | `SOURCE_REV`, `README.md:32-34` | none | we tag, we do not record the tree | ~0 | **Take** (one line in the release job) |
| 22 | Freshness rule: `src/` change without README change is incomplete | `xai-grok-test-support/README.md:5` | none | `pa-perf`/`qa-tasks` READMEs drift | ~0 | **Take as policy** |
| 23 | Two independent classifiers for one fact | theirs is one (`classify_reachable`) | none | `probe_daemon` vs `interactive_mode/daemon.rs` disagree | ~2 h | **Fix before adding anything** |
| 24 | Non-empty / cardinality oracle on a REMOTE-data list at the PTY tier | *they do not have it* at this tier (they count in widget tests and over local lists — see §6) | none | our picker tests inject the catalog, so no test observes what the daemon served; `an_empty_catalog_opens_the_empty_panel` (`crates/pa-tui/src/model_picker/tests.rs:579`) documents empty as normal | ~1 d | **Take first** — see §9.1 |
| 24b | Empty-catalog *path* test (the command returns no dropdown) | *they have it for `/effort`, not for `/model`* (`effort.rs:302-317` vs `model.rs:35-37`) | none | our `/model` on an empty catalog renders the menu with `No matching models` (`model_picker/render.rs:98-104`) — a different, defensible choice that nobody pinned | ~0.5 d | **Take, as a pinned decision** |
| 25 | Empty-state *reason* distinction (filter-empty vs catalog-empty) | *they do not have it* | none | one frame for both (`model_picker/render.rs:98-104`) | ~0.5 d | **Take with 24** |
| 26 | One scaled-timeout knob (`GROK_TEST_TIMEOUT_SCALE`) | `xai-grok-test-support/src/lib.rs:30-39` | none | literal `Duration::from_secs` in every pty test (15 in one file) | ~0 | **Take** |
| 28 | Canary test: plant a secret, assert it is absent from the serialized export | `xai-grok-telemetry/src/otel_layer/redact.rs:422-456` (`allowlist_drops_nonallowlisted_content_keeps_safe_and_numeric`) | none | `pa-telemetry` has the policy (`properties.rs:3-23` rejects non-primitives) and the sanitiser (`catalog.rs`, send path `client.rs:396,457`), but no test plants a secret and greps the batch | ~0.5 d | **Take** |
| 29 | Pin the property allowlist so *widening* it fails | `redact.rs:458` `allowlist_contents_are_pinned` | none | our catalog is data (`crates/pa-telemetry/src/catalog.rs`) with 10 unit tests, none of which fails if a new content-bearing property is added | ~0.5 d | **Take** |
| 30 | Live egress through a real exporter into a mock collector, with `wait_for_silence` | `xai-grok-test-support/src/mock_otel_server.rs`, `otel_recorder.rs`; 15 tests in `xai-grok-telemetry/tests/` | partial | `MockSink` (`crates/pa-telemetry/src/sinks/mock.rs:20-72`) + one wire test (`crates/pa-telemetry/tests/analytics_wire.rs`); no real socket, no silence oracle | ~1.5 d | **Take (later)** — the sink-level canary (28) buys most of it |
| 31 | Bounded-queue + no-egress-while-unauthorized oracle on a side channel | `tests/pty_e2e/storage_upload_parks_on_401_and_drains_after_recovery.rs:59-100` | none | nothing tests our upload/telemetry queue under a refused gate | ~1 d | **Take if we ever ship an upload queue**; today we have none |
| 27 | `pdeathsig` armed in the harness, tested at its boundary | `src/pty.rs:131`, `tests/orphan_reap.rs:1-20` | partial | armed in one test only (`windows_runtime_triage_e2e.rs:177`) | ~0.5 d | **Take (inside item 3)** |

---

## 9. The plan for us, ordered by what it would have caught

Our incident had two independent causes, both real in the tree today: (a) the release shipped a
synthetic `models.bundled.json` that *replaces* the compiled catalog
(`crates/pa-models/src/chain.rs:183`), so the picker rendered 46 placeholder rows and none of the
user's models — every frame perfect; and (b) the TUI could attach to a daemon of the same version
but different code, because `appVersion`/`buildId` are functions of the version
(`crates/pa-daemon/src/supervisor/clients.rs:126`). Items 1-2 below are those two causes.

### 9.1. A pty test whose picker catalog comes from the packaged artifact, with a cardinality and a reason oracle

**What fails without it:** a release whose `models.bundled.json` is synthetic, or a refresh that
lands an empty catalog, renders a valid screen. Nothing in our tree observes this: the `/model`
e2e tests supply the catalog themselves
(`crates/pa-cli/tests/interactive_daemon_e2e.rs:1550-1620`, where
`InteractiveOptions.model_catalog` is injected at `:1602`), and `crates/pa-cli/tests/login_panel_pty_e2e.rs:462-505` gets its rows from
a mock supervisor. Neither ever reads `models.bundled.json`.

**Shape:** one pty test, launched with `PI_PACKAGE_DIR` pointing at an unpacked release layout,
that (i) opens `/model`, (ii) asserts the row count on screen is `> 0` and equals the number of
models the daemon served, (iii) asserts at least one row is a *real* model id from the shipped
asset (not `Fixture …`), and (iv) asserts the empty-state row is **absent** with an empty filter.
The count must come from the screen, not from the option struct, or the test is asserting on its
own fixture.

**Price:** ~1 day (a row counter over `vt100`'s screen text is ~30 lines; the packaged-layout
launch already exists in `crates/pa-cli/tests/packaged_layout_e2e.rs:169-190` (the `Sandbox`,
which stages a layout and scrubs ambient env)).
**Red-first proof:** point `PI_PACKAGE_DIR` at a layout whose `models.bundled.json` is
`{"models": []}` — the test must go red on (ii) before §8 item 24's non-empty message exists.
Second proof, and the cheaper of the two: the current `--fixture` asset (`.github/workflows/release-fork.yml:127`)
must go red on (iii) today, because every row it produces is synthetic.

**Why our tree cannot see this even in principle right now:** our picker tests take the catalog as
an argument (`crates/pa-cli/tests/interactive_daemon_e2e.rs:1602`, `:1748`, `:2837` — three sites, all
`model_catalog: catalog`), and `crates/pa-tui/src/model_picker/tests.rs:579` pins "empty catalog renders the
empty panel" as *correct*. That is a defensible TUI rule (matching TS) and a fatal one for
observability: the same frame is emitted for "the user filtered everything out" and "the daemon
served nothing". Grok has the identical rule (`views/picker.rs:1937-1943`, `"  No matches"`) and the
identical blind spot — their `/model` command does not even open the menu on an empty catalog
(`slash/commands/model.rs:35-37`), and that path has no test while `/effort`'s does
(`effort.rs:302-317`). So this is not a Grok practice to copy; it is a shared defect, and we should
copy the *diagnosis*, not the code.

### 9.2. Build identity in `daemon_hello`, compared by the client, plus one classifier

**What fails without it:** the window talks to a daemon built from different code under the same
version. Today: `runtime.buildId = "pa-daemon-rs-" + CARGO_PKG_VERSION`
(`crates/pa-daemon/src/supervisor/clients.rs:126`), the worker sends `runtime: None`
(`crates/pa-daemon/src/worker/connection.rs:336`), and no code reads the field
(`plan.rs:173`, `format.rs:197` set `None`; `daemon_discovery/mod.rs:363` only copies it).

**Shape, in three parts:**

1. A `build.rs` in `crates/pa-cli` (our composition root): short commit + `ALLSPARK_FORK_BUILD`,
   `rerun-if-changed` on `.git/HEAD` and the branch ref, `cargo:rustc-env=ALLSPARK_BUILD_ID=…`;
   injected at startup into a `OnceLock` exactly like `xai-grok-version/src/lib.rs:19-31` so
   the lib crates do not recompile per commit. Fall back to `"dev"` when git is absent, and keep
   the existing `ALLSPARK_FORK_BUILD` meaning (release counter) so `--version` does not change.
2. Carry it: `DaemonRuntimeIdentity.build_id` becomes the stamp; the **worker** hello fills
   `runtime: Some(...)` too; nothing else in the wire changes (the field is already there, so this
   is not a schema break — `DAEMON_SCHEMA_ID` is untouched, and an old peer simply sees a string
   it ignores).
3. Read it in exactly one place: `classify_reachable` (`daemon_discovery/mod.rs:416-428`) gains
   "same build id as me" as a tier distinct from "stale version", and
   `interactive_mode/daemon.rs:31-45`'s private probe is deleted in favour of it — so the two
   classifiers cannot disagree again.

**Price:** ~1 day. **Red-first proof:** two fake hellos identical except `runtime.buildId` → the
client must refuse (or warn-and-refuse, per the decision below) before attach; the second
classifier deleted is proven by the structural test "one probe, one verdict".

The one design decision to make explicitly, not by accident: **what does the client do on a build
mismatch**? Options, cheapest first — (a) attach and paint a warning row; (b) refuse and relaunch
the daemon if it owns it; (c) refuse and print the doctor remedy. Today's fail-open on unknown
(`leader_is_older_than` returns `false` for unparseable) argues for (a) when the field is absent
and (b) when it is present and different.

### 9.3. One shared pty harness in `pa-perf` (or a new `pa-pty` test-support module) with five primitives

**What fails without it:** 16 files, 10,949 lines, each re-implementing spawn/read/wait; 11 of them
take their own process-wide lock, so the suites do not compose and the gate cannot parallelize
them safely. The primitives that pay for themselves immediately:

| Primitive | Why it is not a helper but a contract |
|---|---|
| `wait_until(desc, timeout, pred)` that reports screen + process-tree + child-lived on timeout | a timeout that says "the child exited at 0.4 s" is a different bug from "it never painted" (Grok's `WaitDiagnostics.ended`, `ptyctl/src/wait.rs:54`) |
| `wait_stable(hold)` | replaces sleeps; a `hold` window cannot mask a hang |
| `screen_rows(pred) -> usize` | the cardinality primitive item 1 needs |
| `raw_output` + `write_cast` | failure artifacts without a rerun |
| pdeathsig + bounded teardown | no orphans on a SIGKILLed runner (`src/pty.rs:238-255`) |

**Price:** 2 days (it is a consolidation, not new machinery; `pa-perf/src/bench/pty.rs` is the
starting point — its `PtyChild` already polls, decodes with `vt100` and answers the kitty probe).
**Test:** move two existing pty e2e files onto it with no assertion changes; both stay green, and
a deliberately hung child produces a timeout message naming the child's exit.

### 9.4. Artifact capture + failure classification on every pty e2e failure

**What fails without it:** a red `*_pty_e2e.rs` is one string; reproducing needs the machine.
Grok writes `.txt/.html/.svg/.json` + `raw_output.bin` and a `bugs.md`
(`src/scripted.rs:1031-1063`, `:1260-1288`). We have no cast writer and no failure capture at all (`rg write_cast crates/` → 0 hits), so
a red pty run leaves nothing behind.

**Shape:** a `Drop` guard on the harness that, on a panicking thread, writes
`target/tmp/pty-failures/<test>-<pid>/screen.txt` + `raw.bin` + `report.json` and prints the path.
**Price:** 1 day. **Test:** deliberately fail one assertion and assert the directory exists with
three files.

### 9.5. A script-consumption guard on our scripted-provider doubles

**What fails without it:** a test that scripts two turns and asserts on one passes forever; the
`.research` corpus already records this class (`docs/agent-errors.md` §1, "a test that passes on
the broken code", three occurrences in one session). Grok's guard is mechanical
(`mock_server.rs:1169-1180`: on `Drop`, unless panicking, assert every script ran).
**Price:** ~1 day in `crates/pa-ai/src/testing/mock_server.rs` (which exists) plus the
`PRIME_AGENT_FAUX_SCRIPT` path. **Test:** a scripted run with an unused turn must fail, and the
failure must name the unused turn.

### 9.6. Release-artifact content contract: the packaged catalog must be real, and the archive must be unpacked and exercised

**What fails without it:** `--fixture` ships synthetic models
(`.github/workflows/release-fork.yml:117-129`), `verify` exempts fixtures, and no test reads the
packaged asset. The uncommitted `bundle_catalog.py` gate already turns this red — which means the
release job is red *today* until one of the three fixes in
`.research/TUI-BLACKBOX-TESTING-2026.md` §2.2 is chosen; that decision is the parent session's.

**Shape:** (a) keep the fork-provider gate for the *fixture* too (it is a packaging gate, not a
network gate), or grow the fixture with real fork rows; (b) an `installer_*` test that unpacks the
built archive and asserts `libexec/models.bundled.json` parses and has ≥ 1 row per
`FORK_PROVIDERS` — the same shape as the workflow's existing
`test -f unpack/…/models.bundled.json` step (`.github/workflows/release-fork.yml:222-230`), moved
into `cargo test` so it cannot be skipped by a YAML edit; (c) `SOURCE_REV`-style provenance
(Grok's `SOURCE_REV`, `README.md:32-34`): one line in the release job writing the commit the tag
was built from into the archive.
**Price:** ~1 day. **Test:** the unpacked-fixture case is red on the current `--fixture` output —
already measured.

### 9.7. A corpus tripwire, and a parse gate for anything declarative we add

**What fails without it:** a scenario/task list that quietly stops covering what it claims. Grok
has both (`tests/scroll_matrix_curated.rs:101-120`; `tests/scripted_scenarios.rs:482-514`) and
still shipped five orphan YAMLs (§2.5) because the parse gate lists files by hand and the runner
list is separate. **Our version must derive both lists from the directory**, not from a literal.
Our `qa-tasks/` corpus (13 RT + 4 ST) has the same exposure: a task directory that no lane runs is
invisible.
**Price:** ~0.5 day. **Test:** add an unreferenced file to a temp dir and assert the tripwire fires.

### 9.8. Frame-time budget from the markers we already emit

**What fails without it:** a paint regression ships; `pa-perf` measures 9 scenarios against a 2x
median ceiling (`crates/pa-perf/src/report.rs:34`) and does not run in the gate. Our TUI emits
`ESC[?2026h/l` (`crates/pa-tui/src/app.rs:279,308`), so the frames are already on the wire.
**Shape:** parse begin/end in the harness's pump loop (Grok: `src/timing.rs:86-118`), record
`p50/p99/jank`, and gate **p99 at +15%** against a per-platform baseline checked into the repo
(Grok: `src/results.rs:86`, `benches/pty_baselines/README.md`). Wire it into
`scripts/fork/prepush-gate.sh`'s serial tail, not the parallel body.
**Price:** ~2 days (measurement exists; equality of baseline across machines is the work).
**Test:** invert one margin (a 20 ms sleep in a paint path) and require red.

### 9.9. A canary test and a pinned allowlist on our telemetry payload

**What fails without it:** a property carrying prompt or file content reaches an analytics sink and
nothing notices. Our guard is a type (`crates/pa-telemetry/src/properties.rs:3-23` rejects
non-primitives) plus a key-dropping sanitiser (`catalog::sanitize`, send path
`crates/pa-telemetry/src/client.rs:396,457`). Both are good, and neither observes the serialized
bytes: a *catalogued* property whose value is content passes both.

**Shape, in two tests, both cheap:**

1. A canary test over `MockSink` (`crates/pa-telemetry/src/sinks/mock.rs:20-72`): build a batch
   whose event carries a planted string in every property, serialize it the way
   `crates/pa-telemetry/tests/analytics_wire.rs` does, and assert the string is absent. Grok's
   equivalent is `redact.rs:422-456` (`allowlist_drops_nonallowlisted_content_keeps_safe_and_numeric`),
   which plants `echo CANARY_SECRET` and asserts the blob does not contain `CANARY_SECRET`.
2. A pin test over `catalog()` (`crates/pa-telemetry/src/catalog.rs:1383`): assert the set of
   `(event, property, type)` triples equals a checked-in list, so *adding* a content-bearing property
   is a failing test. Grok's is `allowlist_contents_are_pinned` (`redact.rs:458`).

**Price:** ~1 day total. **Red-first proof:** add a catalogued `String` property named something
innocuous (`detail`) to one event and fill it with a file path — test 2 must go red on the pin, and
test 1 must go red only if that property is not on the allowlist, which is exactly the distinction
worth encoding.

### 9.10. Explicitly not doing

- The 44-action DSL, the 11 test roots, Bazel parity, and the `ptyctl` HTTP server (§7.7).
- Styled-run asserts before we actually need them; they would force `alacritty_terminal` into the
  test tree.
- Copying `#[ignore]` as the gating mechanism: our gate already runs pty suites; a *new*
  `#[ignore]` would hide the thing this plan exists to make visible.

---

## 10. Honest limits of this report

1. No tracker, no PRs, no CI config (§0). Every "they gate it in CI" statement is their comment,
   not a workflow I read.
2. Code search rate-limited mid-session; some counts come from the git tree listing
   (4,487 blobs) rather than from reading each file. Sizes are blob sizes.
3. I did not build or run their suite, so all costs in §9 are estimates calibrated against our own
   numbers (gate ≈ 275 units, 16 pty files / 10,949 lines, 43 headless test files).
4. The one place I could not resolve their behaviour is §7.4: whether a release client and a
   release leader actually emit the mismatch toast, because one side compares a stamped constant
   with an unstamped one. Reading both call sites is not the same as running it.
5. Our incident is taken from `.research/TUI-BLACKBOX-TESTING-2026.md` §2.2 and
   `.research/RELEASE-ARTIFACT-CONTRACT-TESTING.md` §0, which are this session's own reproductions;
   I re-read every code site they cite and agree with the ones I list, but the live reproductions
   are theirs.
6. **Three claims in this report's first draft were wrong and are corrected in place,** which is
   worth recording because each correction *narrowed* the gap rather than widening it:
   (a) "no row-count assertion" → count oracles exist at the widget tier and at the PTY tier over
   locally produced lists; what is missing is a PTY-tier count over *backend-served* rows (§6);
   (b) "no test for an empty list" → `views/picker.rs:1937` has one for the settings picker
   (`tests/settings_e2e.rs:1763`); what is missing is the `/model` command's empty-catalog path
   (§6);
   (c) DeepWiki's claim that the commit stamp reaches a wire message via `LogEntry.ver` is wrong —
   `unified_log::set_version` is called with `xai_grok_version::VERSION`, the unstamped constant
   (`xai-grok-shell/src/agent/init.rs:461`, `agent/app.rs:233,320,717`), so the commit reaches
   `--version`, telemetry *service* version and Sentry release, and no wire message. §7.4 stands.
   A method note: every DeepWiki answer that mattered was re-read in the raw file; the ~12
   `ask_wiki_question` calls produced three imprecisions, all in the direction of claiming a
   capability exists when it exists at a different tier.
