# ruVector / ruFlo: паттерны для Prime Agent

Дата: 2026-10-12. Режим: read-only research, без установки и запуска чужого кода. Репозиторий Prime Agent не менялся, секреты не читались.

## Canonical источники

- **RuVector**: `ruvnet/RuVector`, default branch `main`, GitHub API HEAD `{ruv_commit}`, MIT. Локальный `/root/vendor-source/ruvector` существует, но его HEAD `c6bb23c842d7c220e06e6141d5e55ae52077fe12`; для canonical-ссылок ниже использую GitHub HEAD `{ruv_commit}`.
- **RuFlo / Claude-flow**: `ruvnet/ruflo`, GitHub `ruvnet/claude-flow` редиректит в тот же repo (`full_name: ruvnet/ruflo`), default branch `main`, HEAD `{ruflo_commit}`, MIT. Root `package.json` публикует package name `claude-flow`; README называет проект “Ruflo — agent meta-harness for Claude Code and Codex”.
- MCP: `codebase-memory-mcp` подтвердил индекс `root-vendor-source-ruvector` (352k nodes) и `prime-agent`; `arsumbris` использован как lead по architecture notes; `gitnexus` не содержит RuVector/RuFlo, но это зафиксировано как ограничение.

## Что реально видно в source, а что является claim

### RuVector

Реальные механизмы:
- `VectorDB` как связка storage + vector index abstraction (`VectorIndex` с `add/search/remove/len`). Evidence: `crates/ruvector-core/src/vector_db.rs`, `crates/ruvector-core/src/index.rs`.
- Embedding provenance / vector-space identity: `EmbeddingSpaceIdentity` фиксирует provider/model/tokenizer/pooling/dim/dtype/role policy. Evidence: `crates/ruvector-core/src/embeddings.rs`.
- Agent-memory lifecycle: coherence compaction, TARL ledger states, proof gate, causal fusion, arbitration. Evidence: `crates/ruvector-agent-memory/src/lib.rs`; tests `tests/tarl_ledger.rs`, `tests/arbitration.rs`.
- Hook-level learning patterns: route/learn/suggest/error/file-sequence/test suggestions. Evidence: `crates/ruvector-cli/src/cli/hooks.rs`.

Claims / caution:
- README-level speed claims are not enough for Prime planning. В canonical source `AgentDBBackend`-подобные claims надо проверять на actual implementation.
- RuVector CLI hook memory uses a 64-dim hash embedding (`fn embed`), so this specific implementation is not a semantic-memory solution.
- RuVector HEAD source file named `HnswIndex` wraps `FlatGraph` in fetched canonical commit; do not promise HNSW speedups without benchmark in our target workload.

### RuFlo / Claude-flow

Реальные механизмы:
- V3 exposes a modular surface: Agent/Task/Memory, `SwarmCoordinator`, `HybridBackend`, MCP tools. Evidence: `v3/src/index.ts`.
- `SwarmCoordinator` implements agent spawn/list/terminate, task distribution by capability and current load, structured failed task result, metrics, concurrent task execution. Evidence: `v3/src/coordination/application/SwarmCoordinator.ts`.
- `plugins/ruflo-swarm` has a UI/action controller that runs fixed CLI argv, verifies some actions against disk state, handles policy-denied process runs, and tests hostile task text as one argv element. Evidence: `plugins/ruflo-swarm/hooks/actions/controller.ts`, `tests/controller.test.ts`.
- `plugins/ruflo-rag-memory` has a tighten-only guard that screens only plugin write tools for secrets, never echoes secret value, exposes local status/scan commands. Evidence: `plugins/ruflo-rag-memory/hooks/register.ts`, `hooks/screen.ts`, `tests/rag.test.ts`.
- `plugins/ruflo-core` hook shim is explicitly regression-tested against real hook invocation shape on Windows/macOS/Linux and Codex/Claude differences. Evidence: `plugins/ruflo-core/scripts/ruflo-hook.cjs`, `scripts/test-hooks.mjs`.

Claims / caution:
- `v3/src/memory/infrastructure/AgentDBBackend.ts` says HNSW/AgentDB, but implementation at `{ruflo_commit}` is in-memory `Map` plus linear cosine similarity. Treat “150x–12,500x” in README/comments as marketing unless measured separately.
- `SwarmCoordinator.reachConsensus` currently simulates votes with `Math.random()`. It is not evidence for a reliable consensus protocol.
- RuFlo has many plugins/agents/commands; the useful unit for Prime is the pattern, not importing the ecosystem.

---

# Паттерны с наибольшим потенциальным эффектом для Prime Agent

## 1) Evidence-backed memory admission, not “write every lesson”

**Pattern**: Memory entries are not just text blobs. They have admission state, provenance, evidence grade, and can be superseded/rejected.

**Mechanism from sources**:
- RuVector `ruvector-agent-memory` documents and exports a TARL-like ledger with operations over `Pending/Accepted/Rejected`, proof-gated acceptance, witness records, revision/supersession, and replay.
- Tests verify state transitions (`five_op_state_machine_transitions`) and poisoning containment / gate-denied acceptance in `tests/tarl_ledger.rs`.
- Arbitration tests verify that ten memories from one source count as one evidence lineage, not ten independent confirmations.

**Concrete Prime problem**:
- Prime already has continual harness memory/refine/rollback. The weak point is not storage. It is *admission quality*: global memories can become stale, over-general, or based on one lucky/failed run. Auto-refine can turn noisy events into persistent policy.

**Expected benefit hypothesis**:
- Fewer harmful global memories and fewer repeated corrections by the user.
- Better distinction between “observed once”, “validated”, “superseded”, and “user-approved preference”.
- Taste model (“личные предпочтения пользователя”) can become safer: preferences should be accepted only after explicit user signal or repeated consistent evidence, not inferred from one session.

**Failure modes**:
- Too much bureaucracy: useful memories stay pending and are never used.
- False confidence: a ledger can make low-quality evidence look official.
- Selection bias: only visible successes/failures are recorded; missing counterexamples bias the taste model.
- User privacy: evidence should cite session/path/category without storing secrets or prompt content unnecessarily.

**Validation experiment**:
1. Add a local-only prototype metadata layer for harness entries: `candidate | accepted | superseded | rejected`, `evidence_count`, `source_sessions`, `validated_by`.
2. Run on replayed Prime sessions: compare current harness retrieval vs gated retrieval.
3. Metrics: number of user corrections to memories; number of rollback/refine reversions; precision@k of retrieved harness entries judged by a reviewer; “stale memory used” incidents.
4. Red-team: seed contradictory memories and verify accepted/superseded behavior.

**Why this fits Prime**:
- Prime already has refinement history and rollback. This pattern extends that mechanism; it does not replace it.

---

## 2) Semantic recall with vector-space identity, but only as an advisory retrieval layer

**Pattern**: Retrieval quality depends on stable embedding identity and mixed-mode search, not merely “add vector DB”.

**Mechanism from sources**:
- RuVector has `EmbeddingSpaceIdentity`: provider/model artifact hashes, tokenizer, prompt template, pooling, normalization, dimensions, dtype, distance metric, role/prefix policy.
- RuVector separates `VectorIndex` from storage and has `VectorDB` selecting an index backend.
- RuFlo V3 “HybridBackend” pattern stores structured memory in SQLite and vector memory separately, then combines filter + vector results. But its fetched AgentDB backend is currently in-memory linear cosine, so use the architecture pattern, not the speed claim.

**Concrete Prime problem**:
- Prime’s harness digest/search is good for explicit terms and freshness. It can miss semantically related memories/skills/subagent specs when wording differs.
- Long-running work creates many session artifacts; term search alone encourages duplicated memories and missed prior lessons.

**Expected benefit hypothesis**:
- Better recall of relevant harness entries and past session summaries when user phrasing changes.
- Better subagent/template reuse without making the prompt huge.
- Rust tools framing: a fast bounded `retrieve_context(query, budget, filters)` interface can return cited snippets, not unbounded prompt stuffing.

**Failure modes**:
- Vector-space drift: changing model/dim/pooling makes old embeddings invalid.
- Latency on startup or UI path. Project rule says paint path must not await network/disk-heavy work.
- Bad embeddings overfit to generic similarity and retrieve plausible but wrong memories.
- Privacy: embeddings of sensitive text may still be sensitive.

**Validation experiment**:
1. Offline build embeddings only for existing local harness entries and compacted session summaries. Keep term ranking as baseline.
2. Use a fixed embedding-space identity; refuse mixed dimensions/model ids.
3. Evaluation set: 50–100 real user queries from sessions, manually label relevant memories/skills. Compare BM25/term-only vs hybrid recall@5, MRR, and token cost.
4. Runtime metrics: retrieval latency p50/p95, prompt token reduction, cases where semantic result is used and later contradicted.
5. UX guard: retrieval is advisory; final prompt includes citations and can fall back to current ranking.

**Why this fits Prime**:
- Prime already has Python REPL, MCP, harness CRUD, and session artifacts. It only needs a bounded observation tool, not a full DB replacement.

---

## 3) Outcome-aware advisor for routing decisions, with cold-start humility

**Pattern**: A small advisor observes outcomes and recommends action choices; it does not silently control the agent.

**Mechanism from sources**:
- RuVector hooks include `learn/suggest/route`, error pattern recording, file-sequence prediction, and “should test” suggestions.
- RuFlo swarm UI asks a router for selected task and shows confidence/threshold; tests show “below threshold” behavior rather than blindly trusting the router.
- RuFlo controller verifies actions against disk state where possible, and reports “exited 0 but disk does not show it”.

**Concrete Prime problem**:
- Prime already has subagents, model failover, compaction, tools, and verifiers. The hard problem is deciding *when* to consult which component: spawn subagent or inline? run focused test now or later? compact now? ask external MCP? choose high/low reasoning?
- Today many of these choices rely on prompt policy and model judgment, which varies by session.

**Expected benefit hypothesis**:
- The advisor can reduce repeated orchestration mistakes after enough data: e.g. “for Rust provider changes, run pa-ai focused tests before broad clippy”; “for architecture research, spawn source-checker + competitor-checker”; “for low-confidence claims, consult independent subagent”.
- Taste model can learn user preferences: concise Russian summaries, no overclaiming speedups, source-backed claims, do not add WorkBuddy as provider. These are not code-porting tasks; they are preference/routing signals.

**Failure modes**:
- Cold start: before enough outcomes, advice is noise.
- Selection bias: only actions the agent chose are observed; unchosen alternatives lack counterfactuals.
- Reward hacking: short-term “tests passed” may not equal user satisfaction.
- Over-automation: advisor starts overriding explicit user/system rules.

**Validation experiment**:
1. Start with passive logging only: state features, candidate actions, selected action, observable outcome, later user correction if any.
2. Require minimum support before advice: e.g. at least N comparable cases and confidence interval above baseline.
3. A/B in replay harness: current policy vs advisor-ranked suggestions. Human reviewer scores “would this have helped?”
4. Metrics: fewer failed commands per task, fewer unnecessary subagents, lower time-to-first-useful-result, user correction rate, and no increase in policy violations.
5. Keep “advisor” selective: recommendation + reason + confidence + evidence count; model may ignore it.

**Why this fits Prime**:
- Prime has rich traces, subagents, and MCP. A bounded Rust-side advisor can expose reliable observations/actions independent of Python model code, while Python REPL remains orchestration layer.

---

## 4) Action verification after tool/CLI success, especially for multi-agent state

**Pattern**: Exit code is not enough. After an action claims success, verify the external state that matters.

**Mechanism from sources**:
- RuFlo swarm controller test: a CLI prints “[OK] Vote recorded”, but if disk state does not show the vote, UI reports “exited 0 but the disk does not show it”.
- Controller actions use fixed argv and verify task claim movement on disk after a “steal” action.
- This is stronger evidence than README claims because tests exercise hostile text, process policy denial, and disk-backed verification.

**Concrete Prime problem**:
- Prime runs many shell/MCP/provider actions. Tool success can mean “process exited 0”, not “desired state changed”. Examples: PR created? branch pushed? session child delivered result? memory actually updated? provider failover took effect?
- Multi-agent work especially risks stale state: parent assumes child did something because a command said OK.

**Expected benefit hypothesis**:
- Fewer false-positive completions.
- Better parent/subagent coordination because state transitions become observable facts.
- More trustworthy final answers: “done” only after verifier sees expected state.

**Failure modes**:
- Extra verification cost and latency.
- Verifier can check the wrong state or become flaky.
- For external services, verification may need network and should not block UI critical path.

**Validation experiment**:
1. Pick 3 Prime actions with common false positives: git push/PR, harness memory update, child-result collection.
2. Add explicit verifier functions that read state after action.
3. Tests: mock action returns success without state change; verifier must fail red-first.
4. Metrics: detected false positives, extra latency, user-visible “claimed done but not done” incidents.

**Why this fits Prime**:
- Prime already values verifiers and red-first tests. This pattern generalizes it to orchestration actions, not just code tests.

---

## 5) Tighten-only safety mods for MCP/tool writes

**Pattern**: A local guard may refuse unsafe writes, but never grants new permission and never leaks the secret it detected.

**Mechanism from sources**:
- RuFlo `ruflo-rag-memory` guard watches plugin write tools, screens nested input for secret patterns, refuses writes, records status, and never echoes token values.
- Tests prove secret is blocked for write/import tools, reader tools pass, nested secret is found, guard-off option works, and `/rag-mod scan` reports secret kind without the value.
- `ruflo-deepseek-harness` README describes the same “tighten-only” pattern for prompts sent to DeepSeek tools.

**Concrete Prime problem**:
- Prime has MCP and skills that may write memory, files, or send content to external providers. A model may accidentally store secrets in memory or send credentials to an external tool.
- Existing general rules say do not read secrets, but runtime guards catch mistakes at the boundary.

**Expected benefit hypothesis**:
- Reduced chance of secrets entering persistent harness memory or third-party MCP calls.
- More confidence in memory/retrieval features because write paths have local filters.

**Failure modes**:
- False positives block legitimate text.
- Regex-only secret detection misses novel secrets.
- Guard status/logs can themselves leak if they include values. Must report kind only.

**Validation experiment**:
1. Add a tighten-only guard around harness memory create/update and selected MCP write calls.
2. Tests with common secret formats and nested JSON; assert blocked message does not contain matched value.
3. Measure false-positive rate on historical non-secret memory entries.
4. Ensure guard never changes deny->allow; it can only allow-through or deny.

**Why this fits Prime**:
- Prime already has MCP/skills boundaries. Guard belongs at those boundaries, not in model prose.

---

# Ranking for Prime Agent

1. **Evidence-backed memory admission** — highest conceptual value for continual harness quality and taste model safety.
2. **Semantic recall with embedding identity** — high value if implemented as advisory hybrid retrieval and measured against term baseline.
3. **Outcome-aware advisor** — promising, but must start passive because cold start and selection bias are real.
4. **Action verification after success** — medium/high value and low conceptual risk; especially useful for multi-agent and git/harness operations.
5. **Tighten-only safety guards** — targeted safety win for memory/MCP writes.

# Explicit non-recommendations

- Do not import RuFlo’s swarm wholesale. Prime already has recursive subagents and family messaging; a second queen/swarm abstraction would duplicate responsibilities.
- Do not promise HNSW/AgentDB speedups from RuFlo V3 source: fetched `AgentDBBackend` is in-memory linear cosine at the pinned commit.
- Do not copy RuVector CLI hook memory as-is: its embedding is 64-dim hash-based.
- Do not make outcome learning authoritative until there is an outcomes baseline and counterfactual-aware evaluation.

# Source links

- RuVector repo: https://github.com/ruvnet/RuVector/tree/{ruv_commit}
- RuVector `VectorDB`: https://github.com/ruvnet/RuVector/blob/{ruv_commit}/crates/ruvector-core/src/vector_db.rs
- RuVector `EmbeddingSpaceIdentity`: https://github.com/ruvnet/RuVector/blob/{ruv_commit}/crates/ruvector-core/src/embeddings.rs
- RuVector agent memory: https://github.com/ruvnet/RuVector/blob/{ruv_commit}/crates/ruvector-agent-memory/src/lib.rs
- RuVector TARL tests: https://github.com/ruvnet/RuVector/blob/{ruv_commit}/crates/ruvector-agent-memory/tests/tarl_ledger.rs
- RuVector arbitration tests: https://github.com/ruvnet/RuVector/blob/{ruv_commit}/crates/ruvector-agent-memory/tests/arbitration.rs
- RuVector hooks: https://github.com/ruvnet/RuVector/blob/{ruv_commit}/crates/ruvector-cli/src/cli/hooks.rs
- RuFlo repo: https://github.com/ruvnet/ruflo/tree/{ruflo_commit}
- RuFlo root package: https://github.com/ruvnet/ruflo/blob/{ruflo_commit}/package.json
- RuFlo V3 index: https://github.com/ruvnet/ruflo/blob/{ruflo_commit}/v3/src/index.ts
- RuFlo HybridBackend: https://github.com/ruvnet/ruflo/blob/{ruflo_commit}/v3/src/memory/infrastructure/HybridBackend.ts
- RuFlo AgentDBBackend caution: https://github.com/ruvnet/ruflo/blob/{ruflo_commit}/v3/src/memory/infrastructure/AgentDBBackend.ts
- RuFlo SwarmCoordinator: https://github.com/ruvnet/ruflo/blob/{ruflo_commit}/v3/src/coordination/application/SwarmCoordinator.ts
- RuFlo swarm controller tests: https://github.com/ruvnet/ruflo/blob/{ruflo_commit}/plugins/ruflo-swarm/tests/controller.test.ts
- RuFlo RAG memory guard: https://github.com/ruvnet/ruflo/blob/{ruflo_commit}/plugins/ruflo-rag-memory/hooks/register.ts
- RuFlo secret screen: https://github.com/ruvnet/ruflo/blob/{ruflo_commit}/plugins/ruflo-rag-memory/hooks/screen.ts
- RuFlo RAG guard tests: https://github.com/ruvnet/ruflo/blob/{ruflo_commit}/plugins/ruflo-rag-memory/tests/rag.test.ts
- RuFlo hook shim tests: https://github.com/ruvnet/ruflo/blob/{ruflo_commit}/plugins/ruflo-core/scripts/test-hooks.mjs
