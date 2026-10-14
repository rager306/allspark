# Prime Agent architecture review (read-only)

Scope: `/root/prime-agent`, clean HEAD `6879027856c55158f5f1286c16c78f20d6bec452` (`feat/fork-providers`). No repository files changed. No secrets read. No benchmarks run, so any performance statement below is a hypothesis or architecture risk, not a measured result.

## MCP evidence and graph limits

### codebase-memory-mcp

Required calls were run and saved under `/tmp/prime-architecture-research/cbm-*.txt`:

- `list_projects`: confirmed project `prime-agent /root/prime-agent feat/fork-providers`.
- `index_status`: `status: ready`, `36687` nodes, `255260` edges, indexed at `2026-10-11T18:43:59Z`; parse partial count is truncated; `.github/VOUCHED.td` is unusable.
- `get_architecture`: entry points are `crates/pa-cli/src/main.rs`, `crates/pa-daemon/src/main.rs`, and `prime-agent-runtime/src/rlm/repl.py`; graph reports 19 node labels and 24 edge types.
- `search_graph`: query saturated (`candidate_window_saturated: true`) for broad architecture terms. Useful for finding symbols, not exhaustive proof.
- `trace_path`: ran on `RlmSubagentHost.spawn`, `score_harness_entry_for_query`, and `run_turn_with_provider_failover`. Some traces include many low-confidence heuristic edges and page truncation, so I used them as leads and verified against source.

Graph limitations:

- The graph can be stale relative to the current worktree; source line checks are the authority.
- `trace_path` for trait methods can be noisy: `RlmSubagentHost.spawn` reported 396 callers with many heuristic matches.
- `get_architecture` “hotspots” contain derived fan-in data but not measured runtime bottlenecks.
- I did not run a heavy index.

### GitNexus

`gitnexus list_repos` did not include `prime-agent`. I did not run indexing. GitNexus was used only to confirm absence.

### Arsumbris

Used as a lightweight knowledge lead source. It is not treated as ground truth for Prime implementation.

---

## Crate architecture and ownership

Prime Agent is split along clear crate boundaries:

- `pa-agent`: agent turn loop only. README says it drives provider streams, dispatches tool calls, enforces max-turn/sequencing semantics, handles partial streams, retries stream failure, and propagates aborts; it has no tool implementations, providers, persistence, or UI (`crates/pa-agent/README.md:5-9`).
- `pa-ai`: providers, model registry, streaming, provider error shapes, JSON repair, faux provider. It is provider-facing, not session-facing (`crates/pa-ai/README.md:5-14`).
- `pa-core`: session engine. It owns MCP host side, tools, file mutation queue, RLM kernel lifecycle, skills, system prompt assembly, compaction, harness refinement, settings/config, packages, session manager, autonomous gates, RLM host contract, provider retry/failover policy, factory host, Decision API, and workspace snapshots (`crates/pa-core/README.md:5-36`, `77-91`).
- `pa-daemon`: supervisor and wire serving. It owns one worker process per active session, restart/backoff, session registry/roster, append-only store, direct attach, agent messaging bridge, RLM child sessions, worker-to-worker peer messaging, archive/wake, model switching, factory activity surface, and roster updates (`crates/pa-daemon/README.md:18-33`, `91-99`).
- `pa-tui`: terminal UI. It renders daemon events and sends user intents; it does not own session logic (`crates/pa-tui/README.md:1-5`, plus non-goals in that README).
- `pa-cli`: composition root / binary. It matches TS CLI shape, wires crates, owns command-line auth/update flows, and has no business logic (`crates/pa-cli/README.md:5-10`).
- `pa-types`: shared wire/domain vocabulary only.
- `pa-telemetry`: telemetry schema, primitive-only properties, non-blocking client, sinks, install id, durable rename primitive. Best-effort by contract and should not block user-facing paths (`crates/pa-telemetry/README.md:8-16`).
- `pa-models`: live model catalog and pinned catalog fetching.

This matches the dependency direction in `AGENTS.md`: `pa-types` is the shared vocabulary; `pa-cli` is the composition root; `pa-tui` depends only on wire types, not the session engine.

---

## Actual runtime architecture

### 1. Agent loop

`pa-agent` is a provider/tool-agnostic loop. The important seam is that tool execution is abstracted away from the loop: README says it receives a `ToolDispatcher` boundary and does not implement tools itself (`crates/pa-agent/README.md:5-9`). This keeps model streaming/retry/abort behavior separate from shell/edit/IPython/MCP behavior.

Extension seam:

- agent behavior hooks and loop config in `pa-agent/src/agent.rs`;
- tool behavior belongs in `pa-core`, not `pa-agent`.

### 2. Session engine

`pa-core` is the central engine. Evidence:

- MCP host requests and OAuth live in `pa-core` (`crates/pa-core/README.md:6-15`).
- Kernel lifecycle, skills, compaction, harness refinement, settings, packages, and session manager live there (`crates/pa-core/README.md:16-26`).
- Autonomous mode is policy/state plus shell gates; the turn loop consults an `AutonomousDriver` after settled turns (`crates/pa-core/README.md:17-20`).
- Provider retry/failover decision logic lives in `session_engine`, while the daemon owns attempts, waits, and model switches (`crates/pa-core/README.md:28-36`).

Extension seams:

- `SessionEngine` facade;
- `ToolRegistry`;
- `McpManager`;
- `RlmSubagentHost`;
- `FactoryHost`;
- autonomous `GateCommandRunner`;
- provider retry/failover policy modules;
- refinement planner/ranking;
- workspace snapshot foundation.

### 3. Supervisor / workers

The daemon is a supervisor, not an in-process session host:

- one worker process per active session with restart/backoff (`crates/pa-daemon/README.md:18-20`);
- session identity survives supervisor restarts via worker re-registration and roster rebuild (`crates/pa-daemon/README.md:19-20`);
- direct attach uses supervisor-issued single-use tickets and worker-side peer grants (`crates/pa-daemon/README.md:25-29`);
- agent messaging joins the supervisor roster with the session’s own RLM children registry; delivery tries direct peer transport then supervisor-routed fallback (`crates/pa-daemon/README.md:29-33`).

Extension seam:

- daemon worker/supervisor wire is the right layer for durable run recovery, roster projection, policy propagation, and peer messaging; not the TUI.

### 4. Persistent Python kernel / PTC

The Python runtime is first-class:

- Rust kernel manager runs a JSON-lines subprocess: `python -m rlm.repl` (`crates/pa-core/src/kernel/manager/mod.rs:1-4`).
- Runtime cells execute with top-level await in one persistent `__main__` namespace on one asyncio loop (`prime-agent-runtime/src/rlm/repl.py:1-5`).
- The manager tracks active executions and host requests (`crates/pa-core/src/kernel/manager/mod.rs:110`, `25`).
- Requests go through enqueue/execute plumbing (`crates/pa-core/src/kernel/manager/requests.rs:58`, `134`).

Extension seam:

- Python is the orchestration layer; Rust should provide bounded typed host surfaces where needed.
- Host requests are the correct path for new controlled primitives; ad hoc shell output should not become a hidden protocol.

### 5. Subagents and RLM recursion

Subagents are real daemon sessions, not fake tool calls:

- `RlmSubagentHost` defines spawn/create/list/delete/collect/rename; collect timeouts return snapshots, not errors (`crates/pa-core/src/session_engine/rlm_host.rs:171-187`).
- Daemon implementation creates real daemon sessions through the worker’s supervisor link; one supervised worker per child; parent registry serves list/collect/delete (`crates/pa-daemon/README.md:91-99`).
- `pa-daemon/src/rlm_children/host.rs` implements `RlmSubagentHost` (`crates/pa-daemon/src/rlm_children/host.rs:54-55`, `359`, `538`).

Existing overlap:

- Do not propose “add subagents” as absent.
- Useful deltas should be about better state, policy, recovery, or evaluation over existing subagents.

### 6. Factory orchestration

Prime already has declarative orchestration:

- `FactoryHost` exposes `factory.graph/status/watch/run/stop/resume` to daemon/TUI (`crates/pa-core/src/session_engine/factory_host.rs:1-3`).
- Factory itself lives in the Python kernel; Rust host validates `run`, prefights before child spawns, and uses out-of-band `factory_activity` so a running cell does not delay the live view (`crates/pa-core/src/session_engine/factory_host.rs:5-10`).
- Factory children use existing `rlm.spawn`, with daemon allowlist enforced per spawn (`crates/pa-core/src/session_engine/factory_host.rs:17-21`).
- Python factory validates machine specs, compiles DAG sugar, and runs canonicalized machines through the RLM supervisor (`prime-agent-runtime/src/rlm/factory.py:19-22`).
- Factory supports state machines with guards, joins, cycles, bounded re-entry, foreach, budgets, retries, `max_parallel`, resident states, rate-limit backoff, stop/resume, graph/watch/status (evidence from `prime-agent-runtime/src/rlm/factory.py:1-30`, `56-73`, `487-500`, `704`, `2510-2700`, `3260-3428`).
- `/factory` TUI renders live machine graph, active nodes, last-fired edges, run state, queued/running counts, budget, and milestone tail (`crates/pa-tui/src/factory_view.rs:1-24`).

Concrete gap:

- Factory run state is kernel-memory only. The module says: “Runs do not survive a kernel restart”; children remain supervisor-owned (`prime-agent-runtime/src/rlm/factory.py:23-26`). `FactoryRun` repeats this: executor-side state does not survive kernel restart (`prime-agent-runtime/src/rlm/factory.py:1422-1424`). `FactoryExecutor` repeats it again (`prime-agent-runtime/src/rlm/factory.py:1503-1505`).
- `factory.py` has no `worktree` references (searched locally). So factory states do not currently bind a durable worktree/artifact snapshot.

Strong extension seam:

- durable factory run ledger/checkpoint plus rehydration over supervisor child state.

### 7. Harness / refinement / memory

Harness state is real persistent state, but retrieval is currently lexical:

- Python harness has `HarnessState`, CRUD methods, `record_refinement`, and `plan_refinement` (`prime-agent-runtime/src/rlm/harness.py:389`, `914`, `1141`, `1174`).
- Rust refinement state loads and updates persisted harness state (`crates/pa-core/src/refinement/mod.rs:85`, `175`, `300`).
- Refinement planner prompt explicitly includes prompt/memory/skill/subagent/factory components and rollback expectations (`crates/pa-core/src/refinement/planner.rs:10`).
- Rollback proposal exists (`crates/pa-core/src/refinement/planner.rs:505-540`).
- Ranking is lexical substring overlap across title/content/path/id, weighted by optional corpus IDF (`crates/pa-core/src/refinement/ranking.rs:139-175`, `108-137`).

Concrete gap:

- The harness can persist facts and factories, but ranking is not semantic or outcome-aware. It can miss a preference/fact if query terms do not overlap.
- Preferences (“taste”) are not first-class distinct objects; they are memories/prompt notes unless the agent structures them manually.

### 8. Compaction and context memory

Compaction exists and should not be described as naive truncation:

- `/compact` execution resolves a cut, runs summary call, handles file ops block and split summaries (`crates/pa-core/src/session_engine/compact_session.rs:1-10`).
- It carries previous summary (`crates/pa-core/src/session_engine/compact_session.rs:129`).
- Conversation serialization pairs tool calls/results with sequential ids (`crates/pa-core/src/session_engine/compaction_utils.rs:195-203`).

Concrete gap:

- Compaction is still a summarization path. Exact reversible recall of old blocks is not evident in the inspected surfaces. This is a hypothesis for improvement, not a claim that current compaction is broken.

### 9. Tools and MCP

Tools are owned by `pa-core`:

- `tools/mod.rs` includes bash/edit/ipython modules (`crates/pa-core/src/tools/mod.rs:4`, `12`, `16`).
- MCP manager owns host-request handlers for `refresh/config/begin_login` (`crates/pa-core/src/mcp/mod.rs:554`) and exposes connection roster (`crates/pa-core/src/mcp/mod.rs:893`).
- README clarifies engine never spawns MCP servers; protocol itself runs kernel-side Python stdio/HTTP (`crates/pa-core/README.md:14-16`).

Extension seam:

- bounded typed observation/action tools should be added only when they replace a repeated unreliable shell/Python loop. Existing MCP and skills should be checked first.

### 10. Retry / failover

Provider resilience is already present:

- failover reroutes after provider retry budget is spent or payment failure spends it immediately (`crates/pa-core/src/session_engine/provider_failover.rs:80-86`).
- policy and delay functions exist in provider retry/failover modules (`crates/pa-core/src/session_engine/provider_retry.rs:142`, `193`; `crates/pa-core/src/session_engine/provider_failover.rs:27`, `94`).

Do not propose generic failover as absent.

### 11. Autonomous quality gates

Autonomous mode already has user-defined shell gates:

- `GateCommandRunner` abstracts gate command execution and snapshots (`crates/pa-core/src/autonomous/gates.rs:31-39`).
- Gates run in order; first failure wins (`crates/pa-core/src/autonomous/gates.rs:144-151`).
- Failed gates are not rerun on unchanged workspace snapshot (`crates/pa-core/src/autonomous/gates.rs:153-180`).
- README advertises bounded autonomous mode with user-defined gates and warns that a passed gate only proves what that gate verifies (`README.md:114`).

Do not propose generic “quality gates” as absent. Useful delta is better artifact linkage, durable factory integration, or verified trajectory memory.

### 12. Decision API / System One

Decision API already exists:

- `decision_api.decide` is gated by `decisionApi.systemOneModel`; it resolves through normal model registry and provider transports; no separate credential store (`crates/pa-core/src/session_engine/decision_api.rs:1-9`).
- Decision loop can run System 1 as a spawned decision child (`docs/decision-api.md:31-37`).
- It supports System One structured-decision protocol (`api: systemone`) or ordinary chat-completion transports (`docs/decision-api.md:39-50`).

Do not propose “add Decision API” as absent. Useful delta could be multi-question/batched decisions or specific model support through the existing path, validated by latency/accuracy experiments.

### 13. Telemetry

Telemetry is designed to be non-blocking and privacy bounded:

- properties are primitive-only (`crates/pa-telemetry/README.md:8-10`);
- `TelemetryClient` is non-blocking with background worker and best-effort contract (`crates/pa-telemetry/README.md:11-14`).

Extension seam:

- Any new user-visible feature should add adoption telemetry, but user-facing code must not await delivery.

---

## Real bottlenecks / risks (not measured performance)

1. **Factory run durability gap.** Existing factory orchestration is strong, but run state disappears on kernel restart while children survive. This is the clearest concrete architecture gap.
2. **Harness retrieval is lexical.** `score_harness_entry_for_query` is substring + IDF. This is simple and robust, but weak for preference/taste recall and semantic recovery.
3. **Exact old context is not obviously reversible.** Compaction summarizes and preserves progress, but exact block recall would need a ledger/artifact layer.
4. **Powerful tools lack policy profiles.** Prime has real workers/subagents/factory/autonomous paths. Worker isolation is not the same as a permission/sandbox policy. Tool dispatch / MCP / shell launch are the enforcement layers to inspect.
5. **Factory has no worktree/artifact binding.** A factory state can spawn children and capture text/JSON outputs, but inspected `factory.py` has no worktree concept. This limits durable phase recovery.
6. **Graph tooling can mislead.** codebase-memory traces are useful, but broad searches saturate and trait traces are noisy; source evidence must remain primary.

---

## High-effect extension seams and validation experiments

### A. Durable factory run state

Mechanism:

- Persist factory run state as session artifact ledger/checkpoint.
- Rehydrate Python factory registry after kernel restart by reconciling child ids with supervisor state.
- Store typed, bounded captured outputs; avoid unbounded transcript copies.

Why this fits Prime:

- It extends the existing factory; it does not add a new DAG engine.
- `FactoryHost` already bridges daemon/TUI to kernel factory activity.
- Children are already supervisor-owned and discoverable.

Validation:

- Run synthetic factories with joins, retries, foreach, residents, and backoff.
- Kill/restart kernel mid-run.
- Assert no duplicate child admissions, no double-applied settlements, same graph shape after rehydrate, residents remain stoppable.

### B. Taste model from feedback

Mechanism:

- Represent user preferences as scoped, evidence-backed atoms, not generic memories.
- Scope by global/repo/task type.
- Store evidence: user correction, accepted/rejected recommendation, rollback, verified outcome.

Concrete Prime problem:

- Harness memory can store preferences, but it does not distinguish “taste” from facts or ensure retrieval under paraphrase.

Validation:

- Use repeated architecture prompts with known corrections.
- Measure repeated violation rate, false application rate, and user edits needed.

### C. Selective independent advisor

Mechanism:

- Use subagents/factory only at decision gates: architecture tradeoffs, risk reviews, plan critique, repeated failure.
- Advisor output must be short: disagreement, missing evidence, strongest counterargument, validation experiment.

Concrete Prime problem:

- Prime can spawn many agents, but more context is not the same as independent judgment. The parent can still anchor on its first plan.

Validation:

- Historical prompts with known user corrections: compare final answers with/without advisor gate.
- Measure overlap mistakes caught before final answer.

### D. Reversible folded context

Mechanism:

- Keep exact old blocks or artifact pointers in a folded-context ledger.
- Prompt carries summary + handles; Python can search/unfold.
- Start lexical/path/recency before vectors.

Concrete Prime problem:

- Compaction is useful but lossy; harness ranking is lexical.

Validation:

- Forced-compaction tasks where exact old evidence is needed later.
- Measure recall precision@k, token cost, and verified completion.

### E. Fast bounded Rust observation/action tools

Mechanism:

- Add Rust-owned typed tools only for repeated high-friction loops: diff impact, relevant tests, factory graph/status, workspace artifact inventory, bounded code outline.
- Python remains the orchestrator.

Concrete Prime problem:

- Ad hoc shell/Python observation can be noisy, unbounded, and hard to render/replay.

Validation:

- Compare current shell/Python workflow vs typed tool on one repeated loop.
- Measure turns to correct plan, output bytes, missed affected files/tests, and UI latency.

### F. Policy profiles for autonomous/factory/subagents

Mechanism:

- Enforce at tool dispatch / process launch / MCP host request layers.
- Profiles: read-only, edit-with-approval, shell-limited, network-off, destructive-confirm, spend caps.
- Inherit into `rlm.spawn` and factory state spawns.

Concrete Prime problem:

- Prime has powerful long-running execution; stronger autonomy needs enforceable boundaries, not prompt-only safety.

Validation:

- Audit-only first: count would-block actions and false positives.
- Then enforce on selected sessions and compare task completion + user aborts.

---

## Summary

Prime Agent already has most core harness ingredients: persistent Python PTC, supervised workers, real subagents, MCP, skills, continual harness refinement, rollback, autonomous gates, provider failover, Decision API, and factory DAG/state-machine orchestration.

The strongest factual gap I found is not missing orchestration. It is **durable factory state**: current factory run state is kernel-memory only, while children survive in the daemon. The next most promising patterns are structured taste learning, selective advisor use, reversible exact context recall, bounded Rust observation/action tools, and enforceable policy profiles.
