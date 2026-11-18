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

Notes on #24, so the fix is not guessed at again:

- The log is fine: `O_APPEND` is atomic per `write()` call, measured with two
  processes appending 1 MB writes (60 lines, 0 torn).
- The collision is at the row key, not the write. A project is meant to hold one
  plan, and its identity (`project_id`) is deliberately shared across a
  repository's worktrees — which is exactly why two concurrent sessions can
  disagree about who owns the plan.
- The honest options are (a) key rows by session and derive the plan from the
  newest session's events, or (b) keep one plan per project and refuse a second
  writer. Both change the contract, so neither belongs in a "small fix" slice.

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

## Still open, with the reason

| Gap | Why it is open |
|---|---|
| W4.3 residual ledger | no residual case exists that has nowhere to be written |
| W7.2 graph-first guidance | needs a measured tool-call count per task; not started |
| W3.3 `code_ref` resolver | no producer writes `ref.added`; a resolver would check an empty set |
| `real-model` QA tasks (CT-001/CT-002) | no access to a paid provider; validated by schema only |
| abort/queue timing units | classified as load-correlated, not fixed; fixing needs a deadline measurement |
