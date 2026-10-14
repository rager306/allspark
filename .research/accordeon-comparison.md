# Accordeon vs Prime Agent context-management patterns

Scope: source-only, read-only comparison of `/root/Accordeon` and `/root/prime-agent`. I did not run builds/tests and did not edit Accordeon. Accordeon is dirty (`main...origin/main [ahead 78]` plus modified/untracked files), so all Accordeon evidence below is from the current working tree. MCP schema discovery was done before calls. GitNexus `gsd-context` was used only as leads and then verified in source; codebase-memory `prime-agent` was used as leads and then verified in source.

## Executive conclusion

Prime Agent already has strong equivalents for **lossy compaction, retained recent tail, durable JSONL history, restart-safe session hydration, persistent Python REPL state, tool-output spill, harness-memory ranking, and branch/session summaries**.

Prime Agent does **not** already have an equivalent of Accordeon's core pattern: **per-block, model-visible, reversible folding with `{#code FOLDED}` handles plus agent `recall`/`unfold` over the folded original**. Prime keeps originals in JSONL, but the running model context after compaction gets a summary and retained suffix, not a set of exact recall handles for folded blocks.

So the real delta is narrow:

- not "add memory/summarization" — Prime already has these;
- not "persist history" — Prime already does;
- yes "exact, per-section reversible recall inside the model context" — currently absent/partial;
- yes "automatic selective folding/ranking of older transcript blocks before each provider call" — currently absent;
- yes "provider-safe fold/unfold command vocabulary with tool-pair/cache invariants" — currently only partially covered by Prime's compaction boundaries and tool-output spill.

## Pattern comparison table

| Pattern / question | Accordeon current source | Prime Agent current source | Status in Prime | Real delta |
|---|---|---|---|---|
| Reversible fold/recall/unfold | Folded blocks carry deterministic `{#<code> FOLDED}` tags; agent gets `unfold` and `recall` tools. `recall` returns original text as a tool result without changing standing context; `unfold` schedules the block to be full next turn. Evidence: `digest.ts` describes the tag and deterministic code; `unfold-recall.ts` stores `lastBlocks` and returns `block.text`; `index.ts` registers both tools. | Compaction creates one `CompactionSummaryMessage` plus retained messages. Original rows remain in append-only JSONL and can be hydrated, but the model-visible summary has no per-block handles. Evidence: `build_session_context` inserts `CompactionSummary` then retained messages; `ensure_full_history` reloads full JSONL. | **Partial / absent for exact context recall** | Prime preserves originals on disk, but does not expose a first-class per-fold `recall/unfold` handle in model context. |
| Protected recent tail | `messagesToPipelineInput` computes `protectedFromIndex`; pipeline refuses protected folds and heals protected blocks; default protect-tokens is 20k. Evidence: `conductor-bridge.ts`, `conductor-pipeline.ts`, `index.ts`. | `keep_recent_tokens` default is 20k; `find_cut_point` walks backward to keep that suffix; compaction summaries include a recent-state anchor from the kept tail. Evidence: `compaction.rs`, `prepare.rs`, `recent_state_anchor.rs`. | **Already present, different application** | Prime protects a suffix by compacting the prefix wholesale. Accordeon protects a suffix while selectively folding older blocks. |
| Budget / automatic pressure handling | Conductor view has `budget`, `contextWindow`, `liveTokens`; cold-score folds candidates until `live <= budget`. Evidence: `ConductorView`, `ColdScoreConductor.conduct`. | Auto-compaction threshold exists: 0.95 context-window ratio, reserve tokens 16,384, keep-recent 20k; `compact.status/run` and turn-boundary scheduling are wired. Evidence: `CompactionSettings`, `COMPACT_THRESHOLD_RATIO`, `turn_boundary.rs`. | **Already present for compaction; absent for selective folding** | Prime decides *when to compact*, not *which older blocks to fold individually*. |
| Ranking / cold score | ACT-R style cold-score ranks by creation/recall events, kind priors, tail call-id warmth, lexical pre-unfold and cooldowns. Evidence: `cold-score.ts`, `score.ts`. | Harness digest ranks *continual harness entries* using recent messages and goal terms; it is not transcript-block ranking. Evidence: `harness_digest.rs` builds terms from goal + recent texts; `ranking.rs` scores harness entries. | **Partial but different domain** | Prime has relevance ranking for memories/skills, not for conversation block folding. |
| Conductor pipeline / command vocabulary | `runConductorPipeline(input) -> output` builds a read-only view, runs a conductor, clamps invalid commands, emits `foldOps`/`groupOps`, and preserves `lastCmds` if a conductor returns null. Evidence: `conductor-pipeline.ts`; `conductor.ts` contract. | No matching provider-call input-shaping pipeline for transcript blocks found. Existing compaction is a session operation; harness digest is a cold-boundary prompt row; REPL persistence is separate. | **Absent** | If Prime needs Accordeon's behavior, the missing pattern is not a name; it is a provider-input transformation layer with stable command semantics. |
| Summarization | Mostly structured digests and optional summaries; some group/Keel paths can be non-recoverable by current vocabulary. Accordeon README positions `/compact` as lossy and Accordeon folds as reversible. | Native compaction summary, iterative update over previous summary, branch summaries, harness-digest snapshots. Summary prompt records kernel-persistence note. Evidence: `compaction_exec.rs`, `messages.rs`, `branch_summarization.rs`. | **Already present** | Do not duplicate summarization. The missing part is exact recall of summarized/folded source, not summary generation. |
| Persistence / restart | Current live extension caches `lastBlocks`, `pendingUnfold`, and conductor instance memory in process; trace JSONL persists instrumentation, not fold state. The original source messages appear to remain host-owned because context hook receives a deep-copy and returns folded messages only for the LLM call. Evidence: `index.ts`, `unfold-recall.ts`, `trace-writer.ts`. | Strong persistence: append-only session JSONL, `CompactionEntry` with `first_kept_entry_id`, windowed reads, `ensure_full_history`, daemon session store, and REPL snapshot/restore with 16 MiB per-variable cap. Evidence: `session.rs`, `session/window.rs`, `repl.py`. | **Already present in Prime, stronger than Accordeon for process restart** | Prime's persisted originals survive. Accordeon's current fold handles/process state are not themselves restart-persistent in source inspected. |
| Provider tool-pair safety | Folding only `text`, `thinking`, `tool_result`; `tool_call` never folds. Group collapse re-derives whole-message and balanced tool-call/result guards. Evidence: `digest.ts`, `mapping.ts`, `conductor.ts`. | Compaction cut points never choose `ToolResult`; split-turn summaries keep suffix; converted context uses a user compaction-summary row before retained provider messages. Evidence: `find_valid_cut_points`, `build_session_context`, `messages.rs`. | **Partial** | Prime avoids invalid compaction cuts, but it does not have selective fold/unfold operations that need Accordeon's stronger pair-balancing semantics. |
| Prompt-cache stability | `cold-epoch` holds a stable fold set between epochs to keep the folded prefix byte-stable and cache-warm. Evidence: `cold-epoch.ts`. | Prime has usage accounting for cache read/write and uses an auxiliary model for summarizer calls so one-off summary prompts stay off the session model's prompt-cache prefix, but no byte-stable fold-set policy for transcript blocks was found. Evidence: `compaction_exec.rs`, `engine.rs`. | **Partial** | Prime compaction is infrequent but summary rows change at compaction. No equivalent epoch fold-set stability policy. |
| Tool full-output exact recall | Not the main Accordeon mechanism. | Bash/tool output has bounded context tail plus temp-file spill path; full output path is recorded in details/message when truncated. Evidence: `OutputAccumulator`, `BashExecutionMessage`, `bash.rs`. | **Already present for tool output only** | This solves exact recall for long tool output, not for arbitrary folded conversation blocks. |

## Evidence notes by mechanism

### Accordeon: reversible fold/recall/unfold is real and model-facing

- Fold marker and stable code: `extension/gsd-context/src/core/engine/digest.ts:1-79` says every digest carries `{#<code> FOLDED}`, code is derived from durable block id, and the agent uses the code with `unfold`.
- Agent recall/unfold implementation: `extension/gsd-context/src/unfold-recall.ts:14-84` has module-level `pendingUnfold`, `lastBlocks`, `resolveCodes`, `unfoldExecute`, and `recallExecute`; recall returns `--- #code ---\n{block.text}`.
- Tool registration and user-facing semantics: `extension/gsd-context/src/index.ts:106-148` registers `unfold` and `recall`; descriptions explicitly say original content is preserved, `unfold` affects next turn, `recall` is immediate and non-mutating.
- Scope: `index.ts:70-91` hooks `pi.on("context")`, converts messages to pipeline input, runs `runConductorPipeline`, filters pending unfolds, returns folded messages, and caches `lastBlocks`.

Prime equivalent check:

- `crates/pa-core/src/session/mod.rs:424-463` inserts a `CompactionSummary` then retained messages after the latest compaction. It does not insert handles for each removed block.
- `crates/pa-core/src/session/window.rs:865-879` can hydrate full JSONL into `entries`, so originals survive outside model context.
- No source lead showed a `recall/unfold` tool over compacted history. Available recall-like tools are harness/memory/session/file access, not exact folded block handles.

### Protected tail exists in both, but with different semantics

Accordeon:

- `ConductorView` exposes `protectedFromIndex` and `protectTokens`: `extension/gsd-context/src/core/contract/conductor.ts:74-97`.
- The pipeline marks blocks protected and blocks fold/group commands that reach protected blocks: `conductor-pipeline.ts:78-94`, `134-163`, `403-421`.
- Defaults are `DEFAULT_PROTECT_TOKENS = 20_000`, `DEFAULT_BUDGET = 120_000`: `index.ts:31-35`.

Prime:

- Defaults: `DEFAULT_KEEP_RECENT_TOKENS = 20_000`, `DEFAULT_RESERVE_TOKENS = 16_384`, `COMPACT_THRESHOLD_RATIO = 0.95`: `crates/pa-core/src/session_engine/compaction.rs:8-17`.
- Cut logic keeps approximately `keep_recent_tokens` by walking backward from the end: `compaction.rs:361-431`.
- Recent-state anchor is the newest retained assistant text, max 2,000 chars, added to the summarizer prompt so summary cannot override retained tail: `recent_state_anchor.rs:1-46`, `compaction_exec.rs:40-79`.

Delta: Prime protects the tail from compaction. Accordeon protects it from selective folding and uses it as relevance signal.

### Ranking exists in Prime, but not for transcript folding

Accordeon cold-score:

- `ColdScoreConductor.conduct` prunes stale state, skips under-budget, builds candidates excluding held/protected/grouped/non-foldable blocks, sorts by cold score, lexically pre-unfolds tail-referenced blocks, applies cooldowns, then emits fold ids: `cold-score.ts:89-213`.
- `activation` and `coldScore` implement ACT-R style decay plus kind priors and tail call-id warmth: `score.ts:80-139`.

Prime ranking:

- Harness digest terms come from goal plus last four user/assistant texts: `harness_digest.rs:476-490`, `560-592`.
- Entry scoring is title/content/id overlap with IDF, used to rank harness entries in prompt render: `ranking.rs:139-260`.

Delta: Prime ranking is for persistent harness entries, not for deciding which transcript blocks remain full in the provider prompt.

### Provider safety and tool pairing

Accordeon:

- Foldable kinds are only `text`, `thinking`, `tool_result`; `tool_call` and `user` do not get fold handles: `digest.ts:34-66`.
- `applyPlan` refuses non-durable ids, folds text/thinking/tool_result in place, never folds tool_call, and group collapse requires whole/durable messages plus balanced call/result closure: `mapping.ts:247-360`.
- Contract clamp reasons include `protected`, `not-foldable`, `human-override`, `grouped`: `conductor.ts:193-228`.

Prime:

- Compaction cut points exclude `ToolResult`: `compaction.rs:297-328`.
- If a cut splits a turn, Prime summarizes the prefix separately and keeps the suffix: `compact_session.rs:169-183`, `compaction_exec.rs:89-95`.
- Model context after compaction is a user compaction-summary message plus retained messages: `messages.rs:353-369` and `session/mod.rs:452-463`.

Delta: Prime has provider-valid compaction boundaries. It does not have an in-place fold vocabulary, so it lacks the exact pair-preserving fold/unfold problem Accordeon solves.

### Persistence and restart

Prime already has the stronger persistence base:

- Session file format is append-only JSONL: `crates/pa-types/src/session.rs:1-2`.
- Compaction persists a `CompactionEntry` with `summary`, `first_kept_entry_id`, `tokens_before`, optional `usage`, and harness digest snapshot: `session.rs:187-209`.
- Windowed store can operate on retained suffix and later `ensure_full_history` by reading the entire file: `session/window.rs:858-879`.
- REPL state snapshots pickle variable records with 256 MiB aggregate and 16 MiB per-variable default caps; oversized variables can be skipped/pruned and restore reports per-name failures: `prime-agent-runtime/src/rlm/repl.py:38-45`, `771-920`, `1095-1208`.

Accordeon current source:

- `lastBlocks` and `pendingUnfold` are module-level state: `unfold-recall.ts:14-28`.
- `ColdScoreConductor` stores recalls/cooldowns in instance fields: `cold-score.ts:31-79`.
- `trace-writer.ts` persists instrumentation JSONL, not fold state/original blocks.

Delta: Prime can survive process restart with original session rows and Python namespace snapshots. Accordeon's current fold handles depend on process-local state until a context pass rehydrates `lastBlocks` from host messages; hysteresis/epoch memory is not obviously durable.

## Answer to the semantic questions

| Semantic question | Accordeon | Prime Agent |
|---|---|---|
| Is the original preserved? | For recoverable folds, yes: original `Block.text` remains in `lastBlocks` from host messages; fold output is substitution for the LLM call. For some group/Keel paths, source comments admit non-recoverable group summaries. | Yes on disk: append-only JSONL rows remain. No in live model context after compaction: older content is replaced by summary. Long bash output original may be in a spill file. |
| Who/how can recall? | Agent can call `recall(codes)` immediately or `unfold(codes)` for next turn; human-facing peek/pin is described in docs but source evidence here focused on agent tools. | Agent can use file/session tools or read paths if it knows them; harness entries can be searched; no first-class exact `recall` for compacted transcript blocks. |
| Is folding automatic and selective? | Yes: conductors run on every context hook and emit specific fold/group ops. | Auto-compaction exists, but it is prefix compaction, not selective per-block folding. |
| Survives process restart? | Original host session likely survives if GSD stores full messages; fold caches/hysteresis/pending unfolds are in process memory in inspected source. | Yes for session JSONL, compaction rows, harness state, branch summaries, and REPL snapshot/restore. |
| Preserves provider tool pairing/cache? | Explicitly engineered: no tool_call folding, durable ids, balanced group removal, cold-epoch stable fold set. | Provider validity is protected at compaction boundaries; no fold-set cache policy. Usage records cache metrics. |
| Applies which context? | Provider input before each LLM call (`pi.on("context")`) over conversation messages. | Compaction rewrites session context after turn boundary; harness digest injects persistent memory at cold boundaries; REPL snapshot is Python state; output spill affects tool-output context. |

## Risks if trying to add the missing delta to Prime

1. **Duplicate-summary risk.** Prime already has compaction, branch summaries, harness digests, and session JSONL. Adding another summarization layer would duplicate existing behavior without providing Accordeon's unique value.
2. **False reversibility risk.** Prime's summaries are intentionally lossy. If a UI or prompt labels them as recallable without a persisted block-id -> original mapping, it would create dangling handles.
3. **Provider validity risk.** Selective folding must not orphan tool calls/results or alter provider-required message shape. Accordeon's durable-id and pair-balancing rules are the relevant pattern; Prime's compaction cut safety is not sufficient for in-place folds.
4. **Prompt-cache regression risk.** A fold plan that changes the prompt prefix every turn can reduce cache hits. Accordeon's `cold-epoch` exists specifically to hold a stable fold set between epochs.
5. **Restart mismatch risk.** Prime's daemon/session model expects restart-safe state. Any fold state or recall map would need durable JSONL/artifact representation, unlike Accordeon's current module-level caches.
6. **Access-control/secret risk.** Exact recall over old tool outputs or files may expose content that compaction currently hides. Prime's existing tool spill and session JSONL already carry full data, but a model-facing recall API would make access easier and should follow existing session/tool boundaries.

## Validation experiments to run later (not run here)

1. **Prime compaction exactness fixture.** Create a tiny JSONL fixture with a unique old token, compact it, then assert: model context contains only compaction summary + retained tail, `ensure_full_history` can still hydrate the original row, and no model-facing recall handle exists.
2. **Provider-pair safety fixture.** Put an assistant tool_call and tool_result around the compaction cut. Assert `find_valid_cut_points` never starts on `ToolResult`, split-turn summary keeps provider messages valid, and converted LLM context has no orphaned tool result.
3. **Restart hydration fixture.** Compact a session, restart/reopen via windowed store, and assert `CompactionEntry.first_kept_entry_id`, summary, retained messages, and full-history hydration survive.
4. **REPL persistence fixture.** Save variables below and above 16 MiB, restart the kernel, and assert small variables revive while oversized ones are skipped/pruned with manifest evidence.
5. **Tool-output spill durability fixture.** Produce output over the truncation limit. Assert model context gets tail + `fullOutputPath`, file content is exact, and classify whether the path survives daemon/process restart on the current platform.
6. **If selective folding is considered:** build a provider-message fixture with text/thinking/tool_call/tool_result/user blocks and verify any fold plan preserves tool pairing, stable cache prefix across no-pressure turns, and durable recall handles after restart.

## Bottom line

Prime already covers the durable-history and summarization half of Accordeon's story. The only non-equivalent Accordeon pattern is **selective, reversible, per-block context folding with exact agent recall/unfold handles and conductor ranking before provider calls**. Treat that as the delta; do not reimplement compaction, session history, harness memory, or tool-output spill as a separate system.
