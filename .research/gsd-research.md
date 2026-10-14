# Pattern review for Prime Agent architecture hypotheses

Scope: read-only synthesis from `/tmp/prime-architecture-research/baseline.md`, `/tmp/prime-architecture-research/gsd-research.md`, `/root/prime-agent/AGENTS.md`, and selected Prime Agent source. I did not run tests, builds, or indexing. Source paths are evidence that the current mechanisms exist or that a gap is concrete. They are not a proposal to port code.

Important framing:

- The useful import is **patterns**, not code portability.
- Prime already has Rust core code and an existing Python factory runtime. “Rust tools” are valuable only when they provide bounded, reliable observation/action at a specific layer, not because Rust is new to Prime.
- The central “taste model” idea is **feedback learning of personal preferences**. I found no proof here of model fine-tuning; treat it as local preference capture + verification unless proven otherwise.
- “Advisor” means selective independent consultation, not another always-on orchestration layer.
- Keep the current Prime surface: `bash`, `edit`, `ipython`, and the existing RLM/skill/harness APIs. Recall should be exposed through REPL skills or harness entries, not new top-level model tools.

## Pattern 1 — Durable factory orchestration, not a new DAG engine

### Pattern

Persist the existing task graph/state-machine run as an event-sourced run ledger. Recover it after kernel restart by reconciling the ledger with supervisor-owned child sessions.

### Mechanism

For each factory run, persist:

- run id and spec id;
- canonical machine hash/version;
- state entries and transition events;
- child ids, child names, and spawn attempts;
- captured outputs and capture failures;
- pause/final state;
- proof/artifact handles, if added later.

On restart, rebuild the in-memory factory registry from this ledger. Then reconcile child state from the supervisor/child session layer. The recovered run must continue using the existing factory semantics for joins, guards, retries, max-parallel, budgets, and resident tasks.

### Concrete Prime problem

Prime already has a real factory DAG/state-machine runtime. The gap is restart safety.

Evidence:

- `prime-agent-runtime/src/rlm/factory.py:1-26` says factory entries are declarative machines, DAG form is sugar, states run through `rlm.spawn`, settle through `rlm.collect`, and children survive under the supervisor while run state does not.
- `prime-agent-runtime/src/rlm/factory.py:632-656` validates join transitions.
- `prime-agent-runtime/src/rlm/factory.py:704-716` compiles DAG dependencies into join transitions.
- `prime-agent-runtime/src/rlm/factory.py:1422-1424` says `FactoryRun` is kernel-memory only and does not survive restart.
- `prime-agent-runtime/src/rlm/factory.py:1483-1505` says `FactoryExecutor` owns run state in kernel memory; children are supervisor-owned.
- `prime-agent-runtime/src/rlm/factory.py:2322-2429` evaluates settles, guards, joins, and transition firing.
- `prime-agent-runtime/src/rlm/factory.py:2432-2476` admits ready work under budgets and `max_parallel`.
- `prime-agent-runtime/src/rlm/factory.py:2785-2865` settles children, retries failures, and captures outputs.
- `crates/pa-core/src/session_engine/factory_host.rs:1-10` says Rust is the host bridge; the factory itself lives in the Python kernel.
- `crates/pa-core/src/session_engine/factory_host.rs:236-287` preflights factory model selectors before children spawn.

So “add DAG orchestration” would duplicate existing machinery. The real pattern is **durable recovery around that machinery**.

### Expected benefit hypothesis

Long multi-agent work should survive kernel restarts and compactions without losing:

- which child belongs to which state;
- whether a join already fired;
- which outputs were captured;
- whether the run can be stopped/resumed by run id.

This should reduce orphaned children, duplicated spawns, lost fan-in, and manual reconstruction after failures.

### Failure modes

- A second scheduler appears beside the factory and disagrees with it.
- Recovery duplicates a child spawn after restart.
- A join fires twice or never fires because persisted join signatures are wrong.
- Stale child status is trusted without reconciliation.
- The TUI factory graph or prompt path blocks on heavy disk scans.
- Persisted proofs include sensitive content by default.

### Validation experiment

Create one factory with two parallel task states and one fan-in join. Start it. Let one child settle and one keep running. Restart only the kernel. Then verify:

- `rlm.factory.status` shows the same run id;
- no duplicate child was spawned;
- the pending child is still connected to its state;
- the join fires once after both children settle;
- `stop` and `resume` still operate on the original run;
- the factory TUI graph does not block first paint.

Stop if any duplicate spawn, missed join, unrecoverable run id, or blocking view path appears.

## Pattern 2 — Proof/artifact handles for completion evidence

### Pattern

Represent task completion as a small proof handle plus bounded summary, not as “the child said it is done” or a large transcript pasted into context.

### Mechanism

A factory state may declare expected proof outputs. A child produces a compact JSON block or file with:

- artifact paths or handles;
- verifier command/output summary;
- optional workspace snapshot handle;
- declared output values for downstream binding.

The factory records the proof handle in its run ledger and status/graph output. Full artifacts stay on disk. The model sees handles and snippets; it asks for details through existing REPL/skill surfaces when needed.

### Concrete Prime problem

Prime can capture child answers and declared outputs, but there is not yet a durable, restart-safe proof ledger tied to artifacts/worktrees.

Evidence:

- `prime-agent-runtime/src/rlm/factory.py:1016-1028` caps captured answers for binding; full child transcripts stay in child sessions.
- `prime-agent-runtime/src/rlm/factory.py:2874-2944` captures declared output ports and records capture failures.
- `prime-agent-runtime/src/rlm/factory.py:1636-1645` reports `needs_verify` when a child exit only produced a provisional answer.
- `crates/pa-core/src/workspace_snapshot/mod.rs:1-8` can stage a self-verifying git worktree snapshot.
- `crates/pa-core/src/workspace_snapshot/mod.rs:21-24` says snapshot code is foundation plumbing only; no user-facing transport yet.
- `crates/pa-core/src/workspace_snapshot/verify.rs:41-56` verifies staged snapshots by parsing the manifest and rehashing blobs.

### Expected benefit hypothesis

A parent/factory can decide whether a task is complete from durable evidence, not from a fragile answer preview. Downstream states can bind stable handles rather than relying on large context.

### Failure modes

- Proofs become free-form prose and are not machine-checkable.
- Proof capture reads large artifacts into prompt context.
- Snapshot blobs expose secrets. The source warns that ordinary-named secrets can be captured: `crates/pa-core/src/workspace_snapshot/mod.rs:13-14` and `176-179`.
- Worktree isolation is mistaken for sandboxing. It is not. Prime warns worker/kernel isolation is not a sandbox in `README.md:86-87`.

### Validation experiment

Add one factory state that must produce a proof JSON. A downstream state consumes the proof handle. Restart after proof capture. Verify:

- status still shows the proof handle;
- missing/malformed proof is a deterministic state error;
- downstream binding uses the handle/declared output;
- no large artifact body enters the prompt;
- secret-like files are excluded or explicitly warned.

Stop if proof data becomes transcript-only, if large artifacts are pasted into context, or if snapshot handling hides the secret risk.

## Pattern 3 — Selective context recall with artifact handles

### Pattern

Keep context lean by recalling the right memory/artifact on demand. Retrieval returns handles and short snippets, not full state dumps.

### Mechanism

Use the existing harness digest and compaction summaries as the first layer. Add an opt-in hybrid retrieval skill behind the REPL/harness surface:

- lexical harness ranking stays as the safe baseline;
- artifact handles and compacted file-operation records become searchable;
- optional semantic ranking can rerank candidates, but it must run off the render path;
- results include small snippets and handles.

No new top-level model tools are needed.

### Concrete Prime problem

Prime already has context compression and lexical harness relevance. The gap is selective recall over artifact/proof handles and compacted work history.

Evidence:

- `crates/pa-core/src/refinement/ranking.rs:104-136` computes IDF over query terms.
- `crates/pa-core/src/refinement/ranking.rs:139-174` scores harness entries by lowercase substring overlap.
- `crates/pa-core/src/refinement/ranking.rs:195-285` renders ranked harness digest entries.
- `crates/pa-core/src/session_engine/harness_digest.rs:36-58` derives query terms from active goal and recent messages.
- `crates/pa-core/src/session_engine/harness_digest.rs:79-107` merges global/local harness state and renders a fingerprinted digest.
- `crates/pa-core/src/session_engine/compaction_exec.rs:40-64` includes previous summary and recent-state-anchor.
- `crates/pa-core/src/session_engine/compaction_exec.rs:13-18` records read/modified files for compaction details.
- `AGENTS.md:172-176` pins the model-visible surface and RLM API; avoid inventing new top-level tools.

### Expected benefit hypothesis

After compaction or restart, agents should find the relevant artifact/memory with fewer prompt tokens and fewer wrong recalls. This is especially valuable once factory proofs and task artifacts exist.

### Failure modes

- Semantic retrieval returns plausible but wrong memories.
- Retrieval leaks sensitive artifact content.
- Indexing blocks first paint or prompt admission.
- A new recall tool violates the surface contract.
- The retriever fights the existing digest and creates two sources of truth.

### Validation experiment

Use a fixed held-out set of past tasks where the needed memory/artifact is known. Compare:

1. current harness digest lexical ranking;
2. hybrid retrieval over harness + artifact handles.

Measure only concrete outcomes:

- whether the needed item appears in bounded top-k;
- whether an irrelevant or sensitive item appears;
- latency on prompt/render paths;
- token size of returned context.

Stop if hybrid retrieval does not improve held-out recall, surfaces sensitive content, or blocks user-visible paths.

## Pattern 4 — Taste model as local verified preference learning

### Pattern

Learn the user’s personal taste from corrections. Treat it as local preference memory with evidence and held-out validation, not as model fine-tuning unless there is proof of actual training.

### Mechanism

Separate “taste” entries from generic task memory. A taste entry should include:

- the user correction or preference;
- the context where it applied;
- the behavior it should change;
- a scope rule: local project, current session, or global user preference;
- conflict notes if it overrides another preference;
- validation outcome after later tasks.

Taste should update only from explicit corrections or repeated clear signals. It should be easy to inspect, roll back, and keep local/private.

### Concrete Prime problem

Prime has continual harness refinement, but generic memory can mix task facts, temporary progress, project decisions, and personal style preferences. That risks either forgetting repeated user taste corrections or overgeneralizing one-off feedback.

Evidence:

- `crates/pa-core/src/refinement/mod.rs:46-82` defines harness entries and refinement events with evidence/outcome fields.
- `crates/pa-core/src/refinement/planner.rs:471-490` records applied changes, evidence, and expected outcome.
- `crates/pa-core/src/refinement/planner.rs:503-540` supports rollback from before snapshots.
- `crates/pa-core/src/session_engine/refine.rs:246-255` documents plan → re-read → apply → persist.
- `crates/pa-core/src/session_engine/refine.rs:337-359` captures baseline state before the model plan.
- `crates/pa-core/src/session_engine/refine.rs:395-407` applies updates under the harness state lock.
- `AGENTS.md:152-156` says telemetry properties must not carry prompt, session, or file content. Taste feedback must respect the same privacy posture.

### Expected benefit hypothesis

The agent should need fewer repeated corrections about style, structure, naming, review depth, and preferred decision criteria. It should adapt to the user without burying personal preferences in generic memory.

### Failure modes

- A single correction becomes a broad hidden rule.
- Taste entries reveal private project or personal content outside local scope.
- Preferences conflict and the agent silently picks one.
- The system claims “fine-tuning” or learned model weights without evidence.
- Held-out tasks show no improvement in style adherence.

### Validation experiment

Create a small local preference suite from explicit user corrections. Hold out some tasks. Compare baseline harness memory vs taste-typed memory on:

- adherence to the stated preference;
- avoidance of overgeneralization;
- user correction count;
- ability to explain which preference was used;
- local-only storage and no content-bearing telemetry.

Stop if preference entries cannot be inspected/rolled back, if they overfit one-off corrections, or if privacy boundaries are unclear.

## Pattern 5 — Selective independent advisor

### Pattern

Use an independent advisor only at decision points where a second view can change the outcome: architecture tradeoffs, risky merges, ambiguous product direction, or repeated failed attempts.

### Mechanism

Run an advisor as a bounded subagent/factory state with:

- a narrow question;
- explicit context packet or artifact handles;
- expected answer format: risks, missing evidence, alternatives, stop/go recommendation;
- no authority to mutate files unless explicitly delegated;
- a budget and stop criteria.

The advisor should be a consultation pattern, not an always-on committee.

### Concrete Prime problem

Prime can spawn subagents and run factories, but independent review is currently a behavior pattern, not a product-level decision aid. The risk is either underusing advisors on high-risk choices or overusing them everywhere.

Evidence:

- `AGENTS.md` instructions in this session already encourage delegation when useful, but that is agent behavior, not a durable decision mechanism.
- Existing family communication/subagent mechanisms are part of the RLM API pinned in `AGENTS.md:172-176`.
- Factory can already represent advisor states; see existing state-machine runtime evidence in Pattern 1.

### Expected benefit hypothesis

Selective advice should catch blind spots like “Prime already has factory DAG orchestration” before a proposal duplicates existing systems. It should improve decision quality on high-risk work without making routine tasks slower.

### Failure modes

- Advisor becomes consensus theater and adds cost/latency.
- Advisor receives too much context and leaks sensitive data.
- Advisor mutates state outside its mandate.
- Parent treats advisor output as proof instead of critique.
- The system blocks interactive paths waiting for advisory work.

### Validation experiment

Run advisor only on pre-labeled high-risk tasks. Compare with similar tasks without advisor:

- number of caught false premises;
- number of accepted advisor objections;
- added latency/cost;
- user correction count after final answer;
- whether the advisor stayed read-only.

Stop if advisor findings rarely change decisions or if latency/cost dominates.

## Pattern 6 — Cost-aware routing and circuit breaker around existing failover

### Pattern

Use recent provider health and model cost as routing signals, but do it around the existing retry/failover loop. Start in dry-run mode.

### Mechanism

Record primitive, content-free events:

- selected model/provider;
- candidate costs;
- retry/failover events;
- failure kind/status;
- verifier outcome when available;
- spend and token totals.

A dry-run router says what it would have chosen. Only after held-out validation should routing affect live choices.

### Concrete Prime problem

Prime already has per-turn provider retry and failover, but I found no cross-task circuit breaker that uses recent failure/cost history to avoid a degraded provider before a turn starts.

Evidence:

- `crates/pa-core/src/session_engine/provider_retry.rs:1-4` documents shared provider retry.
- `crates/pa-core/src/session_engine/provider_retry.rs:25-32` defines default retry policy.
- `crates/pa-core/src/session_engine/provider_retry.rs:138-157` classifies permanent failures.
- `crates/pa-core/src/session_engine/provider_failover.rs:1-5` documents provider failover across providers serving the same model.
- `crates/pa-core/src/session_engine/provider_failover.rs:35-49` defines default failover and a whole-episode retry cap.
- `crates/pa-core/src/session_engine/provider_failover.rs:80-86` explains candidate switching and primary restoration.
- `crates/pa-core/src/models/resolver.rs:506-542` finds failover candidates for the same model.
- `crates/pa-models/src/schema.rs:144-154` maps cost fields into `ModelCost`.
- `crates/pa-models/src/schema.rs:173-180` validates model cost bounds.

### Expected benefit hypothesis

The agent should avoid known-bad or overpriced routes before spending retry budget, while preserving current failover behavior and parity-sensitive request handling.

### Failure modes

- New router duplicates provider_failover and creates conflicting state.
- It routes away from high-quality models on hard tasks based only on cost.
- It stores prompt/session/file content in telemetry.
- It blocks startup, first paint, or prompt admission.
- It optimizes for cheap success on weak tests and hurts real tasks.

### Validation experiment

Run in dry-run mode on a fixed suite with verifiers. For each turn, log the current selected model and the hypothetical router choice. After outcomes are known, compare:

- verified success/failure;
- spend;
- retries/failovers;
- latency;
- cases where dry-run would have made a worse choice.

Stop if dry-run choices reduce verified success or if routing logic cannot consume existing failover events cleanly.

## Pattern 7 — Fast bounded observation/action interfaces

### Pattern

Use native, typed, bounded tools where they make an observation/action safer and more reliable than free-form shell/Python. This is layer-specific tool design, not “rewrite in Rust because Rust.”

### Mechanism

Add small interfaces that return bounded structured results for operations like:

- reading factory run/proof summaries;
- inspecting artifact handles;
- computing workspace diff summaries;
- checking verifier status;
- listing recent run failures.

They should complement `bash` and `ipython`, not replace them.

### Concrete Prime problem

Prime’s flexible shell/Python surface is powerful but easy to use in unbounded ways. Factories and proof ledgers need reliable, bounded, machine-readable observations. At the same time, Prime already has Rust crates and a pinned public surface, so the benefit must be shown per layer.

Evidence:

- `AGENTS.md:172-176` pins the model-visible tools and RLM API surface.
- `README.md:86-87` warns that Prime executes commands with user permissions and is not a sandbox.
- `crates/pa-core/src/workspace_snapshot/mod.rs:50-77` shows an existing bounded interface pattern with entry/file/byte/git timeout caps.
- `prime-agent-runtime/src/rlm/factory.py:1057-1060` caps one factory activity reply frame and fails loudly if it cannot shrink.

### Expected benefit hypothesis

Bounded structured tools should reduce parsing errors, runaway output, and stale observations in factory/retrieval workflows. They should make verification easier without changing the model’s public tool surface.

### Failure modes

- Tool sprawl: many narrow tools with overlapping authority.
- The tool is treated as a sandbox or policy boundary when it is not.
- It duplicates existing bash/ipython functionality without measurable reliability gain.
- It violates parity or surface contracts.

### Validation experiment

Pick one layer, such as factory proof inspection. Compare the current shell/Python path with a bounded structured interface on:

- output size;
- parse failures;
- latency;
- correctness against the source files;
- whether it avoids reading unnecessary content.

Stop if the structured interface does not reduce failures or if it expands the public model surface.

## Final prioritization

1. **Durable factory run ledger** — highest confidence. It addresses a concrete gap in an already-implemented orchestration system.
2. **Proof/artifact handles** — high value once factories are durable; improves completion evidence and restart-safe fan-in.
3. **Taste model as local verified preference learning** — conceptually important, but must be scoped and validated to avoid overgeneralizing corrections.
4. **Selective context recall** — useful if measured against held-out recall tasks and kept behind existing REPL/harness surfaces.
5. **Selective advisor** — useful as a decision pattern, not an always-on process.
6. **Cost-aware circuit breaker** — promising only as dry-run telemetry first because retry/failover already exists.
7. **Bounded native tools** — valuable only where they beat current bash/Python observation on reliability, size, or safety.

The main anti-pattern is adding broad new systems that duplicate Prime’s existing factory, refinement, retry/failover, or tool surfaces. The better route is to strengthen the current layers with durable state, proof handles, measured recall, and verified preference learning.
