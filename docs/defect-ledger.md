# Defect and gap ledger (allspark)

A running record of defects, gaps, and misleading surfaces found while working
this fork, with the commit that closed each. A finding that is not written down
here is a finding that will be found again.

The point is the *class*: one instance is a bug, three of the same shape are a
process defect, and the process is what this file exists to fix.

## 2026-11-18 — session 1: project store and plan hardening

| # | Class | Finding | Closed by |
|---|---|---|---|
| 1 | documented, never wired | `ProjectRecorder::sync` did not exist, so an opted-in session wrote an event log and no `project.db` | `bf96d99d2` |
| 2 | quadratic cost | `sync` read the whole log per plan update: 16.1 ms at 100 events → 41.7 ms at 1 000, 8.34 s over 200 updates | `e39635d40` (now flat 13–14 ms) |
| 3 | derived state as source of truth | `load_plan` answered from the model, so a crash before `sync` lost the last step and a dropped model read as "no plan" | `be24e8218` |
| 4 | invariant bypass | the restore path built `PlanState` directly, so a stored plan could hold two `in_progress` items and duplicate ids | `963d868c9` |
| 5 | setting with no test of its wiring | `[project] enabled` was parsed and tested, but nothing proved a session actually received a recorder | `be0b7f0b1` |
| 6 | dead surface | `meta.rs` (776 lines) and `links.rs` (284) had zero external callers; `ref.added` has no producer | `cfd74e309` (−1155 lines) |
| 7 | false claim in a changelog | "a later tree reports a plan item's proof as stale" — no reader existed for `evidenceRevision` | `8f21bd29a` |
| 8 | write-only field | `/session` rendered the plan without its proof or staleness | `5def37937` |
| 9 | torn write | a plan snapshot was N appends, so a crash left a mix of two plans | `af8ec9571` |

## 2026-11-18 — session 2: paint path and prompt budget

| # | Class | Finding | Closed by |
|---|---|---|---|
| 10 | paint-path work | `/session` read the commit with `capture_git_context`: 3 `git` processes per render (7.69 ms) | `ccaacc454` (24.7 µs, ×311) |
| 11 | same defect, hotter site | the continuation boundary paid the same 7.69 ms per text-only turn | `3fbe880d5` |
| 12 | async worker blocked | `record_plan` + `sync` ran inline: 17.2 ms of file and SQLite work on the tool-call path | `86a0a9e01` |
| 13 | unbounded prompt input | the per-file cap allowed one 256 KiB context file per ancestor, with no total budget | `82b07c066` (512 KiB total) |
| 14 | misleading stub | a cut context file was truncated to the remaining 2 bytes, reporting itself as content | `82b07c066` (`MIN_PROJECT_CONTEXT_SLICE`) |
| 15 | load-correlated red | 3 abort/queue units failed while another cargo ran; 0 failed on the same tree alone | `e9f89306b` (classified) |

## 2026-11-18 — session 3: narrowing a fast path, and over-strict repair

| # | Class | Finding | Closed by |
|---|---|---|---|
| 16 | fast path answers fewer inputs | `read_head_commit` read only loose refs, so packed branches (the default branch, usually) silently disabled the stale check | `5d465492a` |
| 17 | same | it looked only at `cwd/.git`, so a session started in a subdirectory got `None` | `b519b5fd6` |
| 18 | hot path cost | `update_plan` stamped its revision with 3 `git` processes | `ac4d99639` |
| 19 | repeated identity work | `project_recorder()` resolved 3× per session build, 2 `git` processes each | `20407bdc4` (cached per cwd) |
| 20 | unexercised invariant | the sequence under a full replay had no test | `d944b28a7` |
| 21 | over-strict repair | one bad stored item discarded the whole plan; repairing it then exposed a second hole: all-dropped returned an EMPTY plan as "restored" | `984541830` |
| 22 | setting that cannot act | `jev.fallback = "llm"` parsed, changed nothing, and reported itself only in a log line | `33489a173` (refused at read) |
| 23 | async worker blocked, read side | `load_plan` opened SQLite and projected inside `build_session` | `d2cd7d260` |


## 2026-11-18 — session 4: two sessions, one project

| # | Class | Finding | Status |
|---|---|---|---|
| 24a | cross-session delete | `known_item_ids` read the WHOLE project log, so session B emitted `entity.removed` for session A's steps. Measured: `[upserted P1, upserted P2, upserted P1, removed P2]` | `05d65012b` (previous now means "mine") |
| 24b | shared-key collision | `entities.id` is the plan item id alone, so two sessions in one project share one row and the restored plan is whichever wrote last | `869790bdb` (rows keyed `session:item`; red-first 3 rows where 5 were written) |
| 25 | test that documents a bug | a first attempt to "prove" #24 asserted the collapsed result and passed, which would have frozen the defect as the expected behaviour | removed (no test is better than a test that pins a bug) |
| 26 | newest-writer restore | after the per-session key, `load_plan` returned the newest session's plan even for a session that had its own rows, so a restart lost the plan it wrote | `daf061e42` (own scope first; red-first restores `P9` instead of `P1,P2`) |
| 27 | lost wakeup (abort) | `Agent::abort()` with no registered run did nothing, so a stop arriving between the worker's busy flag and the run slot was dropped and the turn waited out the provider response (measured: 60.31 s on a 60 s held reply) | `1cfaf952e` (abort epoch read before the first await; deterministic window test red-first with `Elapsed`). The first attempt, a sticky `abort_pending` flag (`7804fc3a7`), was wrong and is reverted — see #29 |
| 28 | flake bookkeeping | `docs/flaky.md` recorded the kill timing failure as a flake; the measurement says otherwise (passes alone and under CPU load, and the elapsed equals the held reply) | `fa771986f` (reclassified as a fixed defect) |
| 29 | fix that leaks into the next operation | the sticky `abort_pending` flag fixed #27 but cancelled the turn after the abort, so `abort_and_send_queued`'s deliberate follow-up never started. Three full daemon runs, 1389/1 each, a different queue test every time, all green alone; 1390/0 with the flag's apply disabled | `1cfaf952e` (epoch scoped to a run that is starting, so nothing survives the turn) |
| 30 | derived file treated as canonical | `ReadModel::open` refused a model whose schema version was not this build's, and every caller reads a failed open as "no model", so an upgraded build silently stopped restoring a project plan — while the same build rebuilds a *deleted* model from the log. Two rules for one derived file, and the stricter one lost data | `d73b8ff2c` (drop every table from `sqlite_master` and rebuild; red-first by restoring the refusal) |
| 31 | derived state wedged by a guard | `read_from` refuses a checkpoint past the end (correct), `sync` propagated it, and `load_plan` swallows sync errors — so a replaced or truncated log froze the model at its old contents with no signal. Zeroing the offset would not have been enough: the applied-event ledger would skip the new history's reused ids | `b1a9a0442` (`ReadModel::reset` drops the ledger too; red-first shows the refusal message) |
| 32 | silent degradation on a read path | `load_plan` discarded a failed `sync` (`let _ = ...`), so every restore after that served a stale plan with no signal; the write side (`plan_tool`) already reported to stderr and the read side did not | `6c64c7a6d` (reports with the same prefix; the stale-answer contract is unchanged) |
| 33 | canonical record damaged by its own writer | `append_all` calls `write_all`, which stops midway on a full disk and leaves a prefix of a line; no reader parses it and every later append concatenates onto it, so one full disk made the canonical project log permanently unreadable. The session log already repairs this (`pa-core` `repair_jsonl_damage`), and it cannot be reused: `pa-core` depends on `pa-project` | `e0bbd1d59` (writer cuts an unterminated tail before appending; one byte in the common case, measured 17.84 ms vs 17.22 ms per record+sync) |
| 34 | rule documented, never enforced | `docs/flaky.md` opens with "A `#[ignore]` in the tree without a row here is the defect this ledger exists to prevent", and nothing checked it: 37 `#[ignore]` attributes in 25 files against 7 journal rows | `c5d1550eb` (contract test for the two checkable halves). Its first run found the journal naming a **renamed** test and a **reverted** commit |
| 35 | documented consumer that does not exist | `crates/pa-project/README.md` promised `pa-daemon` records "plan, evidence, and review events"; the only producer writes `plan_item` rows. The projector implements and tests evidence/relation/ref events with no writer behind them | `c41280206` (README names the gap). Adding the producer is the open step, and it is now the one place that says so |
| 36 | failed read cached as a fact | `known_item_ids` cached a failed log read as an empty set, and an empty set is exactly a first write — so one transient unreadable log made a session record no removals for the rest of its life | `9dab32c5e` (`Option` answer; only success fills the cache; red-first `[]` vs `["session-1:P2"]`) |
| 37 | wrong hypothesis, corrected by reading | the #36 slice opened on "a failed append poisons the cache"; `record_plan` fills the cache *after* `append_all` succeeds, so that path was already correct | recorded because it costs a slice when trusted: the fix is next to the guess, not at it |
| 38 | two owners of one value | `record_plan` wrote `sessionId` into the event payload while the envelope already carried `session_id` from the same source, and nothing read the payload copy | `57120b908` (deleted; the envelope is canonical) |
| 39 | public surface with no consumer | `pa-project` re-exported `ProjectionReport`, `READ_MODEL_VERSION` and `project` at the crate root; measured zero names outside the crate, and the only use of `READ_MODEL_VERSION` was one test inside it | `0c93281a6` (deleted; still reachable through the public modules, README states the API) |
| 40 | shared derived file refuses a second writer | `ReadModel::open` ran `PRAGMA journal_mode=WAL` and the schema DDL, both needing an exclusive lock, and SQLite refuses that lock immediately instead of through the busy handler — so two sessions in one project racing on `project.db` failed instantly (0.01 s against a 5 s timeout). Measured 1 failure in 6 runs, 3 in 6 with the replay path exercised | `486b1e27f` (pragma is best-effort with a contention `debug_assert`; deterministic 150 ms-hold test red-first) |
| 41 | my wrong diagnosis, corrected by context | I read the instant failure as "the timeout is set after the pragma" and reordered them; the timeout was never the mechanism. The step context (`open model ...`) found the real site | recorded because the reorder looked like a fix and cost a build cycle |
| 42 | harness loses the provider, reported as product failure | `scripts/qa/run_suite.py` pins `HOME` **and** `PRIME_AGENT_CODING_AGENT_DIR` into a throwaway sandbox. The second variable makes the product ignore `$HOME/.prime/agent/models.json` and fall back to the built-in catalog, so no custom provider exists and every `real-model` trial died with `Unknown provider "9router"` — reported as `no session transcript was written`, i.e. as a product failure. Measured both ways: with the variable, providers are `github-copilot/google-vertex/opencode`; without it, `9router/cline/...`. The key must also be planted in the sandbox copy (`models.json` carries `REPLACE_WITH_*`), which is why `--api-key` alone fixed the standalone call and not the trial | **open** — a local-run patch was proven (CT-001 3/3 files, answer `DONE`) and the runner change is deliberately NOT made, because the owner asked for live providers locally only, never in git CI |
| 43 | semantic tool choice fails on a live model | ST-002 (the corpus task for this class) run live on `9router/cmc/…flash`: the model made **4 `python_repl` calls and 0 `graph` calls** while answering a call-chain question, though `graph` is registered and works in that sandbox (proven: a direct request returned `12 node(s), 13 edges, 4 file(s)`). A second run answered the *wrong* order (`validate` last) — the trap the fixture exists for. The task was previously "validated by schema only" | **open** — the anti-pattern reproduces on a real model. The prompt does not steer toward `graph` for call-chain questions, which is exactly W7.2 |

Notes on #24, so the fix is not guessed at again:

- The log is fine: `O_APPEND` is atomic per `write()` call, measured with two
  processes appending 1 MB writes (60 lines, 0 torn).
- The collision is at the row key, not the write. A project is meant to hold one
  plan, and its identity (`project_id`) is deliberately shared across a
  repository's worktrees — which is exactly why two concurrent sessions can
  disagree about who owns the plan.
- The options were (a) key rows by session and derive the plan from the session
  that owns it, or (b) keep one plan per project and refuse a second writer. (a)
  was chosen: rows are keyed `session:item`, this session's own rows win, and a
  session with none adopts the newest plan (#24b, #26).
- The restore rule took two tries and the second one was found by the full suite,
  not by reasoning: "newest wins" loses a restarted session's own plan (#26), and
  a sticky "abort the next run" flag fixed a real window while cancelling the
  follow-up another command deliberately starts (#29).

## What the classes say

| Class | Count | The process fix |
|---|---|---|
| documented, never wired / setting with no consumer | 4 | grep every `pub` symbol and every setting field for a caller before calling a slice done |
| hot-path work added by a "harmless" change | 6 | when adding work to `/session`, a turn boundary or a tool call, measure it first |
| narrowing a fast path silently | 2 | a replacement reader must answer everywhere the original did; test the awkward inputs (packed ref, subdirectory) |
| false claim in a doc or changelog | 2 | a changelog sentence needs a test that fails if the behaviour goes away |
| over-strict repair | 1 | every repair needs a degenerate-case test ("what if nothing survives") |
| unexercised invariant | 1 | a claim of "red-first" requires actually inverting the fix |
| dead surface | 1 | `pub` with no external caller is a maintenance bill; delete or wire it |
| lost signal (abort/cancel dropped) | 1 | a cancel that can arrive before its subject exists must be remembered *by the subject*, and the remembering must expire with it (#27, #29) |
| fix that changes the next operation | 1 | prove the negative case in the same change: the operation after the one you fixed still runs (#29) |
| flake label hiding a cause | 1 | one failure is a flake; three different names in one family is a cause — classify by `elapsed` against the held operation, never retry to green (#28) |

## Still open, with the reason

| Gap | Why it is open |
|---|---|
| W4.3 residual ledger | no residual case exists that has nowhere to be written |
| W7.2 graph-first guidance | **measured, and the measurement is the blocker.** This session's log (`01a1d0d1…jsonl`, 11 228 top-level tool calls over six days): `python_repl` 10 768 (95.9%), `edit` 217, `lsp` 203, `graph` 39, `jev` 1. But `bash()` is called *inside* `python_repl`, so the 95.9% mixes shell work with code execution and the split cannot be recovered from the log. A guidance change based on these numbers would be a claim about a quantity nobody measured |
| W3.3 `code_ref` resolver | no producer writes `ref.added`; a resolver would check an empty set |
| `real-model` QA tasks (CT-001/CT-002) | **run live 2026-11-19, locally only** on `9router/cmc/deepseek/deepseek-v4.1-flash`: CT-001 produced the three files and answered `DONE`; CT-002 created nothing and answered with an analysis. The blocker was the runner (#42), not the product. Not wired into CI by request |
