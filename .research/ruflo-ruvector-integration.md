# RuFlo × RuVector integration: actual wired flows, not code-port advice

Date: 2026-10-12. Mode: read-only research. No repos, credentials, installs, or external execution were changed. RuFlo was not present locally in `/root` or `/root/vendor-source`; I used the pinned GitHub tree/raw files for `ruvnet/ruflo@{sha}` and existing extracted files in `/tmp/prime-architecture-research`. RuVector local checkout `/root/vendor-source/ruvector` exists and is indexed as `root-vendor-source-ruvector`, but this report focuses on **RuFlo's actual use of RuVector/AgentDB/SONA-style substrates**.

## Bottom line

RuFlo has several real adaptive loops, but they are uneven:

1. **Model/tool/agent routing adapts from persisted outcome signals.** This is real code. The strongest wired loop is task/model routing: route → execute → record outcome → next route reads priors/patterns.
2. **Feedback and trajectory storage are real, and failures are preserved.** Post-task/command/edit paths record both success and failure. Failures can lower confidence in similar SONA/ReasoningBank patterns. Learned routing patterns are selected from successful/high-quality rows only, which is correct selection behavior, not loss of negative evidence.
3. **RuVector in RuFlo is mostly an optional substrate/plugin boundary, not a single mandatory engine.** `ruflo-ruvector` is a mod around already-connected RuVector MCP tools. `@ruvector/core`, `@ruvector/tiny-dancer`, and `@ruvector/ruvllm` paths are optional/gated fallbacks. Default paths often use RuFlo's local JS/TS implementations and AgentDB/SQLite-like stores.
4. **Several reward signals are proxies.** `post-edit` from the core hook shim reports success simply because an edit event completed; `agent_execute` counts API response success as quality. These are useful telemetry but **not validated code quality or user taste**.
5. **Do not infer production HNSW/consensus quality from placeholders.** Earlier caution remains: V3 `AgentDBBackend` at this pin is Map + linear search, and `reachConsensus` is simulated. The production-ish integration evidence is in hooks/MCP tools/model-router paths below, not those placeholders.

---

## Flow 1 — agent/model route → RuVector-style router/advisor → execution → outcome → next route

**Classification: actual wired, with optional neural/RuVector acceleration.**

### Event chain

1. `agent_spawn` accepts an optional `task` and calls `determineAgentModel(...)` before writing the agent record. Evidence: `agent-tools.ts` lines 322-358.
2. `determineAgentModel` first tries the enhanced router. It embeds the task via `embedTaskSafe` so the cost-optimal neural backend can fire, then calls `enhancedRouter.route(...)`. Evidence: `agent-tools.ts` lines 200-236.
3. Enhanced routing falls through from deterministic codemods to `baseRouter.route(task, embedding)`. Evidence: `enhanced-model-router.ts` lines 382-472. It only forwards a concrete `modelId` when the final tier still matches the base router's tier, avoiding stale cheap-model overrides after escalation. Evidence: lines 488-552.
4. `ModelRouter.route` uses heuristic complexity + Thompson-sampling bandit. Optional neural routing only runs when an embedding exists and `CLAUDE_FLOW_ROUTER_NEURAL=1`; otherwise the result is `heuristic` or `bandit-fallback`. Evidence: `model-router.ts` lines 619-847.
5. `agent_execute` dispatches the agent's `modelId` first when present, else the tier alias. It retries retryable model failures through `nextCostOptimalAlternative(...)` when a concrete neural model id is available. Evidence: `agent-execute-core.ts` lines 574-684.
6. After execution, `agent_execute` records failures for failed retry attempts, then records the final tier/model-id outcome. Evidence: `agent-execute-core.ts` lines 694-720.
7. The next route reuses this through `.swarm/model-router-state.json` priors. `recordOutcome` writes Beta priors per complexity bucket and saves state. Evidence: `model-router.ts` lines 1175-1237 and 1375-1386.

### Stored state / reuse

- Persistent bandit state: `.swarm/model-router-state.json` via `saveState()`.
- Optional trajectory rows: `.swarm/model-router-trajectories.jsonl` only when `CLAUDE_FLOW_ROUTER_TRAJECTORY=1`. It writes decision rows at route time and outcome rows later. Evidence: `router-trajectory.ts` lines 1-10, 220-288.
- Per-model-id priors are shadow state today; selection still uses tier priors. Evidence: `model-router.ts` lines 346-356 and 1519-1533.

### What is adaptive here

- Real: model tier choice shifts after success/failure outcomes through Thompson sampling.
- Real: retryable failures are negative feedback for the failed model id/tier.
- Optional: neural/cost-optimal path needs env gate + embeddings + backend artifact.
- Not gold quality: source says success means the API returned without error; finer signals are a follow-up. Evidence: `agent-execute-core.ts` lines 694-700 and 750-758.

**Prime pattern:** this maps best to a **model/tool advisor**. Keep it advisory and evidence-counted. Do not treat API success as user taste. Store explicit outcome provenance: `api_success`, `test_passed`, `user_accepted`, `reviewer_rejected`, etc.

---

## Flow 2 — post-task feedback → SONA/ReasoningBank/AgentDB → feedback/patterns/edges → next route/context

**Classification: actual wired, with fallbacks.**

### Event chain

1. `hooks_post-task` takes `taskId`, `task`, `success`, `quality`, `agent`, `patterns`, and lineage fields. Evidence: `hooks-tools.ts` lines 1861-1895.
2. It calls `bridgeRecordFeedback(...)` with the real success flag and quality. Evidence: `hooks-tools.ts` lines 1916-1931.
3. It records a causal edge `taskId -> outcome-taskId` with weight `quality`. Evidence: `hooks-tools.ts` lines 1936-1947.
4. It records a SONA/ReasoningBank trajectory with verdict `success` or `failure`. Evidence: `hooks-tools.ts` lines 1949-1957.
5. It appends a file-based routing outcome `{task, agent, success, quality, keywords}` to `.claude-flow/routing-outcomes.json`, capped at 500 rows. Evidence: `hooks-tools.ts` lines 198-224 and 1993-2015.
6. Optionally, `storeDecisions` writes a `routing-decision:*` entry in the `patterns` namespace and mirrors it into the JSON memory store for dashboard stats. Evidence: `hooks-tools.ts` lines 2018-2050.

### SONA/ReasoningBank internals

- `bridgeRecordFeedback` calls `intelligence.recordTrajectory(...)` with a step whose content includes task context, success, and quality. It always stores a `feedback-*` memory entry in namespace `feedback`. Evidence: `memory-bridge.ts` lines 2572-2703.
- `recordTrajectory` embeds steps when possible, applies the verdict to SONA/ReasoningBank, and stores successful trajectories as patterns. Evidence: `intelligence.ts` lines 1127-1179.
- `LocalSonaCoordinator.endTrajectory` maps `success=1.0`, `partial=0.5`, `failure=-0.5` and adjusts confidence of similar patterns up or down. Evidence: `intelligence.ts` lines 239-295.
- `LocalReasoningBank` stores patterns, deduplicates by content, persists to disk, and searches by cosine over embeddings. Evidence: `intelligence.ts` lines 442-610 and 613-653.

### Reuse on next route

- `hooks_route` first tries AgentDB bridge routing, then eligible persisted SONA suggestions, then local semantic/keyword routing. Evidence: `hooks-tools.ts` lines 1304-1410.
- Local routing merges static task patterns with learned patterns built from routing outcomes. Evidence: `hooks-tools.ts` lines 226-247 and 1435-1520.
- Learned routing patterns are built only from successful outcomes with sufficient quality/support/discriminative share. Evidence: `learned-routing.ts` lines 45-122.
- A simpler keyword fallback also checks successful runtime outcomes directly. Evidence: `hooks-tools.ts` lines 930-969.

### What is adaptive here

- Real: task outcomes produce feedback rows, causal edges, trajectories, and learned routing patterns.
- Real: genuine failures are stored and can reduce confidence of similar SONA/ReasoningBank patterns.
- Correct selection distinction: learned route candidates use successful/high-quality rows; failures remain in feedback/trajectory stores but are not selected as positive routing examples.
- Fallbacks are visible: if AgentDB/ReasoningBank is missing, RuFlo stores fallback memory entries and marks degraded paths in several places.

**Prime pattern:** this maps to **verified refinement**. Store all outcomes, including failures, but only promote a memory/refinement/route rule after enough valid support. Exclude only invalid/corrupted/reward-hacked candidates, not real failures.

---

## Flow 3 — prompt.submit → AgentDB/RuVector recall → screened context → next model turn

**Classification: actual plugin hook, optional/off by default.**

### Event chain

1. `ruflo-agentdb` mod registers `prompt.submit` only when `recall` is enabled and `source != none`. Default is `recall: false`. Evidence: `options.ts` lines 26-34 and `register.ts` lines 176-181.
2. `recallFor` skips slash commands, shell lines, and short prompts; it caches by normalized prompt; it gives up after `recallDeadlineMs`. Evidence: `recall.ts` lines 28-47 and `register.ts` lines 113-155.
3. Readers are discovered from already-connected MCP tools. Preference order is:
   - `memory_search` labelled `agentdb` (semantic reader),
   - `agentdb_hierarchical-recall`,
   - `agentdb_pattern-search`,
   - RuVector `hooks_recall`.
   Evidence: `tools.ts` lines 17-23 and `register.ts` lines 31-35, 83-110.
4. Results are parsed, low scores under `0.25` are skipped, secrets/injection/oversize items are dropped, and remaining memories are framed as untrusted data. Evidence: `recall.ts` lines 79-130.

### Stored state / reuse

- This flow does not itself write learning state. It reuses prior AgentDB/RuVector memory into the next prompt context.
- The status file tracks attached/dropped/error counters for the console. Evidence: `register.ts` lines 137-149 and 176-181.

### What is adaptive here

- Real: per-prompt context changes based on stored memory and semantic readers.
- Optional: off by default; needs connected MCP tools and permissions.
- Safety: memory is framed as stale/wrong data, not instructions.
- RuVector-specific: `hooks_recall` is only one possible reader; docs say it is hash-based/noisier than `memory_search` for paraphrase recall. Evidence: `tools.ts` line 17 and `recall.ts` lines 23-26.

**Prime pattern:** this maps to **context/advisor retrieval**, not automatic policy. Use bounded, cited, untrusted snippets. Keep a score threshold and drop prompt-injection content.

---

## Flow 4 — core hook shim post-command/post-edit → learning stores → later routing/context

**Classification: actual wired, but reward semantics are weak.**

### Event chain

1. The `plugins/ruflo-core` hook shim converts `PostToolUse` JSON into CLI calls. For commands, it sets success from `exitCode === 0`. Evidence: `ruflo-hook.cjs` lines 449-456.
2. For edits, it forwards `post-edit -s true` unconditionally when a file path exists. Evidence: `ruflo-hook.cjs` lines 458-462.
3. `hooks_post-command` computes `success = exitCode === 0 && params.success !== false`, records the command outcome, stores command history unless command recording is disabled, and records a trajectory. Evidence: `hooks-tools.ts` lines 1180-1295.
4. `hooks_post-edit` records the edit through feedback, then also sends a one-step trajectory. Evidence: `hooks-tools.ts` lines 1057-1115.
5. The same SONA/ReasoningBank and routing outcome mechanisms described above can later affect routing/context.

### Reward semantics caution

- `post-command` is closer to a real signal when the command is a verifier/test. But a successful `ls`, `cat`, or formatting command is not code quality.
- `post-edit` in the shim is only “edit event succeeded”, not “code is correct”, not “tests pass”, and not “user liked it”.
- Downstream storage is real, but attribution can optimize a proxy unless corrected by later tests/user feedback.

**Prime pattern:** keep edit/command telemetry, but label reward source. Do not promote preferences or route rules from bare edit success. Tests, user acceptance, and review results should dominate.

---

## Flow 5 — `ruflo-ruvector` plugin boundary around RuVector MCP tools

**Classification: actual mod boundary, not an adaptive loop by itself.**

The `ruflo-ruvector` plugin wraps RuVector as a Claude Code mod. Its register hook does not run RuVector directly; it watches already-connected tools, counts calls, and applies a tighten-only secret guard.

- It registers `session.start`, `tool.call`, and `/ruvector-mod`. Evidence: `plugins/ruflo-ruvector/hooks/register.ts` lines 37-78.
- It treats all tools on server `ruvector` as owned, and guards write-like tools such as `hooks_remember`, `hooks_learn`, `rvf_ingest`, `brain_share`, `memory_store`. Evidence: `tools.ts` lines 13-31.
- Tests prove a secret is refused for writes, not echoed, while reader tools pass. Evidence: `ruvector.test.ts` lines 33-60.
- README documents many RuVector CLI/MCP capabilities, but that is not proof they are all wired into RuFlo default learning. Evidence: README lines 34-41 and 151-166.

**Prime pattern:** a boundary guard is valuable. It should only tighten writes and should not claim learning happened just because a tool call was seen.

---

## Actual vs optional vs docs-only

### Actual wired

- `agent_spawn` → enhanced/model router → agent record → `agent_execute` → `recordModelOutcome` → persisted bandit state.
- `hooks_post-task` → feedback/causal edge/trajectory/routing-outcomes → `hooks_route` learned patterns or SONA suggestion.
- `prompt.submit` recall when `ruflo-agentdb` recall is enabled.
- `hooks_post-command` and `hooks_post-edit` telemetry/trajectory recording.
- `ruflo-ruvector` guard/status around connected RuVector tools.

### Optional / gated / fallback

- Neural/cost-optimal router: requires embedding + `CLAUDE_FLOW_ROUTER_NEURAL=1` + backend artifact.
- Trajectory JSONL recorder: requires `CLAUDE_FLOW_ROUTER_TRAJECTORY=1`.
- `@ruvector/ruvllm` SONA coordinator: loaded opportunistically; local SONA/ReasoningBank runs without it.
- AgentDB/ReasoningBank healthy path: many handlers have memory-store/substring/sql.js fallbacks and mark degraded paths.
- `@ruvector/core` in `QLearningRouter`: optional import, JS fallback. I found tests and source, but not a production caller in the fetched tree.

### Docs-only / do not treat as fact

- `ruflo-ruvector` README claims 91 tools, HNSW speed, SONA/Brain features. Treat as capability documentation unless a specific hook/MCP path above uses it.
- Marketing benchmark SVG/README claims are not verifier evidence.
- V3 `AgentDBBackend` Map+linear and simulated consensus remain non-evidence for production-quality vector search/consensus.

---

## Mapping to Prime Agent

### Taste

RuFlo has no robust “user taste” loop in these flows. It has outcome proxies: API success, command exit code, edit event success, quality number supplied by caller. Prime should not infer user preference from one proxy. Taste should require explicit user signal or repeated validated evidence.

### Advisor

RuFlo's best pattern is an advisor: route recommendations carry confidence, backend, and source (`heuristic`, `hybrid`, `sona-pattern`, AgentDB controller). Prime can mirror this with a read-only advisor that recommends model/tool/skill/subagent choices with evidence counts and provenance.

### Verified refinement

RuFlo stores failures and successes, then filters promotion. Prime should do the same for harness refinement: store raw outcomes, but promote only after validation. Negative examples are useful; only invalid/corrupt/reward-hacked rows should be excluded from training.

### Choice of model/tool/skill

The model router + `hooks_route` are directly relevant. Prime can adapt model/tool/skill choice from outcomes, but reward labels must distinguish:

- API success/failure,
- test pass/fail,
- user accepted/rejected,
- reviewer verified,
- timeout/rate-limit/provider failure,
- “edit applied” vs “edit correct”.

### What is genuinely adaptive

- Adaptive: bandit priors, routing outcomes, SONA/ReasoningBank confidence updates, learned keyword patterns, optional trajectory pairing.
- Not enough: simple edit success, command success without knowing command semantics, docs claims, optional plugin install claims.

---

## Source pins

- RuFlo canonical pin: `ruvnet/ruflo@{sha}`.
- Full GitHub tree fetched to `/tmp/prime-architecture-research/ruflo-tree-full.json` (9542 entries, not truncated).
- Selected raw files saved under `/tmp/prime-architecture-research/ruflo-raw/`.

Key links:

- `plugins/ruflo-core/scripts/ruflo-hook.cjs` lines 449-462: https://github.com/ruvnet/ruflo/blob/{sha}/plugins/ruflo-core/scripts/ruflo-hook.cjs#L449-L462
- `v3/@claude-flow/cli/src/mcp-tools/agent-tools.ts` lines 167-236, 322-358: https://github.com/ruvnet/ruflo/blob/{sha}/v3/@claude-flow/cli/src/mcp-tools/agent-tools.ts#L167-L236
- `v3/@claude-flow/cli/src/ruvector/model-router.ts` lines 619-847, 1175-1237: https://github.com/ruvnet/ruflo/blob/{sha}/v3/@claude-flow/cli/src/ruvector/model-router.ts#L619-L847
- `v3/@claude-flow/cli/src/mcp-tools/agent-execute-core.ts` lines 574-720: https://github.com/ruvnet/ruflo/blob/{sha}/v3/@claude-flow/cli/src/mcp-tools/agent-execute-core.ts#L574-L720
- `v3/@claude-flow/cli/src/ruvector/router-trajectory.ts` lines 1-10, 220-288: https://github.com/ruvnet/ruflo/blob/{sha}/v3/@claude-flow/cli/src/ruvector/router-trajectory.ts#L1-L10
- `v3/@claude-flow/cli/src/mcp-tools/hooks-tools.ts` lines 1861-2050 and 1304-1520: https://github.com/ruvnet/ruflo/blob/{sha}/v3/@claude-flow/cli/src/mcp-tools/hooks-tools.ts#L1861-L2050
- `v3/@claude-flow/cli/src/memory/memory-bridge.ts` lines 2572-2703: https://github.com/ruvnet/ruflo/blob/{sha}/v3/@claude-flow/cli/src/memory/memory-bridge.ts#L2572-L2703
- `v3/@claude-flow/cli/src/memory/intelligence.ts` lines 239-295, 1127-1195: https://github.com/ruvnet/ruflo/blob/{sha}/v3/@claude-flow/cli/src/memory/intelligence.ts#L239-L295
- `v3/@claude-flow/cli/src/services/learned-routing.ts` lines 45-122: https://github.com/ruvnet/ruflo/blob/{sha}/v3/@claude-flow/cli/src/services/learned-routing.ts#L45-L122
- `plugins/ruflo-agentdb/hooks/register.ts` lines 176-181 and `hooks/tools.ts` lines 17-23: https://github.com/ruvnet/ruflo/blob/{sha}/plugins/ruflo-agentdb/hooks/register.ts#L176-L181
- `plugins/ruflo-agentdb/hooks/recall.ts` lines 99-130: https://github.com/ruvnet/ruflo/blob/{sha}/plugins/ruflo-agentdb/hooks/recall.ts#L99-L130
- `plugins/ruflo-ruvector/hooks/register.ts` lines 37-78 and `hooks/tools.ts` lines 13-31: https://github.com/ruvnet/ruflo/blob/{sha}/plugins/ruflo-ruvector/hooks/register.ts#L37-L78
