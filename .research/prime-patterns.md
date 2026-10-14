# Prime Agent: high-effect patterns, not code ports

Scope: conceptual patterns for Prime Agent Rust at HEAD `6879027856c55158f5f1286c16c78f20d6bec452`. This is not a portability or dependency report. Source references are only evidence that a pattern exists elsewhere or already overlaps with Prime.

## Existing overlap to respect

Prime already has:

- persistent Python RLM/PTC, MCP, skills, subagents, direct agent messaging, daemon workers;
- continual harness memories/skills/subagents/factory entries and rollback;
- bounded autonomous mode with user-defined shell quality gates;
- Decision API / System One path via `decisionApi.systemOneModel`;
- provider retry/failover;
- factory DAG/state-machine orchestration: DAG sugar compiles to machine form; machines support guards, joins, cycles, bounded re-entry, foreach, budgets, retries, `max_parallel`, resident states, rate-limit backoff, stop/resume, graph/watch/status, and `/factory` live view.

The useful question is therefore not “what subsystem is missing?” but “which behavior patterns would raise verified coding outcomes when layered on top of these surfaces?”

---

## Pattern 1 — Durable orchestration state, ephemeral workers

**Pattern**

Treat the orchestrator’s plan/state as durable truth, and workers as replaceable execution resources that can be reconciled after crashes.

**Mechanism in Prime terms**

Persist factory run state as a bounded ledger/checkpoint under session artifacts:

- run id, spec id/hash, generation;
- state entries and instance statuses;
- child ids and state-instance mapping;
- captured outputs, bounded and typed;
- fired transitions, retry counters, backoff deadlines, budgets;
- resident children and stop/resume state;
- optional per-state artifact references: expected output files, gate evidence, patch/worktree snapshot id if such a user-facing snapshot exists.

On kernel restart, rehydrate the Python factory registry by reconciling the checkpoint with supervisor-owned children using `rlm.list_subagents` / `rlm.collect`.

**Concrete Prime problem**

Factory orchestration exists, but `prime-agent-runtime/src/rlm/factory.py` documents that run state lives only in kernel memory. Children are supervisor-owned and keep running, but the run registry does not survive kernel restart. Also, `factory.py` has no `worktree` concept, so a factory state cannot anchor “what changed” as a durable phase artifact.

**Expected benefit hypothesis**

Long multi-phase coding runs become recoverable after kernel restart, compaction, TUI detach, or daemon worker churn. This makes Prime’s existing factory closer to a reliable project driver rather than a live-only coordination loop.

**Failure modes**

- replay applies a child settlement twice;
- resume spawns duplicate children;
- checkpoint schema drifts from Python runtime state;
- captured outputs grow too large or leak content;
- a child was deleted/renamed, so reconciliation guesses wrong;
- users trust a recovered factory whose worktree has changed underneath it.

**Validation experiment**

Use synthetic factory specs with joins, retries, foreach, resident nodes, and backoff. Kill/restart the kernel mid-run. Assert:

- no duplicate child admissions;
- all already-settled children remain settled exactly once;
- pending nodes continue from the same guards and budgets;
- resident children remain stoppable;
- graph/watch/status match the pre-crash shape after reconciliation.

Then run one real coding factory on a fixed repo task and compare completion rate and recovery time against current live-only factory.

---

## Pattern 2 — Reversible cold context, not one-way summaries

**Pattern**

Move cold context out of the active prompt without destroying exact recall. Summary is a view; the original blocks remain addressable.

**Mechanism in Prime terms**

Add a session-scoped folded-context ledger:

- each folded block stores exact bounded text or artifact pointer;
- metadata records file paths, commands, tool names, model request ids, errors, subagent/factory ids, commit/base, and user goal;
- prompt only carries the compact summary plus recall handles;
- Python exposes search/unfold calls or a skill wrapper;
- start with lexical + recency + path overlap; only add vectors after a held-out baseline shows lexical fails.

**Concrete Prime problem**

Prime compaction and harness digest already keep long tasks moving, but compaction is lossy. Harness ranking currently uses lexical substring + IDF, so semantic or artifact-level recall is weak when the right clue is not literally in the query.

**Expected benefit hypothesis**

Long coding tasks lose fewer critical details after compaction: exact test output, previous failed patch, subagent finding, or command result can be recalled without bloating every prompt.

**Failure modes**

- retrieval returns stale facts from before a fix;
- model overuses recall instead of inspecting current files;
- exact blocks contain sensitive content;
- too many handles clutter the prompt;
- vector retrieval, if added too early, masks weak corpus design.

**Validation experiment**

Create paired long-task fixtures with forced compaction. Measure:

- verified task completion;
- number of times the agent re-runs or re-derives old evidence;
- recall precision@k for hidden “needed later” facts;
- token cost per verified task;
- user-visible latency of recall.

Red test: current compaction loses a needed exact fact. Green test: folded-context recall retrieves it and the task passes.

---

## Pattern 3 — Taste model from feedback, not generic memory

**Pattern**

Learn the user’s preferences as behavioral constraints from feedback and outcomes, not as a pile of broad memories.

**Mechanism in Prime terms**

Create a small “taste model” layer over continual harness state:

- store preference atoms only when backed by feedback or repeated correction;
- separate stable taste from temporary task facts;
- attach scope: global, repo, branch, task type;
- record evidence: user correction, accepted/rejected PR style, reverted agent choice, test/gate outcome;
- expose it to planning as constraints and tradeoffs, not as prose spam.

Examples of learned taste:

- “Do not propose WorkBuddy as separate provider when limits are shared.”
- “Prefer patterns and validation experiments over porting/dependency analysis for architecture borrowing.”
- “Do not call existing Prime factory orchestration absent; identify deltas over it.”

**Concrete Prime problem**

Prime has harness memories and refinement, but preference learning is not distinguished from factual memory. The agent can repeat a class of unwanted recommendations unless the memory is written very explicitly and retrieved by lexical match.

**Expected benefit hypothesis**

The agent makes fewer repeated taste violations, gives answers closer to the user’s decision style, and uses shorter prompts because preferences are structured and scoped.

**Failure modes**

- overfits one correction into a permanent rule;
- conflicts between global and repo-local preferences;
- stale taste survives after the user changes direction;
- agent treats taste as hard law when it should be a soft prior;
- hidden preference state makes behavior hard to inspect.

**Validation experiment**

Build a small suite of repeated-interaction prompts with known user corrections. Compare baseline harness memory vs structured taste atoms:

- repeated violation rate;
- false application rate in unrelated tasks;
- user edits needed per final answer;
- precision of retrieval under paraphrased prompts.

Require every taste atom to show evidence and offer rollback/update.

---

## Pattern 4 — Selective independent advisor

**Pattern**

Use a separate advisor only for decisions where independence changes the answer: architecture tradeoffs, risk review, plan critique, or “what are we missing?” Not every task needs another agent.

**Mechanism in Prime terms**

Add an advisor workflow on top of existing subagents/factory:

- parent frames a decision, known constraints, and current hypothesis;
- advisor must return disagreement, missing evidence, or confidence-lowering risks;
- advisor does not implement unless explicitly asked;
- output is short and structured: recommendation, strongest counterargument, evidence gaps, validation experiment;
- call only at gates: before major design choice, before large refactor, before adopting a new pattern, after repeated failure.

**Concrete Prime problem**

Prime has subagents and factory, but delegation can become parallel research rather than independent judgment. The parent may still synthesize toward its first plan. For architecture work, the missing value is adversarial/independent critique, not more context volume.

**Expected benefit hypothesis**

Fewer wrong architectural recommendations pass through because an independent model catches overlap, missing evidence, or bad assumptions before the final answer.

**Failure modes**

- advisor becomes a rubber stamp;
- advisor lacks source context and invents risks;
- too many consultations slow work;
- parent ignores dissent or merges it vaguely;
- conflicting advisors create indecision.

**Validation experiment**

Use historical architecture prompts with known corrections. Run with and without advisor gate. Measure:

- number of user corrections needed;
- overlap mistakes caught before final;
- decision latency;
- final recommendation stability;
- percentage of advisor objections that change the final answer usefully.

A good advisor should be called rarely and should often disagree.

---

## Pattern 5 — Fast bounded Rust observation/action tools

**Pattern**

Give agents reliable, bounded, typed observation/action interfaces. The point is not Rust for its own sake; it is predictable behavior, limits, and testability around critical actions.

**Mechanism in Prime terms**

Prefer Rust-owned tools for surfaces where correctness, bounds, or UI latency matter:

- codebase observation: file outline, symbol search, diff impact, test/gate status;
- action surfaces: edit queues, shell/gate runner, workspace snapshots, factory graph/status;
- every tool returns typed bounded data, not arbitrary terminal prose;
- Python remains the orchestration language that composes these tools.

**Concrete Prime problem**

The persistent Python kernel is powerful, but ad hoc Python/shell observation can be slow, unbounded, hard to replay, and hard to render. Prime already benefits from Rust-owned daemon/TUI/session boundaries; the pattern is to extend that discipline only where it improves reliability.

**Expected benefit hypothesis**

Agents spend fewer turns gathering messy context, avoid blocking UI-critical paths, and make fewer mistakes from truncated or ambiguous shell output.

**Failure modes**

- overbuilds tools for one-off needs;
- duplicates existing MCP/search capabilities;
- hides useful raw context behind too-rigid schemas;
- adds public API surface too early;
- tool latency enters paint or prompt critical path.

**Validation experiment**

Pick one high-friction observation loop, such as “what changed and what tests matter?” Compare current shell/Python workflow vs a bounded typed tool:

- turns to correct plan;
- command count;
- output bytes in transcript;
- missed affected files/tests;
- UI latency and timeout behavior.

Only promote to a durable Rust surface if it wins on held-out tasks.

---

## Priority order

1. **Durable factory run state** — strongest concrete gap over existing Prime architecture.
2. **Taste model** — high leverage for this user and architecture work; small storage surface if scoped well.
3. **Selective advisor** — cheap behavioral pattern; improves decision quality without new infrastructure.
4. **Reversible cold context** — high upside, but needs careful corpus and privacy design.
5. **Bounded Rust observation/action tools** — apply surgically after measuring a repeated high-friction loop.

## What not to do

- Do not port another DAG engine. Prime already has factory orchestration.
- Do not add a second Decision API stack. Extend existing `decisionApi.systemOneModel` / `systemone` path if needed.
- Do not treat “learning” as automatic memory writes. Learn only from feedback and verified outcomes.
- Do not add vector search before proving lexical/artifact retrieval fails.
- Do not discuss licenses, FFI, or dependency plumbing for this decision; the requested level is pattern impact.
