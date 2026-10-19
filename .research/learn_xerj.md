# XERJ (xerj-org/xerj) — deep study and applicability to allspark

**Date:** 2026-10-19 · **Repo:** /tmp/xerj (clone, ~272 MB) · **Read-only study**
**Comparand:** our pa-graph (crates/pa-graph + pa-ast, 32.7k LOC) + `graph=auto` (pa-core).

---

## 0. What XERJ actually is (one paragraph)

XERJ is **not a code-graph tool and not an agent tool**. It is a **from-scratch
Elasticsearch-compatible search engine in Rust** (548k LOC in `engine/crates`,
17 crates) whose *primary user is an AI agent*. ES wire compat is the adoption
bridge (`docs/WHY_XERJ.md:1-9`: "XERJ shares no code and no architecture with
Elasticsearch… the primary user is an AI agent"). Its thesis
(`docs/WHY_XERJ.md:14-27`): an agent handed unfamiliar data needs **onboarding**
(point-at-folder → queryable), **orientation** ("what is in here?" → datasets,
field types, example values), and **continuity** (durable memory by meaning).
Everything else — autoindex, MCP, memory, benchmark honesty culture — hangs off
that thesis. Scale comparison: XERJ engine ≈ 548,509 LOC vs our whole repo
717,669 vs pa-graph+pa-ast 32,685. This is a 17×-bigger product than our graph
tool; "port XERJ" is not on the table. The question is which *ideas* transfer.

---

## 1. What it indexes and how (`engine/crates/xerj-autoindex/src/`)

### 1.1 Format detection: content, never extensions
- `sniff.rs:1-4` — "Content-based format detection. NEVER trusts file
  extensions. Order: magic bytes → binary check → text heuristics
  (json/jsonl → html/xml → logs → sql dump → csv → yaml → txt)." 4,713 lines.
- Demotion ladder is never fatal: parse failure downgrades family → txt →
  junk-with-metadata, always counted (`extract/mod.rs:1-3`).

### 1.2 Extractors: 26 format families (`extract/` — one module each)
`code, pdf, docx, doc, xlsx, xls, pptx, epub, html, ipynb, json, jsonl, csv_x,
logs, man, mbox, eml, sqldump, sqlite_x, txt, unity, xml_x, yaml_x, bvh, opc,
json` (`extract/mod.rs:6-31`).
- **PDF:** `extract/pdf.rs` (native Rust extractor, no external binary).
- **Logs:** dedicated `extract/logs.rs` — parses log lines into typed records.
- **Mailboxes:** `mbox.rs` (65K), `eml.rs` (46K) — Google-Takeout-shaped.
- **Notebooks:** `ipynb.rs`; man pages: `man.rs`; SQL dumps incl. INSERT
  streaming: `sqldump.rs`, `sqlite_x.rs` (row-capped per table).

### 1.3 Code: AST-aware via tree-sitter — same tech as our pa-ast, wider
- `extract/code.rs:1-21` — "AST-aware extraction via tree-sitter… Each file
  becomes a file-level document carrying `language`, `symbols`
  ({name,kind,line}), `defs` ("kind name" lines so BM25 matches `class User`),
  `body` (full text). **PLUS one document PER DECLARATION (#500)** under a
  `code:<line>:<name>` locator, so a constant/signature lookup retrieves the
  ~40–80 B declaration line as its own unit."
- Registry: **37 languages** (`code.rs:108`, grammar deps
  `engine/crates/xerj-autoindex/Cargo.toml` — python, js/ts/tsx, rust, go,
  java, c, cpp, ruby, php, c#, bash, kotlin, swift, scala, dart, lua, perl, r,
  julia, haskell, elixir, clojure, scheme, erlang, ocaml, zig, objc, groovy,
  powershell, f#, nix, fortran, matlab, solidity). Extension collisions are
  disambiguated by a content probe (`.m` ObjC vs MATLAB, `code.rs:87-105`).
- Parse failure → indexed as plain text, never dropped (`code.rs:10-11`).
- Machine-generated giants skipped: `CODE_CAP = 2 MB` (`code.rs:33`).

### 1.4 Chunking (this is the part worth copying)
- `extract/mod.rs:89-118`:
  - `SECTION_CHARS = 2 KB` — "Was 32 KB… BM25 scores per document, so a 32 KB
    section dilutes any match into noise… at 2 KB with paragraph overlap it is
    [retrievable]" (comment cites the measured 460-commit history file).
  - `SECTION_OVERLAP = 200` chars — "so an answer spanning a boundary stays
    retrievable from both sides."
  - `MAX_RECORDS_PER_FILE = 4096` with `ExtractStats.truncated` — cap is
    **reported, never silent** (#381).
  - `MAX_LINE = 16 MB`, `MAX_WHOLE_FILE = 64 MB`.
- Every extractor is bounded-memory and streaming (`Sink` callback may stop
  early for sampling, `extract/mod.rs:70-73`).

### 1.5 Storage schema: NOT SQLite — a Lucene-like custom engine
- `crates/xerj-fts/` is a from-scratch inverted index: FST term dictionary +
  **PFOR-compressed 128-doc posting blocks** (`fts/src/postings.rs:5-6,88`),
  BM25 with ES-identical constants k1=1.2, b=0.75 (`fts/src/bm25.rs:1-26`),
  per-segment files `.fst/.post/.meta/.norms` (`fts/src/index.rs:9-20`),
  rayon-parallel per-segment search (`fts/src/search.rs:18-20`).
- `crates/xerj-storage/src/lib.rs:5-13` — "2 files per segment (.seg+.sidx);
  **one durability system: WAL only** — no dual translog/Lucene commit; mmap
  segment reads; `ArcSwap<IndexSnapshot>`; DashMap lock-free version map";
  size-tiered merges with tombstone purge (`storage/src/merge.rs`).
- Vectors: HNSW with exact rescoring; filtered/small cases fall back to exact
  brute force (`xerj-mcp/src/lib.rs:39-42` honesty note).
- **Agent memory** reuses the same engine: `/_memory/{ns}` is a reserved index
  `.xerj-memory-{ns}` — "store/recall reuse the exact same document,
  dense_vector, BM25 and metadata-filter code paths"
  (`xerj-api/src/memory_api.rs:1-6`), with dedup, recency blending, paging
  (`memory_api.rs:9-17`).

---

## 2. Autoindex: what it is and how it runs

### 2.1 Shape: client CLI + running node — NOT an in-process daemon
- `xerj autoindex <folder>` is a **pure HTTP client** of an already-running
  node: "it does NOT link xerj-engine, works against any endpoint"
  (`autoindex/src/lib.rs:1-3`). Two phases: phase A samples/sniffs every file
  and infers datasets (593 datasets on their demo repo, `README.md` "Feed it
  any folder"); phase B indexes with N workers.
- **No autostart at MCP-server start.** The MCP server is a thin proxy; docs
  say "proxies N tools to a XERJ node that is ALREADY RUNNING. This command
  does not start a node" (`xerj-mcp/src/lib.rs:197-200`). `xerj init` only
  writes agent configs (`autoindex/src/init.rs:1-14`).
- Re-runs are incremental and idempotent: journal resume + content-derived
  `_id`s ("the journal resumes and idempotent `_id`s never duplicate",
  `landing/llms.txt` "Start here" step 4).

### 2.2 The 10-minute gate — the best agent-safety idea in the repo
- `gate.rs:1-27`: when phase A's **measured** estimate exceeds `--max-minutes`
  (default 10, `gate.rs:48`) and nobody approved, autoindex **stops before
  touching the index** and emits a decision payload.
- Distinct exit code 4, "never 1. Exit 1 is autoindex's catch-all… an agent
  that could only see 1 would have to parse prose to tell 'your endpoint is
  down' from 'I need you to choose'" (`gate.rs:11-16`).
- Never waits on stdin the user cannot see; never deadlocks a piped agent
  (`gate.rs:17-22`). Options carry **measured** costs: `narrower` re-costs the
  run without the heaviest dirs using phase-A rates; `fast` explicitly refuses
  to claim an unmeasured speedup (`gate.rs:23-27`).
- Estimates are measured *on this machine* from phase A's completed parses
  (per-family throughput), bounded by Graham's list-scheduling result
  (`estimate.rs:1-30`) — "never state a number we did not measure here".

### 2.3 `--watch`: filesystem watcher done right
- One design rule: **exactly one indexing path** — "a burst of filesystem
  events does not index anything itself — it decides which files may skip the
  hash, and then runs the ordinary incremental pass… same journal, same plan,
  same publish machinery" (`watch.rs:1-11`).
- Digest cache is in-memory, never persisted; skip re-hash only if (1) no event
  named the file/dir/ancestors AND (2) (size, mtime, inode) unchanged; restart
  always re-hashes in full (`watch.rs:13-27`). The one known hole is stated
  plainly (`watch.rs:29-32`).
- Debounce 400 ms with a hard 5 s max-hold so an append-forever log can't
  starve indexing forever (`watch.rs:68-80`).
- Watches are placed **per indexed directory** from the same walk/ignore stack
  the indexer uses (`.gitignore` + `.xerjignore`, `watch.rs:35-45`,
  `ignore_rules.rs:75-79`) — not one recursive root watch that would drown in
  `target/` noise.
- Object storage `--watch` is separate: polling ListObjectsV2 with a
  cost/latency/completeness table for poll-vs-events (`objwatch/mod.rs:1-25`)
  and a bounded-window journal (save every 256 objects or 2 s; killed watcher
  re-emits at most the window — `objwatch/journal.rs:3-17`).

### 2.4 "Agents know your data": the catalog + `xerj map`
- After autoindex, the `autoindex-catalog` index holds one doc per dataset with
  inferred field types, cardinality, coverage, example values, min-max, plus
  cross-dataset correlations with evidence (`xerj-mcp/src/map.rs:1-9`).
- `xerj autoindex map` renders the briefing; MCP `xerj_map` exposes it. The
  measured payoff (`benchmarks/map-gate/README.md:1-12`): agents issuing DSL
  with wrong field names went **20/30 → 0/30 unknown-field 400s (100%
  reduction)**, gate threshold was ≥80%.

---

## 3. Search language, response format, and the token claims

### 3.1 Retrieval stack
- BM25 (ES-identical formula/constants, `fts/src/bm25.rs`) + kNN over
  `dense_vector` + hybrid with RRF or linear fusion
  (`xerj-mcp/src/lib.rs:27-31` table). Built-in embedder is **lexical by
  default**; neural (Candle BERT MiniLM, ~90 MB download) is opt-in — a
  documented agent trap (`landing/llms.txt` preamble caveat).
- `xerj search` builds a definition-first `multi_match` for the agent:
  `defs^3` phrase boost, exact `name` term boost 10
  (`autoindex/src/search.rs:156-170,379-399`), and reorders no-line hits to
  sit just after the last lined hit of the same path so agents never cite an
  uncitable header (`search.rs:493` `prefer_citable_line_on_the_same_path`).

### 3.2 Response shaping = the token story
- MCP search tools accept `max_tokens`; the response **never exceeds the
  budget** (`token_budget.rs:1-9`). Mechanism (`token_budget.rs:19-26`):
  1. dedup overlapping file:line passages before budgeting (line 263),
  2. greedy keep in rank order,
  3. over-budget hits degrade to **locator-only** (`file:line`) — line 487
     prints `[passage omitted: over the max_tokens budget — locator only]`,
  4. remainder counted as dropped; a `_token_budget` accounting block reports
     used/deduped/dropped with the named tokenizer (UAX #29 words — the same
     split the BM25 analyzer uses; honesty note says it under-counts BPE).
- Every tool description carries the same honesty notes (rerank sends doc text
  off-box and needs an operator key; hybrid fusion is rrf|linear only; brain
  tools are a link index, not a graph DB) — `xerj-mcp/src/lib.rs:34-56`.

### 3.3 The "40× fewer tokens than grep" claim — **not in the repo**
- The repo's measured claims are narrower: **2.7× fewer *output* tokens than
  grep** (26× vs memory-only) at the same 16/16 solve rate in a controlled
  8-task × 4-language study, ~**5× end-to-end** per field reports
  (`README.md:82-104,356-369`). The "40×" number in our task brief does not
  appear anywhere in the repo; closest honest anchors are those above.
- The flagship grep-vs-retrieval benchmark is a **harness with no published
  results yet** (`benchmarks/grep-vs-retrieval-lucene/README.md:1-7`) — and it
  deliberately includes an **index-hostile question built to make XERJ lose**
  (q09: exhaustive counting across a tree is what top-k retrieval cannot do,
  `README.md` §1) plus prior-hostile questions where the checkout contradicts
  model memory (q01: BLOCK_SIZE 256 where "the world says 128").
- `docs/TOKEN_USAGE.md` is the best token document in the repo and worth
  reading in full: tokens ≈ envelope + answer + materialized intermediates;
  the **corpus-composition law** ("retrieval savings scale with the prose
  fraction": source+docs corpus → XERJ 5.3× FEWER tokens; 458k log/CSV-record
  corpus → XERJ costs *more* tokens but answers more correctly;
  `TOKEN_USAGE.md:96-121`). "A single headline token number across both
  regimes is meaningless."

---

## 4. MCP integration

- **17 tools, 14 without a hub key** (`xerj-mcp/src/lib.rs:9-19`): `xerj_search`,
  `xerj_semantic_search`, `xerj_vector_search`, `xerj_hybrid_search`,
  `xerj_memory_store/recall`, `xerj_brain_ego/link/unlink/overview`,
  `xerj_plan` (NL → *validated* query DSL from a deterministic planner, not an
  LLM), `xerj_field_report`, `xerj_map`, plus hub tools.
- Descriptions are long, honest, and self-guarding: defaults that make the
  common case need no arguments (`index` defaults to `ax-*`,
  `xerj-mcp/src/lib.rs` tool_specs at 695+); "A no-match is NOT an error: it
  says the corpus is wrong for the task" (`xerj_code_search` spec);
  license-aware reference coding — AGPL/SSPL/Elastic/BUSL hits get warnings,
  `strict` strips passage text keeping the locator; 30-day staleness refusal
  with `stale_ok` override (same spec).
- **Progressive disclosure, three mechanisms:**
  1. token budgets + dedup + locator-only degradation (§3.2);
  2. `xerj_map`'s trim ladder: per-index 4 KB cap; examples→notes→fields
     dropped in coverage order, each stage reported in `trimmed`
     (`map.rs:24-35,43-60`); >8 datasets default to a summary page
     (`SUMMARY_AT = 8`, `map.rs:58`);
  3. capability hiding: hub tools are removed from `tools/list` entirely when
     the build lacks the signing key — "advertising them only produced a call
     that exits with the no-key message" (`xerj-mcp/src/lib.rs:180-186`).
- `xerj init` (`autoindex/src/init.rs:1-60`) writes: `.mcp.json` (merged,
  others kept), `.cursor/mcp.json`, a **10-line SKILL.md**, a Cursor rule, and
  an AGENTS.md section — each with `.bak` backups and an `xerj-init-version`
  marker so re-runs upgrade only files nobody edited.

---

## 5. Benchmarks: what's there and is it honest?

| Benchmark | What it does | Honesty signals |
|---|---|---|
| `grep-vs-retrieval-lucene` | 3-arm (native grep / xerj / hybrid) token+cost on real Claude-CLI billing counters | **Harness only, no results published** (README:1-7); index-hostile question included; blind judging; tokens-per-*correct*-answer headline |
| `reference-coding` case study | 8 tasks, 4 langs, 16 runs/arm | 2.7× output-token claim source; "not in this repository; only its outputs are" (harness gap admitted) |
| `beir-hybrid` | nDCG@10 on BEIR test splits, live queries, free local model | standard IR eval, dated run (rc.74, 2026-09-18) |
| `map-gate` | unknown-field 400s no-map vs with-map | pre-registered gate ≥80%, measured 100%; discovery-first probing of all failure forms |
| `idle-budget` | 450 idle indices: CPU/wakeups/RSS in CI | per-index idle RSS 183.8 kB, gate ≤256 kB; THP/allocator pins documented; shared-box load recorded |
| `index-size` | where durable bytes go (zstd levels) | absolute totals flagged not comparable across commits |
| `corpus-ab-2026-10`, `hub-pov`, `systemone-*` | agent-outcome A/B, hub POV, decision gates | **pre-registered, frozen before any run**; `DEVIATIONS.md` discipline |

Verdict: this is the most honest benchmark culture I have seen in a repo of
this kind. Marketing copy on the landing page is stronger than the repo's
numbers ("40×" does not exist in-repo), but every in-repo claim is either
measured with a reproducible harness or explicitly labeled pre-registered /
not-yet-run.

---

## 6. Line-by-line vs our pa-graph / allspark

### 6.1 Their autoindex vs our `graph=auto`
- Ours: `settings.graph.auto = auto|watch|off` (`pa-core/src/settings/types.rs:270-291`),
  fire-and-forget `tokio::spawn` + `spawn_blocking` + rayon at session start
  (`pa-core/src/session_engine/engine.rs:596-631`), single-flight via
  `graph_in_flight.rs`, WAL SQLite store.
- Theirs: client-CLI + node, measured-time **gate with decision exit code 4**,
  incremental resume, watch events only *skip hashing* (one indexing path).
- **Better theirs:** the budget gate (ours will happily index a huge tree in
  the background and burn the user's laptop), the "never silent truncation"
  reporting, estimate-from-phase-A-measured-rates. **Better ours:** zero
  config/zero daemon — our in-session model matches a coding harness; XERJ
  needs a separate server process, which is a product decision we should NOT
  copy (our daemon already exists for other reasons, and a second service per
  project is worse DX for us).

### 6.2 Their search vs our `search` (BM25+RRF+FTS5)
- Ours: 3 channels (name FTS5, body FTS5, graph degree) → RRF (k=60) + MMR
  (`pa-graph/src/search.rs:1-13`, `hybrid.rs:1-8`), symbol-granularity hits:
  the model gets `- name (file:line) [kind] score` locators
  (`pa-core/src/tools/graph.rs` action_search). Full-text coverage = code
  symbols + docstrings/comments. No PDF/docs/logs at all.
- Theirs: document/passage granularity over **any** format; 2 KB sections with
  200-char overlap; per-declaration docs; `defs^3`/name^10 boosts; passages
  returned with citable file:line.
- Gap worth closing **selectively**: our search answers "where is the
  symbol"; XERJ-style passage retrieval answers "show me the text at that
  place" and doc-questions ("how do we rotate the WAL segment") without a
  Read round-trip. Our `body` channel already indexes docstrings; md/docs
  files are a natural next corpus for FTS5 (they need no tree-sitter).

### 6.3 Their MCP descriptions vs our tool descriptions
- Theirs: every description carries defaults, honest limits, and failure
  semantics ("no-match is not an error"), plus response shaping
  (max_tokens/dedup/locators). Ours: one dense paragraph listing actions
  (`graph.rs` description), no output-shaping knobs, no budget.
- Adoptable now: a `limit`/budget-style knob on `graph search` output, dedup
  of same-file hit runs, and a "no hits ≠ build failed; run index" hint in the
  description. Small, hours not days.

### 6.4 Durable storage vs our WAL SQLite
- Common ground: both choose **one durability system** (their "WAL only, no
  dual translog" mirrors SQLite's own design), both mmap/budget reads, both
  resume after kill. Theirs adds segment merges; ours gets them from SQLite.
- Nothing to port. If pa-graph ever outgrows SQLite writes (it won't soon at
  33k LOC / session-scale corpora), the xerj segment format is the reference
  to read — `storage/src/lib.rs:5-13`.

### 6.5 Things we did NOT expect to find that may matter more
- **Second brain / detect/** (`autoindex/src/detect/mod.rs`): deterministic,
  versioned edge detectors (wikilink@1, mdlink, href, emailthread, cratecite,
  pathcite, samedir, sharedterm) over non-code docs; edge_id =
  xxh3(src,type,dst,valid_at) so re-runs converge instead of growing
  (`detect/mod.rs:47-60`); bi-temporal soft-invalidation; "a model in that
  loop would turn every re-run into a new belief set" (`detect/mod.rs:6-11`).
  This is a *doc-graph* complement to our code graph.
- **Agent memory** (`/_memory/{ns}`, `memory_api.rs`) and
  `xerj_map`-style orientation (§2.4) — the "what is in here" surface our
  `graph architecture` action only partially covers for code.
- **`xerj gain`** (`xerj-server/src/main.rs:407`): usage counted from the
  audit log — "what it did for you — counted, not estimated" (README:53).
  Adoption telemetry with a user-facing payoff; matches our AGENTS.md
  telemetry rule.

---

## 7. Verdict

### Adopt (cheap, high value)
1. **Measured-size gate for `graph=auto`** (~1-2 days): before the background
   index at session start, walk the tree (cheap stat pass), estimate from
   bytes and a locally calibrated rate; above a budget (default 10 min) emit a
   structured "needs-decision" into the session transcript instead of
   silently indexing; distinct failure class in the report. Files: new
   `pa-graph/src/gate.rs` + `engine.rs:596-631` wiring + settings knob.
   QA: unit tier (estimate math) + one faux QA task (big fixture → decision,
   not index).
2. **Truncation honesty in graph output** (~2-4 h): every cap in pa-graph
   output (hits, trace depth, architecture listing) reports what was cut —
   XERJ's `truncated`/`fields_omitted`/`trimmed` pattern
   (`extract/mod.rs:97-114`, `map.rs:24-35`).
3. **Budget + dedup shaping for `graph search` results** (~4-8 h): cap the
   rendered hit list, dedup same-file runs behind one locator line, degrade
   over-budget entries to bare locators (token_budget.rs:19-26 pattern). Add
   the "no hits ≠ error; run action=index" line to the tool description.
4. **`xerj init`-style onboarding command** (~1 day, optional): a fork command
   that writes an AGENTS.md section + skill teaching when to prefer
   graph/lsp/grep (mirrors `init.rs:20-60` and the 10-line SKILL.md idea).
   This is adoption plumbing, not engine work.
5. **Docs-level idea for QA:** pre-registered gates with results dirs
   (`benchmarks/map-gate` shape) — our ST-tasks already do half of this; the
   freeze-before-run + DEVIATIONS.md discipline is the add-on.

### Do not adopt
- **The engine itself** (custom inverted index, PFOR postings, HNSW, ES wire
  compat): 17 crates and 548k LOC; SQLite FTS5 serves our scale; a second
  daemon per project worsens our DX. Our architecture layer (pa-graph
  ownership rules) forbids dragging this in.
- **MCP-server-as-proxy product shape**: our tools are in-process native
  tools; XERJ's MCP exists because it must reach foreign agent hosts.
- **Reference-coding / Corpus Hub** (signed packs of pre-indexed OSS repos,
  `harvest/`): interesting product, zero relation to our fork's mission.
- **Memory namespaces in the graph DB**: our continual harness already owns
  memory; duplicating it in pa-graph would be two sources of truth.

### Should pa-graph index non-code (docs/PDF/logs)?
**Partially, and only as text — with a hard boundary.**
- YES to **markdown/docs/README/ADR text in FTS5** (body channel): it is the
  highest-value gap (agents constantly ask doc-questions: AGENTS.md policies,
  ADR decisions — we even have pa-graph/src/adr.rs already), it costs almost
  nothing (no new parsers; file → sections → FTS5), and it does not touch the
  symbol/call/import core. XERJ's data point: on a source+docs corpus ranked
  retrieval saved 5.3× tokens (`TOKEN_USAGE.md:96-121`).
- MAYBE later, same mechanism: logs as text sections (grep still beats
  retrieval for exact/log-tail questions; their own benchmark includes an
  index-hostile counting question grep wins).
- NO to PDF/DOCX/XLSX/mbox extractors: that is XERJ's 200k-line
  `sniff.rs`+`extract/` investment serving a *data-engineering* product. It
  would triple pa-graph's size, blur the crate README scope (one owned area),
  and coding sessions rarely need PDFs. If a user needs it, XERJ exists and
  can run beside allspark as a generic MCP server (`mcp.search_tools`) — our
  generic MCP plumbing already covers that integration path.
- The clean test: an extension is in-scope if it helps answer *questions about
  this codebase* (code + its docs). The moment it indexes *arbitrary user
  data*, it is a different product.

### Effort summary for the adopt list
gate ~1-2 d · truncation honesty ~0.5 d · search shaping ~0.5-1 d ·
onboarding cmd ~1 d · docs-in-FTS5 spike ~1-2 d (needs a QA ST-task to prove
value). Total ≈ 4-7 working days, all inside pa-graph/pa-core, no new deps.
