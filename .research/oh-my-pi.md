# oh-my-pi patterns for Prime Agent

Research target: **oh-my-pi**, canonical repository established as `can1357/oh-my-pi`, not a silent substitution for “oh-me-pi”.
Pinned source commit: `{commit}` (`main`, 2026-10-12).
Canonical URLs:

- Repo: https://github.com/can1357/oh-my-pi
- Pinned tree: https://github.com/can1357/oh-my-pi/tree/{commit}
- Local read-only clone used for source inspection: `/tmp/prime-architecture-research/oh-my-pi-src`

This note is about **patterns**, not code portability. Source links are evidence that the patterns exist in a working agent. They are not a recommendation to copy code or rewrite Prime Agent in Rust.

## Baseline comparison with Prime Agent

Prime Agent already has a Rust product core: `pa-core` owns tools, skills, kernel lifecycle, session engine, settings, compaction, and harness refinement; `pa-ai` owns providers/model registry/streaming; `pa-daemon` owns worker supervision and RLM spawn ledger; `pa-tui` owns terminal UI. Prime also has the distinctive Python RLM orchestration layer: persistent Python state, programmatic tool calling, subagents, factory, continual harness refinement, heartbeats, goals, and agent-to-agent messaging.

So the useful comparison is not “move Prime to Rust”. Prime is already a Rust + Python system. The useful question is which **behavioral contracts** from oh-my-pi should Prime adopt or test:

1. Selective independent consultation (`advisor`).
2. Fast, bounded observation/action interfaces (`Rust tools` pattern).
3. Feedback learning of personal/project taste (`taste model`).

## Pattern 1 — Selective independent consultation

**Behavioral / architectural pattern**

A separate reviewer model observes the main agent at selected boundaries. It has its own model, context, tools, transcript, spend accounting, and advice channel. It does not own the primary task. It can only inject advisory notes with severity, and the primary can weigh or reject them.

**Mechanism in oh-my-pi**

- Invocation: enabled by `advisor.enabled`, `/advisor on`, or `--advisor`; status/config/dump via `/advisor ...`.
- Model: default uses `modelRoles.advisor`; if unset, resolves from slow-role priorities. Explicit invalid advisor model does not silently fall back. Advisors request medium effort unless configured otherwise.
- Cadence: `turn` or `agent-end`, plus interval. `syncBacklog` chooses whether the primary waits: off, bounded thresholds, or strict final review.
- Context: advisor receives primary transcript deltas, reasoning/tool intent, project context, memory instructions, and obfuscated tool args/results. It has an append-only private context with its own compaction/re-prime path.
- Tools: default grants `read`, `grep`, `glob`, and sometimes `recall`. `WATCHDOG.yml` may grant mutating tools such as `edit`, `write`, `bash`, or `eval`, but they run in an isolated advisor tool session and still obey normal approval/tool policies.
- Escalation: the `advise` tool emits `nit`, `concern`, or `blocker`. Nits usually queue; concerns/blockers may steer a live run; terminal-answer, plan-mode, cooldown, deliberate interrupt, and client constraints can preserve notes as visible cards instead of restarting work.
- Verification controls: the advisor prompt tells it to verify with tools. An emission guard dedupes notes, drops content-free noise, limits non-blockers per update, and allows true severity escalation. Unsafe advisor output can be quarantined. Advisor transcript is persisted separately and shown as read-only observability.

**Source evidence**

- README feature summary: https://github.com/can1357/oh-my-pi/blob/{commit}/README.md#L175-L181
- Settings summary: https://github.com/can1357/oh-my-pi/blob/{commit}/docs/settings.md#L460-L475
- Advisor design doc: https://github.com/can1357/oh-my-pi/blob/{commit}/docs/advisor-watchdog.md#L1-L5
- Invocation/model/cadence: https://github.com/can1357/oh-my-pi/blob/{commit}/docs/advisor-watchdog.md#L24-L113
- What advisor sees: https://github.com/can1357/oh-my-pi/blob/{commit}/docs/advisor-watchdog.md#L115-L132
- Tools/isolation/escalation: https://github.com/can1357/oh-my-pi/blob/{commit}/docs/advisor-watchdog.md#L134-L192
- Catch-up/failure handling: https://github.com/can1357/oh-my-pi/blob/{commit}/docs/advisor-watchdog.md#L198-L243
- Watchdog roster: https://github.com/can1357/oh-my-pi/blob/{commit}/docs/advisor-watchdog.md#L320-L374
- Runtime/source paths: `packages/coding-agent/src/advisor/runtime.ts`, `packages/coding-agent/src/advisor/advise-tool.ts`, `packages/coding-agent/src/advisor/emission-guard.ts`, `packages/coding-agent/src/session/session-advisors.ts`.

**Concrete Prime problem**

Prime has strong subagents and factory, but independent review is still an explicit delegation choice. The main agent often performs and reviews its own work in one context. That increases risk of missed constraints, premature convergence, and “looks done” answers when a second model would object.

**Expected benefit hypothesis**

A selective advisor lane should reduce obvious missed constraints and bad final answers without forcing every task into multi-agent planning. It is especially useful when:

- the task is long-running or high-risk;
- the main agent made edits or ran verification;
- the user cares about taste/style/safety;
- the agent is about to deliver a final answer.

**Failure modes**

- Cost and latency creep if the advisor reviews every tool loop.
- Advisor noise distracts the executor or causes churn.
- The advisor becomes a hidden authority instead of advice.
- Re-entrant loops: advisor note wakes primary, primary update wakes advisor, repeat.
- Tool grants too broad: “reviewer” becomes an unsafe second executor.
- False blockers from stale context.

**Validation experiment for Prime**

Implement a narrow experiment without changing the main architecture:

1. Add an opt-in “advisor mode” as an RLM child or daemon-side reviewer lane for final-boundary review only.
2. Default tools: read-only observation through transcript/session artifacts and repo search; no edits.
3. Advice schema: `nit | concern | blocker`, with a strict max of 1 blocker or 3 non-blockers per final review.
4. Deliver as explicit `<advisory severity="...">...</advisory>` steer or visible card. The primary must respond by accepting, rejecting with reason, or asking the user.
5. Benchmark on a fixed corpus of Prime tasks: compare baseline vs advisor mode on missed constraints, extra turns, wall time, model cost, user-visible rework, and final verifier pass rate.

A good first verifier is **red-first replay**: build 20 historical task prompts where Prime previously missed a requirement or almost merged a broken fix. The advisor passes only if it catches materially more issues than a no-advisor baseline while keeping median extra turns below a chosen cap.

## Pattern 2 — Fast, bounded observation/action interfaces

**Behavioral / architectural pattern**

Make common agent observations and actions available as structured, bounded interfaces with deterministic output, cancellation, limits, and backpressure. The key idea is not “Rust because Rust”. The key idea is that the model should not have to improvise shell pipelines, parse huge text dumps, or depend on host tools for routine operations.

**Mechanism in oh-my-pi**

Oh-my-pi exposes many hot-path operations through Rust-backed N-API bindings and embedded executables. The agent layer sees bounded APIs for search, glob, AST matching/editing, shell execution, workspace scans, diff/patch/VCS operations, syntax summaries, token counts, PTY, desktop/computer use, image handling, and text formatting.

Important details:

- Search/glob can cap results, respect gitignore, stream results, apply per-file budgets, and support cancellation/timeouts.
- AST operations use ast-grep/tree-sitter-style structured matching and replacement, including dry-run style results.
- Shell is a persistent embedded brush session with in-process builtins. The reliability goal is no fork/exec on hot paths and consistent behavior on Windows/macOS/Linux.
- Workspace scan returns tree entries and context-file candidates in one pass.
- Code summarization can run async off the JS thread.
- Native ports are expected only for demonstrated hot paths with data-oriented boundaries. The docs explicitly require JS/native behavior comparison and representative benchmarks.

**Source evidence**

- README native overview: https://github.com/can1357/oh-my-pi/blob/{commit}/README.md#L199-L202
- README Rust/native module map: https://github.com/can1357/oh-my-pi/blob/{commit}/README.md#L452-L496
- Native package description/exports: https://github.com/can1357/oh-my-pi/blob/{commit}/packages/natives/package.json
- Shell binding: https://github.com/can1357/oh-my-pi/blob/{commit}/packages/natives/native/index.d.ts#L459-L497
- AST search/edit binding: https://github.com/can1357/oh-my-pi/blob/{commit}/packages/natives/native/index.d.ts#L885-L988
- Glob/grep binding: https://github.com/can1357/oh-my-pi/blob/{commit}/packages/natives/native/index.d.ts#L1943-L2115
- Workspace scan: https://github.com/can1357/oh-my-pi/blob/{commit}/packages/natives/native/index.d.ts#L2437-L2479
- Code summary/diff/VCS contracts: https://github.com/can1357/oh-my-pi/blob/{commit}/packages/natives/native/index.d.ts#L3508-L3745
- Porting/benchmark discipline: https://github.com/can1357/oh-my-pi/blob/{commit}/docs/porting-to-natives.md#L5-L10 and https://github.com/can1357/oh-my-pi/blob/{commit}/docs/porting-to-natives.md#L93-L152
- Changelog evidence for bounded search/stream/backpressure: https://github.com/can1357/oh-my-pi/blob/{commit}/packages/natives/CHANGELOG.md#L299-L306, #L882-L898, #L948-L949

**Concrete Prime problem**

Prime’s model-facing surface is intentionally small: `bash`, `edit`, and `ipython`. This is powerful, but it means routine observations often happen through Python scripts or shell commands. That has three costs:

- The model designs one-off searches instead of calling a bounded search primitive.
- Large outputs can enter context through ad hoc truncation rather than a tool-owned budget.
- Cross-platform behavior and cancellation depend on host commands and wrapper code.

Prime already has Rust crates. The gap is not implementation language. The gap is **first-class bounded observation/action contracts** that the Python RLM can call directly and that the UI can render consistently.

**Expected benefit hypothesis**

Adding a small number of bounded native/programmatic primitives should improve speed and reliability more than adding broad shell power:

- `repo_search(pattern, path, glob, max_matches, max_per_file, context, timeout)`
- `repo_glob(pattern, path, max_results, gitignore, sort)`
- `ast_query(lang/path, pattern, limit, include_meta)`
- maybe `workspace_scan(max_depth, collect_context_files)`

Expected gains:

- less token waste from huge shell output;
- fewer missed files due to wrong grep flags;
- more deterministic fan-out inputs for subagents;
- better UI cards and telemetry for observation steps;
- clearer permission boundary between observation and mutation.

**Failure modes**

- Duplicating capabilities already available through MCP, GitNexus, codebase-memory, or Python helpers.
- Too many tools confuse the model and dilute the simple Prime contract.
- Bounded results hide important evidence if limits are too aggressive.
- Native APIs become “magic grep” with behavior users cannot reproduce locally.
- Benchmarks optimize micro-latency but do not improve task outcomes.

**Validation experiment for Prime**

Do not start with a large native tool suite. Start with one bounded observation primitive and compare against current practice.

Experiment:

1. Add or prototype a Python-callable `repo_search` skill backed by existing Rust or a controlled CLI path.
2. Keep it read-only and result-limited. Include total hit counts, truncation flags, files searched, and exact command/algorithm metadata.
3. Run a benchmark corpus of repository-navigation tasks:
   - find symbol definition;
   - find all call sites;
   - find config setting usage;
   - locate tests for a module;
   - fan out context packets for subagents.
4. Compare with baseline `bash rg`/Python file-walk on wall time, bytes returned to model, error rate, missing-target rate, and final task success.

Pass condition: not “Rust is faster”, but “bounded primitive reduces context bytes and mistakes while preserving or improving target recall”.

## Pattern 3 — Feedback learning of taste and project decisions

**Behavioral / architectural pattern**

Convert repeated user corrections and project preferences into concise, reviewable durable guidance. This is “taste” as learned preference: the agent should remember how this user/project wants tradeoffs made, without injecting a huge autobiography into every prompt.

**Mechanism in oh-my-pi**

Oh-my-pi has multiple memory/personalization paths rather than one monolithic memory:

- `retain` / `recall` / `reflect` / `memory_edit` style memory tools, depending on backend.
- Sharpshooter project decision memory: friction-gated project decision files for architecture/product/style, consolidated in the background.
- `PERSONALITY.md` for a user-level personality block override.
- `WATCHDOG.md` for advisor-only review priorities, separate from primary context.
- Text prediction has explicit accept/reject feedback, proving the product already treats user feedback as state, not just transcript.

**Source evidence**

- README memory summary: https://github.com/can1357/oh-my-pi/blob/{commit}/README.md#L219-L220 and tools list around https://github.com/can1357/oh-my-pi/blob/{commit}/README.md#L306-L313
- Memory backend docs: https://github.com/can1357/oh-my-pi/blob/{commit}/docs/memory.md
- Sharpshooter evidence: `packages/coding-agent/src/prompts/memories/sharpshooter-consolidate-system.md`, `packages/coding-agent/src/sharpshooter/consolidate.ts`, `packages/coding-agent/src/sharpshooter/extract.ts`
- Personality override: https://github.com/can1357/oh-my-pi/blob/{commit}/docs/system-prompt-customization.md#L189-L236
- Advisor-only taste/review guidance: https://github.com/can1357/oh-my-pi/blob/{commit}/docs/advisor-watchdog.md#L261-L318
- Text feedback protocol: https://github.com/can1357/oh-my-pi/blob/{commit}/docs/rpc.md#L272-L294 and `packages/coding-agent/src/predict/daemon.ts`

**Concrete Prime problem**

Prime already has Continual Harness refinement, memories, prompt notes, skills, and subagent specs. The missing product behavior is not storage. It is a sharper feedback loop:

- Which user corrections should become taste?
- Which should become project policy?
- Which should become advisor-only review criteria?
- Which should remain ephemeral?

Without that routing, the agent may either forget useful preferences or overfit a one-off correction into global behavior.

**Expected benefit hypothesis**

A taste-learning loop should reduce repeated user corrections and improve “this feels like my agent” behavior without bloating the base prompt. Prime’s existing Continual Harness is a strong substrate for this, because entries are scoped, inspectable, and editable.

**Failure modes**

- Over-learning from one interaction.
- Capturing private/secrets-adjacent data as “preference”.
- Globalizing a project-specific rule.
- Conflicting memories and prompt notes.
- Turning taste into rigid policy that blocks user intent.

**Validation experiment for Prime**

Add a local “taste candidate” path before automatic refinement:

1. Detect repeated correction patterns or explicit “remember this preference”.
2. Classify candidate scope: session, project, global, advisor-only, or discard.
3. Store as a lean Continual Harness memory/prompt note only after a threshold or explicit user consent.
4. Show a small diff: “learned preference: X; scope: project; can delete/update”.
5. Measure repeated-correction rate on future tasks.

Benchmark verifier:

- Use a synthetic preference suite: 30 prompts with hidden expected style/architecture preferences seeded from prior turns.
- Baseline Prime vs taste-loop Prime.
- Metrics: repeated correction count, preference recall precision, false application rate, prompt-token growth, and user override compliance.

## Three concrete imports for Prime

### Import 1: Advisor lane as a mode, not a generic subagent

- **Pattern imported:** selective independent consultation.
- **Prime mechanism:** daemon/RLM child reviewer with a fixed read-only tool grant and severity-limited advice schema.
- **Cost:** medium. Prime already has RLM children, session artifacts, messaging, and Rust daemon supervision; the new work is delivery semantics, UI/status, cost accounting, and guardrails.
- **Risk:** advisor noise, model spend, re-entrant wakeups, user confusion over authority.
- **Benchmark verifier:** historical missed-constraint replay + live A/B on final-answer quality. Track caught blockers, false blockers, added turns, added cost, and final verifier pass rate.

### Import 2: Bounded repo observation primitives

- **Pattern imported:** fast reliable observation/action interfaces independent of language.
- **Prime mechanism:** keep Python RLM orchestration, but give it a small Rust-backed or controlled programmatic `repo_search` / `repo_glob` / `ast_query` surface with strict budgets and metadata.
- **Cost:** low-to-medium if starting with one primitive; high if it turns into a large tool suite.
- **Risk:** duplicates MCP/codebase-memory, hides evidence under limits, expands model tool surface too much.
- **Benchmark verifier:** repo-navigation benchmark comparing current `bash rg`/Python search against bounded primitive on target recall, output bytes, wall time, and downstream task success.

### Import 3: Taste candidate routing

- **Pattern imported:** feedback learning of personal/project preferences.
- **Prime mechanism:** build on Continual Harness refinement. Add a routing step that distinguishes project decisions, global preferences, advisor-only review priorities, and ephemeral notes.
- **Cost:** low-to-medium. Storage exists; the main work is classification, UX, and validation.
- **Risk:** overfitting, scope leaks, stale/conflicting guidance, privacy mistakes.
- **Benchmark verifier:** preference-replay suite measuring repeated correction reduction and false application rate under user overrides.

## Unknowns and assumptions

- I treated `can1357/oh-my-pi` as canonical because web search returned it as the direct GitHub project for “oh-my-pi”, and the repo’s own README/homepage present it as `omp`. If the user intended a different “oh-me-pi”, this should be corrected before deeper comparison.
- I did not run external oh-my-pi code or install dependencies. Evidence comes from docs and source reads at the pinned commit.
- I did not evaluate oh-my-pi’s runtime claims independently. Where docs mention speed or reliability, I treat that as evidence of the pattern and propose Prime-specific verifiers before adoption.
- I intentionally omitted license/FFI/dependency-plumbing analysis per the user’s clarification that the output should focus on patterns, not portability.
- Prime’s exact future tool surface is a product decision. The recommendation is to test bounded primitives, not to replace `bash`, `edit`, `ipython`, MCP, or Python orchestration.
