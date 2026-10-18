# AGENTS.md

Development rules for Prime Agent (Rust) on PrimeIntellect-ai/prime-agent, branch `main`.
Every contributor (human or agent) must read this before working on this repo.

## Repository

- **Never set git identity for commits to this repo's history.** Agents must not set
  `user.name`/`user.email` (config or `-c`) or `GIT_AUTHOR_NAME`/`GIT_AUTHOR_EMAIL`/
  `GIT_COMMITTER_NAME`/`GIT_COMMITTER_EMAIL` for commits that land in this repository.
  Those commits carry the identity of the ambient git config. If such a commit fails for a
  missing identity, report it instead of adding config. Allowed: the `*_DATE` variables
  (the golden corpus pins them for reproducibility), throwaway temp repos created by
  tests, and the release automation's bot identity.
- **Never add `Co-authored-by` trailers** — not in commit messages, not in squash
  suggestions, not as attribution for work merged from branches. A PR's commits carry
  one identity: the ambient git config's. Fabricated or hand-written trailers (including
  noreply forms) misattribute commits to unrelated GitHub accounts.
- The repo is PrimeIntellect-ai/prime-agent; the Rust implementation lives on the `main` branch.
- PRs go to the org repo with base `main`:
  `gh pr create --repo PrimeIntellect-ai/prime-agent --base main`.
- CI runs on the org's billing: `.github/workflows/continuous.yml` on `main` pushes and
  `release.yml` on version tags.
- Parity ground truth is unchanged: a read-only checkout of the TS implementation (this
  repo at tag `v0.9.8`, the last TS release before the Rust port merged to `main`). Clone
  it to `~/prime-agent`, the conventional location the parity scripts and code fall back
  to; tooling that honors an override reads `PA_TS_REFERENCE`.

## Style and structure

- Workspace crates are prefixed `pa-`. The hard ownership rules: one owned area per crate,
  pa-types is the only shared crate, cycle-free dependency direction, minimal public APIs,
  no god-modules. The dependency direction is pinned in the Crates table below; each crate's
  README.md states its scope, non-goals, and public API surface.
- Prefer private modules with an explicitly exported public crate API. Internals are `pub(crate)`.
- Aim for files under 2,000 lines, tests included. The 2,000-2,100 range is a soft review
  signal, not a merge limit: when adding to an already-large file, consider splitting by
  responsibility rather than growing it further. Move related tests and docs with extracted code.
- Name modules for what they own, not `utils` or `common`. Follow existing structure: in
  `crates/pa-core/src/kernel/manager/`, `mod.rs` connects `execution.rs`, `requests.rs`, and
  `teardown.rs`; put a new kernel-manager concern beside them. In
  `crates/pa-core/src/cron/store/`, `mod.rs` connects `heartbeat.rs`, `jobs.rs`, and
  `session_artifacts.rs`; keep storage work there rather than growing the scheduler.
- Inline format args: always prefer `format!("{x}")` over positional.
- Collapse if statements per clippy::collapsible_if.
- Prefer method references over closures per clippy::redundant_closure_for_method_calls.
- Make `match` statements exhaustive; avoid wildcard arms.
- New traits need doc comments explaining their role and how implementations are expected to behave.
- No opaque positional `bool`/`Option` parameters (`foo(false)` is unreadable). Prefer enums,
  named methods, or newtypes. If you must pass an opaque literal by position, use an exact
  `/*param_name*/` comment matching the callee signature.
- Prefer native RPITIT trait methods with explicit `Send` bounds
  (`fn foo(&self) -> impl Future<Output = T> + Send;`) over `#[async_trait]` or
  `#[allow(async_fn_in_trait)]`. Implementations may use `async fn` when they satisfy the contract.
- No single-use helper methods. Do not create a helper referenced only once.
- Instrument async work at the definition (`#[tracing::instrument(...)]`), not with
  `.instrument(...)` at call sites. Check whether the callee is already instrumented first.

## Change hygiene

- One logical change per PR. Fixes ride with the change that introduced them when possible; a PR
  that mixes a feature with unrelated lint fixes or refactors hides both from review.
- If you change dependencies (`Cargo.toml`), regenerate/commit `Cargo.lock` in the same change.
  A new dependency is an architectural change: state in the PR why the existing crates cannot
  host the functionality, and `make deny` must stay clean for it.
- If a change starts forcing edits across many crate internals, stop and fix the boundary instead.

## Tests

- Prefer whole-object equality comparisons over field-by-field checks.
- Do not add tests for statically defined values.
- Do not add negative tests for logic that was removed.
- Bug fixes need a regression test that fails without the fix.
- Verifiers over self-assessment: tmux user-level tests, differential tests against the TS binary
  on PATH, golden corpora replayed against real captured data. No lane merges without its verifier
  passing, rerun by the reviewer where feasible.
- Tests must wait for observable readiness, not fixed sleeps or retry-to-green loops. If a
  test must be disabled, give a reason at the test. Use isolated ports and temporary paths, and
  restore shared state. Run focused tests when changing their behavior.

### The test matrix: pick the cheapest tier that can fail

A change belongs to exactly one tier. Climbing a tier costs CI minutes, so climb only when
the cheaper tier cannot observe the defect.

| Tier | What it proves | Where | Cost |
|---|---|---|---|
| Unit | one function's contract, in isolation | `#[cfg(test)]` beside the code | ms |
| Integration | two or more real components, no model | `crates/<crate>/tests/` | seconds |
| Provider wire | the exact request/response bytes a provider sees | `crates/pa-ai/src/providers/*/tests`, mock server | seconds |
| QA task (faux) | one behavioural invariant through the real binary, scripted model | `qa-tasks/`, `--lane faux` | ~30 s |
| QA task (mock-provider) | the same, with a provider that misbehaves on purpose | `qa-tasks/`, `--lane mock-provider` | ~60 s |
| QA task (real-model) | semantic behaviour: did the agent choose the right tool | `qa-tasks/`, `--lane real-model` | minutes |
| Daemon lane | supervisor/worker lifecycle, crash recovery | `qa-tasks/`, `--lane daemon` | minutes |

Rules that follow from the table:

- **A bug fix needs a regression test at the tier where the bug lived.** A wrong tool choice
  needs a `real-model` QA task; a broken cache breakpoint needs a provider-wire test; a
  fail-open default needs a unit test.
- **Prove the test can fail.** Temporarily invert the fix (or `if false &&`) and confirm the
  test goes red. A regression test that passes on the broken code documents nothing.
- **Assert state, not prose.** Oracles read files, counts, exit codes, and tool-call
  patterns. Text matching on the model's answer is a last resort and belongs in the
  advisory judge, never in the blocking gate.

### Semantic QA tasks (`ST-*`)

The `RT-*` tasks guard reliability: a 402 is retried a bounded number of times, a stalled
provider times out, compaction fires before overflow. None of them can observe *which tool
the agent chose*, which is how the fork shipped a session where `python_repl` was called 132
times and `lsp` 15 times on work that needed the symbol layer.

`ST-*` tasks close that gap. Each one is a small fixture repository plus a prompt whose
correct answer is only reachable through one specialized tool, and each asserts both sides:

```yaml
must:                      # the semantic core: the right tool was used
  - tool: lsp
    args_regex: "references|find_symbol"
must_not:                  # the anti-pattern the task exists to catch
  - tool: bash
    args_regex: "sed -i|perl -pi"
```

The fixture is the load-bearing part: it must contain a *trap* that the wrong tool cannot
see through (two methods with the same name and the same body, a call chain spread across
three files, a decision that needs a calibrated answer). Without the trap the task passes on
luck. `ST-001` is the reference shape.

Run them with the free-tier provider when no paid credentials are available:

```bash
python3 scripts/qa/run_suite.py --lane real-model --tasks ST-001 ST-002
```

### Prompt-input caps are part of the contract

Anything that reads a file into the prompt (context files, prompt-input files, skill bodies,
model-prompt layers) must cap its size and report the truncation. A prompt is model input and
cache payload at once, so an unbounded read is both a denial of service and a permanent cost.
`MAX_CONTEXT_FILE_BYTES` and `MAX_SKILLS_INVENTORY*` are the current ceilings; a new input path
must reuse `truncate_context_content` rather than reading whole.

## Lint discipline

- Zero-warning posture: the merge gates run fmt + clippy + tests at `-D warnings`, so a `warn`
  lint is a merge blocker. Keep the enabled set deliberate and selective; enable a new lint family
  only with every current site fixed in the same change.
- Explain lint exceptions next to the `#[allow(...)]` or workspace configuration; keep
  exceptions narrow. Advisory ignores in `deny.toml` need a reason and review date.

## Performance and the critical path

The UI critical path is paint-latency-critical. First paint, and every
frame the user waits on, must never block on network, disk sync, or
background bookkeeping. The launch benchmarks measure this surface
(cold/warm ready, first paint); a merge that regresses them undoes the
product's core claim.

Rules:

- Nothing user-visible awaits the network. Any HTTP/socket call on a
  render path (telemetry, catalog fetch, update check) runs on a
  background worker; the paint path hands the work off and returns
  immediately. Delivery semantics (timeout, retry, ordering) live with
  the worker, not the caller.
- Telemetry is fire-and-forget from user-facing code. `pa-telemetry`'s
  background worker owns delivery end to end; no startup, session
  create, tab open, or first-render path may hold a flush or await a
  sink.
- Fire-and-forget boundaries need a red-first test: a hanging sink
  (one that never replies) must not delay paint. The test fails if any
  await on the critical path reaches the worker's delivery.
- Tests that mock a sink or run offline cannot prove the above (the
  benchmark's mock hides the POST). The regression test with a hanging
  sink is the required verifier.

Example (the #3288 regression this rule exists to prevent): the
telemetry startup flush was awaited on the interactive launch path, so
the analytics POST — a real HTTPS round-trip, ~157 ms — ran before the
first frame. Every CI check passed because tests set DO_NOT_TRACK=1 and
the benchmark mocked the provider offline; only real-network launches
paid the cost. The fix moved the flush to the background worker and
added `the_startup_flush_never_blocks_the_first_frame` (a hanging sink
must not delay paint) as the permanent guard. Any change touching the
launch path must keep that test red-first.


## Merge gates

- `cargo fmt --all --check`, `cargo clippy --workspace --all-targets -- -D warnings`,
  `cargo test --workspace` must pass before every merge. Run `make check` — the local mirror of
  the same gates; CI runs on the org's billing (`.github/workflows/continuous.yml` on `main`
  pushes; `release.yml` on version tags). The PR-only codebase-health check reports growth
  past 2,000 lines as review guidance; it never blocks a merge.
- **Parity-diff evidence is a merge gate** (the port's definition, not optional polish): every PR
  that touches a user-visible surface must include a "parity-diff evidence" section in its
  description showing the TS-binary comparison for what it changed: (1) rendered output —
  frame-diff vs the TS binary;
  (2) interactive behavior — the same input handled identically (keys, mouse, timing); 
  (3) wire parity — byte-compare the TS daemon's traffic for protocol changes; (4) user-visible
  invariants — every user action produces the same visible reaction as TS (`/compact` shows
  started+completed; `/model` shows the selector; a refinement shows its decoration). A feature
  that "works" but was never diffed against the TS binary does not pass review. If TS shows it,
  Rust shows it identically; if Rust shows something TS does not, that is also a parity bug.
- PRs must state ownership compliance (crate README scope/non-goals/public API, dependency
  direction) and classify the change: internals, new `pub` surface, or a new
  dependency/cross-crate re-export. The latter two are architectural changes — an innocent-looking
  `pub use` is a very simple way to break encapsulation.
- The compiled fallback model catalog `crates/pa-ai/src/models_generated.rs` is
  hand-maintained Rust mirroring the TS `packages/ai/src/models.generated.ts` object
  literal — edit it by hand, exactly like its TS source (there is no generator). The
  parity fixture `crates/pa-models/tests/fixtures/catalog.v1.json` refreshes with
  `scripts/generate-catalog-fixture.py`. Bulk data files are exempt from the size guidance.

## Adoption telemetry

Every user-visible feature ships its adoption telemetry event in the same PR as the feature:
the event name + properties join the versioned schema (`pa-telemetry`), and a
seam emits it from day one. Telemetry properties never carry prompt, session, or file content
(primitives only; see `pa-telemetry`); delivery is fire-and-forget from user-facing code
(see Performance and the critical path).

## Branding

The product is Prime Agent - we are not a pi fork. Scrub "pi"/"pi-mono"/"pi-ai"/
"Prime Intellect"-style naming from all user-visible surfaces (docs, READMEs,
CLI help text, error messages, splash/onboarding strings, keybinding hints, TUI
labels, and code comments that quote user-facing strings); brand everything
Prime Agent. Audit with a repo-wide grep and classify every hit (user-visible
vs wire-internal vs comment) before scrubbing, and list the preserved wire
identifiers in the PR body so the reviewer can verify none were wrongly scrubbed.

EXPLICIT EXCEPTION: wire-protocol identifiers that must stay byte-compatible with the TS product (e.g. the PI_PACKAGE_DIR env var, settings keys, provider IDs like prime-inference, harness _meta namespaces like ai.primeintellect.prime-agent, lockfile names) stay until/unless the TS side renames them — PARITY BEATS BRANDING ON THE WIRE.

## Fork identity and isolation (allspark overlay)

The fork's user-facing name is **allspark**; the compiled binary deliberately
keeps the upstream name **prime-agent**. This split is the fork's identity
model — do not "fix" either half.

- **Payload name stays `prime-agent`** (`crates/pa-cli/Cargo.toml` `[[bin]]`).
  Upstream merges and TS-parity diffs stay clean when the tree keeps upstream
  names. Never rename the binary, the `pa-*` crates, or the `rlm.*` kernel API
  as part of branding.
- **The user-facing entry is the `allspark` launcher**
  (`scripts/allspark-launcher.sh`, the tracked source of truth; deployed as
  `bin/allspark` with the payload at `libexec/prime-agent`). A hand-copied
  launcher that drifts from the tracked script is a bug — edit the script and
  redeploy.
- **Isolation is structural, one knob: `HOME`.** The launcher points `HOME` at
  `~/.allspark-home` (`ALLSPARK_HOME` overrides) and pins the daemon socket
  with `PRIME_AGENT_DAEMON_SOCKET=${TMPDIR:-/tmp}/allspark-daemon.sock`. Every
  product path derives from `$HOME`, so a co-installed upstream `prime-agent`
  shares no sessions, auth, config, kernel venv, or socket. Keep upstream
  env-var names (`PRIME_AGENT_*`, `PI_*` wire names) — the collision is
  avoided by the isolated `HOME`, so renaming env vars buys nothing and breaks
  the launcher's socket pin plus parity diffs.
- **`--version` names the fork and the fork build**:
  `allspark <version> (fork build <id>; upstream prime-agent <version>)`. The
  build id is stamped at compile time via
  `ALLSPARK_FORK_BUILD=fork.NNN cargo build ...` and must match the release tag
  `allspark-v<version>-fork.NNN` (zero-padded counter: `fork.001`...); an unstamped build reports `dev`. Two allspark
  binaries are told apart by this id. The differential CLI test strips the
  branding before comparing against the upstream TypeScript binary; the words
  are the accepted divergence.
- **No seeding from the upstream install.** The fork starts standalone: its
  own providers catalog, settings and auth only. Nothing under `$HOME/.prime`
  is ever read or copied (upstream `models.json` providers must never leak
  into the fork's picker); `auth.json` in particular stays upstream-only
  (rotating refresh tokens invalidate each other) — run `allspark login`.
- The full in-binary rebrand (branch `feat/allspark-identity`, re-land attempt
  `feat/allspark-identity-reland`) is the **abandoned** alternative: it breaks
  the launcher's upstream-name socket pin and drags ~700 rename sites into
  every future upstream merge. Keep it unmerged.

Changes to the launcher, the payload name, or the isolation variables must say
so in the PR and re-verify the no-shared-state property: run the launcher
against a populated upstream `~/.prime` and confirm nothing upstream is
written and no upstream socket is contacted.



## Surface contract (must not change)

- Tools exposed to the model: `bash`, `edit`, `python_repl` (internal helpers: `rename`, `stdout`).
- RLM kernel API in the persistent Python REPL: `rlm.spawn/find_models/collect/list_subagents/delete_subagent/create_session/progress_note`, `rlm.harness` CRUD, `agent_message.send`, `agent_observe`, `compact`, `goal`, `refine`, `attach_image`, skills (markdown + Python) per the skill contract in the base system prompt.
- System prompt structure: layered — cache-stable static layer files (core harness description with the full API surface, mandatory usage rules, opinionated guidelines, per-model additions from the model-prompts TOML rule map) followed by one dynamic tail (packages, project context, skills inventory, MCP servers, environment, session role); the harness digest stays a separate `[harness-digest]` user message. `prime-agent prompt` dumps the assembled prompt with its layer breakdown.
- CLI shape: `prime-agent` with the same commands/flags as the TS product; headless modes (RPC/daemon/session-worker) with identical behavior.

## Crates

One owned area per crate; `pa-types` is the only shared vocabulary crate.
Dependencies flow from the composition root toward lower crates, never back upward.
The table summarizes each crate's current direct workspace dependencies:

| Crate | Purpose | Direct workspace dependencies |
| --- | --- | --- |
| `pa-types` | Shared wire and domain types | none |
| `pa-telemetry` | Events and sinks | none |
| `pa-agent` | Provider-independent agent loop | none |
| `pa-ai` | Providers, model registry, streaming | `pa-types` |
| `pa-models` | Live model catalog and transport | `pa-ai`, `pa-types` |
| `pa-core` | Session engine, tools, skills, kernel, settings | `pa-types`, `pa-ai`, `pa-models`, `pa-agent`, `pa-telemetry` |
| `pa-daemon` | Supervisor, workers, wire protocol | `pa-types`, `pa-core`, `pa-agent`, `pa-ai`, `pa-telemetry`, `pa-models` |
| `pa-tui` | Terminal UI, daemon-wire client | `pa-types` |
| `pa-cli` | `prime-agent` binary, composition root | `pa-types`, `pa-core`, `pa-ai`, `pa-agent`, `pa-daemon`, `pa-tui`, `pa-telemetry` |
| `pa-perf` | Performance corpora + pty bench harness (dev/CI tooling, never shipped) | `pa-types`, `pa-core`, `pa-daemon` |

Layering rules:

- Dependencies only point from higher crates to lower crates shown above. No cycles or
  reverse edges. The leaf crates are peers; add only the dependencies needed for their work.
  Shared wire/domain vocabulary belongs in `pa-types`, not in a higher crate or a duplicate.
- `pa-tui` renders from wire types and events; it does not link the session engine.
- `pa-cli` is a composition root: it wires crates together and contains no business logic.

## Reliability model

- The daemon is a supervisor: it spawns one worker process per active session instead of hosting sessions in-process. Workers are supervised, restarted with backoff, and sessions persist on disk (append-only JSONL, same layout as `~/.prime/agent/sessions`) so reattach works even if the supervisor restarts.
- No stubs, no `todo!()`, no swallowed errors (`anyhow` bubbling to UI is fine).

## References

- Cursor Bugbot's PR review rules (`.cursor/BUGBOT.md`) mirror the standards in this file - update both whenever either changes.
