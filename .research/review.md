# Independent review: architecture hypotheses vs current Prime Agent

Scope: read-only review of `/tmp/prime-architecture-research/baseline.md`, `/tmp/prime-architecture-research/gsd-research.md`, `/root/prime-agent/AGENTS.md`, and selected source under `/root/prime-agent`. I did not run tests, builds, or indexing. I inspected the codebase-memory MCP schemas only; source checks below are direct file reads/grep.

## Executive finding

The strongest useful delta is **not “add a DAG orchestrator”**. Prime Agent already has a gated factory DAG/state-machine runtime with joins, guards, retries, max-parallel admission, resident tasks, graph/watch/status, and TUI surfaces. The concrete gap is narrower and more valuable:

> make factory runs restart-safe and evidence-bearing, then optionally add worktree/artifact fan-in on top.

The second useful delta is **better recall/retrieval**, but it should be added as a measured retrieval layer behind the existing harness digest / REPL skill surface, not as new top-level model tools.

The third useful delta is **cost/circuit routing**, but Prime already has provider retry/failover. Any new routing must be telemetry/dry-run first and must not duplicate the existing failover loop.

## Existing implementation evidence

### 1) Factory DAG/state-machine orchestration already exists

Runtime implementation:

- `prime-agent-runtime/src/rlm/factory.py:1-26` documents factory entries as declarative state machines, with DAG form as sugar; it says states run through `rlm.spawn`, settle through `rlm.collect`, cancel through `rlm.delete_subagent`, and that run state is kernel-memory only.
- `prime-agent-runtime/src/rlm/factory.py:632-656` validates join transitions: `from` may be a list and joins cannot carry guards.
- `prime-agent-runtime/src/rlm/factory.py:704-716` compiles DAG nodes into machine states and turns full effective dependencies into one join transition.
- `prime-agent-runtime/src/rlm/factory.py:1422-1424` states `FactoryRun` does not survive kernel restart.
- `prime-agent-runtime/src/rlm/factory.py:1483-1505` defines `FactoryExecutor`, with supervisor-owned children and kernel-memory run registry.
- `prime-agent-runtime/src/rlm/factory.py:1524-1537` starts runs nonblocking and hands continuation to a background task.
- `prime-agent-runtime/src/rlm/factory.py:2322-2429` evaluates settles, guards, fan-out, join signatures, `max_transitions`, and transition firing.
- `prime-agent-runtime/src/rlm/factory.py:2432-2476` admits ready work under `max_parallel`, budget, child budget, and backoff.
- `prime-agent-runtime/src/rlm/factory.py:2688-2781` spawns subagents and handles rate-limit backoff/deferred admission.
- `prime-agent-runtime/src/rlm/factory.py:2785-2865` applies settlements, captures answers, retries child failures, and captures outputs.
- `prime-agent-runtime/src/rlm/factory.py:3218-3246` defines quiescence, including special treatment for resident instances.
- `prime-agent-runtime/src/rlm/factory.py:3278-3357` runs the control loop: collect, apply settlements, evaluate transitions, finalize, budget pause, spawn more work.

Host/TUI bridge:

- `crates/pa-core/src/session_engine/factory_host.rs:1-10` says factory itself lives in the Python kernel; the Rust bridge validates requests, preflights `run`, and sends `factory_activity` frames.
- `crates/pa-core/src/session_engine/factory_host.rs:236-287` preflights declared models against catalog, allowlist, and auth before any child spawns.
- `crates/pa-daemon/src/factory_activity.rs:1-9` exposes the daemon lane and notes model preflight happens before spawning.
- `crates/pa-daemon/src/factory_activity.rs:18-40` hides the factory lane unless `factory.enabled` is on.
- `crates/pa-tui/src/factory_view/tests.rs:990-1011` fixture comments cover a PR-manager graph with triage, review, fix, verify, merge, resident watchdog, and fan-in join; later tests assert join rendering and resident behavior.

Concrete gap:

- Factory run state is explicitly **not durable**: `prime-agent-runtime/src/rlm/factory.py:23-26`, `1422-1424`, `1503-1505`. Children survive as supervisor-owned subagents, but the factory registry and transition state do not.
- I found no worktree integration in `prime-agent-runtime/src/rlm/factory.py`; the factory runtime manages subagents and answers, not per-task git worktrees or merge fan-in.

### 2) Workspace snapshots exist, but they are plumbing, not task fan-in

- `crates/pa-core/src/workspace_snapshot/mod.rs:1-8` captures a git worktree delta plus untracked files as a self-verifying artifact.
- `crates/pa-core/src/workspace_snapshot/mod.rs:21-24` explicitly says it is foundation plumbing and has no user-facing toggle or transport.
- `crates/pa-core/src/workspace_snapshot/mod.rs:169-179` says staged blobs are content-addressed and owner-only, but also warns ordinary-named secret files stage verbatim and the snapshot directory is secret-bearing.
- `crates/pa-core/src/workspace_snapshot/verify.rs:41-56` verifies a staged snapshot by parsing the manifest and rehashing blobs.

This is useful for future worktree fan-in, but it is not a task-level durable DAG or merge protocol today.

### 3) Context recall / harness retrieval already has lexical ranking and compaction anchors

- `crates/pa-core/src/refinement/ranking.rs:104-136` computes IDF over query terms.
- `crates/pa-core/src/refinement/ranking.rs:139-174` scores harness entries by weighted lowercase substring overlap across title/content/identifier.
- `crates/pa-core/src/refinement/ranking.rs:195-285` renders the harness digest and ranks entries per kind.
- `crates/pa-core/src/session_engine/harness_digest.rs:36-58` builds digest query terms from the active goal and recent texts.
- `crates/pa-core/src/session_engine/harness_digest.rs:79-107` merges global/local harness state and renders a fingerprinted digest.
- `crates/pa-core/src/session_engine/compaction_exec.rs:13-18` records compacted read/modified file details.
- `crates/pa-core/src/session_engine/compaction_exec.rs:40-64` includes previous summary and recent-state-anchor in summarization requests.

Gap: this is lexical digest ranking plus summary/file tracking, not hybrid semantic retrieval over artifact handles.

### 4) Verified learning / corrections already has rollback and conflict controls

- `crates/pa-core/src/refinement/mod.rs:46-82` defines harness entries and refinement events with evidence/outcome.
- `crates/pa-core/src/refinement/planner.rs:471-490` records applied changes, evidence, and expected outcome.
- `crates/pa-core/src/refinement/planner.rs:503-540` builds rollback proposals from before snapshots.
- `crates/pa-core/src/session_engine/refine.rs:246-255` documents the full plan → re-read → apply → persist flow.
- `crates/pa-core/src/session_engine/refine.rs:337-359` captures a baseline state before the LLM plan.
- `crates/pa-core/src/session_engine/refine.rs:395-407` applies under `update_harness_state`, rejecting concurrent clobber classes through the baseline path.

Gap: I did not find an explicit “correction became verified only after a later task proves it” gate. The current system is evidence-backed and rollbackable, but not outcome-verified by a held-out check.

### 5) Retry/failover and model cost data already exist

- `crates/pa-core/src/session_engine/provider_retry.rs:1-4` documents the shared provider retry policy.
- `crates/pa-core/src/session_engine/provider_retry.rs:25-32` defines default retry settings.
- `crates/pa-core/src/session_engine/provider_retry.rs:138-157` marks permanent failures.
- `crates/pa-core/src/session_engine/provider_failover.rs:1-5` documents provider failover across configured providers serving the same model.
- `crates/pa-core/src/session_engine/provider_failover.rs:35-49` defines default failover and a whole-episode retry ceiling.
- `crates/pa-core/src/session_engine/provider_failover.rs:80-86` explains candidate switching and primary restoration.
- `crates/pa-core/src/models/resolver.rs:506-542` finds failover candidates: other providers serving the same model in catalog order.
- `crates/pa-models/src/schema.rs:144-154` maps model cost fields into `ModelCost`.
- `crates/pa-models/src/schema.rs:173-180` validates model cost bounds.

Gap: I found retry/failover and cost metadata, but not a cross-task cost-aware router or provider circuit breaker that learns from recent failure/cost history.

## Traps and duplicate-state risks

1. **Do not duplicate the existing factory.** GSD “DAG” language maps onto an existing Prime factory surface. The right delta is durable run registry + proof/artifact ledger, not a second orchestrator.

2. **Do not treat docs or hooks as enforcement.** GSD-style instructions are not runtime guarantees. Hooks around `bash`/`edit` cannot guard arbitrary Python code, subprocesses, MCP calls, or project CLIs. Prime must keep the existing model tool surface (`bash`, `edit`, `ipython`) per `AGENTS.md:172-176`; recall should be a REPL skill/harness feature, not new top-level tools.

3. **Worktrees are not a sandbox.** `README.md:86-87` says worker/kernel isolation is not a security sandbox. Worktree isolation can reduce merge collisions, not contain untrusted code.

4. **Snapshot secret trap.** Workspace snapshots explicitly stage ordinary-named secrets verbatim (`crates/pa-core/src/workspace_snapshot/mod.rs:13-14`, `176-179`). Any artifact fan-in must keep denylist/secret warnings and should avoid uploading or exposing snapshot blobs without user-visible policy.

5. **Duplicate state trap for durable factories.** Persisting factory run state risks diverging from supervisor-owned child sessions. The source already says children survive while factory registry does not (`factory.py:23-26`). A durable layer should record child IDs, transition ledger, and machine version as the factory authority, then reconcile from `rlm.list_subagents`/`rlm.collect` on restart. It should not create a second independent scheduler.

6. **Performance trap.** AGENTS requires user-visible paths not to await network/disk sync (`AGENTS.md:96-107`). Retrieval, routing, or factory graph views cannot block first paint or prompt admission on network calls.

## Recommended minimal experiments

### Experiment A — Durable factory run ledger and restart recovery

Goal: make existing factory runs restart-safe without changing the model-facing tool surface.

Minimal shape:

- Persist a per-run append-only ledger under the session artifact directory.
- Persist: run id, spec id, canonical machine hash, state entries, transition events, child ids, child names, output capture status, pause/final state.
- On kernel restart, reconstruct the `FactoryRun` registry from the ledger and reconcile live child state through existing supervisor-owned children (`rlm.list_subagents` / `rlm.collect` equivalent host path).
- Do not add worktrees in this first experiment.

Verifier:

- Start a factory with two parallel task states and a fan-in join.
- Kill/restart only the kernel after one child settles and one remains running.
- Reattach and confirm `rlm.factory.status/graph/watch` show the same run id, no duplicate child spawns, the join fires once after both children settle, and stop/resume still act on original child IDs.

Stop criteria:

- Any restart causes duplicate spawn, missed join, lost output capture, or a run that cannot be stopped by run id.
- Recovery needs a second scheduler that disagrees with the existing factory state machine.
- The TUI factory view or prompt path waits on network or heavy disk scans.

### Experiment B — Proof/artifact handles for factory states, not automatic worktree merge yet

Goal: add evidence of completion to existing states without turning worktrees into a sandbox.

Minimal shape:

- Extend factory state output conventions: child must write a small proof file or JSON block with declared artifact paths, verifier command/output summary, and optional diff/snapshot handle.
- Parent/factory records the proof handle in the event ledger and status output.
- Keep raw files on disk; put handles and summaries in context.
- If using workspace snapshots, reuse `workspace_snapshot` as a staged/verifiable artifact, but do not claim secret safety.

Verifier:

- One factory state produces a proof JSON; downstream state binds it as input; status shows the proof handle and capture errors if missing.
- Restart after proof capture still shows the same handle.

Stop criteria:

- Proofs become free-form transcript text only.
- Proof capture needs to read large artifacts into the prompt.
- Any path exposes ordinary-named secret blobs by default.

### Experiment C — Hybrid recall as a measured REPL/harness retrieval layer

Goal: improve recall without replacing current compaction/harness digest or adding new top-level tools.

Minimal shape:

- Keep current lexical digest ranking as the safe default.
- Add an opt-in hybrid retrieval command/skill callable from the REPL that searches harness entries + compacted file/artifact handles.
- Return small handles and snippets, not full blobs.
- Run retrieval off the render path; cache/index in background.

Verifier:

- Fixed held-out set of past tasks/corrections: compare lexical digest vs hybrid retrieval for whether the needed memory/artifact appears in a bounded top-k result.
- Include “wrong memory retrieved” and “sensitive artifact surfaced” as failure modes.

Stop criteria:

- No clear improvement on held-out recall tasks.
- Any network or embedding call blocks prompt admission, factory graph paint, or first frame.
- Retrieval requires new model-visible top-level tools instead of the existing REPL/skill/harness surfaces.

### Experiment D — Cost-aware routing as dry-run telemetry first

Goal: avoid duplicating retry/failover while learning whether cost routing helps.

Minimal shape:

- Do not change model selection initially.
- Record candidate model cost, selected model, retry/failover events, provider failure kind, and final verified outcome in telemetry-safe primitive fields only.
- Add a “what would the cost-aware router have picked?” dry-run log for comparable tasks.

Verifier:

- For a small fixed task suite with verifiers, compare dry-run choices against actual success/failure/spend after the fact.

Stop criteria:

- Dry-run choices would route away from models that pass verifiers.
- The router duplicates provider_failover logic instead of consuming its events.
- It adds content-bearing telemetry or blocks user-visible paths.

## Bottom line

The highest-confidence improvement is: **durabilize existing factory runs and attach proof/artifact handles**. That uses Prime’s current state-machine runtime instead of replacing it. Worktree fan-in should come after that, and only as collision control plus verifiable artifacts, not as sandboxing. Hybrid recall and cost routing are plausible, but they need held-out comparisons and strict stop criteria before becoming default behavior.
