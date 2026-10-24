# pa-graph

`FORK-ONLY` — never pushed upstream. The fork's flagship crate: the `graph` tool's
entire backend.

On-disk SQLite code graph + full-text symbol search. A native Rust port of the
`codebase-memory-mcp` store patterns and the `ruvector` hybrid-retrieval
techniques, built on `pa-ast` for parsing.

## Scope

- **Schema (`schema.rs`)**: `nodes`/`edges` with GENERATED columns, `projects`
  registry, `file_hashes`, `index_coverage`, and a contentless FTS5
  `symbols_fts` table.
- **Store (`store.rs`)**: `GraphStore::open`/`open_in_memory`, project
  registry, node/edge upserts, degree queries, file-hash classification
  (unchanged/modified/added/removed).
- **Tokenizer (`tokenize.rs`)**: `camelCase`/`snake_case` identifier splitting
  feeding a dedicated FTS column.
- **BM25 + RRF (`bm25.rs`, `hybrid.rs`)**: Robertson BM25 (k1=1.2, b=0.75),
  reciprocal rank fusion (k=60), and MMR diversity reranking.
- **Search (`search.rs`)**: three-channel (name/body/graph) retrieval fused by
  RRF, optional regex, optional payload filter, optional MMR.
- **Trace (`trace.rs`)**: callers/callees, data-flow (call args), and
  cross-service edges, with BFS `depth` and `direction`.
- **Filter (`filter.rs`)**: `AND`/`OR`/`NOT` payload-filter expressions
  compiled to a parameterized SQL `WHERE` fragment.
- **Indexer (`indexer.rs`, `extract.rs`)**: `ignore`-based walk, `pa-ast`
  parse, symbol/call/import extraction, incremental reindex.
- **Strictness (`strictness.rs`)**: the ast-grep strictness ladder wrapper.
- **Changes (`changes.rs`, G5)**: `detect_changes` runs `git diff` between two
  revisions, maps changed paths to graph files, narrows modified files to the
  symbols whose line ranges the hunks touch, and writes `Branch` / `HAS_BRANCH`
  / `FILE_CHANGES_WITH` provenance back into the graph.
- **Coverage (`coverage.rs`, G9)**: honest index accounting. Classifies each
  file as clean / `parse_partial` / `parse_unusable` / `skipped` and aggregates
  a `CoverageReport` (counts + reason histogram). "Clean ≠ completeness."
- **Architecture (`architecture.rs`, G6)**: eight file-level views — overview,
  structure, layers (longest-path on the condensation), clusters
  (weakly-connected components), cycles (SCCs), hotspots, entry_points,
  file_tree — built on `petgraph`.
- **Repair (`repair.rs`, E10)**: `auto_repair` splices a `RepairModel`
  proposal over the culprit hunks and accepts it **only** if the result parses
  cleanly through `pa-ast`.

## Non-goals

- No LSP client, no edit application, no daemon/session state.
- No dense vector search yet; `search::Embedder` is the reserved seam.
- No Cypher query engine.

## Public API

```rust
use pa_graph::{GraphStore, SearchOptions, TraceOptions, Direction};

use pa_graph::ArchitectureView;

let mut store = GraphStore::open("graph.db")?;
let report = store.index_project(Path::new("/repo"), &IndexOptions::new("repo"))?;
let hits = store.search("parse config", &SearchOptions::new("repo"))?;
let trace = store.trace(&TraceOptions::new("repo", "GraphStore::open"))?;
let changed = store.detect_changes("repo", "HEAD~1", "HEAD")?;
let coverage = store.coverage("repo")?;
let arch = store.architecture("repo", ArchitectureView::Cycles)?;
```
