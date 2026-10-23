<!-- FORK-ONLY: research notes for the allspark fork; not shipped -->
# .research INDEX — the reports that carry a decision

216 top-level `*.md`: 215 reports plus this index. With subdirectories, ~1,600 files and
611 MB — fixtures, captured payloads and logs. Most of it is evidence for a decision taken
elsewhere. This file indexes the reports that **decide** something — a mechanism the fork
adopted, rejected, or is blocked on.

**What a clone contains.** `.research/` is in `.gitignore` line 14 ("fork-local research
scratch; never committed"), but 534 files were committed before that rule: 228 of them
`*.md`. Of the 105 reports referenced below, **52 are in the repository and 53 exist only in
this working tree**. A row whose file is missing after a fresh clone is not a broken link — it
is a report that was never committed. `git ls-files .research | grep '.*\.md$'` says which.

**Grouping rule.** A report is listed only if (a) it ends in a keep/reject/verify verdict that
changed code, a plan, or a default, or (b) it is the measurement a later decision cites. Pure
source inventories with no verdict ("here are 400 features of project X"), captured payloads,
issue dumps, CI logs and per-run artifacts are not indexed. Reports whose verdict was later
overturned stay in the table with the superseding file named. Dates are the report's own.

Status legend: **adopted** (shipped in a fork crate), **planned** (in a roadmap, not shipped),
**rejected** (not doing it), **open** (measurement stands, decision pending), **superseded**
(read the named file instead). `(ru)` marks a report written in Russian; the verdict column
here is in English so the index reads in one pass.

Where two reports disagree, the newer file wins and the older one says so: the graph
reachability count alone changes four times (`DEADWEIGHT-AUDIT` 20/51 → `ATTACH-VERDICT`
23/48 → `GLUE-STATUS` 29/45 → `R1-MODULE-VERDICTS` 31/46). Read the newest, not the loudest.

## 0. Read these first

| document | question it answers | verdict |
| --- | --- | --- |
| `R1-MODULE-VERDICTS.md` | keep-or-delete for every module no tool action reaches | **open** — the newest count (31 reachable / 46 unreachable) and the current decision list |
| `SESSION-SYNTHESIS.md` | what is true at the end of the 2026-10-22 graph session | **open** — the narrative of that session; its 29/45 count is superseded by `R1-MODULE-VERDICTS.md` |
| `STATE.md` | what the `graph` tool implements today, measured | **open** — the baseline every later graph report corrects |
| `SYNTHESIS.md` | four reports verified: what to accept, catch, refuse | **open** — the shortest honest summary of the graph backlog |
| `SYNTHESIS-2026-10-21.md` (ru) | what five parallel subagents found about providers, context, graph/lsp | **adopted in part** — 8 findings, 5 shipped as fixes |
| `ROADMAP-V2-2026-10-21.md` (ru) | what to build next, re-estimated from that synthesis | **planned** — R01..R11 with crate impact |

## 1. Code graph (`pa-graph`)

The module-reachability series starts at `DEADWEIGHT-AUDIT.md` and ends at
`R1-MODULE-VERDICTS.md` in §0.

| document | question it answers | verdict |
| --- | --- | --- |
| `our-graph-audit.md` | what the graph actually is, cited `file:line` against a live 36k-node store | **open** — audit of record at `d521dd953`; names `R1-MODULE-VERDICTS.md` as its continuance |
| `GRAPH-AMPLIFICATION-PLAN.md` | which additions pay, ranked, with a work graph | **planned** — ~900 lines in `pa-graph`, no new crate |
| `OPTIMIZATION-MATRIX.md` | where indexing time goes (extraction 28% / store 72%) | **open** — the profile stands; the six-row plan is ranked, not landed |
| `COUPLING-MODEL.md` | is the graph coupled enough to answer blast-radius questions | **open** — recommends option C (hybrid); corrected the comparison's star counts |
| `BLOCKER-VERIFICATION.md` | are the plan's three blockers real | **open** — two hold; IMPORTS is staleness, not a broken write path |
| `GAP-INHERITS.md` | why `IMPLEMENTS`/`INHERITS` edges are declared but never written | **open** — extractor has no `bases` field |
| `DEADWEIGHT-AUDIT.md` | how much of `pa-graph` has no caller | **open** — 16.6k lines with no caller; names the P0 as a decision, not a wiring ticket |
| `WIRING-STATUS.md` | can token/time efficiency be measured, and should orphans be wired | **open** — efficiency signals already collect; the answer is a keep-or-delete review, which `R1-MODULE-VERDICTS.md` gives |
| `PLAN-F6-GLUE.md` (ru) | the wiring plan for the orphan modules | **superseded** — the review replaced the wiring sprint (`WIRING-STATUS.md`) |
| `GLUE-STATUS.md` | what got wired after the audit | **superseded** — 29/45, then `SESSION-SYNTHESIS.md`, then `R1-MODULE-VERDICTS.md` |
| `ATTACH-VERDICT.md` | where to attach the unreachable module clusters | **superseded** by `R1-MODULE-VERDICTS.md` |
| `GRAPH-DAEMON-DESIGN.md` | does the graph need a daemon | **planned** — shared store handles first |
| `REACTIVEGRAPH-2026-10-19.md` (ru) | is ReactiveGraph's deterministic-FSM design worth taking | **adopted in part** — the `Completeness` enum and `next_step_hint`; the FSM is rejected |
| `learn_graph_auto.md` (ru) | how to build the graph in the background without blocking the session | **adopted** — `graph=auto`; `Watch` is declared but behaves as `auto` (`engine.rs:613`) |
| `SQLITE-PRACTICES.md` | which SQLite knobs mattered | **adopted** — `ANALYZE`; the rest measured and rejected |
| `HYBRID-PROBE.md` | one probe over every staleness source: cost and findings | **adopted** — `pa-graph/src/probe.rs`, wired through `status::cached_report`, 77-80 ms |
| `GIT-STATUS-COST.md` | is `git status` a better staleness probe than ours | **rejected** — hash join wins; git only adds rename detection |
| `IDENTITY-FINDING.md` | do CALLS edges survive a file rename | **open** — they do not; the probe now *reports* the rename, the index path still keys by path |
| `three-servers.md` | what three code-intelligence MCP servers do that we do not | **planned** — four transferable mechanisms, one line each |
| `subagent-issue2-graph-lsp.md` (ru) | what issue #2's "25% loss" actually is | **adopted** — two defects, one a counter bug; `subagent-R07-R08-graph.md` carries the fix (`e75a2c2da`) |
| `subagent-issues-3-4.md` | issues #3/#4 plus an MCP stderr/timeout repro | **adopted** — the startup deadline now keeps the child's stderr (`7ffed85d2`) |
| `subagent-R07-R08-graph.md` | the indexing defects behind issue #2 | **adopted** — fixed |
| `subagent-R09-arch.md` | `architecture` output invisible in `details` | **adopted** — moved into `content` |

## 2. LSP and edit (`pa-lsp`, `pa-edit`, `pa-ast`, `pa-diff`)

| document | question it answers | verdict |
| --- | --- | --- |
| `native-rust-lsp-plan.md` | native Rust LSP or Serena over MCP | **adopted** — native; the Serena-MCP plan was rejected |
| `lsp-synthesis-model.md` | how to model Serena + oh-my-pi into one symbol layer | **superseded** by `native-rust-lsp-plan.md` |
| `serena-deep.md` | every Serena tool, its naming and flags | **adopted** — action names and name_path addressing |
| `learn_serena.md` (ru) | Serena's bugs and edge cases, as regression material | **adopted** — the invariants became `pa-lsp` tests |
| `synthesis-serena-omp.md` (ru) | what to take from each, module by module | **adopted** — the `pa-lsp` module list |
| `oh-my-pi-deep.md` (ru) | oh-my-pi's feature inventory, per crate | **adopted in part** — behaviour contracts, not code |
| `learn_omp.md` (ru) | oh-my-pi's LSP/rename defects, `file:line` | **adopted** — rename and diagnostics guards |
| `oh-my-pi-hashline-edit-audit.md` | is hashline worth its token cost | **open** — recommendation was *not* to port the grammar; the fork shipped it anyway |
| `omp-tool-steering.md` (ru) | does oh-my-pi redirect a mis-chosen tool | **adopted** — no primitive exists; the arbiter is a note, not a redirect |
| `TOOL-AUDIT.md` | are the native tools lazy, and is laziness right | **planned** — LSP needs warming, not more laziness (22 s cold start) |
| `accordeon-comparison.md` | Accordeon's context management vs ours | **open** — one real delta (reversible per-block folding); do not reimplement compaction |

## 3. Memory (`pa-mem`)

| document | question it answers | verdict |
| --- | --- | --- |
| `pogocache-techniques.md` (ru) | which pogocache memory/CPU tricks are portable | **adopted** — 6-byte pointers, Robin Hood, block-LZ4, 2-random eviction |
| `pa-mem-f9.md` | the F9 backlog as an implementation map | **adopted** — 85 unit + 8 integration tests, hashes pinned to the C source |
| `PLAN-ROBINHOOD-SATURATION.md` (ru) | why `RobinHoodMap` loses keys above 58% load | **open** — `dib` is `u8`; the failing test is `#[ignore]`d |
| `ruvector-techniques.md` (ru) | which ruvector memory techniques survive `forbid(unsafe_code)` | **planned** — adapt RaBitQ, do not copy |

## 4. Typed judgment (`pa-jev`)

| document | question it answers | verdict |
| --- | --- | --- |
| `adr-pa-jev-judge-layer.md` (ru) | should the fork have a typed-judgment layer, and where | **adopted** — the ADR; `pa-jev` → `pa-core::jev` layering |
| `jev-deep-research.md` (ru) | what TypeSafe/System One is and what it measures | **adopted** — three primitives, calibrated probabilities |
| `jev-fork-integration.md` | how to wire the judge without a third-party SDK | **adopted** — (A) tool + internal layer, not failover |
| `JEV-DECISION-ROLLOUT.md` (ru) | gaps between the research and the shipped layer | **open** — explicitly "a plan, not a proven saving" |
| `prime-typesafe-jev.md` (ru) | how a third-party client compares to ours | **adopted** — the router pattern (shortlist → `choice`) |
| `subq-snap.md` (ru) | is SubQ Snap a usable decision model / chat provider | **adopted in part** — judge yes, chat no |
| `ruvector-jev-openjev.md` (ru) | OpenJev wire fields vs our `pa-jev` types | **adopted** — no contract divergence found |
| `learn_arsumbris_jev_harness.md` (ru) | field evidence that typed decisions beat free text | **adopted** — 30-50% token saving, "selector not planner" |
| `subagent-R05-overflow-watchdog.md` | the overflow classifier and stream idle watchdog | **adopted** — real provider strings recognized |

## 5. Build, tooling, verification

| document | question it answers | verdict |
| --- | --- | --- |
| `BUILD-OPTIMIZATION-PLAN-V2.md` (ru) | where 100+ GB of build disk and the minutes go | **planned** — three open questions at the end, unanswered |
| `BUILD-OPTIMIZATION-PLAN.md` (ru) | the first estimate | **superseded** by V2 |
| `rust-tools-creusot-hotpath.md` (ru) | Creusot vs hotpath: adopt or refuse | **adopted in part** — hotpath behind `--features profiling`; Creusot refused |
| `VERUS-SOFT-ADAPTATION.md` (ru) | does Verus work here, and what does it cost | **adopted** — soft, opt-in, removable |
| `VERUS-ADAPTATION-PLAN.md` | what to prove, how far it parallelises | **adopted** — three mirrors, 11 contracts |
| `PDD-VERUS-ASSESSMENT.md` | is PDD Kit real software | **rejected** — announcement only; Verus is the real one |
| `TEST-MATRIX.md` (ru) | how upstream tests 142 candidate features | **planned** — test matrix per feature |
| `fork-testing-matrix.md` (ru) | what the fork already covers, what is missing | **planned** — gap analysis |
| `fork-testing-research.md` (ru) | how Codex / oh-my-pi / Grok Build test | **adopted** — faux-model and state-oracle patterns |
| `learn_blackbox_qa.md` (ru) | how six harnesses verify agent behaviour | **adopted** — the `RT-*` / `ST-*` lane design |
| `subagent-gate-classification.md` | why four gate units were red | **adopted** — classified, not retried |
| `PREEXISTING-FAILURES-2026-10-21.md` | what was already red before today's work | **open** — one commit does not build alone |
| `ci-recovery-report.md` | the CI run recovery, unit by unit | **adopted** — the fixture fixes that followed |

## 6. External systems (read for mechanisms, not for code)

| document | question it answers | verdict |
| --- | --- | --- |
| `codebase-memory-deep.md` (ru) | what CBM's 31-pass indexer and closure repair do | **adopted in part** — 4-5 signals, contentless FTS5; Cypher rejected |
| `codebase-memory-features.md` (ru) | which CBM features map to which crate | **adopted** — the crate split (`pa-lsp`/`pa-graph`/`pa-ast`) |
| `cbm-daemon.md` | is CBM's daemon pattern worth taking | **planned** — one daemon per account, atomic publish |
| `CBM-DAEMON-FINAL.md` | the same, verified against the cloned C source | **planned** — supersedes `cbm-daemon.md` and `CBM-DAEMON-PATTERNS.md` on every claim |
| `arsumbris-mechanisms.md` | what a 49-tool knowledge vault does that a graph should | **adopted in part** — progressive disclosure; three things do not transfer |
| `layerfs-report.md` | what LayerFS actually enforces | **rejected** — honest evidence, wrong summary; content store has nothing to act on |
| `layerfs-x-graph-synthesis.md` | which LayerFS mechanisms transfer to `pa-graph` | **adopted** — identity discipline only (digest pinning, schema check, one transaction) |
| `learn_layerfs.md` (ru) | LayerFS applicability at first read | **superseded** by `layerfs-x-graph-synthesis.md` |
| `ruvector-full-inventory.md` (ru) | the full ruvector crate inventory | **open** — reference, no verdict |
| `harness-competitors.md` | how Grok Build / DeepSeek / Command Code differ | **adopted in part** — tool-call repair, token-efficient tools |
| `learn_grok2.md` (ru) | Grok Build's prompts, compaction, caching | **adopted in part** — the input ladder idea |
| `learn_commandcode.md` (ru) | Command Code's tool descriptions and "taste" gate | **adopted** — description shape; the taste gate was only planned |
| `taste-error-reduction.md` (ru) | how to cut repeat-edit errors with a taste admission gate | **planned** — `taste_admission.rs` does not exist; the report is a v1 design |
| `prompt_codex.md` · `prompt_grok.md` · `prompt_commandcode.md` · `prompt_ohmy_pi.md` | the full system prompts as actually sent | **open** — primary source for the compression work |
| `learn_prompts_caching.md` (ru) | four harnesses' prompts vs ours, and cache behaviour | **adopted in part** — volatile layer to the tail |
| `learn_caching_2026.md` (ru) | provider cache rules, checked against our P0 | **adopted in part** — breakpoint rules, prewarm |
| `learn_provider_caching.md` | official provider cache rules vs our implementation | **adopted** — the rules the cache tests encode |
| `COMPRESSION-2026-10-19.md` (ru) | how to compress the system prompt without breaking the cache | **planned** — deltas protocol, truncation marker |
| `learn_tool_descriptions.md` (ru) | why the model called `python_repl` 132× and `graph` once | **adopted in part** — the native-tools prompt section and the arbiter; the tool descriptions are still action menus |
| `subagent-smart-routing.md` | should routing choose the model per turn | **adopted in part** — the pure ordering function behind `failover_candidates` shipped (`subagent-routing-settings.md`) |
| `subagent-routing-settings.md` | did the routing setting and the pure router ship | **adopted** — `routing.mode` gate + pure decision function + telemetry, three commits |
| `adr-ratelimit-admission.md` (ru) | how to stop 9 subagents from colliding on one provider limit | **planned** — wave 2a, not shipped |
| `learn_harness2.md` (ru) | does harness self-improvement actually pay | **open** — +11.5 pts measured on other substrates |

## 7. Fork operations

| document | question it answers | verdict |
| --- | --- | --- |
| `subagent-update-crosskill-fix.md` | why a fork update killed a co-installed upstream daemon | **adopted** — stop only a daemon under the prefix we write |
| `subagent-update-crosskill-verification.md` | does the fix cover every stop path | **open** — one hole named |
| `subagent-rca-child-unreachable.md` | why four subagents wedged as unreachable | **open** — read the child's JSONL, not `rlm.collect()` |
| `subagent-quota-liveness-research.md` | how subscription quota and liveness pings work | **planned** — quota park |
| `subagent-statusline-cache.md` | cache-hit stats in the status line | **adopted** — `/cache` |
| `subagent-R01-cachekey.md` | does `prompt_cache_key` reach the wire | **adopted** — it did not |
| `subagent-R04-catalog.md` | dead 9router routes in the provider catalog | **adopted** — removed |
| `subagent-R06-release-assets.md` | missing catalog assets in the fork release | **adopted** — bundled catalogs shipped |

## Not indexed

- `learn_*.md` inventories without a verdict beyond the ones above (raw source walks).
- `prompt_*.md` beyond the four listed (captured payloads, archived for lineage).
- `MATRIX-*`, `design-matrix.*`, `PHASES.md`, `matrix-analysis.md`, `MANIFEST.md` — the
  142-feature design matrix and its JSON-LD graph. It is a feature *catalogue*, not a
  decision; `PHASES.md` derives an order from it. Use it to look up a feature, not to
  learn what the fork does.
- `ci-*.md`, `b1-*`, `re-verify-report.md`, `VERIFICATION*.md`, `verify-*.md` — per-run
  verification of a fix that has since landed; read them only when a specific fix is in
  question.
- The isolation model itself is not a report: it is `docs/fork-identity.md`.
- Subdirectories (`.research/{sources,ci-rca,gate,codex-experiments,...}`) hold fixtures,
  captured payloads and logs. `.research/` is gitignored apart from files already tracked.
